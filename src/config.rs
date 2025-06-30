//! [Config] file structure.
use serde::{Deserialize, Serialize};

/// Representation of a "config.toml" file
#[derive(Deserialize)]
pub struct Config {
    /// Jenkins CI/CD server
    pub jenkins_url: String,

    /// Project to query for
    pub project: String,

    /// Optional username
    pub username: Option<String>,

    /// Optional password
    pub password: Option<String>,

    /// Sqlite3 database to cache build information
    pub database: String,

    /// Timezone in UTC+`timezone`
    pub timezone: i8,

    /// [Vec] of [ConfigTag] to be parsed as [crate::parse::TagSet]
    pub tag: Vec<ConfigTag>,
}

/// Represents one tag to be loaded as [crate::parse::Tag]
#[derive(Deserialize)]
pub struct ConfigTag {
    /// Unique name of the tag
    pub name: String,

    /// Description of the tag
    pub desc: String,

    /// [regex::Regex] pattern to match for tag
    pub pattern: String,

    /// [Field] to apply `pattern` to
    pub from: Field,

    /// [Severity] category for tag
    pub severity: Severity,
}

macro_rules! fields {
    ($name:ident, $docstring:expr, $($member:tt),*) => {
        #[doc = $docstring]
        #[derive(Deserialize, Serialize, Clone, Copy, Eq, PartialEq, Hash)]
        pub enum $name {$($member),*}

        impl $name {
            #[doc = concat!("Iterate through all values of ", stringify!($name), ".")]
            #[allow(dead_code)]
            pub fn iter() -> impl Iterator<Item = $name> {
                vec![$($name::$member,)*].into_iter()
            }
        }
    }
}

fields!(
    Field,
    "Valid fields to pattern match from",
    Console,
    RunName
);

fields!(
    Severity,
    "Represents how severe a tag is",
    Metadata,
    Info,
    Warning,
    Error
);
