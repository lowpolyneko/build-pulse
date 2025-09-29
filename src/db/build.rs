use jenkins_api::build::BuildStatus;

use crate::{
    db::{Queryable, Upsertable},
    read_value, schema, write_value,
};

/// [JobBuild] in [super::Database]
pub struct JobBuild {
    /// Build url
    pub url: String,

    /// Build status
    pub status: Option<BuildStatus>,

    /// Build number
    pub number: u32,

    /// Build timestamp
    pub timestamp: u64,

    /// ID of associated [super::Job]
    pub job_id: i64,
}

schema! {
    builds for JobBuild {
        id          INTEGER PRIMARY KEY,
        url         TEXT NOT NULL UNIQUE,
        status      TEXT,
        number      INTEGER NOT NULL,
        timestamp   INTEGER NOT NULL,
        job_id      INTEGER NOT NULL REFERENCES jobs(id)
    }
}

impl Queryable for JobBuild {
    fn map_row(_: ()) -> impl FnMut(&rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        |row| {
            Ok(super::InDatabase::new(
                row.get(0)?,
                JobBuild {
                    url: row.get(1)?,
                    status: read_value!(row, 2),
                    number: row.get(3)?,
                    timestamp: row.get(4).map(i64::cast_unsigned)?,
                    job_id: row.get(5)?,
                },
            ))
        }
    }

    fn as_params(&self, _: ()) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((
            &self.url,
            write_value!(self.status),
            self.number,
            self.timestamp.cast_signed(),
            self.job_id,
        ))
    }
}

impl Upsertable for JobBuild {
    fn upsert(self, db: &super::Database, params: ()) -> rusqlite::Result<super::InDatabase<Self>> {
        db.prepare_cached(
            "
                INSERT INTO builds (
                    url,
                    status,
                    number,
                    timestamp,
                    job_id
                ) VALUES (?, ?, ?, ?, ?)
                    ON CONFLICT(url) DO UPDATE SET
                        status = excluded.status,
                        number = excluded.number,
                        timestamp = excluded.timestamp,
                        job_id = excluded.job_id
                ",
        )?
        .execute(self.as_params(params)?)?;

        Self::select_one_by_job(db, self.job_id, self.number, ())
    }
}

impl JobBuild {
    /// Get a [JobBuild] from [super::Database] by [super::Job] id and build number
    pub fn select_one_by_job(
        db: &super::Database,
        job_id: i64,
        number: u32,
        params: (),
    ) -> rusqlite::Result<super::InDatabase<Self>> {
        db.prepare_cached(
            "
                SELECT * FROM builds
                WHERE job_id = ?
                AND number = ?
                ",
        )?
        .query_one((job_id, number), Self::map_row(params))
    }

    /// Remove all [JobBuild]s which aren't referenced by [super::Job] from [super::Database]
    pub fn delete_all_orphan(db: &super::Database) -> rusqlite::Result<()> {
        db.execute_batch(
            "
            BEGIN;
            DELETE FROM similarities WHERE similarity_hash IN (
                SELECT DISTINCT similarities.similarity_hash FROM similarities
                JOIN issues ON issues.id = similarities.issue_id
                JOIN runs ON runs.id = issues.run_id
                JOIN builds ON builds.id = runs.build_id
                JOIN jobs ON jobs.id = builds.job_id
                WHERE number < last_build
            );
            DELETE FROM issues WHERE id IN (
                SELECT issues.id FROM issues
                JOIN runs ON runs.id = issues.run_id
                JOIN builds ON builds.id = runs.build_id
                JOIN jobs ON jobs.id = builds.job_id
                WHERE number < last_build
            );
            DELETE FROM artifacts WHERE id IN (
                SELECT artifacts.id FROM artifacts
                JOIN runs ON runs.id = artifacts.run_id
                JOIN builds ON builds.id = runs.build_id
                JOIN jobs ON jobs.id = builds.job_id
                WHERE number < last_build
            );
            DELETE FROM runs WHERE id IN (
                SELECT runs.id FROM runs
                JOIN builds ON builds.id = runs.build_id
                JOIN jobs ON jobs.id = builds.job_id
                WHERE number < last_build
            );
            DELETE FROM builds WHERE id IN (
                SELECT builds.id FROM builds
                JOIN jobs ON jobs.id = builds.job_id
                WHERE number < last_build
            );
            COMMIT;
            ",
        )
    }
}
