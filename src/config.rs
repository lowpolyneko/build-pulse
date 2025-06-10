use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    pub jenkins_url: String,
    pub project: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub issue: Vec<ConfigIssue>,
}

#[derive(Deserialize)]
pub struct ConfigIssue {
    pub name: String,
    pub pattern: String,
}
