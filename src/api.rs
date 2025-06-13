use anyhow::{Error, Result};
use jenkins_api::{
    Jenkins,
    build::{Build, BuildStatus, ShortBuild},
    client::{Path, TreeBuilder},
    job::Job,
};
use serde::Deserialize;

use crate::db::Log;

#[derive(Deserialize)]
pub struct SparseMatrixProject {
    pub jobs: Vec<SparseJob>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SparseJob {
    pub name: String,
    pub url: String,
    pub last_build: Option<SparseBuild>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SparseBuild {
    pub url: String,
    pub display_name: String,
    pub timestamp: u64,
    pub result: Option<BuildStatus>,
    pub runs: Vec<ShortBuild>,
}

pub trait ToLog {
    fn to_log(&self, jenkins_client: &Jenkins) -> Result<Log>;
}

impl Job for SparseJob {
    fn name(&self) -> &str {
        self.name.as_str()
    }
    fn url(&self) -> &str {
        self.url.as_str()
    }
}

impl Build for SparseBuild {
    type ParentJob = SparseJob;

    fn url(&self) -> &str {
        self.url.as_str()
    }
}

impl<T> ToLog for T
where
    T: Build,
{
    fn to_log(&self, jenkins_client: &Jenkins) -> Result<Log> {
        Ok(Log {
            id: None,
            build_url: self.url().to_string(),
            data: self
                .get_console(jenkins_client)
                .map_err(Error::from_boxed)?,
        })
    }
}

impl SparseMatrixProject {
    pub fn pull_jobs(client: &Jenkins, project_name: &str) -> Result<Self> {
        client
            .get_object_as(
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
            .map_err(Error::from_boxed)
    }
}
