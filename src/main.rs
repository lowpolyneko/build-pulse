use jenkins_api::{
    JenkinsBuilder,
    build::BuildStatus,
    client::{Path, TreeBuilder},
};
use serde::Deserialize;

#[derive(Deserialize)]
struct ViewJobs {
    jobs: Vec<ViewJob>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewJob {
    name: String,
    last_build: Option<ViewBuild>,
}

#[derive(Debug, Deserialize)]
struct ViewBuild {
    timestamp: u64,
    result: Option<BuildStatus>,
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
            TreeBuilder::new()
                .with_field(
                    TreeBuilder::object("jobs")
                        .with_subfield("name")
                        .with_subfield(
                            TreeBuilder::object("lastBuild")
                                .with_subfield("timestamp")
                                .with_subfield("result"),
                        ),
                )
                .build(),
        )
        .expect("failed to query jobs");

    for job in view.jobs {
        println!("last build for job {} is {:?}", job.name, job.last_build);
    }
}
