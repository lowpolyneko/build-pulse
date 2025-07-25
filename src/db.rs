//! [rusqlite] based ORM to cache build results.
use std::{
    collections::HashMap,
    hash::Hash,
    ops::{Deref, DerefMut},
};

use jenkins_api::build::BuildStatus;
use rusqlite::{Connection, Error, Result};
use serde_json::{from_value, to_value};

use crate::{
    config::{Field, Severity},
    parse::{Tag, TagSet},
    tag_expr::TagExpr,
};

/// Read [serde] serialized value from `row` and `idx`
#[macro_export]
macro_rules! read_value {
    ($row:ident, $idx:literal) => {
        from_value($row.get($idx)?).map_err(|e| {
            Error::FromSqlConversionFailure($idx, rusqlite::types::Type::Text, e.into())
        })?
    };
}

/// Write as [serde] serializable value
#[macro_export]
macro_rules! write_value {
    ($val:expr) => {
        to_value($val).map_err(|e| Error::ToSqlConversionFailure(e.into()))?
    };
}

/// Database object
pub struct Database {
    /// Internal [rusqlite] connection
    conn: Connection,
}

/// [Job] stored in [Database]
pub struct Job {
    /// Unique name of [Job]
    pub name: String,

    /// [Job] url
    pub url: String,

    /// Number of last [JobBuild]
    pub last_build: Option<u32>,
}

/// [JobBuild] in [Database]
pub struct JobBuild {
    /// Build url
    pub url: String,

    /// Build status
    pub status: Option<BuildStatus>,

    /// Build number
    pub number: u32,

    /// Build timestamp
    pub timestamp: u64,

    /// ID of associated [Job]
    pub job_id: i64,
}

/// [Run] stored in [Database]
pub struct Run {
    /// Run url
    pub url: String,

    /// Build status
    pub status: Option<BuildStatus>,

    /// Run `display_name`
    pub display_name: String,

    /// Full console log
    pub log: Option<String>,

    /// Schema [Run] was parsed with
    pub tag_schema: Option<u64>,

    /// ID of associated [JobBuild]
    pub build_id: i64,
}

/// [Issue] stored in [Database]
#[derive(PartialEq, Eq, Hash)]
pub struct Issue<'a> {
    /// String snippet from [Run]
    pub snippet: &'a str,

    /// [Tag] associated with [Issue]
    pub tag: i64,

    /// Number of duplicate emits in the same [Run]
    pub duplicates: u64,
}

/// Statistics of [Issue]s and [Run]s in [Database]
#[derive(Default)]
pub struct Statistics {
    /// Number of [BuildStatus::Failure] [Job]s
    pub failed_jobs: u64,

    /// Total number of tracked [Job]s
    pub total_jobs: u64,

    /// Successful [Run]s
    pub successful: u64,

    /// Unstable [Run]s
    pub unstable: u64,

    /// Failed [Run]s
    pub failures: u64,

    /// Aborted [Run]s
    pub aborted: u64,

    /// Not built [Run]s
    pub not_built: u64,

    /// Total [Issue]s found
    pub issues_found: u64,

    /// [Run]s with unknown issues
    pub unknown_runs: u64,
}

/// List of similar [Run]s by [TagInfo] in [Database]
pub struct Similarity {
    pub tag: InDatabase<TagInfo>,
    pub related: Vec<i64>,
}

#[derive(PartialEq, Eq, Hash)]
pub struct TagInfo {
    /// Name of [Tag]
    pub name: String,

    /// Description of [Tag]
    pub desc: String,

    /// Field of [Tag]
    pub field: Field,

    /// Severity of [Tag]
    pub severity: Severity,
}

/// Represents an item `T` in [Database]
pub struct InDatabase<T> {
    /// Row ID of `item`
    pub id: i64,

    /// Item itself
    item: T,
}

impl<T> InDatabase<T> {
    /// Wrap item in [InDatabase] with new `id` from [Database]
    fn new(id: i64, item: T) -> Self {
        InDatabase { id, item }
    }
}

// Hash only considers the id property for [InDatabase]
impl<T> Hash for InDatabase<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

