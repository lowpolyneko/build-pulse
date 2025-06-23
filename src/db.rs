use std::ops::{Deref, DerefMut};

use jenkins_api::build::BuildStatus;
use rusqlite::{Connection, Error, Result};
use serde_json::{from_value, to_value};

use crate::{
    config::Field,
    parse::{Tag, TagSet},
};

macro_rules! read_value {
    ($row:ident, $idx:literal) => {
        from_value($row.get($idx)?).map_err(|e| {
            Error::FromSqlConversionFailure($idx, rusqlite::types::Type::Text, e.into())
        })?
    };
}

macro_rules! write_value {
    ($val:expr) => {
        to_value($val).map_err(|e| Error::ToSqlConversionFailure(e.into()))?
    };
}

pub struct Database {
    conn: Connection,
}

pub struct Run {
    pub build_url: String,
    pub display_name: String,
    pub status: Option<BuildStatus>,
    pub log: Option<String>,
    pub tag_schema: Option<u64>,
}

pub struct Issue<'a> {
    pub snippet: &'a str,
    pub tag: i64,
}

#[derive(Default)]
pub struct Statistics {
    pub successful: u64,
    pub unstable: u64,
    pub failures: u64,
    pub aborted: u64,
    pub not_built: u64,
    pub issues_found: u64,
    pub tag_counts: Vec<(String, String, u64)>,
}

pub struct InDatabase<T> {
    pub id: i64,
    item: T,
}

impl<T> InDatabase<T> {
    fn new(id: i64, item: T) -> Self {
        InDatabase { id, item }
    }
}

impl<T> Deref for InDatabase<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

impl<T> DerefMut for InDatabase<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.item
    }
}

