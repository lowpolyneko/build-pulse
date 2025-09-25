//! A Jenkins CI/CD-based build analyzer and issue prioritizer.
use std::{
    cell::Cell,
    hash::{DefaultHasher, Hash, Hasher},
    path::Path,
    process::Stdio,
    str::from_utf8,
    sync::Arc,
};

use anyhow::{Error, Result};
use clap::{Parser, crate_name, crate_version};
use env_logger::Env;
use jenkins_api::{
    Jenkins, JenkinsBuilder,
    build::{Build, BuildStatus, ShortBuild},
};
use log::{Level, info, log, warn};
use regex::Regex;
use time::UtcOffset;
use tokio::{
    fs,
    io::AsyncWriteExt,
    process::Command,
    sync::Semaphore,
    task::{self, JoinSet},
};

use crate::{
    api::{AsBuild, AsJob, AsRun, SparseMatrixProject},
    config::{Config, ConfigArtifact, Field, Severity},
    db::{
        Artifact, Database, InDatabase, Issue, Job, JobBuild, Queryable, Run, SimilarityInfo,
        TagInfo, Upsertable,
    },
    parse::{Tag, TagSet, normalized_levenshtein_distance},
};

mod api;
mod config;
#[macro_use]
mod db;
mod page;
mod parse;
mod tag_expr;

/// CLI arguments
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Config file path
    #[arg(default_value = "config.toml")]
    config: String,

    /// Report output (`Some(None)` for stdout) path
    #[arg(short, long)]
    output: Option<Option<String>>,

    /// Whether or not to purge cache
    #[arg(short, long)]
    purge_cache: bool,
}

// [reqwest] will open new connections until the system `ulimit`,
// we have to limit parallelism ourselves
static RATE_LIMIT: Semaphore = Semaphore::const_new(20);
macro_rules! rate_limit {
    ($closure:expr) => {
        async move {
            let _permit = RATE_LIMIT.acquire().await.unwrap();
            $closure.await // _permit dropped here
        }
    };
}

/// Spawns a process, pipes stdin, and waits for stdout
#[inline]
async fn spawn_process<I, S>(
    program: S,
    args: I,
    run_name: &str,
    run_url: &str,
    stdin: &[u8],
) -> std::io::Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut child = Command::new(program)
        .args(args)
        .env("BUILD_PULSE_RUN_NAME", run_name)
        .env("BUILD_PULSE_RUN_URL", run_url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    child
        .stdin
        .take()
        .ok_or(std::io::Error::from(std::io::ErrorKind::NotFound))?
        .write_all(stdin)
        .await?;

    Ok(child.wait_with_output().await?.stdout)
}

