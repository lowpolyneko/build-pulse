use jenkins_api::build::BuildStatus;
use maud::{DOCTYPE, Markup, html};

use crate::model::SparseMatrixProject;

pub fn render(project: &SparseMatrixProject) -> Markup {
    html! {
        (DOCTYPE)
        html {
            head {
                meta charset="utf-8";
            }
            body {
                p {
                    "Health:"
                    progress value="10" max="100" {}
                }
                h1 {
                    "Job Status"
                }
                @for j in &project.jobs {
                    p {
                        @if let Some(r) = j.last_build.as_ref().map(|b| b.result).map(|r| match r {
                            Some(BuildStatus::Success) => "good",
                            Some(BuildStatus::Failure) => "bad",
                            _ => "?",
                        }) {
                            (r)
                        }
                        (j.name)
                    }
                }
            }
        }
    }
}
