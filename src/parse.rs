//! [Tag] and [TagSet] parsing behavior.
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    ops::Deref,
};

use regex::{Regex, RegexSet};

use crate::{
    config::{ConfigTag, Field, Severity},
    db::{InDatabase, Issue},
};

/// Set of [Tag]s
pub struct TagSet<T> {
    /// [Vec] of underlying [Tag]s
    tags: Vec<T>,

    /// [RegexSet] matching [Tag]s
    match_set: RegexSet,
}

/// [Tag] that can be parsed for [Issue]s
pub struct Tag<'a> {
    /// Unique name
    pub name: &'a str,

    /// Description of [Tag]
    pub desc: &'a str,

    /// [Regex] pattern to match for
    regex: Regex,

    /// [Field] to pattern match
    pub from: &'a Field,

    /// [Severity] of [Tag]
    pub severity: &'a Severity,
}

impl<'a, T> Hash for TagSet<T>
where
    T: Deref<Target = Tag<'a>>,
{
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.tags.iter().for_each(|t| t.hash(state));
    }
}

impl Hash for Tag<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.regex.as_str().hash(state);
        self.from.hash(state);
    }
}

impl<'a, T> Deref for TagSet<T>
where
    T: Deref<Target = Tag<'a>>,
{
    type Target = Vec<T>;
    fn deref(&self) -> &Self::Target {
        &self.tags
    }
}

impl<'a> TagSet<Tag<'a>> {
    /// Load an array of [ConfigTag] into a [TagSet]
    pub fn from_config(config_tags: &'a [ConfigTag]) -> Result<Self, regex::Error> {
        let tags = config_tags
            .iter()
            .map(|i| {
                Ok(Tag {
                    name: &i.name,
                    desc: &i.desc,
                    regex: Regex::new(&i.pattern)?,
                    from: &i.from,
                    severity: &i.severity,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let match_set = RegexSet::new(config_tags.iter().map(|i| &i.pattern))?;

        Ok(Self { tags, match_set })
    }
}

impl<'a, T> TagSet<T>
where
    T: Deref<Target = Tag<'a>>,
{
    /// Grep `field` for [Tag]s
    pub fn grep_tags(&self, field: &str, from: Field) -> impl Iterator<Item = &T> {
        // matches using the match set first, then the regex of all valid matches are ran again to find them
        self.match_set
            .matches(field)
            .into_iter()
            .map(|i| &self.tags[i])
            .filter(move |t| *t.from == from)
    }

    /// Get the [TagSet] schema/hash
    pub fn schema(&self) -> u64 {
        let mut s = DefaultHasher::new();
        self.hash(&mut s);
        s.finish()
    }
}

impl<T> TagSet<T> {
    /// Try to mutate tags in-place from `T` -> `U`
    pub fn try_swap_tags<F, U, E>(self, f: F) -> Result<TagSet<U>, E>
    where
        F: FnMut(T) -> Result<U, E>,
    {
        Ok(TagSet {
            tags: self
                .tags
                .into_iter()
                .map(f)
                .collect::<Result<Vec<_>, _>>()?,
            match_set: self.match_set,
        })
    }
}

impl<'a> InDatabase<Tag<'a>> {
    /// Grep `field` for [Issue]s
    pub fn grep_issue(&'a self, field: &'a str) -> impl Iterator<Item = Issue<'a>> {
        self.regex.find_iter(field).map(|m| Issue {
            snippet: m.as_str(),
            tag: self.id,
        })
    }
}
