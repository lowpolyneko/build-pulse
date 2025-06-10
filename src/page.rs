use jenkins_api::build::BuildStatus;
use maud::{DOCTYPE, Markup, html};

use crate::api::SparseMatrixProject;

pub fn render(project: &SparseMatrixProject) -> Markup {
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
                @let mut health = 0;
                @let mut total = 0;
                @for j in &project.jobs {
                    p {
                        @if let Some(r) = j.last_build.as_ref().map(|b| b.result).map(|r| match r {
                            Some(BuildStatus::Success) => { health += 1; "good " },
                            Some(BuildStatus::Failure) => "bad ",
                            Some(BuildStatus::Unstable) => "unstable ",
                            None => "",
                            _ => "? ",
                        }) {
                            (r)
                        }
                        ({ total += 1; j.name.as_str() })
                        @if let Some(n) = j.last_build.as_ref().map(|b| b.display_name.as_str()) {
                            (n)
                        }
                    }
                }
                p {
                    "Health:"
                    progress value=(health) max=(total) {}
                }
            }
        }
    }
}
