use std::{error::Error, fs::File, io::Write};

use clap::Parser;
use jenkins_api::{
    JenkinsBuilder,
    build::{Build, BuildStatus},
};
use log::{info, warn};

mod api;
mod model;
mod page;
mod parse;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(default_value = "https://jenkins-pmrs.cels.anl.gov")]
    jenkins_url: String,

    #[arg(default_value = "mpich-main-nightly")]
    project: String,

    #[arg(default_value = "report.html")]
    output: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    // initialize logging
    env_logger::init();

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
                    parse::grep_issues(&console)?;
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

    let mut report = File::create(args.output)?;
    report.write(page::render(&project).into_string().as_bytes())?;

    Ok(())
}
