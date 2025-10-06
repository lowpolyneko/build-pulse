use arcstr::ArcStr;
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
    pub url: ArcStr,

    /// Build status
    pub status: Option<BuildStatus>,

    /// Run `display_name`
    pub display_name: ArcStr,

    /// Full console log
    pub log: Option<ArcStr>,

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
        build_id        INTEGER NOT NULL REFERENCES builds(id)
    }
}

impl Queryable<'_> for Run {
    fn map_row(row: &rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        Ok(super::InDatabase::new(
            row.get(0)?,
            Run {
                url: row.get::<_, String>(1)?.into(),
                status: read_value!(row, 2),
                display_name: row.get::<_, String>(3)?.into(),
                log: row.get::<_, Option<String>>(4)?.map(ArcStr::from),
                tag_schema: row.get::<_, Option<i64>>(5)?.map(i64::cast_unsigned),
                build_id: row.get(6)?,
            },
        ))
    }

    fn as_params(&self) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((
            self.url.as_str(),
            write_value!(self.status),
            self.display_name.as_str(),
            self.log.as_ref().map(|s| s.as_str()),
            self.tag_schema.map(u64::cast_signed),
            self.build_id,
        ))
    }
}

impl Upsertable<'_> for Run {
    async fn upsert(self, db: &super::Database) -> rusqlite::Result<super::InDatabase<Self>> {
        let url = self.url.clone();
        db.call(move |conn| {
            conn.prepare_cached(
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
            .execute(self.as_params()?)
        })
        .await?;

        Self::select_one_by_url(db, url).await
    }
}

impl Run {
    /// Get a [Run] from [super::Database] by url
    pub async fn select_one_by_url(
        db: &super::Database,
        url: ArcStr,
    ) -> rusqlite::Result<super::InDatabase<Self>> {
        db.call(move |conn| {
            conn.prepare_cached(
                "
                SELECT * FROM runs
                WHERE url = ?
                ",
            )?
            .query_one((url.as_str(),), Self::map_row)
        })
        .await
    }

    /// Get all [Run]s by [super::JobBuild]
    pub async fn select_all_by_build(
        db: &super::Database,
        build: &super::InDatabase<JobBuild>,
    ) -> rusqlite::Result<Vec<super::InDatabase<Self>>> {
        let id = build.id;
        db.call(move |conn| {
            conn.prepare_cached(
                "
                SELECT * FROM runs
                WHERE build_id = ?
                ",
            )?
            .query_and_then((id,), Self::map_row)?
            .collect()
        })
        .await
    }

    /// Get all [Run] ID by [TagExpr] in [super::Database]
    pub async fn select_all_id_by_expr(
        db: &super::Database,
        expr: &TagExpr,
    ) -> rusqlite::Result<Vec<i64>> {
        let (stmt, params) = expr.to_sql_select()?;
        db.call(move |conn| {
            conn.prepare(&stmt)?
                .query_and_then(rusqlite::params_from_iter(params), |row| row.get(0))?
                .collect()
        })
        .await
    }

    /// Get a [Run]'s display name by id in [super::Database]
    pub async fn select_one_display_name(
        db: &super::Database,
        id: i64,
    ) -> rusqlite::Result<String> {
        db.call(move |conn| {
            conn.prepare_cached("SELECT display_name FROM runs WHERE id = ?")?
                .query_one((id,), |row| row.get(0))
        })
        .await
    }

    /// Get a [Run]'s url by id in [super::Database]
    pub async fn select_one_url(db: &super::Database, id: i64) -> rusqlite::Result<String> {
        db.call(move |conn| {
            conn.prepare_cached("SELECT url FROM runs WHERE id = ?")?
                .query_one((id,), |row| row.get(0))
        })
        .await
    }

    /// Check whether or not there are untagged [Run]s in [super::Database]
    pub async fn has_untagged(db: &super::Database) -> rusqlite::Result<bool> {
        db.call(|conn| {
            conn.prepare_cached("SELECT 1 FROM runs WHERE tag_schema IS NULL")?
                .exists(())
        })
        .await
    }

    /// Update the [crate::parse::TagSet] schema for all [Run]s in [super::Database]
    pub async fn update_all_tag_schema(
        db: &super::Database,
        new_schema: Option<u64>,
    ) -> rusqlite::Result<usize> {
        db.call(move |conn| {
            conn.execute(
                "UPDATE runs SET tag_schema = ?",
                (new_schema.map(u64::cast_signed),),
            )
        })
        .await
    }
}