// Implicit deref to `T` from [InDatabase]
impl<T> Deref for InDatabase<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

// Implicit deref_mut to `T` from [InDatabase]
impl<T> DerefMut for InDatabase<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.item
    }
}

// Establish ordering by the `id` primary key
impl<T> Ord for InDatabase<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
    }
}

impl<T> PartialOrd for InDatabase<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> PartialEq for InDatabase<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T> Eq for InDatabase<T> {}

impl From<&Tag<'_>> for TagInfo {
    fn from(value: &Tag<'_>) -> Self {
        TagInfo {
            name: value.name.to_string(),
            desc: value.desc.to_string(),
            field: *value.from,
            severity: *value.severity,
        }
    }
}

impl Database {
    /// Open or create an `sqlite3` database at `path` returning [Database]
    pub fn open(path: &str) -> Result<Database> {
        // Enable REGEXP
        rusqlite_regex::enable_auto_extension()?;

        // try to open existing, otherwise create a new one
        let conn = Connection::open(path)?;

        // create the necessary tables
        conn.execute_batch(
            "
            BEGIN;
            CREATE TABLE IF NOT EXISTS jobs (
                id              INTEGER PRIMARY KEY,
                name            TEXT NOT NULL,
                url             TEXT NOT NULL,
                last_build      INTEGER,
                UNIQUE(name, url)
            ) STRICT;
            CREATE TABLE IF NOT EXISTS builds (
                id              INTEGER PRIMARY KEY,
                url             TEXT NOT NULL,
                status          TEXT,
                number          INTEGER NOT NULL,
                timestamp       INTEGER NOT NULL,
                job_id          INTEGER NOT NULL,
                UNIQUE(url),
                FOREIGN KEY(job_id)
                    REFERENCES jobs(id)
            ) STRICT;
            CREATE TABLE IF NOT EXISTS runs (
                id              INTEGER PRIMARY KEY,
                url             TEXT NOT NULL,
                status          TEXT,
                display_name    TEXT NOT NULL,
                log             TEXT,
                tag_schema      INTEGER,
                build_id        INTEGER NOT NULL,
                UNIQUE(url),
                FOREIGN KEY(build_id)
                    REFERENCES builds(id)
            ) STRICT;
            CREATE TABLE IF NOT EXISTS issues (
                id              INTEGER PRIMARY KEY,
                snippet_start   INTEGER NOT NULL,
                snippet_end     INTEGER NOT NULL,
                run_id          INTEGER NOT NULL,
                tag_id          INTEGER NOT NULL,
                duplicates      INTEGER NOT NULL,
                FOREIGN KEY(run_id)
                    REFERENCES runs(id),
                FOREIGN KEY(tag_id)
                    REFERENCES tags(id)
            ) STRICT;
            CREATE TABLE IF NOT EXISTS tags (
                id              INTEGER PRIMARY KEY,
                name            TEXT NOT NULL,
                desc            TEXT NOT NULL,
                field           TEXT NOT NULL,
                severity        TEXT NOT NULL,
                UNIQUE(name)
            ) STRICT;
            CREATE TABLE IF NOT EXISTS similarities (
                id              INTEGER PRIMARY KEY,
                similarity_hash INTEGER NOT NULL,
                issue_id        INTEGER NOT NULL,
                FOREIGN KEY(issue_id)
                    REFERENCES issues(id)
            ) STRICT;
            COMMIT;
            ",
        )?;

        Ok(Database { conn })
    }

    /// Upsert a [Job] into [Database]
    pub fn upsert_job(&self, job: Job) -> Result<InDatabase<Job>> {
        self.conn
            .prepare_cached(
                "
                INSERT INTO jobs (
                    name,
                    url,
                    last_build
                ) VALUES (?, ?, ?)
                    ON CONFLICT(name, url) DO UPDATE SET
                        last_build = excluded.last_build
                ",
            )?
            .execute((&job.name, job.url, job.last_build))?;

        // get the job as a second query in-case of an insert conflict
        self.get_job(&job.name)
    }

