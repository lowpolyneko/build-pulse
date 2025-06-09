use jenkins_api::{
    Jenkins,
    client::{Path, Result, TreeBuilder},
};

use crate::model::SparseMatrixProject;

pub fn pull_jobs(client: &Jenkins, project_name: &str) -> Result<SparseMatrixProject> {
    client.get_object_as(
        Path::View { name: project_name },
        TreeBuilder::new()
            .with_field(
                TreeBuilder::object("jobs")
                    .with_subfield("name")
                    .with_subfield("url")
                    .with_subfield(
                        TreeBuilder::object("lastBuild")
                            .with_subfield("url")
                            .with_subfield("displayName")
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
