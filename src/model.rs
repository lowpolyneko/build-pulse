use jenkins_api::{
    build::{Build, BuildStatus, ShortBuild},
    job::Job,
};
use serde::Deserialize;

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
