use jenkins_api::build::BuildStatus;

use crate::{
    db::{JobBuild, Queryable, Upsertable},
    read_value, schema,
    tag_expr::TagExpr,
    write_value,
};

/// [Run] stored in [super::Database]
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

schema! {
    runs for Run {
        id              INTEGER PRIMARY KEY,
        url             TEXT NOT NULL UNIQUE,
        status          TEXT,
        display_name    TEXT NOT NULL,
        log             TEXT,
        tag_schema      INTEGER,
        build_id        INTEGER NOT NULL REFERENCES (builds)
    }
}

impl Queryable for Run {
    fn map_row(_: ()) -> impl FnMut(&rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        |row| {
            Ok(super::InDatabase::new(
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
        }
    }

    fn as_params(&self, _: ()) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((
            &self.url,
            write_value!(self.status),
            &self.display_name,
            &self.log,
            self.tag_schema.map(u64::cast_signed),
            self.build_id,
        ))
    }
}

impl Upsertable for Run {
    fn upsert(self, db: &super::Database, params: ()) -> rusqlite::Result<super::InDatabase<Self>> {
        db.conn
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
            .execute(self.as_params(params)?)?;

        Self::select_by_url(db, &self.url, ())
    }
}

impl Run {
    fn select_by_url(
        db: &super::Database,
        url: &str,
        params: (),
    ) -> rusqlite::Result<super::InDatabase<Self>> {
        db.conn
            .prepare_cached(
                "
                SELECT * FROM runs
                WHERE url = ?
                ",
            )?
            .query_one((url,), Self::map_row(params))
    }

    fn select_by_build(
        db: &super::Database,
        build: &super::InDatabase<JobBuild>,
        params: (),
    ) -> rusqlite::Result<Vec<super::InDatabase<Self>>> {
        db.conn
            .prepare_cached(
                "
                SELECT * FROM runs
                WHERE build_id = ?
                ",
            )?
            .query_map((build.id,), Self::map_row(params))?
            .collect()
    }

    fn select_ids_by_expr(db: &super::Database, expr: &TagExpr) -> rusqlite::Result<Vec<i64>> {
        let (stmt, params) = expr.to_sql_select()?;
        db.conn
            .prepare(&stmt)?
            .query_map(params, |row| row.get(0))?
            .collect()
    }

    fn select_display_name(db: &super::Database, id: i64) -> rusqlite::Result<String> {
        db.conn
            .prepare_cached("SELECT display_name FROM runs WHERE id = ?")?
            .query_one((id,), |row| row.get(0))
    }

    /// Check whether or not there are untagged runs
    pub fn has_untagged(db: &super::Database) -> rusqlite::Result<bool> {
        db.conn
            .prepare_cached("SELECT 1 FROM runs WHERE tag_schema IS NULL")?
            .exists(())
    }

    /// Update the [TagSet] schema for all [Run]s in [Database]
    pub fn update_tag_schema_for_runs(
        db: &super::Database,
        new_schema: Option<u64>,
    ) -> rusqlite::Result<usize> {
        db.conn.execute(
            "UPDATE runs SET tag_schema = ?",
            (new_schema.map(u64::cast_signed),),
        )
    }
}
