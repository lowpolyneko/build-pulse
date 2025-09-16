//! HTML report generation using [maud] templating.
use std::time::SystemTime;

use anyhow::{Error, Result};
use base64::{display::Base64Display, engine::general_purpose::STANDARD};
use jenkins_api::build::BuildStatus;
use maud::{DOCTYPE, Markup, html};
use time::{OffsetDateTime, UtcOffset, macros::format_description};

use crate::{
    config::TagView,
    db::{
        Artifact, Database, InDatabase, Issue, Job, JobBuild, Queryable, Run, Similarity,
        Statistics, TagInfo,
    },
    tag_expr::TagExpr,
};

/// Format `time` as a [String]
fn format_timestamp<T>(time: T) -> Result<String>
where
    T: Into<OffsetDateTime>,
{
    time.into()
        .format(
            format_description!("[month repr:short] [day], [year] [hour repr:12]:[minute]:[second] [period] UTC[offset_hour padding:none sign:mandatory]")
        )
        .map_err(Error::from)
}

/// Render a [crate::api::SparseJob]
fn render_job(job: &InDatabase<Job>, db: &Database, tz: UtcOffset) -> Result<Markup> {
    let last_build = match job.last_build {
        Some(n) => Some(JobBuild::select_one_by_job(db, job.id, n, ())?),
        None => None,
    };
    let sorted_runs = match &last_build {
        Some(b) => {
            let mut sr = Run::select_all_by_build(db, b, ())?;
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
    };

    Ok(html! {
        h2 {
            a href=(job.url) {
                (job.name)
            }
        }
        @if let Some(build) = last_build {
            p {
                "Last build: "
                a href=(build.url) {
                    "#"
                    (build.number)
                }
                " on "
                i {
                    (format_timestamp(
                        OffsetDateTime::from_unix_timestamp(
                            (build.timestamp/1000).cast_signed()
                        )?
                        .to_offset(tz)
                    )?)
                }
                " was "
                b {
                    @match build.status {
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
                details open[matches!(build.status, Some(BuildStatus::Failure))] {
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
    })
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
        tr #(run.id) style=(row_border) {
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
                a href=(run.url) {
                    (run.display_name)
                }
            }
            td style="border: 1px solid black;" { // issues
                @if let Ok(issues) = Issue::select_all_not_metadata(db, (db, run)) {
                    @if !issues.is_empty() {
                        @if let Ok(tags) = TagInfo::select_all_by_run(db, run, ()) {
                            b {
                                "Identified Tags: "
                            }
                            @for t in tags {
                                code title=(t.desc) {
                                    (t.name)
                                    ", "
                                }
                            }
                        }
                        hr;
                        @for i in issues {
                            pre {
                                (i.snippet)
                            }
                            @if i.duplicates > 0 {
                                b {
                                    (i.duplicates)
                                    " duplicate emits"
                                }
                            }
                            hr;
                        }
                    } @else if matches!(run.status, Some(BuildStatus::Failure)) {
                        b {
                            "Unknown issue(s)!"
                        }
                        hr;
                    }
                }
                a href={(run.url) "/consoleFull"} {
                    "Full Build Log"
                }
            }
            td style="border: 1px solid black;" { // artifacts
                @if let Ok(artifacts) = Artifact::select_all_by_run(db, run.id, ()) && !artifacts.is_empty() {
                    @for a in artifacts.into_iter().map(|a| a.item()) {
                        details {
                            summary {
                                b {
                                    (a.path)
                                }
                            }
                            hr;
                            @match a.contents.as_slice() {
                                [] => i {
                                    "no data"
                                },
                                [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, ..] =>
                                    // PNG magic
                                    img src={
                                        "data:image/png;base64,"
                                        (Base64Display::new(&a.contents, &STANDARD))
                                    };,
                                _ => @match String::from_utf8(a.contents) {
                                    Ok(blob) => @if blob.contains("<svg") {
                                        // SVG XML data
                                        img src={
                                            "data:image/svg+xml;base64,"
                                            (Base64Display::new(blob.as_bytes(), &STANDARD))
                                        };
                                    } @else {
                                        pre {
                                            (blob)
                                        }
                                    },
                                    Err(_) => p {
                                        "can't display"
                                    }
                                },
                            }
                            hr;
                        }
                    }
                }
            }
        }
    }
}

/// Render [crate::db::Statistics]
fn render_stats(db: &Database) -> Result<Markup> {
    let stats = Statistics::query(db)?;
    Ok(html! {
        h3 {
            "Run Statistics"
        }
        p {
            "Overall Job Health:"
            progress value=(stats.successful_jobs) max=(stats.total_jobs) {}
            br;
            (stats.successful_jobs)
            " out of "
            (stats.total_jobs)
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
                    (render_run_ids(stats.failures.iter(), db)?)
                }
            }
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    "Unstable"
                }
                td style="border: 1px solid black;" {
                    (render_run_ids(stats.unstable.iter(), db)?)
                }
            }
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    "Healthy"
                }
                td style="border: 1px solid black;" {
                    (render_run_ids(stats.successful.iter(), db)?)
                }
            }
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    "Aborted"
                }
                td style="border: 1px solid black;" {
                    (render_run_ids(stats.aborted.iter(), db)?)
                }
            }
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    "Not Built"
                }
                td style="border: 1px solid black;" {
                    (render_run_ids(stats.not_built.iter(), db)?)
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
                        (stats.successful.len()
                         + stats.failures.len()
                         + stats.unstable.len()
                         + stats.aborted.len()
                         + stats.not_built.len())
                        " runs"
                    }
                }
            }
        }
        br;
        table style="border: 1px solid black;" {
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    b {
                        "Issues Found"
                    }
                }
                td style="border: 1px solid black;" {
                    b {
                        (stats.issues_found)
                        " issues"
                    }
                }
            }
            tr style="border: 1px solid black;" {
                td style="border: 1px solid black;" {
                    b {
                        "Unknown Issues"
                    }
                }
                td style="border: 1px solid black;" {
                    b {
                        (render_run_ids(stats.unknown_runs.iter(), db)?)
                    }
                }
            }
        }
    })
}

