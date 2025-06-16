use jenkins_api::{build::BuildStatus, job::Job};
use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::{
    api::{SparseJob, SparseMatrixProject},
    db::{Database, InDatabase, Run},
};

fn render_job(job: &SparseJob, db: &Database) -> Markup {
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
                " at "
                i {
                    (build.timestamp)
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
            details open="true" {
                table style="border: 1px solid black;" {
                    @for run in build.runs.iter().map(|r| db.get_run(&r.url)) {
                        (render_run(&run.expect("Expecting valid run here..."), &db))
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
    html! {
        tr style="border: 1px solid black;" {
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
                            "Identified Issues"
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

pub fn render(project: &SparseMatrixProject, db: &Database) -> Markup {
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
                    "Health:"
                    progress value=(health) max=(total) {}
                }
                @for job in &project.jobs {
                    (render_job(job, db))
                }
            }
        }
    }
}
