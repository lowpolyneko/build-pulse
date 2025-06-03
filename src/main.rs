use jenkins_api::{
    JenkinsBuilder,
    client::{AdvancedQuery, Path},
    job::CommonJob,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct ViewJobs {
    jobs: Vec<CommonJob>,
}

fn main() {
    let jenkins = JenkinsBuilder::new("https://jenkins-pmrs.cels.anl.gov/")
        .build()
        .expect("failed to query Jenkins");

    let view: ViewJobs = jenkins
        .get_object_as(
            Path::View {
                name: "mpich-main-nightly",
            },
            AdvancedQuery::Depth(1),
        )
        .expect("failed to query jobs");

    for job in view.jobs {
        println!(
            "{} {}",
            job.name,
            job.last_build
                .map_or(String::from("null"), |b| format!("#{}", b.number))
        );
    }
    // let view = jenkins
    //     .get_view("mpich-main-nightly")
    //     .expect("failed to get view");
    //
    // for job in view.jobs {
    //     let build = job
    //         .get_full_job(&jenkins)
    //         .expect("failed to get jobs")
    //         .last_build
    //         .expect("failed to get last build")
    //         .get_full_build(&jenkins)
    //         .expect("failed to get details for last build");
    //
    //     println!(
    //         "last build for job {} at {} was {:?}",
    //         job.name, build.timestamp, build.result
    //     );
    // }
}
