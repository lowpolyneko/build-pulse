//! [Config] file structure.
use std::fmt;

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

    /// Blocklist of jobs by name
    pub blocklist: Vec<String>,

    /// List of custom [TagView] to be rendered
    pub view: Vec<TagView>,

    /// [Vec] of [ConfigTag] to be parsed as [crate::parse::TagSet]
    pub tag: Vec<ConfigTag>,
}

/// Represesnts one [crate::parse::Tag] view to be rendered
#[derive(Deserialize)]
pub struct TagView {
    /// Name of the view
    pub name: String,

    /// TagExpr to query [crate::db::Database] with
    pub expr: String,
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
    (
        #[doc = $docstring:expr]
        pub enum $name:ident {
            $($member:tt),*,
        }
    ) => {
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

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    $($name::$member => write!(f, stringify!($member))),*
                }
            }
        }
    }
}

fields! {
    #[doc = "Valid fields to pattern match from"]
    pub enum Field {
        Console,
        RunName,
    }
}

fields! {
    #[doc = "Represents how severe a tag is"]
    pub enum Severity {
        Metadata,
        Info,
        Warning,
        Error,
    }
}
