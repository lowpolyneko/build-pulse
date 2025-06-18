use anyhow::{Error, Result};
use jenkins_api::{
    Jenkins,
    build::{Build, BuildStatus, ShortBuild},
    client::{Path, TreeBuilder},
    job::Job,
};
use serde::Deserialize;

use crate::db::Run;

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
    pub number: u32,
    pub url: String,
    pub display_name: String,
    pub timestamp: u64,
    pub result: Option<BuildStatus>,
    pub runs: Vec<ShortBuild>,
}

pub trait AsRun {
    fn as_run(&self, jenkins_client: &Jenkins) -> Result<Run>;
}

pub trait HasBuildFields {
    fn build_status(&self) -> Option<BuildStatus>;
    fn full_display_name_or_default(&self) -> &str;
}

macro_rules! impl_HasBuildFields {
    (for $($t:ty),+) => {
        $(impl HasBuildFields for $t {
            fn build_status(&self) -> Option<BuildStatus> {
                self.result
            }

            fn full_display_name_or_default(&self) -> &str {
                self.full_display_name
                    .as_ref()
                    .unwrap_or(&self.display_name)
            }
        })*
    }
}

impl_HasBuildFields!(for jenkins_api::build::CommonBuild);

impl Job for SparseJob {
    fn name(&self) -> &str {
        &self.name
    }
    fn url(&self) -> &str {
        &self.url
    }
}

impl<T> AsRun for T
where
    T: Build + HasBuildFields,
{
    fn as_run(&self, jenkins_client: &Jenkins) -> Result<Run> {
        let status = self.build_status();
        Ok(Run {
            build_url: self.url().to_string(),
            display_name: self.full_display_name_or_default().to_string(),
            status,
            log: match status {
                Some(BuildStatus::Failure) => Some(
                    // only get log on failure
                    self.get_console(jenkins_client)
                        .map_err(Error::from_boxed)?,
                ),
                _ => None,
            },
            tag_schema: None,
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
                                    .with_subfield("number")
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
