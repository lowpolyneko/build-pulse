use std::{fs, sync::Mutex};

use anyhow::{Error, Result};
use clap::{Parser, crate_name, crate_version};
use env_logger::Env;
use jenkins_api::{Jenkins, JenkinsBuilder, build::BuildStatus};
use log::{Level, info, log};
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
}

fn pull_build_logs(
    project: &SparseMatrixProject,
    tags: &TagSet<InDatabase<Tag>>,
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
        .try_for_each(|((job, build), mb)| {
            // check if processed first
            let existing_run = db.lock().unwrap().get_run(&mb.url);
            match existing_run {
                // check if already processed first
                Ok(r) => {
                    log!(
                        match r.status {
                            Some(BuildStatus::Failure | BuildStatus::Unstable) => Level::Warn,
                            _ => Level::Info,
                        },
                        "Job '{}{}' run '{}' already cached with status {:?}",
                        job.name,
                        build.display_name,
                        r.display_name,
                        r.status
                    );

                    Ok::<_, Error>(())
                } // cached
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    // retrieve the log and parse the issues
                    let full_mb = mb.get_full_build(jenkins).map_err(Error::from_boxed)?;
                    let run = full_mb.as_run(jenkins)?;
                    let committed_run = db.lock().unwrap().insert_run(run)?;
                    if let Some(failure_log) = &committed_run.log {
                        tags.grep_tags(failure_log)
                            .flat_map(|t| t.grep_issue(&failure_log))
                            .try_for_each(|i| {
                                db.lock().unwrap().insert_issue(&committed_run, i)?;

                                Ok::<_, Error>(())
                            })?;
                    };

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

                    Ok::<_, Error>(())
                }
                Err(e) => Err(Error::from(e)),
            }
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
    let database = Database::open(&config.database)?;

    // update TagSet
    info!("Updating tags...");
    let tags = database.insert_tags(tags)?;

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
    pull_build_logs(&project, &tags, &jenkins, &database)?;
    let database = database.into_inner().unwrap();

    info!("----------------------------------------");

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
