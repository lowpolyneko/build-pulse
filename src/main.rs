//! A Jenkins CI/CD-based build analyzer and issue prioritizer.
use std::{
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Mutex,
};

use anyhow::{Error, Result};
use clap::{Parser, crate_name, crate_version};
use env_logger::Env;
use jenkins_api::{Jenkins, JenkinsBuilder, build::BuildStatus};
use log::{Level, info, log, warn};
use rayon::prelude::*;
use time::UtcOffset;

use crate::{
    api::{AsBuild, AsJob, AsRun, SparseMatrixProject},
    config::{Config, Field, Severity},
    db::{Database, InDatabase, Issue},
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
fn pull_build_logs(
    project: &SparseMatrixProject,
    blocklist: &[String],
    jenkins: &Jenkins,
    db: &Mutex<Database>,
) -> Result<()> {
    project
        .jobs
        .par_iter()
        .filter(|job| !blocklist.contains(&job.name))
        .filter_map(|job| {
            // filter out jobs with no builds
            job.last_build.as_ref().map_or_else(
                || {
                    info!("Job '{}' has no builds.", job.name);
                    None
                },
                |build| Some((job, build)),
            )
        })
        .filter_map(|(job, build)| {
            build
                .runs
                .as_ref()
                .map(|r| r.par_iter().map(move |mb| (job, build, mb)))
        })
        .flatten()
        .filter(|(_, build, mb)| mb.number == build.number) // filter out runs w/o matching build
        .map(|(job, build, mb)| {
            Ok((
                db.lock()
                    .unwrap()
                    .upsert_job(job.as_job())
                    .map_err(Error::from)?,
                build,
                mb,
            ))
        })
        .map(|res| match res {
            Ok((db_job, build, mb)) => {
                let job_id = db_job.id;
                Ok((
                    db_job,
                    db.lock()
                        .unwrap()
                        .upsert_build(build.as_build(job_id))
                        .map_err(Error::from)?,
                    mb,
                ))
            }
            Err(e) => Err(e),
        })
        .filter_map(|res| match res {
            Ok((db_job, db_build, mb)) => match db.lock().unwrap().get_run_by_url(&mb.url) {
                Ok(_) => None, // cached
                Err(rusqlite::Error::QueryReturnedNoRows) => Some(Ok((db_job, db_build, mb))),
                Err(e) => Some(Err(Error::from(e))),
            },
            Err(e) => Some(Err(e)),
        })
        .map(|res| match res {
            Ok((db_job, db_build, mb)) => Ok((
                db_job,
                db_build,
                mb.get_full_build(jenkins).map_err(Error::from_boxed)?,
            )),
            Err(e) => Err(e),
        })
        .map(|res| match res {
            Ok((db_job, db_build, full_mb)) => {
                let build_id = db_build.id;
                Ok((db_job, db_build, full_mb.as_run(build_id, jenkins))) // build log is pulled here
            }
            Err(e) => Err(e),
        })
        .map(|res| match res {
            Ok((db_job, db_build, run)) => {
                Ok((db_job, db_build, db.lock().unwrap().upsert_run(run)?))
            }
            Err(e) => Err(e),
        })
        .try_for_each(|res| match res {
            Ok((db_job, db_build, db_run)) => {
                log!(
                    match db_run.status {
                        Some(BuildStatus::Failure | BuildStatus::Unstable) => Level::Warn,
                        _ => Level::Info,
                    },
                    "Job '{}#{}' run '{}' finished with status {:?}.",
                    db_job.name,
                    db_build.number,
                    db_run.display_name,
                    db_run.status
                );

                Ok(())
            }
            Err(e) => Err(e),
        })
}

/// Parse all untagged runs for `tags` and cache them into database `db`
fn parse_unprocessed_runs(tags: &TagSet<InDatabase<Tag>>, db: &Mutex<Database>) -> Result<()> {
    let runs = db.lock().unwrap().get_runs()?;

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
            let i = db.lock().unwrap().insert_issue(run, i)?;

            if !matches!(t.severity, Severity::Metadata) {
                warn!(
                    "Found issue '#{}' tagged '{}' in run '{}'",
                    i.id, t.name, run.display_name
                );
            }

            Ok::<_, Error>(())
        })?;

    // batch update tag schema for runs afterwards
    db.lock()
        .unwrap()
        .update_tag_schema_for_runs(Some(tags.schema()))?;

    Ok(())
}

/// Calculate similarities against all issues and soft insert the groupings into [Database]
fn calculate_similarities(db: &Mutex<Database>) -> Result<()> {
    let runs = db.lock().unwrap().get_runs()?;

    // conservatively group by levenshtein distance
    let mut groups: Vec<Vec<InDatabase<Issue>>> = Vec::new();
    runs.iter()
        .filter_map(|r| db.lock().unwrap().get_issues(r, false).ok())
        .flatten()
        .for_each(|i| {
            match groups.par_iter_mut().find_any(|g| {
                g.par_iter()
                    .all(|i2| normalized_levenshtein_distance(i.snippet, i2.snippet) > 0.9)
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
            db.lock().unwrap().insert_similarity(hash, i)?;
            info!(
                "Issue '#{}' likely matches with similarity group '#{}'!",
                i.id, hash
            );

            Ok(())
        })
}

fn main() -> Result<()> {
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
    let tags = database.upsert_tags(tags)?;

    // purge outdated issues
    let outdated = database.purge_invalid_issues_by_tag_schema(tags.schema())?;
    if outdated > 0 {
        warn!("Purged {outdated} runs' issues that parsed with an outdated tag schema!");
    }

    // purge blocklisted jobs
    let blocked = database.purge_blocklisted_jobs(&config.blocklist)?;
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

    let project = SparseMatrixProject::pull_jobs(&jenkins, &config.project)?;

    let mut database = Mutex::new(database);
    pull_build_logs(&project, &config.blocklist, &jenkins, &database)?;

    info!("Done!");
    info!("----------------------------------------");

    if database.get_mut().unwrap().has_untagged_runs()? {
        info!("Parsing unprocessed run logs...");
        parse_unprocessed_runs(&tags, &database)?;

        info!("Done!");
        info!("----------------------------------------");

        // purge old data
        info!("Purging old runs...");
        database.get_mut().unwrap().purge_old_builds()?;

        info!("Purging extraneous tags...");
        database.get_mut().unwrap().purge_orphan_tags()?;

        info!("Calculating issue similarities...");
        calculate_similarities(&database)?;
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
