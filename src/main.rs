use jenkins_api::JenkinsBuilder;

fn main() {
    let jenkins = JenkinsBuilder::new("https://jenkins-pmrs.cels.anl.gov/")
        .build()
        .expect("failed to query Jenkins");

    let job = jenkins
        .get_job("mpich-main-32-bit")
        .expect("failed to get job");

    let build = job
        .last_build
        .expect("failed to get last build")
        .get_full_build(&jenkins)
        .expect("failed to get details for last build");

    println!(
        "last build for job {} at {} was {:?}",
        job.name, build.timestamp, build.result
    );
}
