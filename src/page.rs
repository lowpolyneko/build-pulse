//! HTML report generation using [maud] templating.
use std::time::SystemTime;

use jenkins_api::{build::BuildStatus, job::Job};
use maud::{DOCTYPE, Markup, html};
use time::{OffsetDateTime, UtcOffset, macros::format_description};

use crate::{
    api::{SparseJob, SparseMatrixProject},
    config::Severity,
    db::{Database, InDatabase, Run},
};

/// Format `time` as a [String]
fn format_timestamp<T>(time: T) -> String
where
    T: Into<OffsetDateTime>,
{
    time.into()
        .format(
            format_description!("[month repr:short] [day], [year] [hour repr:12]:[minute]:[second] [period] UTC[offset_hour padding:none sign:mandatory]")
        )
        .unwrap()
}

/// Render a [SparseJob]
fn render_job(job: &SparseJob, db: &Database, tz: UtcOffset) -> Markup {
    let sorted_runs = match job.last_build.as_ref() {
        Some(b) => match b.runs.as_ref() {
            Some(runs) => {
                let mut sr = runs
                    .iter()
                    .filter(|r| r.number == b.number)
                    .map(|r| db.get_run(&r.url).expect("Expecting valid run here..."))
                    .collect::<Vec<_>>();

                sr.sort_by_key(|r| match r.status {
                    Some(BuildStatus::Failure) => 0,
                    Some(BuildStatus::Unstable) => 1,
                    Some(BuildStatus::Success) => 2,
                    Some(BuildStatus::NotBuilt) => 3,
                    Some(BuildStatus::Aborted) => 4,
                    None => 4,
                });

                Some(sr)
            }
            None => None,
        },
        None => None,
    };

    html! {
        h2 {
            a href=(job.url()) {
                (job.name())
            }
        }
        @if let Some(build) = job.last_build.as_ref() {
            p {
                "Last build: "
                a href=(build.url) {
                    (build.display_name)
                }
                " on "
                i {
                    (format_timestamp(OffsetDateTime::from_unix_timestamp((build.timestamp/1000).cast_signed()).expect("Jenkins returned an invalid timestamp!").to_offset(tz)))
                }
                " was "
                b {
                    @match build.result {
                        Some(BuildStatus::Success) => "good",
                        Some(BuildStatus::Failure) => "bad",
                        Some(BuildStatus::Unstable) => "unstable",
                        Some(BuildStatus::NotBuilt) => "not built",
                        Some(BuildStatus::Aborted) => "aborted",
                        None => "no build",
                    }
                }
            }
            @if let Some(runs) = sorted_runs {
                details open[matches!(build.result, Some(BuildStatus::Failure | BuildStatus::Unstable | BuildStatus::Aborted))] {
                    summary {
                        "Run Information"
                    }
                    table style="border: 1px solid black;" {
                        @for run in runs {
                            (render_run(&run, &db))
                        }
                    }
                }
            }
        } @else {
            p {
                "No builds available."
            }
        }
    }
}

/// Render a [Run]
fn render_run(run: &InDatabase<Run>, db: &Database) -> Markup {
    let row_border = match run.status {
        Some(BuildStatus::Failure | BuildStatus::Unstable) => {
            "border: 1px solid black; background-color: lightgray"
        }
        _ => "border: 1px solid black;",
    };
    html! {
        tr style=(row_border) {
            td style="border: 1px solid black;" { // status
                b {
                    @match run.status {
                        Some(BuildStatus::Success) => "good",
                        Some(BuildStatus::Failure) => "bad",
                        Some(BuildStatus::Unstable) => "unstable",
                        Some(BuildStatus::NotBuilt) => "not built",
                        Some(BuildStatus::Aborted) => "aborted",
                        None => "no build",
                    }
                }
            }
            td style="border: 1px solid black;" { // name
                a href=(run.build_url) {
                    (run.display_name)
                }
            }
            td style="border: 1px solid black;" { // issues
                @if let Ok(issues) = db.get_issues(run) {
                    @if !issues.is_empty() {
                        b {
                            "Identified Tags: "
                        }
                        @if let Ok(tags) = db.get_tags(run) {
                            @for (name, desc) in tags {
                                code title=(desc) {
                                    (name)
                                    ", "
                                }
                            }
                        }
                        hr;
                        @for (i, s) in issues {
                            @if !matches!(s, Severity::Metadata) {
                                pre {
                                    (i.snippet)
                                }
                                @if i.duplicates > 0 {
                                    b {
                                        (i.duplicates)
                                        " duplicate emit(s)"
                                    }
                                }
                                hr;
                            }
                        }
                    }
                }
                a href=(format!("{}/consoleFull", run.build_url)) {
                    "Full Build Log"
                }
            }
        }
    }
}

