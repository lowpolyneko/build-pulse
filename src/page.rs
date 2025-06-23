use std::time::SystemTime;

use jenkins_api::{build::BuildStatus, job::Job};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use time::{OffsetDateTime, UtcOffset, macros::format_description};

use crate::{
    api::{SparseJob, SparseMatrixProject},
    db::{Database, InDatabase, Run},
};

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

fn render_job(job: &SparseJob, db: &Database, tz: UtcOffset) -> Markup {
    let sorted_runs = job.last_build.as_ref().map(|j| {
        let mut runs = j
            .runs
            .iter()
            .filter(|r| r.number == j.number)
            .map(|r| db.get_run(&r.url).expect("Expecting valid run here..."))
            .collect::<Vec<_>>();
        runs.sort_by_key(|r| match r.status {
            Some(BuildStatus::Failure) => 0,
            Some(BuildStatus::Unstable) => 1,
            Some(BuildStatus::Success) => 2,
            Some(BuildStatus::NotBuilt | BuildStatus::Aborted) => 3,
            None => 4,
        });

        runs
    });

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
                        Some(BuildStatus::NotBuilt | BuildStatus::Aborted) => "not built",
                        None => "no build",
                    }
                }
            }
            details open[matches!(build.result, Some(BuildStatus::Failure | BuildStatus::Unstable))] {
                table style="border: 1px solid black;" {
                    @if let Some(runs) = sorted_runs {
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
                        Some(BuildStatus::NotBuilt | BuildStatus::Aborted) => "not built",
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
                            "Identified Issues: "
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
                        @for i in issues {
                            pre {
                                (PreEscaped(i.snippet))
                            }
                            hr;
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
            @for (name, desc, count) in stats.tag_counts {
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
    }
}

pub fn render(project: &SparseMatrixProject, db: &Database, tz: UtcOffset) -> Markup {
    let time: OffsetDateTime = SystemTime::now().into();
    html! {
        (DOCTYPE)
        html {
            head {
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
