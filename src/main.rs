//! A Jenkins CI/CD-based build analyzer and issue prioritizer.
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::Path,
    process::Stdio,
    str::from_utf8,
    sync::Arc,
};

use anyhow::{Error, Result};
use arcstr::ArcStr;
use clap::{Parser, crate_name, crate_version};
use env_logger::Env;
use futures::{FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt, future, stream};
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
    api::{AsBuild, AsJob, AsRun, SparseBuild, SparseJob, SparseMatrixProject},
    config::{Config, ConfigArtifact, Field, Severity},
    db::{
        Artifact, Database, InDatabase, Issue, IssueInfo, Job, JobBuild, Queryable, Run,
        SimilarityInfo, TagInfo, Upsertable,
    },
    parse::{PatternSet, normalized_levenshtein_distance},
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

/// State to move to each task when pulling a [SparseMatrixProject]
struct SparseMatrixProjectContext<'a> {
    project: SparseMatrixProject,
    blocklist: &'a [String],
}

/// State to move to each task when pulling a [Job]
struct JobContext {
    sj: SparseJob,
}

/// State to move to each task when pulling a [JobBuild]
struct BuildContext {
    job: InDatabase<Job>,
    sb: SparseBuild,
}

/// State to move to each task when pulling a [Run]
struct RunContext {
    job: InDatabase<Job>,
    build: InDatabase<JobBuild>,
    mb: ShortBuild,
}

/// State to move to each task when pulling an [Artifact]
struct ArtifactContext<T: Build> {
    full_mb: Arc<T>,
    run: InDatabase<Run>,
    artifact: jenkins_api::build::Artifact,
}

/// Puller for Jenkins CI
#[derive(Clone)]
struct JenkinsPuller {
    db: Database,
    jenkins: Arc<Jenkins>,
    artifacts: Arc<PatternSet<ConfigArtifact>>,
    last_n_history: usize,
}

trait TryPull<C, T>: Sized {
    type Error;

    async fn pull(self, ctx: C) -> Result<T, Self::Error>;
}

trait TryPullNested<C, T> {
    type Error;

    async fn pull_stream(
        self,
        ctx: C,
    ) -> Result<impl Stream<Item = Result<T, Self::Error>>, Self::Error>;
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

impl TryPullNested<JobContext, BuildContext> for JenkinsPuller {
    type Error = rusqlite::Error;

    /// Pull a [Job] and cache into [Database], returning the last-$n$ [BuildContext]s
    async fn pull_stream(
        self,
        JobContext { sj }: JobContext,
    ) -> Result<impl Stream<Item = Result<BuildContext, Self::Error>>, Self::Error> {
        sj.as_job(self.last_n_history)
            .upsert(&self.db)
            .map_ok(|job| {
                if sj.builds.is_empty() {
                    info!("Job '{}' has no builds.", &sj.name)
                }

                stream::iter(sj.builds)
                    .take(self.last_n_history)
                    .map(move |sb| {
                        Ok(BuildContext {
                            job: job.clone(),
                            sb,
                        })
                    })
            })
            .await
    }
}

impl TryPullNested<BuildContext, RunContext> for JenkinsPuller {
    type Error = rusqlite::Error;

    /// Pull a [JobBuild] and cache into [Database], returning its [RunContext]s
    async fn pull_stream(
        self,
        BuildContext { job, sb }: BuildContext,
    ) -> Result<impl Stream<Item = Result<RunContext, Self::Error>>, Self::Error> {
        sb.as_build(job.id)
            .upsert(&self.db)
            .map_ok(|build| {
                stream::iter(sb.runs.into_iter().flatten())
                    .filter(move |mb| future::ready(mb.number == sb.number))
                    .map(move |mb| {
                        Ok(RunContext {
                            job: job.clone(),
                            build: build.clone(),
                            mb,
                        })
                    })
            })
            .await
    }
}

impl TryPull<RunContext, InDatabase<Run>> for JenkinsPuller {
    type Error = anyhow::Error;

    /// Pull a [Run] and cache into [Database]
    async fn pull(
        self,
        RunContext { job, build, mb }: RunContext,
    ) -> Result<InDatabase<Run>, Self::Error> {
        match Run::select_one_by_url(&self.db, ArcStr::from(&mb.url)).await {
            Ok(run) => Ok(run),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                mb.get_full_build(&self.jenkins.clone())
                    .ok_into()
                    .and_then(|full_mb: Arc<_>| async move {
                        let run = full_mb
                            .as_run(build.id, &self.jenkins)
                            .await
                            .upsert(&self.db)
                            .await?;

                        // pull artifacts
                        // FIXME: asyncify parsing
                        stream::iter(full_mb.artifacts.clone())
                            .map(|artifact| ArtifactContext {
                                full_mb: full_mb.clone(),
                                run: run.clone(),
                                artifact,
                            })
                            .then(|ctx| self.clone().pull(ctx))
                            .inspect_err(|e| {
                                log::error!(
                                    "Failed to retrieve artifact for run {}: {}",
                                    run.display_name,
                                    e
                                );
                            });

                        log!(
                            match run.status {
                                Some(
                                    BuildStatus::Failure
                                    | BuildStatus::Unstable
                                    | BuildStatus::Aborted,
                                ) => Level::Warn,
                                _ => Level::Info,
                            },
                            "Job '{}#{}' run '{}' finished with status {:?}.",
                            job.name,
                            build.number,
                            run.display_name,
                            run.status
                        );

                        Ok(run)
                    })
                    .map_err(Error::from_boxed)
                    .await
            }
            Err(e) => Err(Error::from(e)),
        }
    }
}