/// Render [crate::db::Statistics]
fn render_stats(project: &SparseMatrixProject, db: &Database) -> Markup {
    let stats = db
        .get_stats()
        .expect("Failed to get statistics from database.");
    html! {
        h3 {
            "Run Statistics"
        }
        @let health = project
                        .jobs
                        .iter()
                        .filter_map(|j| j.last_build.as_ref())
                        .fold(0, |h, b|
                            h + match b.result {
                                Some(BuildStatus::Success) => 1,
                                _ => 0,
                            }
        );
        @let total = project.jobs.len();
        p {
            "Overall Job Health:"
            progress value=(health) max=(total) {}
            br;
            (health)
            " out of "
            (total)
            " jobs successful."
        }

        h4 {
            "Run Status"
        }
        table style="border: 1px solid black;" {
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    "Failures"
                }
                td style="border: 1px solid black;" {
                    (stats.failures)
                    " runs"
                }
            }
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    "Unstable"
                }
                td style="border: 1px solid black;" {
                    (stats.unstable)
                    " runs"
                }
            }
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    "Healthy"
                }
                td style="border: 1px solid black;" {
                    (stats.successful)
                    " runs"
                }
            }
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    "Aborted"
                }
                td style="border: 1px solid black;" {
                    (stats.aborted)
                    " runs"
                }
            }
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    "Not Built"
                }
                td style="border: 1px solid black;" {
                    (stats.not_built)
                    " runs"
                }
            }
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    b {
                        "Total"
                    }
                }
                td style="border: 1px solid black;" {
                    b {
                        (stats.successful + stats.failures + stats.unstable + stats.aborted + stats.not_built)
                        " runs"
                    }
                }
            }
        }

        h4 {
            "By Tag"
        }
        table style="border: 1px solid black;" {
            @for (name, desc, severity, count) in &stats.tag_counts {
                @if !matches!(severity, Severity::Metadata) {
                    tr style="border: 1px solid black;" {
                        td style="border: 1px solid black;" {
                            code title=(desc) {
                                (name)
                            }
                        }
                        td style="border: 1px solid black;" {
                            (count)
                            " issues"
                        }
                    }
                }
            }
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    b {
                        "Total"
                    }
                }
                td style="border: 1px solid black;" {
                    b {
                        (stats.issues_found)
                        " issues found!"
                    }
                }
            }
        }

        b {
            (stats.unknown_issues)
            " runs with unknown issues!"
        }

        h4 {
            "By Metadata"
        }
        table style="border: 1px solid black;" {
            @for (name, desc, severity, count) in &stats.tag_counts {
                @if matches!(severity, Severity::Metadata) {
                    tr style="border: 1px solid black;" {
                        td style="border: 1px solid black;" {
                            code title=(desc) {
                                (name)
                            }
                        }
                        td style="border: 1px solid black;" {
                            (count)
                            " runs"
                        }
                    }
                }
            }
        }

        h4 {
            "Related Issues"
        }
        table style="border: 1px solid black;" {
            @for (name, desc, group) in db.get_similarities()
                .expect("Failed to get similarities from database.") {
                tr style="border: 1px solid black;" {
                    td style="border: 1px solid black;" {
                        code title=(desc) {
                            (name)
                        }
                    }
                    td style="border: 1px solid black;" {
                        ul {
                            @for display_name in group {
                                li {
                                    (display_name)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Render an HTML report for `project`
pub fn render(project: &SparseMatrixProject, db: &Database, tz: UtcOffset) -> Markup {
    let time: OffsetDateTime = SystemTime::now().into();
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                title {
                    "build-pulse report"
                }
                meta charset="utf-8";
            }
            body {
                h1 {
                    "Job Status"
                }
                (render_stats(project, db))
                @for job in &project.jobs {
                    (render_job(job, db, tz))
                }
                p {
                    "Report generated on "
                    code {
                        (format_timestamp(time.to_offset(tz)))
                    }
                }
            }
        }
    }
}
