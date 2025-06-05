use jenkins_api::{
    JenkinsBuilder,
    client::{Path, Result, TreeBuilder},
};

use crate::model::SparseMatrixProject;

pub fn pull_jobs(url: &str, project_name: &str) -> Result<SparseMatrixProject> {
    let jenkins = JenkinsBuilder::new(url).build()?;

    jenkins.get_object_as(
        Path::View { name: project_name },
        TreeBuilder::new()
            .with_field(
                TreeBuilder::object("jobs")
                    .with_subfield("name")
                    .with_subfield("url")
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
}
