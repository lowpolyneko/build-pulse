use regex::{Regex, RegexSet};

use crate::{config::ConfigIssue, db::Issue};

pub struct IssuePatterns {
    issues: Vec<IssuePattern>,
    match_set: RegexSet,
}

pub struct IssuePattern {
    name: String,
    regex: Regex,
}

pub fn load_regex(issues: &Vec<ConfigIssue>) -> Result<IssuePatterns, regex::Error> {
    let compiled_issues = issues
        .iter()
        .map(|i| {
            Ok(IssuePattern {
                name: i.name.clone(),
                regex: Regex::new(&i.pattern)?,
            })
        })
        .collect::<Result<Vec<IssuePattern>, _>>()?;

    Ok(IssuePatterns {
        issues: compiled_issues,
        match_set: RegexSet::new(issues.iter().map(|i| &i.pattern))?,
    })
}

pub fn grep_issues<'a>(
    patterns: &IssuePatterns,
    log: &'a String,
) -> impl Iterator<Item = Issue<'a>> {
    // matches using the match set first, then the regex of all valid matches are ran again to find them
    patterns
        .match_set
        .matches(log)
        .into_iter()
        .map(|i| &patterns.issues[i].regex)
        .flat_map(|re| re.find_iter(log))
        .map(|m| Issue {
            id: None,
            snippet: m.as_str(),
        })
}