/// Pull builds from `project.jobs` and cache them into database `db`
async fn pull_build_logs(
    project: SparseMatrixProject,
    artifacts: Arc<[(Regex, ConfigArtifact)]>,
    blocklist: &[String],
    last_n_history: usize,
    jenkins: Arc<Jenkins>,
    db: &Database,
) -> Result<Vec<InDatabase<Run>>> {
    // Context struct to move around to each task
    struct Context {
        artifacts: Arc<[(Regex, ConfigArtifact)]>,
        jenkins: Arc<Jenkins>,
        job: Arc<InDatabase<Job>>,
        build: Arc<InDatabase<JobBuild>>,
        mb: ShortBuild,
    }

    // https://morestina.net/1607/fallible-iteration
    let err: Cell<Result<()>> = Cell::new(Ok(()));
    fn until_err<T, E>(err: &mut &Cell<Result<(), E>>, res: Result<T, E>) -> Option<T> {
        res.map_err(|e| err.set(Err(e))).ok()
    }

    // spawn tasks to pull builds
    let mut runs = Vec::new();
    let mut handles: JoinSet<_> = project
        .jobs
        .into_iter()
        .filter(|sj| {
            !blocklist.contains(&sj.name)
                && sj
                    .builds
                    .is_empty()
                    .then(|| info!("Job '{}' has no builds.", &sj.name))
                    .is_none()
        })
        .map(|sj| {
            let job: Arc<_> = sj.as_job().upsert(db, ())?.into();
            Ok(sj
                .builds
                .into_iter()
                .map(move |sb| (job.clone(), sb))
                .take(last_n_history))
        })
        .scan(&err, until_err)
        .flatten()
        .map(|(job, sb)| {
            let artifacts = artifacts.clone();
            let jenkins = jenkins.clone();
            let build: Arc<_> = sb.as_build(job.id).upsert(db, ())?.into();
            Ok(sb
                .runs
                .into_iter()
                .flatten()
                .filter(move |mb| mb.number == sb.number)
                .map(move |mb| Context {
                    artifacts: artifacts.clone(),
                    jenkins: jenkins.clone(),
                    job: job.clone(),
                    build: build.clone(),
                    mb,
                }))
        })
        .scan(&err, until_err)
        .flatten()
        .filter_map(|ctx| match Run::select_one_by_url(db, &ctx.mb.url, ()) {
            Ok(run) => {
                runs.push(run);
                None
            } // cached
            Err(rusqlite::Error::QueryReturnedNoRows) => Some(Ok(ctx)),
            Err(e) => Some(Err(Error::from(e))),
        })
        .scan(&err, until_err)
        .map(
            |Context {
                 artifacts,
                 jenkins,
                 job,
                 build,
                 mb,
             }| {
                rate_limit!(async move {
                    let full_build: Arc<_> = mb.get_full_build(&jenkins).await.unwrap().into();
                    let run = full_build.as_run(build.id, &jenkins).await;

                    let artifacts = artifacts.clone();
                    let display_name = run.display_name.clone();
                    let url = run.url.clone();
                    let artifacts: JoinSet<_> = full_build
                        .clone()
                        .artifacts
                        .iter()
                        .filter_map(move |artifact| {
                            let jenkins = jenkins.clone();
                            let full_build = full_build.clone();
                            let artifact = artifact.clone();
                            let display_name = display_name.clone();
                            let url = url.clone();
                            artifacts
                                .iter()
                                .find(|(re, _)| re.is_match(&artifact.relative_path))
                                .map(move |(_, c)| {
                                    let post_process = c.post_process.clone();
                                    rate_limit!(async move {
                                        let blob = full_build
                                            .get_artifact(&jenkins, &artifact)
                                            .await
                                            .inspect_err(|e| {
                                                log::error!(
                                                    "Failed to retrieve artifact for run {}: {}",
                                                    &display_name,
                                                    e
                                                )
                                            })
                                            .ok()?;

                                        let contents = if let Some(mut iter) =
                                            post_process.as_ref().map(|argv| argv.iter())
                                            && let Some(program) = iter.next()
                                        {
                                            spawn_process(program, iter, &display_name, &url, &blob)
                                                .await
                                                .unwrap()
                                        } else {
                                            blob.to_vec()
                                        };

                                        Some(|run_id| Artifact {
                                            path: artifact.relative_path,
                                            contents,
                                            run_id,
                                        })
                                    })
                                })
                        })
                        .collect();

                    log!(
                        match run.status {
                            Some(
                                BuildStatus::Failure | BuildStatus::Unstable | BuildStatus::Aborted,
                            ) => Level::Warn,
                            _ => Level::Info,
                        },
                        "Job '{}#{}' run '{}' finished with status {:?}.",
                        job.name,
                        build.number,
                        run.display_name,
                        run.status
                    );

                    (run, artifacts)
                })
            },
        )
        .collect();

    err.into_inner()?; // check for failures before continuing
    runs.reserve(handles.len());

    // collect them all here
    while let Some(h) = handles.join_next().await {
        let (run, mut artifacts) = h?;
        let run = run.upsert(db, ())?;

        while let Some(artifact) = artifacts.join_next().await {
            if let Ok(Some(artifact)) = artifact {
                artifact(run.id).insert(db, ())?;
            }
        }

        runs.push(run);
    }

    Ok(runs)
}