    /// Upsert a [Run] into [Database]
    pub fn upsert_run(&self, run: Run) -> Result<InDatabase<Run>> {
        self.conn
            .prepare_cached(
                "
                INSERT INTO runs (
                    url,
                    status,
                    display_name,
                    log,
                    tag_schema,
                    build_id
                ) VALUES (?, ?, ?, ?, ?, ?)
                    ON CONFLICT(url) DO UPDATE SET
                        status = excluded.status,
                        display_name = excluded.display_name,
                        log = excluded.log,
                        tag_schema = excluded.tag_schema,
                        build_id = excluded.build_id
                ",
            )?
            .execute((
                &run.url,
                write_value!(run.status),
                &run.display_name,
                &run.log,
                run.tag_schema.map(u64::cast_signed),
                run.build_id,
            ))?;

        self.get_run(&run.url)
    }

    /// Upsert a [JobBuild] into [Database]
    pub fn upsert_build(&self, build: JobBuild) -> Result<InDatabase<JobBuild>> {
        self.conn
            .prepare_cached(
                "
                INSERT INTO builds (
                    url,
                    status,
                    number,
                    timestamp,
                    job_id
                ) VALUES (?, ?, ?, ?, ?)
                    ON CONFLICT(url) DO UPDATE SET
                        status = excluded.status,
                        number = excluded.number,
                        timestamp = excluded.timestamp,
                        job_id = excluded.job_id
                ",
            )?
            .execute((
                &build.url,
                write_value!(build.status),
                build.number,
                build.timestamp.cast_signed(),
                build.job_id,
            ))?;

        self.get_build(build.job_id, build.number)
    }

