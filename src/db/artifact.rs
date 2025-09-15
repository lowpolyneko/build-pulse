use crate::{db::Queryable, schema};

/// [Artifact] stored in [super::Database]
pub struct Artifact {
    /// Byte contents of [Artifact]
    pub path: String,

    /// Byte contents of [Artifact]
    pub contents: Vec<u8>,

    /// [super::Run] associated with [Artifact]
    pub run_id: i64,
}

schema! {
    artifacts for Artifact {
        id              INTEGER PRIMARY KEY,
        path            TEXT NOT NULL,
        contents        BLOB NOT NULL,
        run_id          INTEGER NOT NULL REFERENCES runs(id)
    }
}

impl Queryable for Artifact {
    fn map_row(_: ()) -> impl FnMut(&rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        |row| {
            Ok(super::InDatabase::new(
                row.get(0)?,
                Artifact {
                    path: row.get(1)?,
                    contents: row.get(2)?,
                    run_id: row.get(3)?,
                },
            ))
        }
    }

    fn as_params(&self, _: ()) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((&self.path, &self.contents, self.run_id))
    }
}

impl Artifact {
    /// Get all [Artifact] from [super::Database] by [super::Run]
    pub fn select_all_by_run(
        db: &super::Database,
        run_id: i64,
        params: (),
    ) -> rusqlite::Result<Vec<super::InDatabase<Self>>> {
        db.prepare_cached(
            "
                SELECT * FROM artifacts
                WHERE run_id = ?
                ",
        )?
        .query_map((run_id,), Self::map_row(params))?
        .collect()
    }
}
