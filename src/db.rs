use jenkins_api::build::BuildStatus;
use rusqlite::{Connection, Result};

use crate::parse::Parse;

pub struct Database {
    conn: Connection,
}

pub struct Run {
    pub id: Option<i64>,
    pub build_url: String,
    pub display_name: String,
    pub status: Option<BuildStatus>,
    pub log: Option<String>,
}

pub struct Issue<'a> {
    pub id: Option<i64>,
    pub snippet: &'a str,
}

impl Parse for Run {
    fn data(&self) -> &str {
        self.log.as_ref().map_or("", |s| s)
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
                log_id          INTEGER NOT NULL,
                FOREIGN KEY(log_id)
                    REFERENCES runs(id)
            ) STRICT;
            COMMIT;
            ",
        )?;

        Ok(Database { conn })
    }

    pub fn insert_run(&self, mut run: Run) -> Result<Run> {
        assert!(run.id.is_none(), "Run has a pre-existing id!");

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
        run.id = Some(self.conn.last_insert_rowid());
        Ok(run)
    }

    pub fn insert_issue<'a>(&self, run: &'a Run, mut issue: Issue<'a>) -> Result<Issue<'a>> {
        assert!(issue.id.is_none(), "Issue has a pre-existing id!");
        assert!(run.log.is_some(), "Issue references non-existant log!");

        unsafe {
            // SAFETY: `Run` owns all underlying `Issue`s
            let start = issue
                .snippet
                .as_ptr()
                .offset_from_unsigned(run.log.as_ref().unwrap().as_ptr());
            let end = start + issue.snippet.len();
            self.conn.execute(
                "INSERT INTO issues (snippet_start, snippet_end, log_id) VALUES (?, ?, ?)",
                (start, end, run.id),
            )?;
        }
        issue.id = Some(self.conn.last_insert_rowid());
        Ok(issue)
    }

    pub fn get_run(&self, build_url: &str) -> Result<Run> {
        self.conn.query_one(
            "SELECT id, build_url, display_name, status, log FROM runs WHERE build_url = ?",
            (build_url,),
            |row| {
                Ok(Run {
                    id: row.get(0)?,
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
                })
            },
        )
    }

    pub fn get_issues<'a>(&self, run: &'a Run) -> Result<Vec<Issue<'a>>> {
        self.conn
            .prepare("SELECT id, snippet_start, snippet_end, log_id FROM issues WHERE log_id = ?")?
            .query_map((run.id.expect("Log has not been committed!"),), |row| {
                Ok(Issue {
                    id: Some(row.get(0)?),
                    snippet: &run
                        .log
                        .as_ref()
                        .expect("Issue references non-existant log!")
                        [row.get(1)?..row.get(2)?],
                })
            })?
            .collect::<Result<Vec<_>, _>>()
    }
}
