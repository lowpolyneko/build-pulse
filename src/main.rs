//! A Jenkins CI/CD-based build analyzer and issue prioritizer.
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

use anyhow::{Error, Result};
use clap::{Parser, crate_name, crate_version};
use env_logger::Env;
use jenkins_api::{Jenkins, JenkinsBuilder, build::BuildStatus};
use log::{Level, info, log, warn};
use time::UtcOffset;
use tokio::{
    fs,
    task::{self, JoinSet},
};

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
            false => sj.as_job().upsert(db, ())?.into(),
            true => continue,
        };
        let (build, mut mbs): (Arc<_>, _) = match sj.last_build {
            Some(build @ SparseBuild { runs: Some(_), .. }) => (
                build.as_build(job.id).upsert(db, ())?.into(),
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

        mbs.try_for_each(|mb| match Run::select_one_by_url(db, &mb.url, ()) {
            Ok(run) => {
                runs.push(run);
                Ok(())
            } // cached
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
            Err(e) => Err(Error::from(e)),
        })?;
    }

    runs.reserve(handles.len());

    // collect them all here
    while let Some(h) = handles.join_next().await {
        runs.push(h?.upsert(db, ())?);
    }

    Ok(runs)
}

/// Parse all untagged runs for `tags` and cache them into database `db`
async fn parse_unprocessed_runs(
    runs: Vec<InDatabase<Run>>,
    tags: Arc<TagSet<InDatabase<Tag>>>,
    db: &Database,
) -> Result<Vec<InDatabase<Issue>>> {
    let mut handles = JoinSet::new();
    let mut inserted_issues = Vec::new();

    runs.into_iter().for_each(|run| match run.tag_schema {
        None => {
            let tags = tags.clone();
            handles.spawn_blocking(move || -> (_, Vec<_>) {
                let issues = {
                    let warn = |t: &InDatabase<Tag>| match t.severity {
                        Severity::Metadata => {}
                        _ => warn!(
                            "Found issue(s) tagged '{}' in run '{}'",
                            t.name, run.display_name
                        ),
                    };
                    let run_name = tags
                        .grep_tags(&run.display_name, Field::RunName)
                        .flat_map(|t| t.grep_issue(&run.display_name));
                    let console = run.log.iter().flat_map(|l| {
                        tags.grep_tags(l, Field::Console).flat_map(|t| {
                            warn(t);
                            t.grep_issue(l)
                        })
                    });

                    run_name.chain(console).collect()
                };

                (run, issues)
            });
        }
        _ => inserted_issues.extend(
            Issue::select_all_not_metadata(db, (db, &run))
                .into_iter()
                .flatten(),
        ),
    });

    while let Some(h) = handles.join_next().await {
        let (run, issues) = h?;
        inserted_issues = issues.into_iter().try_fold(inserted_issues, |mut acc, i| {
            let issue = i.insert(db, (&run,))?;
            match TagInfo::select_one(db, issue.tag_id, ())?.severity {
                Severity::Metadata => {}
                _ => acc.push(issue),
            }
            Ok::<_, Error>(acc)
        })?;
    }

    // batch update tag schema for runs afterwards
    Run::update_all_tag_schema(db, Some(tags.schema()))?;
    Ok(inserted_issues)
}

