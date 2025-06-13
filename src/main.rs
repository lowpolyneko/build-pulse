#![feature(reentrant_lock)]

use std::{fs, sync::ReentrantLock};

use anyhow::{Error, Result, bail};
use clap::{Parser, crate_name, crate_version};
use env_logger::Env;
use jenkins_api::{Jenkins, JenkinsBuilder, build::BuildStatus};
use log::{info, warn};
use rayon::prelude::*;

use crate::{
    api::{AsRun, HasBuildFields, SparseMatrixProject},
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

fn pull_build_logs(
    project: &SparseMatrixProject,
    patterns: &IssuePatterns,
    jenkins: &Jenkins,
    db: &ReentrantLock<Database>,
) -> Result<()> {
    project
        .jobs
        .par_iter()
        .try_for_each(|job| match &job.last_build {
            Some(build) => build.runs.par_iter().try_for_each(|mb| {
                let full_mb = mb.get_full_build(jenkins).map_err(Error::from_boxed)?;

                match full_mb.result {
                    Some(BuildStatus::Failure) => match db.lock().get_run(&full_mb.url) {
                        Ok(_) => {
                            warn!(
                                "Job '{}{}' run '{}' failed and was previously cached.",
                                job.name,
                                build.display_name,
                                full_mb.full_display_name_or_default()
                            );
                            Ok(())
                        }
                        Err(rusqlite::Error::QueryReturnedNoRows) => {
                            warn!(
                                "Job '{}{}' run '{}' is a fresh failure. Processing log...",
                                job.name,
                                build.display_name,
                                full_mb.full_display_name_or_default()
                            );
                            let run = db.lock().insert_run(full_mb.as_run(jenkins)?)?;
                            run.grep_issues(patterns).try_for_each(|i| {
                                db.lock().insert_issue(&run, i)?;

                                Ok(())
                            })
                        }
                        _ => bail!(
                            "Failed to query database for Job '{}{}' run '{}' log.",
                            job.name,
                            build.display_name,
                            full_mb.full_display_name_or_default()
                        ),
                    },
                    Some(BuildStatus::Success) => {
                        info!(
                            "Job '{}{}' run '{}' succeeded.",
                            job.name,
                            build.display_name,
                            full_mb.full_display_name_or_default()
                        );
                        Ok::<_, Error>(())
                    }
                    Some(BuildStatus::Aborted | BuildStatus::NotBuilt) | None => {
                        info!(
                            "Job '{}{}' run '{}' not ran.",
                            job.name,
                            build.display_name,
                            full_mb.full_display_name_or_default()
                        );
                        Ok::<_, Error>(())
                    }
                    Some(BuildStatus::Unstable) => {
                        warn!(
                            "Job '{}{}' run '{}' has runtime errors!",
                            job.name,
                            build.display_name,
                            full_mb.full_display_name_or_default()
                        );
                        Ok::<_, Error>(())
                    }
                }
            }),
            None => {
                info!("Job '{}' has no builds.", job.name);
                Ok(())
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
    let config: Config = toml::from_str(fs::read_to_string(args.config)?.as_str())?;
    let issue_patterns = IssuePatterns::load_regex(&config.issue)?;

    // open db
    info!("Opening database...");
    let database = Database::open(&config.database)?;

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

    info!("Pulling build info for each job...");
    info!("----------------------------------------");

    let project = SparseMatrixProject::pull_jobs(&jenkins, &config.project)?;

    let database = ReentrantLock::new(database);
    pull_build_logs(&project, &issue_patterns, &jenkins, &database)?;
    let database = database.into_inner();

    info!("----------------------------------------");

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