/// Parse all untagged runs for `tags` and cache them into database `db`
async fn parse_unprocessed_runs(
    runs: Vec<InDatabase<Run>>,
    tags: Arc<TagSet<InDatabase<Tag>>>,
    db: &Database,
) -> Result<Vec<InDatabase<Issue>>> {
    let mut inserted_issues = Vec::new();

    enum Dependent {
        Run(Issue),
        Artifact(Issue, Arc<InDatabase<Artifact>>),
    }

    let mut handles: JoinSet<_> = runs
        .into_iter()
        .filter_map(|run| match run.tag_schema {
            None => {
                let tags = tags.clone();
                let artifacts = Artifact::select_all_by_run(db, run.id, ());
                Some(async move {
                    let issues: Vec<_> = {
                        let warn = |t: &InDatabase<Tag>| match t.severity {
                            Severity::Metadata => {}
                            _ => warn!(
                                "Found issue(s) tagged '{}' in run '{}'",
                                t.name, run.display_name
                            ),
                        };
                        let run_name = tags
                            .grep_tags(run.display_name.clone(), Field::RunName)
                            .flat_map(|t| t.grep_issue(run.display_name.clone()))
                            .map(Dependent::Run);
                        let console = run
                            .log
                            .iter()
                            .flat_map(|l| {
                                tags.grep_tags(l.clone(), Field::Console).flat_map(|t| {
                                    warn(t);
                                    t.grep_issue(l.clone())
                                })
                            })
                            .map(Dependent::Run);
                        let artifact = artifacts
                            .into_iter()
                            .flatten()
                            .map(|a| a.into())
                            .filter_map(|a: Arc<_>| {
                                from_utf8(&a.contents)
                                    .ok()
                                    .map(|b| -> arcstr::ArcStr { b.into() })
                                    .map(|blob| {
                                        tags.grep_tags(blob.clone(), Field::Artifact)
                                            .flat_map(move |t| {
                                                warn(t);
                                                t.grep_issue(blob.clone())
                                            })
                                            .map(move |i| Dependent::Artifact(i, a.clone()))
                                    })
                            })
                            .flatten();

                        run_name.chain(console).chain(artifact).collect()
                    };

                    (run, issues)
                })
            }
            _ => {
                inserted_issues.extend(
                    Issue::select_all_not_metadata(db, (db, &run))
                        .into_iter()
                        .flatten(),
                );
                None
            }
        })
        .collect();

    while let Some(h) = handles.join_next().await {
        let (run, issues) = h?;
        inserted_issues = issues.into_iter().try_fold(inserted_issues, |mut acc, i| {
            let issue = match i {
                Dependent::Run(issue) => issue.insert(db, (&run, None))?,
                Dependent::Artifact(issue, artifact) => {
                    issue.insert(db, (&run, Some(&artifact)))?
                }
            };
            match TagInfo::select_one(db, issue.tag_id, ())?.severity {
                Severity::Metadata => {}
                _ => acc.push(issue),
            }
            Ok::<_, Error>(acc)
        })?;
    }

    // batch update tag schema for runs afterwards
    Run::update_all_tag_schema(db, Some(tags.schema()))?;
    Ok(inserted_issues)
}

/// Calculate similarities against all issues and soft insert the groupings into [Database]
async fn calculate_similarities(
    issues: Vec<InDatabase<Issue>>,
    threshold: f32,
    db: &Database,
) -> Result<()> {
    // conservatively group by levenshtein distance
    let mut groups: Vec<Vec<Arc<InDatabase<Issue>>>> = Vec::new();
    for issue in issues.into_iter().map(Arc::new) {
        let mut handles: JoinSet<_> = groups
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, g)| {
                let issue = issue.clone();
                async move {
                    let mut inner: JoinSet<_> = g
                        .into_iter()
                        .map(|issue2| {
                            let issue = issue.clone();
                            async move {
                                normalized_levenshtein_distance(&issue.snippet, &issue2.snippet)
                                    > threshold
                            }
                        })
                        .collect();

                    loop {
                        match inner.join_next().await {
                            Some(Ok(true)) => continue,
                            Some(Ok(false)) => return None,
                            None => return Some(i),
                            Some(Err(e)) => std::panic::resume_unwind(e.into_panic()),
                        }
                    }
                }
            })
            .collect();

        loop {
            match handles.join_next().await {
                Some(Ok(None)) => continue,
                Some(Ok(Some(i))) => {
                    groups[i].push(issue.clone());
                    break;
                }
                None => {
                    groups.push(vec![issue.clone()]);
                    break;
                }
                Some(Err(e)) => std::panic::resume_unwind(e.into_panic()),
            }
        }
    }

    // sort resultant groups
    let mut handles: JoinSet<_> = groups
        .into_iter()
        .map(|mut g| async {
            g.sort();

            let mut hasher = DefaultHasher::new();
            g.hash(&mut hasher);
            (hasher.finish(), g)
        })
        .collect();

    // store relations in database
    while let Some(h) = handles.join_next().await {
        let (hash, g) = h?;
        // unique issues are discarded
        if g.len() > 1 {
            g.iter().try_for_each(|i| {
                SimilarityInfo {
                    similarity_hash: hash,
                    issue_id: i.id,
                }
                .insert(db, ())?;

                info!(
                    "Issue '#{}' likely matches with similarity group '#{}'!",
                    i.id, hash
                );

                Ok::<_, Error>(())
            })?;
        }
    }

    Ok(())
}

