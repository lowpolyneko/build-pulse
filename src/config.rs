use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    pub issue: Vec<Issue>,
}

#[derive(Deserialize)]
pub struct Issue {
    pub name: String,
    pub pattern: String,
}