    /// Insert a [Run]'s [Issue] into [Database]
    pub fn insert_issue<'a>(
        &self,
        run: &'a InDatabase<Run>,
        issue: Issue<'a>,
    ) -> Result<InDatabase<Issue<'a>>> {
        unsafe {
            // SAFETY: `Run` owns all underlying `Issue`s
            let start = issue.snippet.as_ptr().offset_from_unsigned(
                match self.get_tag(issue.tag)?.field {
                    Field::Console => run.log.as_ref().unwrap(),
                    Field::RunName => &run.display_name,
                }
                .as_ptr(),
            );
            let end = start + issue.snippet.len();
            self.conn
                .prepare_cached(
                    "
                    INSERT INTO issues (
                        snippet_start,
                        snippet_end,
                        run_id,
                        tag_id,
                        duplicates
                    ) VALUES (?, ?, ?, ?, ?)
                    ",
                )?
                .execute((
                    start,
                    end,
                    run.id,
                    issue.tag,
                    issue.duplicates.cast_signed(),
                ))?;
        }
        Ok(InDatabase::new(self.conn.last_insert_rowid(), issue))
    }

    /// Insert an [Issue] similarity into [Database]
    pub fn insert_similarity(
        &self,
        similarity_hash: u64,
        issue_id: &InDatabase<Issue>,
    ) -> Result<i64> {
        self.conn
            .prepare_cached(
                "
                INSERT OR IGNORE INTO similarities (
                    similarity_hash,
                    issue_id
                ) VALUES (?, ?)
                ",
            )?
            .execute((similarity_hash.cast_signed(), issue_id.id))?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Upsert a [TagSet] into [Database]
    pub fn upsert_tags<'a>(&self, tags: TagSet<Tag<'a>>) -> Result<TagSet<InDatabase<Tag<'a>>>> {
        let mut stmt = self.conn.prepare(
            "
            INSERT INTO tags (name, desc, field, severity) VALUES (?, ?, ?, ?)
                ON CONFLICT(name) DO UPDATE SET
                    desc = excluded.desc,
                    field = excluded.field,
                    severity = excluded.severity
            ",
        )?;
        tags.try_swap_tags(|t| {
            stmt.execute((
                t.name,
                t.desc,
                write_value!(t.from),
                write_value!(t.severity),
            ))?;

            // get the tag id as a second query in-case of an insert conflict
            Ok(InDatabase::new(self.get_tag_by_name(t.name)?.id, t))
        })
    }

    /// Get a [Job] from [Database]
    pub fn get_job(&self, name: &str) -> Result<InDatabase<Job>> {
        self.conn
            .prepare_cached(
                "
                SELECT
                    id,
                    url,
                    last_build
                FROM jobs WHERE name = ?
                ",
            )?
            .query_one((name,), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    Job {
                        name: name.to_string(),
                        url: row.get(1)?,
                        last_build: row.get(2)?,
                    },
                ))
            })
    }

    /// Get all [Job]s from [Database]
    pub fn get_jobs(&self) -> Result<Vec<InDatabase<Job>>> {
        self.conn
            .prepare_cached(
                "
                SELECT
                    id,
                    name,
                    url,
                    last_build
                FROM jobs
                ",
            )?
            .query_map((), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    Job {
                        name: row.get(1)?,
                        url: row.get(2)?,
                        last_build: row.get(3)?,
                    },
                ))
            })?
            .collect()
    }

    /// Get a [JobBuild] from [Database]
    pub fn get_build(&self, job_id: i64, number: u32) -> Result<InDatabase<JobBuild>> {
        self.conn
            .prepare_cached(
                "
                SELECT
                    id,
                    url,
                    status,
                    timestamp
                FROM builds
                WHERE job_id = ?
                AND number = ?
                ",
            )?
            .query_one((job_id, number), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    JobBuild {
                        url: row.get(1)?,
                        status: read_value!(row, 2),
                        number,
                        timestamp: row.get(3).map(i64::cast_unsigned)?,
                        job_id,
                    },
                ))
            })
    }

    /// Get a [Run] from [Database]
    pub fn get_run(&self, url: &str) -> Result<InDatabase<Run>> {
        self.conn
            .prepare_cached(
                "
                SELECT
                    id,
                    url,
                    status,
                    display_name,
                    log,
                    tag_schema,
                    build_id
                FROM runs WHERE url = ?
                ",
            )?
            .query_one((url,), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    Run {
                        url: row.get(1)?,
                        status: read_value!(row, 2),
                        display_name: row.get(3)?,
                        log: row.get(4)?,
                        tag_schema: row.get::<_, Option<i64>>(5)?.map(i64::cast_unsigned),
                        build_id: row.get(6)?,
                    },
                ))
            })
    }

    /// Get all [Run]s from [Database]
    pub fn get_runs(&self) -> Result<Vec<InDatabase<Run>>> {
        self.conn
            .prepare_cached(
                "
                SELECT
                    id,
                    url,
                    status,
                    display_name,
                    log,
                    tag_schema,
                    build_id
                FROM runs
                ",
            )?
            .query_map((), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    Run {
                        url: row.get(1)?,
                        status: read_value!(row, 2),
                        display_name: row.get(3)?,
                        log: row.get(4)?,
                        tag_schema: row.get::<_, Option<i64>>(5)?.map(i64::cast_unsigned),
                        build_id: row.get(6)?,
                    },
                ))
            })?
            .collect()
    }

    /// Get [Run]s from [Database] by [JobBuild]
    pub fn get_runs_by_build(&self, build: &InDatabase<JobBuild>) -> Result<Vec<InDatabase<Run>>> {
        self.conn
            .prepare_cached(
                "
                SELECT
                    id,
                    url,
                    status,
                    display_name,
                    log,
                    tag_schema,
                    build_id
                FROM runs WHERE build_id = ?
                ",
            )?
            .query_map((build.id,), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    Run {
                        url: row.get(1)?,
                        status: read_value!(row, 2),
                        display_name: row.get(3)?,
                        log: row.get(4)?,
                        tag_schema: row.get::<_, Option<i64>>(5)?.map(i64::cast_unsigned),
                        build_id: row.get(6)?,
                    },
                ))
            })?
            .collect()
    }

    pub fn get_run_display_name(&self, id: i64) -> Result<String> {
        self.conn
            .prepare_cached("SELECT display_name FROM runs WHERE id = ?")?
            .query_one((id,), |row| row.get(0))
    }

    /// Get all [Run] IDs by [TagExpr] in [Database]
    pub fn get_run_ids_by_expr(&self, expr: &TagExpr) -> Result<Vec<i64>> {
        let (stmt, params) = expr.to_sql_select()?;
        self.conn
            .prepare(&stmt)?
            .query_map(params, |row| row.get(0))?
            .collect::<Result<Vec<_>>>()
    }

    /// Get all [Issue]s from [Database]
    pub fn get_issues<'a>(
        &self,
        run: &'a InDatabase<Run>,
    ) -> Result<Vec<(InDatabase<Issue<'a>>, InDatabase<TagInfo>)>> {
        self.conn
            .prepare_cached(
                "
                SELECT
                    issues.id,
                    snippet_start,
                    snippet_end,
                    tag_id,
                    duplicates,
                    name,
                    desc,
                    field,
                    severity
                FROM issues
                JOIN tags ON tags.id = issues.tag_id
                WHERE issues.run_id = ?
                ",
            )?
            .query_map((run.id,), |row| {
                let field = read_value!(row, 7);
                Ok((
                    InDatabase::new(
                        row.get(0)?,
                        Issue {
                            snippet: &match field {
                                Field::Console => run
                                    .log
                                    .as_ref()
                                    .expect("Issue references non-existent log!"),
                                Field::RunName => &run.display_name,
                            }[row.get(1)?..row.get(2)?],
                            tag: row.get(3)?,
                            duplicates: row.get(4).map(i64::cast_unsigned)?,
                        },
                    ),
                    InDatabase::new(
                        row.get(3)?,
                        TagInfo {
                            name: row.get(5)?,
                            desc: row.get(6)?,
                            field,
                            severity: read_value!(row, 8),
                        },
                    ),
                ))
            })?
            .collect()
    }

    /// Get a [Tag]'s [TagInfo] from [Database]
    pub fn get_tag(&self, id: i64) -> Result<InDatabase<TagInfo>> {
        self.conn
            .prepare_cached("SELECT id, name, desc, severity, field FROM tags WHERE tags.id = ?")?
            .query_one((id,), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    TagInfo {
                        name: row.get(1)?,
                        desc: row.get(2)?,
                        severity: read_value!(row, 3),
                        field: read_value!(row, 4),
                    },
                ))
            })
    }

    /// Get a [TagInfo] from [Database] by name
    pub fn get_tag_by_name(&self, name: &str) -> Result<InDatabase<TagInfo>> {
        self.conn
            .prepare_cached("SELECT id, name, desc, severity, field FROM tags WHERE name = ?")?
            .query_one((name,), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    TagInfo {
                        name: row.get(1)?,
                        desc: row.get(2)?,
                        severity: read_value!(row, 3),
                        field: read_value!(row, 4),
                    },
                ))
            })
    }

    /// Get all [TagInfo]s in [Database]
    pub fn get_tags(&self) -> Result<Vec<InDatabase<TagInfo>>> {
        self.conn
            .prepare_cached("SELECT id, name, desc, severity, field FROM tags")?
            .query_map((), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    TagInfo {
                        name: row.get(1)?,
                        desc: row.get(2)?,
                        severity: read_value!(row, 3),
                        field: read_value!(row, 4),
                    },
                ))
            })?
            .collect()
    }

    /// Get all [TagInfo]s from [Run]
    pub fn get_tags_by_run(&self, run: &InDatabase<Run>) -> Result<Vec<InDatabase<TagInfo>>> {
        self.conn
            .prepare_cached(
                "
                SELECT DISTINCT tags.id, name, desc, field, severity FROM tags
                JOIN issues ON tags.id = issues.tag_id
                WHERE issues.run_id = ?
                ",
            )?
            .query_map((run.id,), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    TagInfo {
                        name: row.get(1)?,
                        desc: row.get(2)?,
                        field: read_value!(row, 3),
                        severity: read_value!(row, 4),
                    },
                ))
            })?
            .collect()
    }

    /// Get all similarities by [Tag] in [Database]
    pub fn get_similarities(&self) -> Result<Vec<Similarity>> {
        let mut hm: HashMap<u64, Similarity> = HashMap::new();
        self.conn
            .prepare_cached(
                "
                SELECT DISTINCT similarity_hash, tag_id, run_id FROM similarities
                JOIN issues ON issues.id = similarities.issue_id
                ",
            )?
            .query_map((), |row| {
                Ok((
                    row.get(0).map(i64::cast_unsigned)?,
                    self.get_tag(row.get(1)?)?,
                    row.get(2)?,
                ))
            })?
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .for_each(|(hash, tag, run_id)| {
                hm.entry(hash)
                    .or_insert(Similarity {
                        tag: tag,
                        related: Vec::new(),
                    })
                    .related
                    .push(run_id)
            });

        Ok(hm.into_values().collect())
    }

    /// Gets [Database]'s [Statistics]
    pub fn get_stats(&self) -> Result<Statistics> {
        // calculate success/failures for all runs
        let mut stats = self
            .conn
            .prepare("SELECT status, COUNT(*) FROM runs GROUP BY status")?
            .query_map((), |row| Ok((read_value!(row, 0), row.get::<_, u64>(1)?)))?
            .collect::<Result<Vec<_>>>()?
            .iter()
            .fold(Statistics::default(), |mut stats, (status, count)| {
                match status {
                    Some(BuildStatus::Aborted) => stats.aborted += count,
                    Some(BuildStatus::Failure) => stats.failures += count,
                    Some(BuildStatus::NotBuilt) => stats.not_built += count,
                    Some(BuildStatus::Success) => stats.successful += count,
                    Some(BuildStatus::Unstable) => stats.unstable += count,
                    _ => {}
                };

                stats
            });

        stats.failed_jobs = self
            .conn
            .prepare(
                "
                SELECT COUNT(*) FROM jobs j
                WHERE EXISTS (
                    SELECT 1 FROM builds
                    WHERE builds.job_id = j.id
                    AND status = ?
                )
                ",
            )?
            .query_one((write_value!(Some(BuildStatus::Failure)),), |row| {
                row.get(0)
            })?;

        stats.total_jobs = self
            .conn
            .prepare("SELECT COUNT(*) FROM jobs")?
            .query_one((), |row| row.get(0))?;

        // don't count metadata issues in total
        stats.issues_found = self
            .conn
            .prepare(
                "
                SELECT COUNT(*) FROM issues
                JOIN tags ON tags.id = issues.tag_id
                WHERE tags.severity != ?
                ",
            )?
            .query_one((write_value!(Severity::Metadata),), |row| row.get(0))?;

        stats.unknown_runs = self
            .conn
            .prepare(
                "
                SELECT COUNT(*) FROM runs r
                WHERE r.status = ?
                    AND NOT EXISTS (
                        SELECT 1 FROM issues
                        JOIN tags ON tags.id = issues.tag_id
                        WHERE
                            issues.run_id = r.id
                            AND tags.severity != ?
                    )
                ",
            )?
            .query_one(
                (
                    write_value!(Some(BuildStatus::Failure)),
                    write_value!(Severity::Metadata),
                ),
                |row| row.get(0),
            )?;

        Ok(stats)
    }

    /// Check whether or not there are untagged runs
    pub fn has_untagged_runs(&self) -> Result<bool> {
        self.conn
            .prepare_cached("SELECT 1 FROM runs WHERE tag_schema IS NULL")?
            .exists(())
    }

    /// Update the [TagSet] schema for all [Run]s in [Database]
    pub fn update_tag_schema_for_runs(&self, new_schema: Option<u64>) -> Result<usize> {
        self.conn.execute(
            "UPDATE runs SET tag_schema = ?",
            (new_schema.map(u64::cast_signed),),
        )
    }

    /// Remove all [Issue]s with an outdated [TagSet] schema from [Database]
    pub fn purge_invalid_issues_by_tag_schema(&mut self, current_schema: u64) -> Result<usize> {
        let mut tx = self.conn.transaction()?;
        tx.set_drop_behavior(rusqlite::DropBehavior::Commit);

        // delete similarities first
        tx.execute(
            "
            DELETE FROM similarities WHERE similarity_hash IN (
                SELECT DISTINCT similarities.similarity_hash FROM similarities
                JOIN issues ON issues.id = similarities.issue_id
                JOIN runs ON runs.id = issues.run_id
                WHERE runs.tag_schema != ?
            )
            ",
            (current_schema.cast_signed(),),
        )?;

        // then issues
        tx.execute(
            "
            DELETE FROM issues WHERE id IN (
                SELECT i.id FROM issues i
                JOIN runs r ON i.run_id = r.id
                WHERE r.tag_schema != ?
            )
            ",
            (current_schema.cast_signed(),),
        )?;

        // also set the run tag_schema to NULL to indicate an unparsed run
        tx.execute(
            "UPDATE runs SET tag_schema = NULL WHERE tag_schema != ?",
            (current_schema.cast_signed(),),
        )
    }

    /// Remove all [Job]s from [Database] by name
    pub fn purge_blocklisted_jobs(&mut self, names: &[String]) -> Result<usize> {
        let mut tx = self.conn.transaction()?;
        tx.set_drop_behavior(rusqlite::DropBehavior::Commit);

        names.iter().try_fold(0, |acc, name| {
            // delete similarities first
            tx.execute(
                "
                DELETE FROM similarities WHERE similarity_hash IN (
                    SELECT DISTINCT similarities.similarity_hash FROM similarities
                    JOIN issues ON issues.id = similarities.issue_id
                    JOIN runs ON runs.id = issues.run_id
                    JOIN builds ON builds.id = runs.build_id
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE name = ?
                )
                ",
                (name,),
            )?;

            // then issues
            tx.execute(
                "
                DELETE FROM issues WHERE id IN (
                    SELECT issues.id FROM issues
                    JOIN runs ON runs.id = issues.run_id
                    JOIN builds ON builds.id = runs.build_id
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE name = ?
                )
                ",
                (name,),
            )?;

            // then runs
            tx.execute(
                "
                DELETE FROM runs WHERE id IN (
                    SELECT runs.id FROM runs
                    JOIN builds ON builds.id = runs.build_id
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE name = ?
                );
                ",
                (name,),
            )?;

            // then builds
            tx.execute(
                "
                DELETE FROM builds WHERE id IN (
                    SELECT builds.id FROM builds
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE name = ?
                );
                ",
                (name,),
            )?;

            // finally the job
            Ok(acc + tx.execute("DELETE FROM jobs WHERE name = ?", (name,))?)
        })
    }

    /// Remove all [JobBuild]s which aren't referenced by [Job] from [Database]
    pub fn purge_old_builds(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            BEGIN;
            DELETE FROM similarities WHERE similarity_hash IN (
                SELECT DISTINCT similarities.similarity_hash FROM similarities
                JOIN issues ON issues.id = similarities.issue_id
                JOIN runs ON runs.id = issues.run_id
                JOIN builds ON builds.id = runs.build_id
                JOIN jobs ON jobs.id = builds.job_id
                WHERE number != last_build
            );
            DELETE FROM issues WHERE id IN (
                SELECT issues.id FROM issues
                JOIN runs ON runs.id = issues.run_id
                JOIN builds ON builds.id = runs.build_id
                JOIN jobs ON jobs.id = builds.job_id
                WHERE number != last_build
            );
            DELETE FROM runs WHERE id IN (
                SELECT runs.id FROM runs
                JOIN builds ON builds.id = runs.build_id
                JOIN jobs ON jobs.id = builds.job_id
                WHERE number != last_build
            );
            DELETE FROM builds WHERE id IN (
                SELECT builds.id FROM builds
                JOIN jobs ON jobs.id = builds.job_id
                WHERE number != last_build
            );
            COMMIT;
            ",
        )
    }

    /// Remove all [Tag]s which aren't referenced by [Issue]s from [Database]
    pub fn purge_orphan_tags(&self) -> Result<usize> {
        self.conn.execute(
            "
            DELETE FROM tags WHERE NOT EXISTS (
                SELECT 1 FROM issues
                WHERE tags.id = issues.tag_id
            )
            ",
            (),
        )
    }

    /// Purge all rows (but not tables) from [Database]
    pub fn purge_cache(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            BEGIN;
            DELETE FROM similarities;
            DELETE FROM issues;
            DELETE FROM runs;
            DELETE FROM builds;
            DELETE FROM jobs;
            DELETE FROM tags;
            COMMIT;
            ",
        )
    }
}
