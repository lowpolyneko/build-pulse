use clap::Parser;

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
}

fn main() {
    let args = Args::parse();

    let project = api::pull_jobs(&args.jenkins_url, &args.project).expect("failed to pull jobs");

    // for job in view.jobs {
    //     println!("last build for job {} is {:?}", job.name, job.last_build);
    //     if let Some(build) = job.last_build {
    //         build.runs.iter().for_each(|mb| {
    //             println!(
    //                 "{:?}",
    //                 match mb.get_full_build(&jenkins) {
    //                     Ok(x) => x.get_console(&jenkins),
    //                     Err(x) => Err(x),
    //                 }
    //             )
    //         });
    //     }
    // }

    println!("{}", page::render(&project).into_string());
}
