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
};

/// Read [serde] serialized value from `row` and `idx`
macro_rules! read_value {
    ($row:ident, $idx:literal) => {
        from_value($row.get($idx)?).map_err(|e| {
            Error::FromSqlConversionFailure($idx, rusqlite::types::Type::Text, e.into())
        })?
    };
}
macro_rules! try_read_value {
    ($row:ident, $idx:literal) => {
        from_value($row.get($idx)?).map_err(|e| {
            Error::FromSqlConversionFailure($idx, rusqlite::types::Type::Text, e.into())
        })
    };
}

/// Write as [serde] serializable value
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

    /// Last build number
    pub last_build: Option<u32>,
}

/// [Run] stored in [Database]
pub struct Run {
    /// ID of associated [Job]
    pub job: i64,

    /// Build url
    pub build_url: String,

    /// Build `display_name`
    pub display_name: String,

    /// Build number
    pub build_no: u32,

    /// Build status
    pub status: Option<BuildStatus>,

    /// Full console log
    pub log: Option<String>,

    /// Schema [Run] was parsed with
    pub tag_schema: Option<u64>,
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

    /// [Run]s with unknown issues
    pub unknown_issues: u64,

    /// Total [Issue]s found
    pub issues_found: u64,

    /// Counts of each [Tag] found
    pub tag_counts: Vec<(String, String, Severity, u64)>,
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

impl Database {
    /// Open or create an `sqlite3` database at `path` returning [Database]
    pub fn open(path: &str) -> Result<Database> {
        // try to open existing, otherwise create a new one
        let conn = Connection::open(path)?;

        // create the necessary tables
        conn.execute_batch(
            "
            BEGIN;
            CREATE TABLE IF NOT EXISTS jobs (
                id              INTEGER PRIMARY KEY,
                name            TEXT NOT NULL,
                last_build      INTEGER,
                UNIQUE(name)
            ) STRICT;
            CREATE TABLE IF NOT EXISTS runs (
                id              INTEGER PRIMARY KEY,
                build_url       TEXT NOT NULL,
                display_name    TEXT NOT NULL,
                build_no        INTEGER NOT NULL,
                status          TEXT,
                log             TEXT,
                tag_schema      INTEGER,
                job_id          INTEGER NOT NULL,
                UNIQUE(build_url),
                FOREIGN KEY(job_id)
                    REFERENCES jobs(id)
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
                INSERT INTO jobs (name, last_build) VALUES (?, ?)
                    ON CONFLICT(name) DO UPDATE SET
                        last_build = excluded.last_build
                ",
            )?
            .execute((&job.name, job.last_build))?;

        // get the job as a second query in-case of an insert conflict
        self.get_job(&job.name)
    }

    /// Insert a [Run] into [Database]
    pub fn insert_run(&self, run: Run) -> Result<InDatabase<Run>> {
        self.conn
            .prepare_cached(
                "
                INSERT INTO runs (
                    build_url,
                    display_name,
                    build_no,
                    status,
                    log,
                    tag_schema,
                    job_id
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                ",
            )?
            .execute((
                &run.build_url,
                &run.display_name,
                run.build_no,
                write_value!(run.status),
                &run.log,
                run.tag_schema.map(u64::cast_signed),
                run.job,
            ))?;
        Ok(InDatabase::new(self.conn.last_insert_rowid(), run))
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
                match self.get_tag_field(issue.tag)? {
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
            Ok(InDatabase::new(self.get_tag_id(t.name)?, t))
        })
    }

    /// Get a [Job] from [Database]
    pub fn get_job(&self, name: &str) -> Result<InDatabase<Job>> {
        self.conn
            .prepare_cached("SELECT id, last_build FROM jobs WHERE name = ?")?
            .query_one((name,), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    Job {
                        name: name.to_string(),
                        last_build: row.get(1)?,
                    },
                ))
            })
    }

    /// Get a [Run] from [Database]
    pub fn get_run(&self, build_url: &str) -> Result<InDatabase<Run>> {
        self.conn
            .prepare_cached(
                "
                SELECT
                    id,
                    build_url,
                    display_name,
                    build_no,
                    status,
                    log,
                    tag_schema,
                    job_id
                FROM runs WHERE build_url = ?
                ",
            )?
            .query_one((build_url,), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    Run {
                        build_url: row.get(1)?,
                        display_name: row.get(2)?,
                        build_no: row.get(3)?,
                        status: read_value!(row, 4),
                        log: row.get(5)?,
                        tag_schema: row.get::<_, Option<i64>>(6)?.map(i64::cast_unsigned),
                        job: row.get(7)?,
                    },
                ))
            })
    }

    /// Get all [Run]s from [Database]
    pub fn get_all_runs(&self) -> Result<Vec<InDatabase<Run>>> {
        self.conn
            .prepare_cached(
                "
                SELECT
                    id,
                    build_url,
                    display_name,
                    build_no,
                    status,
                    log,
                    tag_schema,
                    job_id
                FROM runs
                ",
            )?
            .query_map((), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    Run {
                        build_url: row.get(1)?,
                        display_name: row.get(2)?,
                        build_no: row.get(3)?,
                        status: read_value!(row, 4),
                        log: row.get(5)?,
                        tag_schema: row.get::<_, Option<i64>>(6)?.map(i64::cast_unsigned),
                        job: row.get(7)?,
                    },
                ))
            })?
            .collect()
    }

    /// Get all [Issue]s from [Database]
    pub fn get_issues<'a>(
        &self,
        run: &'a InDatabase<Run>,
    ) -> Result<Vec<(InDatabase<Issue<'a>>, Severity)>> {
        self.conn
            .prepare_cached(
                "
                SELECT
                    issues.id,
                    snippet_start,
                    snippet_end,
                    tag_id,
                    duplicates,
                    field,
                    severity
                FROM issues
                JOIN tags ON tags.id = issues.tag_id
                WHERE issues.run_id = ?
                ",
            )?
            .query_map((run.id,), |row| {
                Ok((
                    InDatabase::new(
                        row.get(0)?,
                        Issue {
                            snippet: &match read_value!(row, 5) {
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
                    read_value!(row, 6),
                ))
            })?
            .collect()
    }

    /// Get all [Tag]s from [Run]
    pub fn get_tags(&self, run: &InDatabase<Run>) -> Result<Vec<(String, String)>> {
        self.conn
            .prepare_cached(
                "
                SELECT DISTINCT name, desc FROM tags
                JOIN issues ON issues.tag_id = tags.id
                WHERE issues.run_id = ?
                ",
            )?
            .query_map((run.id,), |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    }

    /// Get a [Tag]'s ID from [Database]
    pub fn get_tag_id(&self, name: &str) -> Result<i64> {
        self.conn
            .prepare_cached("SELECT id FROM tags WHERE name = ?")?
            .query_one((name,), |row| row.get(0))
    }

    /// Get a [Tag]'s [Field] from [Database]
    pub fn get_tag_field(&self, id: i64) -> Result<Field> {
        self.conn
            .prepare_cached("SELECT field FROM tags WHERE tags.id = ?")?
            .query_one((id,), |row| try_read_value!(row, 0))
    }

    /// Get all similarities by [Tag] in [Database]
    pub fn get_similarities(&self) -> Result<Vec<(String, String, Vec<String>)>> {
        let mut hm: HashMap<u64, (String, String, Vec<String>)> = HashMap::new();
        self.conn
            .prepare_cached(
                "
                SELECT DISTINCT similarity_hash, name, desc, display_name FROM similarities
                JOIN issues ON issues.id = similarities.issue_id
                JOIN tags ON tags.id = issues.tag_id
                JOIN runs ON runs.id = issues.run_id
                ",
            )?
            .query_map((), |row| {
                Ok((
                    row.get(0).map(i64::cast_unsigned)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            })?
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .for_each(|(hash, name, desc, id)| {
                hm.entry(hash)
                    .or_insert((name, desc, Vec::new()))
                    .2
                    .push(id)
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

        // runs with unknown issues are runs
        stats.unknown_issues = self
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

        stats.tag_counts = self
            .conn
            .prepare(
                "
                SELECT name, desc, severity, COUNT(*) FROM issues
                JOIN tags ON tags.id = issues.tag_id
                GROUP BY issues.tag_id
                ",
            )?
            .query_map((), |row| {
                Ok((row.get(0)?, row.get(1)?, read_value!(row, 2), row.get(3)?))
            })?
            .collect::<Result<Vec<_>>>()?;

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
                    WHERE runs.tag_schema = ?
            )
            ",
            (current_schema.cast_signed(),),
        )?;

        // then issues
        tx.execute(
            "
            DELETE FROM issues WHERE id IN (
                SELECT i.id FROM issues i
                INNER JOIN runs r ON i.run_id = r.id
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

    /// Remove all [Run]s which aren't referenced by [Job] from [Database]
    pub fn purge_old_runs(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            BEGIN;
            DELETE FROM similarities WHERE similarity_hash IN (
                SELECT DISTINCT similarities.similarity_hash FROM similarities
                    JOIN issues ON issues.id = similarities.issue_id
                    JOIN runs ON runs.id = issues.run_id
                    JOIN jobs ON jobs.id = runs.job_id
                    WHERE build_no != last_build
            );
            DELETE FROM issues WHERE id IN (
                SELECT issues.id FROM issues
                    JOIN runs ON runs.id = issues.run_id
                    JOIN jobs ON jobs.id = runs.job_id
                    WHERE build_no != last_build
            );
            DELETE FROM runs WHERE id IN (
                SELECT runs.id FROM runs
                    JOIN jobs ON jobs.id = runs.job_id
                    WHERE build_no != last_build
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
            DELETE FROM jobs;
            DELETE FROM runs;
            DELETE FROM issues;
            DELETE FROM tags;
            DELETE FROM similarities;
            COMMIT;
            ",
        )
    }
}
