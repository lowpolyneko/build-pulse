use regex::{Regex, RegexSet};

use crate::{config::ConfigIssue, db::Issue};

pub struct IssuePatterns {
    issues: Vec<IssuePattern>,
    match_set: RegexSet,
}

struct IssuePattern {
    name: String,
    regex: Regex,
}

impl IssuePatterns {
    pub fn load_regex(issues: &[ConfigIssue]) -> Result<Self, regex::Error> {
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
}

pub trait Parse {
    fn get_data(&self) -> &str;

    fn grep_issues(&self, patterns: &IssuePatterns) -> impl Iterator<Item = Issue> {
        // matches using the match set first, then the regex of all valid matches are ran again to find them
        patterns
            .match_set
            .matches(self.get_data())
            .into_iter()
            .map(|i| &patterns.issues[i].regex)
            .flat_map(|re| re.find_iter(self.get_data()))
            .map(|m| Issue {
                id: None,
                snippet: m.as_str(),
            })
    }
}
