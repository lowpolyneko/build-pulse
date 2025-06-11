use rusqlite::{Connection, OpenFlags};

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
        if let Some(_) = log.id {
            return Err(Box::from("Log has a pre-existing id!"));
        }

        self.conn
            .execute("INSERT INTO logs (data) VALUES (?)", (log.data.as_str(),))?;
        log.id = Some(self.conn.last_insert_rowid());
        Ok(log)
    }
}