/// Calculate similarities against all issues and soft insert the groupings into [Database]
async fn calculate_similarities(
    issues: Vec<InDatabase<Issue>>,
    threshold: f32,
    db: &Database,
) -> Result<()> {
    // conservatively group by levenshtein distance
    let mut groups: Vec<Vec<Arc<InDatabase<Issue>>>> = Vec::new();
    for issue in issues.into_iter().map(Arc::new) {
        let mut handles: JoinSet<_> = groups
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, g)| {
                let issue = issue.clone();
                async move {
                    let mut inner = JoinSet::new();
                    g.into_iter().for_each(|issue2| {
                        let issue = issue.clone();
                        inner.spawn_blocking(move || {
                            normalized_levenshtein_distance(&issue.snippet, &issue2.snippet)
                                > threshold
                        });
                    });

                    while matches!(inner.join_next().await, Some(Ok(true))) {}
                    match inner.is_empty() {
                        true => Some(i),
                        false => None,
                    }
                }
            })
            .collect();

        while let Some(h) = handles.join_next().await {
            if let Some(i) = h? {
                groups[i].push(issue.clone());
                break;
            }
        }
        if handles.is_empty() {
            groups.push(vec![issue.clone()]);
        }
    }

    // sort resultant groups
    let mut handles = JoinSet::new();
    for mut g in groups {
        handles.spawn_blocking(|| {
            g.sort();

            let mut hasher = DefaultHasher::new();
            g.hash(&mut hasher);
            (hasher.finish(), g)
        });
    }

    // store relations in database
    while let Some(h) = handles.join_next().await {
        let (hash, g) = h?;
        // unique issues are discarded
        if g.len() > 1 {
            g.iter().try_for_each(|i| {
                SimilarityInfo {
                    similarity_hash: hash,
                    issue_id: i.id,
                }
                .insert(db, ())?;

                info!(
                    "Issue '#{}' likely matches with similarity group '#{}'!",
                    i.id, hash
                );

                Ok::<_, Error>(())
            })?;
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // initialize logging
    env_logger::init_from_env(Env::default().default_filter_or("info"));
    info!("{} {}", crate_name!(), crate_version!());

    // load config
    info!("Compiling issue patterns...");
    let Config {
        blocklist,
        database,
        jenkins_url,
        password,
        project,
        tag,
        threshold,
        timezone,
        username,
        view,
    } = toml::from_str(&fs::read_to_string(args.config).await?)?;
    let tags = TagSet::from_config(tag)?;

    // open db
    info!("Opening database...");
    let mut database = Database::open(&database)?;

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
    let blocked = Job::delete_all_by_blocklist(&mut database, &blocklist)?;
    if blocked > 0 {
        warn!("Purged {blocked} jobs that are on the blocklist.");
    }

    info!(
        "Pulling associated jobs for {} from {}...",
        project, jenkins_url
    );

    let jenkins = JenkinsBuilder::new(&jenkins_url);
    let jenkins = match username {
        Some(user) => jenkins.with_user(&user, password.as_deref()),
        None => jenkins,
    }
    .build()
    .map_err(Error::from_boxed)?;

    info!("Pulling build info for each job...");
    info!("----------------------------------------");

    let project = SparseMatrixProject::pull_jobs(&jenkins, &project).await?;
    let runs = pull_build_logs(project, &blocklist, jenkins.into(), &database).await?;

    info!("Done!");
    info!("----------------------------------------");

    if Run::has_untagged(&database)? {
        info!("Parsing unprocessed run logs...");
        let issues = parse_unprocessed_runs(runs, tags.into(), &database).await?;

        info!("Done!");
        info!("----------------------------------------");

        // purge old data
        info!("Purging old runs...");

        JobBuild::delete_all_orphan(&database)?;

        info!("Purging extraneous tags...");
        TagInfo::delete_all_orphan(&database)?;

        info!("Calculating issue similarities...");
        calculate_similarities(issues, threshold, &database).await?;
    } else {
        info!("No runs to process.");
    }

    info!("Done!");
    info!("----------------------------------------");

    if let Some(output) = args.output {
        info!("Generating report...");

        let markup = task::spawn_blocking(move || {
            page::render(
                &database,
                &view,
                UtcOffset::from_hms(timezone, 0, 0).unwrap(),
            )
            .unwrap()
            .into_string()
        })
        .await?;

        if let Some(filepath) = output {
            fs::write(&filepath, markup).await?;

            info!("Written to {filepath}");
        } else {
            info!("Dumping to stdout --");
            println!("{markup}");
        }
    }

    info!("Done!");

    Ok(())
}
