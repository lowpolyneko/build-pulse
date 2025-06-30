use std::{fs, sync::Mutex};

use anyhow::{Error, Result};
use clap::{Parser, crate_name, crate_version};
use env_logger::Env;
use jenkins_api::{Jenkins, JenkinsBuilder, build::BuildStatus};
use log::{Level, info, log, warn};
use rayon::prelude::*;
use time::UtcOffset;

use crate::{
    api::{AsJob, AsRun, SparseMatrixProject},
    config::{Config, Field, Severity},
    db::{Database, InDatabase},
    parse::{Tag, TagSet},
};

mod api;
mod config;
mod db;
mod page;
mod parse;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(default_value = "config.toml")]
    config: String,

    #[arg(short, long)]
    output: Option<Option<String>>,

    #[arg(short, long)]
    purge_cache: bool,
}

fn pull_build_logs(
    project: &SparseMatrixProject,
    jenkins: &Jenkins,
    db: &Mutex<Database>,
) -> Result<()> {
    project
        .jobs
        .par_iter()
        .filter_map(|job| {
            job.last_build.as_ref().map_or_else(
                || {
                    info!("Job '{}' has no builds.", job.name);
                    None
                },
                |build| Some((job, build)),
            )
        })
        .flat_map(|(job, build)| build.runs.par_iter().map(move |mb| (job, build, mb)))
        .filter(|(_, build, mb)| mb.number == build.number)
        .map(|(job, build, mb)| {
            Ok((
                db.lock()
                    .unwrap()
                    .upsert_job(job.as_job())
                    .map_err(Error::from)?,
                build,
                mb,
            ))
        })
        .filter_map(|res| match res {
            Ok((db_job, build, mb)) => match db.lock().unwrap().get_run(&mb.url) {
                Ok(_) => None,
                Err(rusqlite::Error::QueryReturnedNoRows) => Some(Ok((db_job, build, mb))),
                Err(e) => Some(Err(Error::from(e))),
            },
            Err(e) => Some(Err(e)),
        })
        .map(|res| match res {
            Ok((db_job, build, mb)) => Ok((
                db_job,
                build,
                mb.get_full_build(jenkins).map_err(Error::from_boxed)?,
            )),
            Err(e) => Err(e),
        })
        .map(|res| match res {
            Ok((db_job, build, full_mb)) => {
                let id = db_job.id;
                Ok((db_job, build, full_mb.as_run(id, jenkins)))
            }
            Err(e) => Err(e),
        })
        .map(|res| match res {
            Ok((db_job, build, run)) => Ok((db_job, build, db.lock().unwrap().insert_run(run)?)),
            Err(e) => Err(e),
        })
        .try_for_each(|res| match res {
            Ok((db_job, build, committed_run)) => {
                log!(
                    match committed_run.status {
                        Some(BuildStatus::Failure | BuildStatus::Unstable) => Level::Warn,
                        _ => Level::Info,
                    },
                    "Job '{}{}' run '{}' finished with status {:?}.",
                    db_job.name,
                    build.display_name,
                    committed_run.display_name,
                    committed_run.status
                );

                Ok(())
            }
            Err(e) => Err(e),
        })
}

fn parse_unprocessed_runs(tags: &TagSet<InDatabase<Tag>>, db: &Mutex<Database>) -> Result<()> {
    let runs = db.lock().unwrap().get_all_runs()?;

    runs.par_iter()
        .flat_map_iter(|run| Field::iter().map(move |f| (run, f)))
        .filter_map(|(run, field)| match (run.tag_schema, &run.log, field) {
            (None, Some(log), Field::Console) => Some((run, field, log)),
            (None, Some(_), Field::RunName) => Some((run, field, &run.display_name)),
            _ => None,
        })
        .flat_map_iter(|(run, field, data)| {
            tags.grep_tags(data, field).map(move |t| (run, data, t))
        })
        .flat_map_iter(|(run, data, t)| t.grep_issue(data).map(move |i| (run, t, i)))
        .try_for_each(|(run, t, i)| {
            db.lock().unwrap().insert_issue(run, i)?;

            if !matches!(t.severity, Severity::Metadata) {
                warn!(
                    "Found issue tagged '{}' in run '{}'",
                    t.name, run.display_name
                );
            }

            Ok::<_, Error>(())
        })?;

    // batch update tag schema for runs afterwards
    db.lock()
        .unwrap()
        .update_tag_schema_for_runs(Some(tags.schema()))?;

    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    // initialize logging
    env_logger::init_from_env(Env::default().default_filter_or("info"));
    info!("{} {}", crate_name!(), crate_version!());

    // load config
    info!("Compiling issue patterns...");
    let config: Config = toml::from_str(&fs::read_to_string(args.config)?)?;
    let tags = TagSet::from_config(&config.tag)?;

    // open db
    info!("Opening database...");
    let mut database = Database::open(&config.database)?;

    // check for cache purge
    if args.purge_cache {
        warn!("Purging cache!");
        database.purge_cache()?;
    }

    // update TagSet
    info!("Updating tags...");
    let tags = database.upsert_tags(tags)?;

    // purge outdated issues
    let outdated = database.purge_invalid_issues_by_tag_schema(tags.schema())?;
    if outdated > 0 {
        warn!("Purged {outdated} issues that parsed with an outdated tag schema!");
    }

    info!(
        "Pulling associated jobs for {} from {}...",
        config.project, config.jenkins_url
    );

    let jenkins = JenkinsBuilder::new(&config.jenkins_url);
    let jenkins = match config.username {
        Some(user) => jenkins.with_user(&user, config.password.as_deref()),
        None => jenkins,
    }
    .build()
    .map_err(Error::from_boxed)?;

    info!("Pulling build info for each job...");
    info!("----------------------------------------");

    let project = SparseMatrixProject::pull_jobs(&jenkins, &config.project)?;

    let database = Mutex::new(database);
    pull_build_logs(&project, &jenkins, &database)?;

    info!("Done!");
    info!("----------------------------------------");
    info!("Parsing unprocessed run logs...");

    parse_unprocessed_runs(&tags, &database)?;

    info!("Done!");
    info!("----------------------------------------");

    let database = database.into_inner().unwrap();

    info!("Purging old runs...");
    database.purge_old_runs()?;

    info!("Purging extraneous tags...");
    database.purge_orphan_tags()?;

    if let Some(output) = args.output {
        info!("Generating report...");

        let markup = page::render(
            &project,
            &database,
            UtcOffset::from_hms(config.timezone, 0, 0)?,
        )
        .into_string();

        if let Some(filepath) = output {
            fs::write(&filepath, markup)?;

            info!("Written to {filepath}");
        } else {
            info!("Dumping to stdout --");
            println!("{markup}");
        }
    }

    info!("Done!");

    Ok(())
}