/// Render [crate::db::Similarity]
fn render_similarities(db: &Database) -> Result<Markup> {
    Ok(html! {
        h4 {
            "Related Issues"
        }
        table style="border: 1px solid black;" {
            @for s in Similarity::query_all(db, ())?
                .into_iter()
                // ignore similarities within the same run
                .filter(|s| s.related.len() > 1) {
                    tr style="border: 1px solid black; background-color: lightgray" {
                        td style="border: 1px solid black;" {
                            code title=(s.tag.desc) {
                                (s.tag.name)
                            }
                        }
                        td style="border: 1px solid black;" {
                            b {
                                "Example Snippet"
                            }
                            hr;
                            pre {
                                (s.example)
                            }
                        }
                        td style="border: 1px solid black;" {
                            (render_run_ids(s.related.iter(), db)?)
                        }
                    }
            }
        }
    })
}

/// Render a [TagView]
fn render_view(view: &TagView, db: &Database) -> Result<Markup> {
    let expr = match TagExpr::parse(&view.expr) {
        Ok(expr) => Ok(expr),
        Err(e) => Err(Error::msg(
            e.iter().fold(String::new(), |acc, e| format!("{acc}\n{e}")),
        )),
    }?;
    let rows = expr.eval_rows(&TagInfo::select_all(db, ())?);

    Ok(html! {
        h4 {
            (view.name)
        }
        table style="border: 1px solid black;" {
            @for expr in rows {
                @let matches = Run::select_all_id_by_expr(db, &expr)?;
                @if !matches.is_empty() {
                    tr style="border: 1px solid black;" {
                        td style="border: 1px solid black;" {
                            code {
                                (expr)
                            }
                        }
                        td style="border: 1px solid black;" {
                            (render_run_ids(matches.iter(), db)?)
                        }
                    }
                }
            }
        }
    })
}

fn render_run_ids<'a, T>(ids: T, db: &Database) -> Result<Markup>
where
    T: ExactSizeIterator + Iterator<Item = &'a i64>,
{
    Ok(html! {
        @let len = ids.len();
        @if len > 0 {
            details {
                summary {
                    (len)
                    " runs"
                }
                ul {
                    @for id in ids {
                        li {
                            a href={"#" (id)} {
                                (Run::select_one_display_name(db, *id)?)
                            }
                        }
                    }
                }
            }
        } @else {
            "0 runs"
        }
    })
}

/// Render an HTML report for [Database] info
pub fn render(db: &Database, views: &[TagView], tz: UtcOffset) -> Result<Markup> {
    Ok(html! {
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
                (render_stats(db)?)
                (render_similarities(db)?)
                @for view in views {
                    (render_view(view, db)?)
                }
                @for job in Job::select_all(db, ())? {
                    (render_job(&job, db, tz)?)
                }
                p {
                    "Report generated on "
                    code {
                        (format_timestamp(
                            OffsetDateTime::from(SystemTime::now())
                            .to_offset(tz)
                        )?)
                    }
                }
            }
        }
    })
}
