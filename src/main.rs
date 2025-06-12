use std::{error::Error, fs, sync::Mutex};

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

fn main() -> Result<(), Box<dyn Error>> {
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
    let database = Mutex::new(Database::open(&config.database)?);

    info!(
        "Pulling associated jobs for {} from {}...",
        config.project, config.jenkins_url
    );

    let jenkins = match config.username {
        Some(user) => JenkinsBuilder::new(&config.jenkins_url)
            .with_user(user.as_str(), config.password.as_deref())
            .build()?,
        None => JenkinsBuilder::new(&config.jenkins_url).build()?,
    };
    let project = SparseMatrixProject::pull_jobs(&jenkins, &config.project)?;

    info!("Pulling build info for each job...");
    info!("----------------------------------------");

    project.jobs.iter().for_each(|job| {
        info!("Job {}", job.name);
        if let Some(build) = &job.last_build {
            info!("Last build for job {} is {}", job.name, build.display_name);
            build.runs.par_iter().for_each(|mb| {
                let x = mb.get_full_build(&jenkins).expect("Failed to get build");
                if let Some(log) = match x.result {
                    Some(BuildStatus::Failure) => match database.lock().unwrap().get_log(&x.url) {
                        Ok(log) => {
                            info!("Cached log");
                            Some(log)
                        }
                        Err(rusqlite::Error::QueryReturnedNoRows) => {
                            warn!("Fresh log, grabbing...");
                            let log = database
                                .lock()
                                .unwrap()
                                .insert_log(x.to_log(&jenkins).expect("Failed to download log"))
                                .expect("Failed to insert log");
                            log.grep_issues(&issue_patterns).for_each(|i| {
                                database
                                    .lock()
                                    .unwrap()
                                    .insert_issue(&log, i)
                                    .expect("Failed to insert issue");
                            });
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
                        database
                            .lock()
                            .unwrap()
                            .get_issues(&log)
                            .expect("Failed to select issues")
                            .len()
                    );
                }
            });
            info!("----------------------------------------");
        }
    });

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