impl<T: Build> TryPull<ArtifactContext<T>, Option<InDatabase<Artifact>>> for JenkinsPuller {
    type Error = anyhow::Error;

    async fn pull(
        self,
        ArtifactContext {
            full_mb,
            run,
            artifact,
        }: ArtifactContext<T>,
    ) -> Result<Option<InDatabase<Artifact>>, Self::Error> {
        match self.artifacts.grep_matches(&artifact.relative_path).next() {
            Some(config) => {
                let run_id = run.id;
                full_mb
                    .get_artifact(&self.jenkins, &artifact)
                    .map_err(Error::from_boxed)
                    .and_then(|blob| async move {
                        match config
                            .post_process
                            .as_ref()
                            .map(|cmd| cmd.split_first())
                            .flatten()
                        {
                            Some((program, argv)) => {
                                spawn_process(program, argv, &run.display_name, &run.url, &blob)
                                    .map_err(Error::from)
                                    .await
                            }
                            None => Ok(blob.to_vec()),
                        }
                    })
                    .and_then(|contents| {
                        Artifact {
                            path: artifact.relative_path.clone(), // FIXME: make this an arc?
                            contents,
                            run_id,
                        }
                        .insert(&self.db)
                        .map_err(Error::from)
                    })
                    .ok_into()
                    .await
            }
            None => Ok(None),
        }
    }
}

impl TryPullNested<SparseMatrixProjectContext<'_>, InDatabase<Run>> for JenkinsPuller {
    type Error = Error;

    /// Pull jobs plus their underlying builds, runs, and artifacts from [SparseMatrixProject] and cache them into [Database]
    async fn pull_stream(
        self,
        SparseMatrixProjectContext { project, blocklist }: SparseMatrixProjectContext<'_>,
    ) -> Result<impl Stream<Item = Result<InDatabase<Run>, Self::Error>>, Self::Error> {
        Ok(stream::iter(
            project
                .jobs
                .into_iter()
                .filter(|sj| !blocklist.contains(&sj.name)),
        )
        .then({
            let puller = self.clone();
            move |sj| puller.clone().pull_stream(JobContext { sj })
        })
        .try_flatten_unordered(None)
        .and_then({
            let puller = self.clone();
            move |ctx| puller.clone().pull_stream(ctx)
        })
        .try_flatten_unordered(None)
        .map_err(Error::from)
        .and_then({
            let puller = self.clone();
            move |ctx: RunContext| puller.clone().pull(ctx)
        }))
    }
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
    let artifact: Arc<_> = PatternSet::new(artifact, |a| a.path)?.into();

    // open db
    info!("Opening database...");
    let mut database = Database::open(&database).await?;

    // check for cache purge
    if args.purge_cache {
        warn!("Purging cache!");
        database.purge_cache().await?;
    }

    // update TagSet
    info!("Updating tags...");
    let tags = tags.upsert_tag_set(&database).await?;

    // purge outdated issues
    let outdated = IssueInfo::delete_all_invalid_by_tag_schema(&database, tags.schema()).await?;
    if outdated > 0 {
        warn!("Purged {outdated} runs' issues that parsed with an outdated tag schema!");
    }

    // purge blocklisted jobs
    let blocked = Job::delete_all_by_blocklist(&database, blocklist.into_iter()).await?;
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

    if Run::has_untagged(&database).await? {
        info!("Parsing unprocessed run logs...");
        let issues = parse_unprocessed_runs(runs, tags.into(), &database).await?;

        info!("Done!");
        info!("----------------------------------------");

        // purge old data
        info!("Purging old runs...");

        JobBuild::delete_all_orphan(&database).await?;

        info!("Purging extraneous tags...");
        TagInfo::delete_all_orphan(&database).await?;

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

        let markup = task::spawn(async move {
            page::render(
                &database,
                &view,
                UtcOffset::from_hms(timezone, 0, 0).unwrap(),
            )
            .unwrap()
            .into_string()
        });

        if let Some(filepath) = output {
            fs::write(&filepath, markup.await?).await?;

            info!("Written to {filepath}");
        } else {
            info!("Dumping to stdout --");
            println!("{}", markup.await?);
        }
    }

    info!("Done!");

    Ok(())
}
