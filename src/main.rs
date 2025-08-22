//! A Jenkins CI/CD-based build analyzer and issue prioritizer.
use std::{
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    sync::{Arc, Mutex},
};

use anyhow::{Error, Result};
use clap::{Parser, crate_name, crate_version};
use env_logger::Env;
use jenkins_api::{Jenkins, JenkinsBuilder, build::BuildStatus};
use log::{Level, info, log, warn};
use rayon::prelude::*;
use time::UtcOffset;
use tokio::task::JoinSet;

use crate::{
    api::{AsBuild, AsJob, AsRun, SparseBuild, SparseMatrixProject},
    config::{Config, Field, Severity},
    db::{
        Database, InDatabase, Issue, Job, JobBuild, Queryable, Run, SimilarityInfo, TagInfo,
        Upsertable,
    },
    parse::{Tag, TagSet, normalized_levenshtein_distance},
};

mod api;
mod config;
#[macro_use]
mod db;
mod page;
mod parse;
mod tag_expr;

/// CLI arguments
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Config file path
    #[arg(default_value = "config.toml")]
    config: String,

    /// Report output (`Some(None)` for stdout) path
    #[arg(short, long)]
    output: Option<Option<String>>,

    /// Whether or not to purge cache
    #[arg(short, long)]
    purge_cache: bool,
}

/// Pull builds from `project.jobs` and cache them into database `db`
async fn pull_build_logs(
    project: SparseMatrixProject,
    blocklist: &[String],
    jenkins: Arc<Jenkins>,
    db: &Database,
) -> Result<Vec<InDatabase<Run>>> {
    let mut handles = JoinSet::new();
    let mut runs = Vec::new();

    // spawn tasks to pull builds
    for sj in project.jobs {
        let job: Arc<_> = match blocklist.contains(&sj.name) {
            false => sj.as_job().upsert(&db, ())?.into(),
            true => continue,
        };
        let (build, mut mbs): (Arc<_>, _) = match sj.last_build {
            Some(build @ SparseBuild { runs: Some(_), .. }) => (
                build.as_build(job.id).upsert(&db, ())?.into(),
                build
                    .runs
                    .into_iter()
                    .flatten()
                    .filter(move |mb| mb.number == build.number),
            ),
            _ => {
                info!("Job '{}' has no builds.", job.name);
                continue;
            }
        };

        mbs.try_for_each(|mb| match Run::select_one_by_url(&db, &mb.url, ()) {
            Ok(run) => Ok(runs.push(run)), // cached
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                let jenkins = jenkins.clone();
                let job = job.clone();
                let build = build.clone();
                handles.spawn(async move {
                    let run = mb
                        .get_full_build(&jenkins)
                        .await
                        .unwrap()
                        .as_run(build.id, &jenkins)
                        .await;

                    log!(
                        match run.status {
                            Some(BuildStatus::Failure | BuildStatus::Unstable) => Level::Warn,
                            _ => Level::Info,
                        },
                        "Job '{}#{}' run '{}' finished with status {:?}.",
                        job.name,
                        build.number,
                        run.display_name,
                        run.status
                    );

                    run
                });

                Ok(())
            }
            Err(e) => return Err(Error::from(e)),
        })?;
    }

    // collect them all here
    while let Some(h) = handles.join_next().await {
        runs.push(h?.upsert(db, ())?);
    }

    Ok(runs)
}

/// Parse all untagged runs for `tags` and cache them into database `db`
fn parse_unprocessed_runs(tags: &TagSet<InDatabase<Tag>>, db: &Mutex<Database>) -> Result<()> {
    let runs = Run::select_all(&db.lock().unwrap(), ())?;

    runs.par_iter()
        .flat_map_iter(|run| Field::iter().map(move |f| (run, f))) // parse all fields
        .filter_map(|(run, field)| match (run.tag_schema, &run.log, field) {
            (None, Some(log), Field::Console) => Some((run, field, log)),
            (None, Some(_), Field::RunName) => Some((run, field, &run.display_name)),
            _ => None,
        })
        .flat_map_iter(|(run, field, data)| {
            tags.grep_tags(data, field).map(move |t| (run, data, t))
        })
        .flat_map_iter(|(run, data, t)| t.grep_issue(data).map(move |i| (run, t, i)))
        .try_for_each(|(run, t, i)| {
            let i = {
                let db = &db.lock().unwrap();
                i.insert(db, (run,))
            }?;

            if !matches!(t.severity, Severity::Metadata) {
                warn!(
                    "Found issue '#{}' tagged '{}' in run '{}'",
                    i.id, t.name, run.display_name
                );
            }

            Ok::<_, Error>(())
        })?;

    // batch update tag schema for runs afterwards
    Run::update_all_tag_schema(&db.lock().unwrap(), Some(tags.schema()))?;
    Ok(())
}