impl Database {
    pub fn open(path: &str) -> Result<Database> {
        // try to open existing, otherwise create a new one
        let conn = Connection::open(path)?;

        // create the necessary tables
        conn.execute_batch(
            "
            BEGIN;
            CREATE TABLE IF NOT EXISTS runs (
                id              INTEGER PRIMARY KEY,
                build_url       TEXT NOT NULL,
                display_name    TEXT NOT NULL,
                status          TEXT,
                log             TEXT,
                tag_schema      INTEGER
            ) STRICT;
            CREATE TABLE IF NOT EXISTS issues (
                id              INTEGER PRIMARY KEY,
                snippet_start   INTEGER NOT NULL,
                snippet_end     INTEGER NOT NULL,
                run_id          INTEGER NOT NULL,
                tag_id          INTEGER NOT NULL,
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
            COMMIT;
            ",
        )?;

        Ok(Database { conn })
    }

    pub fn insert_run(&self, run: Run) -> Result<InDatabase<Run>> {
        self.conn.prepare_cached(
            "INSERT INTO runs (build_url, display_name, status, log, tag_schema) VALUES (?, ?, ?, ?, ?)")?
            .execute(
                (
                    &run.build_url,
                    &run.display_name,
                    write_value!(run.status),
                    &run.log,
                    run.tag_schema.map(u64::cast_signed),
                ),
            )?;
        Ok(InDatabase::new(self.conn.last_insert_rowid(), run))
    }

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
            self.conn.prepare_cached(
                "INSERT INTO issues (snippet_start, snippet_end, run_id, tag_id) VALUES (?, ?, ?, ?)")?
                .execute((start, end, run.id, issue.tag))?;
        }
        Ok(InDatabase::new(self.conn.last_insert_rowid(), issue))
    }

    pub fn insert_tags<'a>(&self, tags: TagSet<Tag<'a>>) -> Result<TagSet<InDatabase<Tag<'a>>>> {
        // TODO: Prune old tags
        let mut stmt = self.conn.prepare(
            "INSERT OR REPLACE INTO tags (name, desc, field, severity) VALUES (?, ?, ?, ?)",
        )?;
        tags.try_swap_tags(|t| {
            stmt.execute((
                t.name,
                t.desc,
                write_value!(t.from),
                write_value!(t.severity),
            ))?;

            Ok(InDatabase::new(self.conn.last_insert_rowid(), t))
        })
    }

    pub fn get_run(&self, build_url: &str) -> Result<InDatabase<Run>> {
        self.conn.prepare_cached(
            "SELECT id, build_url, display_name, status, log, tag_schema FROM runs WHERE build_url = ?")?
            .query_one((build_url,), |row| {
                    Ok(InDatabase::new(
                        row.get(0)?,
                        Run {
                            build_url: row.get(1)?,
                            display_name: row.get(2)?,
                            status: read_value!(row, 3),
                            log: row.get(4)?,
                            tag_schema: row.get::<_, Option<i64>>(5)?.map(i64::cast_unsigned),
                        },
                    ))
                },
            )
    }

    pub fn get_all_runs(&self) -> Result<Vec<InDatabase<Run>>> {
        self.conn
            .prepare_cached(
                "SELECT id, build_url, display_name, status, log, tag_schema FROM runs",
            )?
            .query_map((), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    Run {
                        build_url: row.get(1)?,
                        display_name: row.get(2)?,
                        status: read_value!(row, 3),
                        log: row.get(4)?,
                        tag_schema: row.get::<_, Option<i64>>(5)?.map(i64::cast_unsigned),
                    },
                ))
            })?
            .collect()
    }

    pub fn get_issues<'a>(&self, run: &'a InDatabase<Run>) -> Result<Vec<InDatabase<Issue<'a>>>> {
        self.conn
            .prepare_cached("SELECT issues.id, snippet_start, snippet_end, run_id, tag_id, field FROM issues JOIN tags ON tags.id = issues.tag_id WHERE issues.run_id = ?")?
            .query_map((run.id,), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    Issue {
                        snippet: &match read_value!(row, 5) {
                                Field::Console => run
                                                    .log
                                                    .as_ref()
                                                    .expect("Issue references non-existant log!"),
                                Field::RunName => &run.display_name,
                            }
                            [row.get(1)?..row.get(2)?],
                        tag: row.get(4)?,
                    }
                ))
            })?
            .collect()
    }

    pub fn get_tags(&self, run: &InDatabase<Run>) -> Result<Vec<(String, String)>> {
        self.conn
            .prepare_cached(
                "SELECT DISTINCT name, desc FROM tags JOIN issues ON issues.tag_id = tags.id WHERE issues.run_id = ?",
            )?
            .query_map((run.id,), |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    }

    pub fn get_tag_field(&self, id: i64) -> Result<Field> {
        self.conn
            .prepare_cached("SELECT field FROM tags WHERE tags.id = ?")?
            .query_one((id,), |row| {
                from_value(row.get(0)?).map_err(|e| {
                    Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
                })
            })
    }

    pub fn get_stats(&self) -> Result<Statistics> {
        // calculate success/failures for all runs
        let mut stats = self
            .conn
            .prepare("SELECT status,COUNT(*) FROM runs GROUP BY status")?
            .query_map((), |row| {
                Ok((
                    from_value::<Option<BuildStatus>>(row.get(0)?).map_err(|e| {
                        Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
                    })?,
                    row.get::<_, u64>(1)?,
                ))
            })?
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

        stats.issues_found = self
            .conn
            .prepare("SELECT COUNT(*) FROM issues")?
            .query_one((), |row| row.get(0))?;

        stats.tag_counts = self
            .conn
            .prepare("SELECT name, desc, COUNT(*) FROM issues JOIN tags ON tags.id = issues.tag_id GROUP BY issues.tag_id")?
            .query_map((), |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>>>()?;

        Ok(stats)
    }
    pub fn update_tag_schema_for_runs(&self, new_schema: Option<u64>) -> Result<usize> {
        self.conn.execute(
            "UPDATE runs SET tag_schema = ?",
            (new_schema.map(u64::cast_signed),),
        )
    }

    pub fn purge_invalid_issues_by_tag_schema(&mut self, current_schema: u64) -> Result<usize> {
        let mut tx = self.conn.transaction()?;
        tx.set_drop_behavior(rusqlite::DropBehavior::Commit);

        tx.execute(
            "DELETE FROM issues WHERE ROWID IN (SELECT i.ROWID FROM issues i INNER JOIN runs r ON i.run_id = r.id WHERE r.tag_schema != ?)",
            (current_schema.cast_signed(),),
        )?;

        // also set the run tag_schema to NULL to indicate an unparsed run
        tx.execute(
            "UPDATE runs SET tag_schema = NULL WHERE tag_schema != ?",
            (current_schema.cast_signed(),),
        )
    }

    pub fn purge_cache(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            BEGIN;
            DELETE FROM runs;
            DELETE FROM issues;
            DELETE FROM tags;
            COMMIT;
            ",
        )
    }
}