/// Copies the rendered versions of every [Artifact] into `folder`
async fn copy_artifacts<P: AsRef<Path>>(
    folder: P,
    artifacts: Arc<[(Regex, ConfigArtifact)]>,
    db: &Database,
) -> Result<()> {
    // create dir first
    fs::create_dir_all(&folder).await?;

    // render and copy
    let mut handles: JoinSet<_> = Artifact::select_all(db, ())?
        .into_iter()
        .map(|artifact| {
            let artifacts = artifacts.clone();
            let display_name = Run::select_one_display_name(db, artifact.run_id)?;
            let url = Run::select_one_url(db, artifact.run_id)?;
            let path = folder.as_ref().join(artifact.id.to_string());
            Ok(async move {
                let blob = if let Some((_, c)) =
                    artifacts.iter().find(|(re, _)| re.is_match(&artifact.path))
                    && let Some(mut iter) = c.render.as_ref().map(|argv| argv.iter())
                    && let Some(program) = iter.next()
                {
                    spawn_process(program, iter, &display_name, &url, &artifact.contents)
                        .await
                        .unwrap()
                } else {
                    artifact.item().contents
                };

                fs::write(path, blob).await.unwrap()
            })
        })
        .collect::<Result<_>>()?;

    while let Some(h) = handles.join_next().await {
        h?;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // initialize logging
    env_logger::init_from_env(Env::default().default_filter_or("info"));
    info!("{} {}", crate_name!(), crate_version!());

    // load config
    info!("Compiling issue patterns...");
    let Config {
        artifact,
        blocklist,
        database,
        jenkins_url,
        last_n_history,
        password,
        project,
        tag,
        threshold,
        timezone,
        username,
        view,
    } = toml::from_str(&fs::read_to_string(args.config).await?)?;
    let tags = TagSet::from_config(tag)?;
    let artifact: Arc<[_]> = artifact
        .into_iter()
        .map(|a| Regex::new(&a.path).map(|re| (re, a)))
        .collect::<Result<Vec<_>, _>>()?
        .into();

    // open db
    info!("Opening database...");
    let mut database = Database::open(&database)?;

    // check for cache purge
    if args.purge_cache {
        warn!("Purging cache!");
        database.purge_cache()?;
    }

    // update TagSet
    info!("Updating tags...");
    let tags = TagInfo::upsert_tag_set(&database, tags, ())?;

    // purge outdated issues
    let outdated = Issue::delete_all_invalid_by_tag_schema(&mut database, tags.schema())?;
    if outdated > 0 {
        warn!("Purged {outdated} runs' issues that parsed with an outdated tag schema!");
    }

    // purge blocklisted jobs
    let blocked = Job::delete_all_by_blocklist(&mut database, &blocklist)?;
    if blocked > 0 {
        warn!("Purged {blocked} jobs that are on the blocklist.");
    }

    info!(
        "Pulling associated jobs for {} from {}...",
        project, jenkins_url
    );

    let jenkins = JenkinsBuilder::new(&jenkins_url);
    let jenkins = match username {
        Some(user) => jenkins.with_user(&user, password.as_deref()),
        None => jenkins,
    }
    .build()
    .map_err(Error::from_boxed)?;

    info!("Pulling build info for each job...");
    info!("----------------------------------------");

    let project = SparseMatrixProject::pull_jobs(&jenkins, &project).await?;
    let runs = pull_build_logs(
        project,
        artifact.clone(),
        &blocklist,
        last_n_history,
        jenkins.into(),
        &database,
    )
    .await?;

    info!("Done!");
    info!("----------------------------------------");

    if Run::has_untagged(&database)? {
        info!("Parsing unprocessed run logs...");
        let issues = parse_unprocessed_runs(runs, tags.into(), &database).await?;

        info!("Done!");
        info!("----------------------------------------");

        // purge old data
        info!("Purging old runs...");

        JobBuild::delete_all_orphan(&database)?;

        info!("Purging extraneous tags...");
        TagInfo::delete_all_orphan(&database)?;

        info!("Calculating issue similarities...");
        calculate_similarities(issues, threshold, &database).await?;
    } else {
        info!("No runs to process.");
    }

    info!("Done!");
    info!("----------------------------------------");

    if let Some(output) = args.output {
        info!("Generating report...");

        copy_artifacts("artifacts", artifact, &database).await?;

        let markup = task::spawn_blocking(move || {
            page::render(
                &database,
                &view,
                UtcOffset::from_hms(timezone, 0, 0).unwrap(),
            )
            .unwrap()
            .into_string()
        })
        .await?;

        if let Some(filepath) = output {
            fs::write(&filepath, markup).await?;

            info!("Written to {filepath}");
        } else {
            info!("Dumping to stdout --");
            println!("{markup}");
        }
    }

    info!("Done!");

    Ok(())
}
