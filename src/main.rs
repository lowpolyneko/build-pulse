use clap::Parser;

mod api;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    project: String,
}

fn main() {
    let args = Args::parse();

    // api::pull_jobs(&args.project);
}
