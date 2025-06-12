use rusqlite::{Connection, Result};

use crate::parse::Parse;

pub struct Database {
    conn: Connection,
}

pub struct Log {
    pub id: Option<i64>,
    pub build_url: String,
    pub data: String,
}

pub struct Issue<'a> {
    pub id: Option<i64>,
    pub snippet: &'a str,
}

impl Parse for Log {
    fn data(&self) -> &str {
        &self.data
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
            CREATE TABLE IF NOT EXISTS logs (
                id          INTEGER PRIMARY KEY,
                build_url   TEXT NOT NULL,
                data        TEXT NOT NULL
            ) STRICT;
            CREATE TABLE IF NOT EXISTS issues (
                id              INTEGER PRIMARY KEY,
                snippet_start   INTEGER NOT NULL,
                snippet_end     INTEGER NOT NULL,
                log_id          INTEGER NOT NULL,
                FOREIGN KEY(log_id)
                    REFERENCES logs(id)
            ) STRICT;
            COMMIT;
            ",
        )?;

        Ok(Database { conn })
    }

    pub fn insert_log(&self, mut log: Log) -> Result<Log> {
        assert!(log.id.is_none(), "Log has a pre-existing id!");

        self.conn.execute(
            "INSERT INTO logs (build_url, data) VALUES (?, ?)",
            (log.build_url.as_str(), log.data.as_str()),
        )?;
        log.id = Some(self.conn.last_insert_rowid());
        Ok(log)
    }

    pub fn insert_issue<'a>(&self, log: &'a Log, mut issue: Issue<'a>) -> Result<Issue<'a>> {
        assert!(issue.id.is_none(), "Issue has a pre-existing id!");

        unsafe {
            // SAFETY: `Log` owns all underlying `Issue`s
            let start = issue
                .snippet
                .as_ptr()
                .offset_from_unsigned(log.data.as_ptr());
            let end = start + issue.snippet.len();
            self.conn.execute(
                "INSERT INTO issues (snippet_start, snippet_end, log_id) VALUES (?, ?, ?)",
                (start, end, log.id),
            )?;
        }
        issue.id = Some(self.conn.last_insert_rowid());
        Ok(issue)
    }
}
