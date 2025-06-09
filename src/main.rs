use std::{fs::File, io::Write};

use clap::Parser;
use jenkins_api::{
    JenkinsBuilder,
    build::{Build, BuildStatus},
};

mod api;
mod model;
mod page;

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

fn main() {
    let args = Args::parse();

    let jenkins = JenkinsBuilder::new(args.jenkins_url.as_str())
        .build()
        .expect("failed to connect to Jenkins");
    let project = api::pull_jobs(&jenkins, &args.project).expect("failed to pull jobs");

    for job in &project.jobs {
        println!("last build for job {} is {:?}", job.name, job.last_build);
        if let Some(build) = &job.last_build {
            build.runs.iter().for_each(|mb| {
                if let Some(console) = match mb.get_full_build(&jenkins) {
                    Ok(x) => match x.result {
                        Some(BuildStatus::Failure) => x.get_console(&jenkins).ok(),
                        _ => None,
                    },
                    Err(_) => None,
                } {
                    println!("{}", console);
                } else {
                    println!("no build log available");
                }
            });
            println!("----------------------------------------");
        }
    }

    let mut report = File::create(args.output).expect("failed to open output report file");
    report
        .write(page::render(&project).into_string().as_bytes())
        .expect("failed to write to output report file");
}
