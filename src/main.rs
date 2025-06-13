#![feature(reentrant_lock)]

use std::{fs, sync::ReentrantLock};

use anyhow::{Error, Result};
use clap::{Parser, crate_name, crate_version};
use env_logger::Env;
use jenkins_api::{JenkinsBuilder, build::BuildStatus};
use log::{info, warn};
use rayon::prelude::*;

use crate::{
    api::{SparseMatrixProject, ToLog},
    config::Config,
    db::Database,
    parse::{IssuePatterns, Parse},
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
    output: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // initialize logging
    env_logger::init_from_env(Env::default().default_filter_or("info"));
    info!("{} {}", crate_name!(), crate_version!());

    // load config
    info!("Compiling issue patterns...");
    let config: Config = toml::from_str(fs::read_to_string(args.config)?.as_str())?;
    let issue_patterns = IssuePatterns::load_regex(&config.issue)?;

    // open db
    info!("Opening database...");
    let database = ReentrantLock::new(Database::open(&config.database)?);

    info!(
        "Pulling associated jobs for {} from {}...",
        config.project, config.jenkins_url
    );

    let jenkins = JenkinsBuilder::new(&config.jenkins_url);
    let jenkins = match config.username {
        Some(user) => jenkins.with_user(user.as_str(), config.password.as_deref()),
        None => jenkins,
    }
    .build()
    .map_err(Error::from_boxed)?;
    let project = SparseMatrixProject::pull_jobs(&jenkins, &config.project)?;

    info!("Pulling build info for each job...");
    info!("----------------------------------------");

    project.jobs.iter().try_for_each(|job| {
        info!("Job {}", job.name);
        if let Some(build) = &job.last_build {
            info!("Last build for job {} is {}", job.name, build.display_name);
            build.runs.par_iter().try_for_each(|mb| {
                let x = mb.get_full_build(&jenkins).map_err(Error::from_boxed)?;
                if let Some(log) = match x.result {
                    Some(BuildStatus::Failure) => match database.lock().get_log(&x.url) {
                        Ok(log) => {
                            info!("Cached log");
                            Some(log)
                        }
                        Err(rusqlite::Error::QueryReturnedNoRows) => {
                            warn!("Fresh log, grabbing...");
                            let log = database.lock().insert_log(x.to_log(&jenkins)?)?;
                            log.grep_issues(&issue_patterns).try_for_each(|i| {
                                database.lock().insert_issue(&log, i)?;

                                Ok::<_, Error>(())
                            })?;
                            Some(log)
                        }
                        _ => panic!("Failed to get log"),
                    },
                    Some(BuildStatus::Success) => {
                        info!("Run is ok.");
                        None
                    }
                    Some(BuildStatus::Aborted | BuildStatus::NotBuilt) | None => {
                        info!("Run not finished.");
                        None
                    }
                    Some(BuildStatus::Unstable) => {
                        warn!("Run has runtime errors!");
                        None
                    }
                } {
                    // Get cached issues
                    warn!(
                        "Run failed with {} found issues!",
                        database.lock().get_issues(&log)?.len()
                    )
                }

                Ok::<_, Error>(())
            })?;
            info!("----------------------------------------");
        }

        Ok::<_, Error>(())
    })?;

    info!("Generating report...");

    if let Some(filepath) = args.output {
        let output = page::render(&project).into_string();

        if filepath.is_empty() {
            info!("Dumping to stdout --");
            println!("{output}");
        } else {
            fs::write(&filepath, output)?;

            info!("Written to {filepath}");
        }
    }

    Ok(())
}
