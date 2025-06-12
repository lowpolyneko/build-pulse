use std::{error::Error, fs};

use clap::{Parser, crate_name, crate_version};
use env_logger::Env;
use jenkins_api::{JenkinsBuilder, build::BuildStatus};
use log::{info, warn};

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
    let database = Database::open(&config.database)?;

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

    project.jobs.iter().try_for_each(|job| {
        info!("Job {}", job.name);
        if let Some(build) = &job.last_build {
            info!("Last build for job {} is {}", job.name, build.display_name);
            build.runs.iter().try_for_each(|mb| {
                let x = mb.get_full_build(&jenkins)?;
                if let Some(log) = match x.result {
                    Some(BuildStatus::Failure) => x.to_log(&jenkins).ok(),
                    _ => None,
                } {
                    warn!("{}", "Run failed!");
                    warn!("{}", log.data);
                    let committed_log = database.insert_log(log)?;
                    let issues: Vec<_> = committed_log.grep_issues(&issue_patterns).collect();
                    issues
                        .into_iter()
                        .map(|i| database.insert_issue(&committed_log, i))
                        .try_for_each(|issue| {
                            info!("START MATCH ----------");
                            info!("{}", issue?.snippet);
                            info!("END MATCH ------------");

                            Ok::<(), Box<dyn Error>>(())
                        })?;
                } else {
                    info!("Run is okay");
                }

                Ok::<(), Box<dyn Error>>(())
            })?;
            info!("----------------------------------------");
        }

        Ok::<(), Box<dyn Error>>(())
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