/// Calculate similarities against all issues and soft insert the groupings into [Database]
fn calculate_similarities(db: &Mutex<Database>, threshold: f32) -> Result<()> {
    let runs = Run::select_all(&db.lock().unwrap(), ())?;

    // conservatively group by levenshtein distance
    let mut groups: Vec<Vec<InDatabase<Issue>>> = Vec::new();
    runs.iter()
        .filter_map(|r| {
            let db = &db.lock().unwrap();
            Issue::select_all_not_metadata(db, (db, r)).ok()
        })
        .flatten()
        .for_each(|i| {
            match groups.par_iter_mut().find_any(|g| {
                g.par_iter()
                    .all(|i2| normalized_levenshtein_distance(&i.snippet, &i2.snippet) > threshold)
            }) {
                Some(g) => g.push(i),
                None => groups.push(vec![i]),
            }
        });

    // sort resultant groups
    groups.par_iter_mut().for_each(|g| g.par_sort());

    // store relations in database
    groups
        .par_iter()
        .filter(|g| g.len() > 1) // unique issues are discarded
        .flat_map(|g| {
            let mut hasher = DefaultHasher::new();
            g.hash(&mut hasher);
            g.par_iter().map(move |i| (hasher.finish(), i))
        })
        .try_for_each(|(hash, i)| {
            SimilarityInfo {
                similarity_hash: hash,
                issue_id: i.id,
            }
            .insert(&db.lock().unwrap(), ())?;

            info!(
                "Issue '#{}' likely matches with similarity group '#{}'!",
                i.id, hash
            );

            Ok(())
        })
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // initialize logging
    env_logger::init_from_env(Env::default().default_filter_or("info"));
    info!("{} {}", crate_name!(), crate_version!());

    // load config
    info!("Compiling issue patterns...");
    let config: Config = toml::from_str(&fs::read_to_string(args.config)?)?;
    let tags = TagSet::from_config(&config.tag)?;

    // open db
    info!("Opening database...");
    let mut database = Database::open(&config.database)?;

    // check for cache purge
    if args.purge_cache {
        warn!("Purging cache!");
        database.purge_cache()?;
    }

    // update TagSet
    info!("Updating tags...");
    let tags = TagInfo::upsert_tag_set(&database, tags, ())?;

    // purge outdated issues
    let outdated = Issue::delete_all_invalid_by_tag_schema(&mut database, tags.schema())?;
    if outdated > 0 {
        warn!("Purged {outdated} runs' issues that parsed with an outdated tag schema!");
    }

    // purge blocklisted jobs
    let blocked = Job::delete_all_by_blocklist(&mut database, &config.blocklist)?;
    if blocked > 0 {
        warn!("Purged {blocked} jobs that are on the blocklist.");
    }

    info!(
        "Pulling associated jobs for {} from {}...",
        config.project, config.jenkins_url
    );

    let jenkins = JenkinsBuilder::new(&config.jenkins_url);
    let jenkins = match config.username {
        Some(user) => jenkins.with_user(&user, config.password.as_deref()),
        None => jenkins,
    }
    .build()
    .map_err(Error::from_boxed)?;

    info!("Pulling build info for each job...");
    info!("----------------------------------------");

    let project = SparseMatrixProject::pull_jobs(&jenkins, &config.project).await?;
    let _runs = pull_build_logs(project, &config.blocklist, jenkins.into(), &database).await?;

    info!("Done!");
    info!("----------------------------------------");

    let mut database = Mutex::new(database);
    if Run::has_untagged(database.get_mut().unwrap())? {
        info!("Parsing unprocessed run logs...");
        parse_unprocessed_runs(&tags, &database)?;

        info!("Done!");
        info!("----------------------------------------");

        // purge old data
        info!("Purging old runs...");
        JobBuild::delete_all_orphan(database.get_mut().unwrap())?;

        info!("Purging extraneous tags...");
        TagInfo::delete_all_orphan(database.get_mut().unwrap())?;

        info!("Calculating issue similarities...");
        calculate_similarities(&database, config.threshold)?;
    } else {
        info!("No runs to process.");
    }

    info!("Done!");
    info!("----------------------------------------");

    let database = database.into_inner().unwrap();

    if let Some(output) = args.output {
        info!("Generating report...");

        let markup = page::render(
            &database,
            &config.view,
            UtcOffset::from_hms(config.timezone, 0, 0)?,
        )?
        .into_string();

        if let Some(filepath) = output {
            fs::write(&filepath, markup)?;

            info!("Written to {filepath}");
        } else {
            info!("Dumping to stdout --");
            println!("{markup}");
        }
    }

    info!("Done!");

    Ok(())
}
