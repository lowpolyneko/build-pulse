use std::ops::{Deref, DerefMut};

use jenkins_api::build::BuildStatus;
use rusqlite::{Connection, Result};

use crate::parse::{Tag, TagSet};

pub struct Database {
    conn: Connection,
}

pub struct Run {
    pub build_url: String,
    pub display_name: String,
    pub status: Option<BuildStatus>,
    pub log: Option<String>,
}

pub struct Issue<'a> {
    pub snippet: &'a str,
    pub tag: i64,
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
                log             TEXT
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
                UNIQUE(name)
            ) STRICT;
            COMMIT;
            ",
        )?;

        Ok(Database { conn })
    }

    pub fn insert_run(&self, run: Run) -> Result<InDatabase<Run>> {
        self.conn.execute(
            "INSERT INTO runs (build_url, display_name, status, log) VALUES (?, ?, ?, ?)",
            (
                &run.build_url,
                &run.display_name,
                run.status.map(|s| match s {
                    BuildStatus::Aborted => "aborted",
                    BuildStatus::Failure => "failure",
                    BuildStatus::NotBuilt => "not_built",
                    BuildStatus::Success => "success",
                    BuildStatus::Unstable => "unstable",
                }),
                &run.log,
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
            let start = issue
                .snippet
                .as_ptr()
                .offset_from_unsigned(run.log.as_ref().unwrap().as_ptr());
            let end = start + issue.snippet.len();
            self.conn.execute(
                "INSERT INTO issues (snippet_start, snippet_end, run_id, tag_id) VALUES (?, ?, ?, ?)",
                (start, end, run.id, issue.tag),
            )?;
        }
        Ok(InDatabase::new(self.conn.last_insert_rowid(), issue))
    }

    pub fn insert_tags<'a>(&self, tags: TagSet<Tag<'a>>) -> Result<TagSet<InDatabase<Tag<'a>>>> {
        tags.try_swap_tags(|t| {
            self.conn
                .execute("INSERT OR IGNORE INTO tags (name) VALUES (?)", (t.name,))?;

            Ok(InDatabase::new(self.conn.last_insert_rowid(), t))
        })
    }

    pub fn get_run(&self, build_url: &str) -> Result<InDatabase<Run>> {
        self.conn.query_one(
            "SELECT id, build_url, display_name, status, log FROM runs WHERE build_url = ?",
            (build_url,),
            |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    Run {
                        build_url: row.get(1)?,
                        display_name: row.get(2)?,
                        status: row.get::<_, Option<String>>(3)?.map(|s| match s.as_str() {
                            "aborted" => BuildStatus::Aborted,
                            "failure" => BuildStatus::Failure,
                            "not_built" => BuildStatus::NotBuilt,
                            "success" => BuildStatus::Success,
                            "unstable" => BuildStatus::Unstable,
                            _ => panic!("Failed to serialize run status!"),
                        }),
                        log: row.get(4)?,
                    },
                ))
            },
        )
    }

    pub fn get_issues<'a>(&self, run: &'a InDatabase<Run>) -> Result<Vec<InDatabase<Issue<'a>>>> {
        self.conn
            .prepare("SELECT id, snippet_start, snippet_end, run_id, tag_id FROM issues WHERE run_id = ?")?
            .query_map((run.id,), |row| {
                Ok(InDatabase::new(
                    row.get(0)?,
                    Issue {
                        snippet: &run
                            .log
                            .as_ref()
                            .expect("Issue references non-existant log!")
                            [row.get(1)?..row.get(2)?],
                        tag: row.get(4)?,
                    }
                ))
            })?
            .collect()
    }

    pub fn get_tag(&self, name: &str) -> Result<i64> {
        self.conn
            .query_one("SELECT id, name FROM tags WHERE name = ?", (name,), |row| {
                row.get(0)
            })
    }
}
