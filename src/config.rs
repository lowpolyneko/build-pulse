use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    pub issue: Vec<ConfigIssue>,
}

#[derive(Deserialize)]
pub struct ConfigIssue {
    pub name: String,
    pub pattern: String,
}
