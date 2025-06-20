use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    pub jenkins_url: String,
    pub project: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub database: String,
    pub timezone: i8,
    pub tag: Vec<ConfigTag>,
}

#[derive(Deserialize)]
pub struct ConfigTag {
    pub name: String,
    pub desc: String,
    pub pattern: String,
    pub from: Field,
}

#[derive(Deserialize, Clone, Copy, Hash)]
pub enum Field {
    Console,
    RunName,
}
