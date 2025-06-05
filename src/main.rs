use jenkins_api::{
    JenkinsBuilder,
    build::{Build, BuildStatus, ShortBuild},
    client::{Path, TreeBuilder},
    job::CommonJob,
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
#[allow(dead_code)]
struct ViewBuild {
    url: String,
    timestamp: u64,
    result: Option<BuildStatus>,
    runs: Vec<ShortBuild>,
}

impl Build for ViewBuild {
    type ParentJob = CommonJob;

    fn url(&self) -> &str {
        self.url.as_str()
    }
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
                                .with_subfield("url")
                                .with_subfield("timestamp")
                                .with_subfield("result")
                                .with_subfield(
                                    TreeBuilder::object("runs")
                                        .with_subfield("url")
                                        .with_subfield("number"),
                                ),
                        ),
                )
                .build(),
        )
        .expect("failed to query jobs");

    for job in view.jobs {
        println!("last build for job {} is {:?}", job.name, job.last_build);
        if let Some(build) = job.last_build {
            build.runs.iter().for_each(|mb| {
                println!(
                    "{:?}",
                    match mb.get_full_build(&jenkins) {
                        Ok(x) => x.get_console(&jenkins),
                        Err(x) => Err(x),
                    }
                )
            });
        }
    }
}
