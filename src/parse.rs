//! [Tag] and [TagSet] parsing behavior.
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    ops::Deref,
};

use arcstr::ArcStr;
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
    pub fn grep_issue(&'a self, field: &ArcStr) -> impl Iterator<Item = Issue> {
        let mut hm: HashMap<Issue, u64> = HashMap::new();
        self.regex
            .find_iter(field)
            .map(|m| Issue {
                snippet: field.substr_from(m.into()),
                tag_id: self.id,
                duplicates: 0,
            })
            .for_each(|i| {
                hm.entry(i).and_modify(|e| *e += 1).or_insert(0);
            });

        hm.into_iter().map(|(mut i, d)| {
            i.duplicates = d;
            i
        })
    }
}

/// Calculate the Levenshtein Distance between two strings
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    // https://en.wikipedia.org/wiki/Levenshtein_distance#Iterative_with_two_matrix_rows
    let b_len = match b.len() {
        0 => return a.len(), // if b is empty, then distance is a
        x => x,
    };
    let mut v1: Vec<usize> = (0..=b_len).collect(); // current row (init with edit distance from "" to b)

    for (i, c1) in a.chars().enumerate() {
        // calculate v1 distances from v0 in-place
        let mut v0 = v1[0]; // save last substitution cost

        // v1[0] is the edit distance from a[..=i] to ""
        v1[0] = i + 1;

        for (j, c2) in b.chars().enumerate() {
            // v1[j + 1] is the character being calculated in a
            // v1[j] is the previous character in a
            let delete_cost = v1[j] + 1; // not including this character is better
            let insert_cost = v1[j + 1] + 1; // keeping this character is better
            let substitution_cost = if c1 == c2 {
                v0 // no change
            } else {
                v0 + 1 // substituting this character is better
            };

            v0 = v1[j + 1]; // save last substitution cost
            v1[j + 1] = delete_cost.min(insert_cost).min(substitution_cost);
        }
    }

    v1[b_len]
}

/// Calculate a normalized [levenshtein_distance] using an exponential decay model
///
/// <https://www.cse.lehigh.edu/%7Elopresti/Publications/1996/sdair96.pdf>
#[inline]
pub fn normalized_levenshtein_distance(a: &str, b: &str) -> f32 {
    let d = levenshtein_distance(a, b) as f32;
    let m = a.len().max(b.len()) as f32;
    (d / (m - d)).exp().recip()
}
