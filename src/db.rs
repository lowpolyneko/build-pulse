use rusqlite::Connection;

use crate::parse::Parse;

pub struct Database {
    conn: Connection,
}

pub struct Log {
    pub id: Option<i64>,
    pub data: String,
}

pub struct Issue<'a> {
    pub id: Option<i64>,
    pub snippet: &'a str,
}

impl Parse for Log {
    fn get_data(&self) -> &str {
        &self.data
    }
}

impl Database {
    pub fn open(path: &str) -> Result<Database, rusqlite::Error> {
        // try to open existing, otherwise create a new one
        let conn = Connection::open(path)?;

        // create the necessary tables
        conn.execute_batch(
            "
            BEGIN;
            CREATE TABLE IF NOT EXISTS logs (
                id      INTEGER PRIMARY KEY,
                data    TEXT NOT NULL
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

    pub fn insert_log(&self, mut log: Log) -> Result<Log, Box<dyn std::error::Error>> {
        if log.id.is_some() {
            return Err(Box::from("Log has a pre-existing id!"));
        }

        self.conn
            .execute("INSERT INTO logs (data) VALUES (?)", (log.data.as_str(),))?;
        log.id = Some(self.conn.last_insert_rowid());
        Ok(log)
    }

    pub fn insert_issue<'a>(
        &self,
        log: &'a Log,
        mut issue: Issue<'a>,
    ) -> Result<Issue<'a>, Box<dyn std::error::Error>> {
        if issue.id.is_some() {
            return Err(Box::from("Issue has a pre-existing id!"));
        }

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
