use serde::{Deserialize, Serialize};

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
    pub severity: Severity,
}

macro_rules! fields {
    ($name:ident, $($member:tt),*) => {
        #[derive(Deserialize, Serialize, Clone, Copy, Eq, PartialEq, Hash)]
        pub enum $name {$($member),*}

        impl $name {
            pub fn iter() -> impl Iterator<Item = $name> {
                vec![$($name::$member,)*].into_iter()
            }
        }
    }
}

fields!(Field, Console, RunName);
fields!(Severity, Metadata, Info, Warning, Error);
