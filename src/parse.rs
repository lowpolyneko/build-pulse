use regex::{Regex, RegexSet};

use crate::config::Issue;

pub struct IssuePatterns {
    issues: Vec<IssuePattern>,
    match_set: RegexSet,
}

pub struct IssuePattern {
    name: String,
    regex: Regex,
}

pub fn load_regex(issues: &Vec<Issue>) -> Result<IssuePatterns, regex::Error> {
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

pub fn grep_issues(patterns: &IssuePatterns, log: &String) {
    patterns.issues.iter().for_each(|p| {
        p.regex.find_iter(log).for_each(|m| {
            println!("START MATCH--------");
            println!("{}", m.as_str());
            println!("END MATCH--------");
        })
    });
}
