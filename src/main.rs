use std::{error::Error, fs};

use clap::{Parser, crate_name, crate_version};
use env_logger::Env;
use jenkins_api::{
    JenkinsBuilder,
    build::{Build, BuildStatus},
};
use log::{info, warn};

use crate::{config::Config, parse::load_regex};

mod api;
mod config;
mod page;
mod parse;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(default_value = "https://jenkins-pmrs.cels.anl.gov")]
    jenkins_url: String,

    #[arg(default_value = "mpich-main-nightly")]
    project: String,

    #[arg(short, long, default_value = "config.toml")]
    config: String,

    #[arg(short, long)]
    output: Option<String>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    // initialize logging
    env_logger::init_from_env(Env::new().filter(format!("{}=trace", crate_name!())));
    info!("{} {}", crate_name!(), crate_version!());

    // load config
    info!("Compiling issue patterns...");
    let config: Config = toml::from_str(fs::read_to_string(args.config)?.as_str())?;
    let issue_patterns = load_regex(&config.issue)?;

    info!(
        "Pulling associated jobs for {} from {}...",
        args.project, args.jenkins_url
    );

    let jenkins = JenkinsBuilder::new(args.jenkins_url.as_str()).build()?;
    let project = api::pull_jobs(&jenkins, &args.project)?;

    info!("Pulling build info for each job...");
    info!("----------------------------------------");

    project.jobs.iter().try_for_each(|job| {
        info!("Job {}", job.name);
        if let Some(build) = &job.last_build {
            info!("Last build for job {} is {}", job.name, build.display_name);
            build.runs.iter().try_for_each(|mb| {
                let x = mb.get_full_build(&jenkins)?;
                if let Some(console) = match x.result {
                    Some(BuildStatus::Failure) => x.get_console(&jenkins).ok(),
                    _ => None,
                } {
                    warn!("{}", "Run failed!");
                    warn!("{}", console);
                    parse::grep_issues(&issue_patterns, &console);
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
            println!("{}", output);
        } else {
            fs::write(&filepath, output)?;

            info!("Written to {}", filepath);
        }
    }

    Ok(())
}
