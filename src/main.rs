use std::{fs, sync::Mutex};

use anyhow::{Error, Result};
use clap::{Parser, crate_name, crate_version};
use env_logger::Env;
use jenkins_api::{Jenkins, JenkinsBuilder, build::BuildStatus};
use log::{Level, info, log, warn};
use rayon::prelude::*;

use crate::{
    api::{AsRun, SparseMatrixProject},
    config::Config,
    db::{Database, InDatabase},
    parse::{Tag, TagSet},
};

mod api;
mod config;
mod db;
mod page;
mod parse;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(default_value = "config.toml")]
    config: String,

    #[arg(short, long)]
    output: Option<Option<String>>,

    #[arg(short, long)]
    purge_cache: bool,
}

fn pull_build_logs(
    project: &SparseMatrixProject,
    jenkins: &Jenkins,
    db: &Mutex<Database>,
) -> Result<()> {
    project
        .jobs
        .par_iter()
        .filter_map(|job| {
            job.last_build.as_ref().map_or_else(
                || {
                    info!("Job '{}' has no builds.", job.name);
                    None
                },
                |build| Some((job, build)),
            )
        })
        .flat_map(|(job, build)| rayon::iter::repeat((job, build)).zip(&build.runs))
        .filter(|((_, build), mb)| mb.number == build.number)
        .try_for_each(|((job, build), mb)| {
            let existing_run = db.lock().unwrap().get_run(&mb.url);
            match existing_run {
                // check if already processed first
                Ok(_) => Ok(()), // cached
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    // retrieve the log
                    let full_mb = mb.get_full_build(jenkins).map_err(Error::from_boxed)?;
                    let run = full_mb.as_run(jenkins)?;
                    let committed_run = db.lock().unwrap().insert_run(run)?;

                    log!(
                        match committed_run.status {
                            Some(BuildStatus::Failure | BuildStatus::Unstable) => Level::Warn,
                            _ => Level::Info,
                        },
                        "Job '{}{}' run '{}' finished with status {:?}.",
                        job.name,
                        build.display_name,
                        committed_run.display_name,
                        committed_run.status
                    );

                    Ok(())
                }
                Err(e) => Err(Error::from(e)),
            }
        })
}

fn parse_unprocessed_logs(tags: &TagSet<InDatabase<Tag>>, db: &Mutex<Database>) -> Result<()> {
    let runs = db.lock().unwrap().get_all_runs()?;

    runs.par_iter()
        .try_for_each(|run| match (run.tag_schema, &run.log) {
            (None, Some(log)) => {
                let count: u64 = tags // only process runs with no schema and a log
                    .grep_tags(log)
                    .flat_map(|t| t.grep_issue(&log))
                    .try_fold(0, |acc, i| {
                        db.lock().unwrap().insert_issue(&run, i)?;
                        Ok::<_, Error>(acc + 1)
                    })?;

                log!(
                    if count > 0 { Level::Warn } else { Level::Info },
                    "Found {} issues for {:?} run '{}'",
                    count,
                    run.status,
                    run.display_name
                );

                Ok::<_, Error>(())
            }
            _ => Ok(()),
        })?;

    // batch update tag schema for runs afterwards
    db.lock()
        .unwrap()
        .update_tag_schema_for_runs(Some(tags.schema()))?;

    Ok(())
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
    let tags = database.set_tags(tags)?;

    // purge outdated issues
    let outdated = database.purge_invalid_issues_by_tag_schema(tags.schema())?;
    if outdated > 0 {
        warn!(
            "Purged {} issues that parsed with an outdated tag schema!",
            outdated
        );
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

    let database = Mutex::new(database);
    pull_build_logs(&project, &jenkins, &database)?;

    info!("Done!");
    info!("----------------------------------------");
    info!("Parsing unprocessed run logs...");

    parse_unprocessed_logs(&tags, &database)?;

    info!("Done!");
    info!("----------------------------------------");

    let database = database.into_inner().unwrap();

    if let Some(output) = args.output {
        info!("Generating report...");

        let markup = page::render(&project, &database).into_string();

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
