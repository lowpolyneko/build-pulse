use crate::{
    db::{Queryable, Upsertable},
    schema,
};

/// [Job] stored in [Database]
pub struct Job {
    /// Unique name of [Job]
    pub name: String,

    /// [Job] url
    pub url: String,

    /// Number of last [JobBuild]
    pub last_build: Option<u32>,
}

schema! {
    jobs for Job {
        id          INTEGER PRIMARY KEY,
        name        TEXT NOT NULL UNIQUE,
        url         TEXT NOT NULL,
        last_build  INTEGER
    }
}

impl Queryable for Job {
    fn map_row(_: ()) -> impl FnMut(&rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        |row| {
            Ok(super::InDatabase::new(
                row.get(0)?,
                Job {
                    name: row.get(1)?,
                    url: row.get(2)?,
                    last_build: row.get(3)?,
                },
            ))
        }
    }

    fn as_params(&self, _: ()) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((&self.name, &self.url, self.last_build))
    }
}

impl Upsertable for Job {
    fn upsert(self, db: &super::Database, params: ()) -> rusqlite::Result<super::InDatabase<Self>> {
        db.conn
            .prepare_cached(
                "
                INSERT INTO jobs (
                    name,
                    url,
                    last_build
                ) VALUES (?, ?, ?)
                    ON CONFLICT(name) DO UPDATE SET
                        last_build = excluded.last_build
                ",
            )?
            .execute(self.as_params(params)?)?;

        // get the job as a second query in-case of an insert conflict
        Self::select_one_by_name(db, &self.name, ())
    }
}

impl Job {
    /// Get a [Job] from [super::Database] by name
    pub fn select_one_by_name(
        db: &super::Database,
        name: &str,
        params: (),
    ) -> rusqlite::Result<super::InDatabase<Self>> {
        db.conn
            .prepare_cached(
                "
                SELECT * FROM jobs
                WHERE name = ?
                ",
            )?
            .query_one((name,), Self::map_row(params))
    }

    /// Remove all [Job]s from [super::Database] by name
    pub fn delete_all_by_blocklist(
        db: &mut super::Database,
        names: &[String],
    ) -> rusqlite::Result<usize> {
        let mut tx = db.conn.transaction()?;
        tx.set_drop_behavior(rusqlite::DropBehavior::Commit);

        names.iter().try_fold(0, |acc, name| {
            // delete similarities first
            tx.execute(
                "
                DELETE FROM similarities WHERE similarity_hash IN (
                    SELECT DISTINCT similarities.similarity_hash FROM similarities
                    JOIN issues ON issues.id = similarities.issue_id
                    JOIN runs ON runs.id = issues.run_id
                    JOIN builds ON builds.id = runs.build_id
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE name = ?
                )
                ",
                (name,),
            )?;

            // then issues
            tx.execute(
                "
                DELETE FROM issues WHERE id IN (
                    SELECT issues.id FROM issues
                    JOIN runs ON runs.id = issues.run_id
                    JOIN builds ON builds.id = runs.build_id
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE name = ?
                )
                ",
                (name,),
            )?;

            // then runs
            tx.execute(
                "
                DELETE FROM runs WHERE id IN (
                    SELECT runs.id FROM runs
                    JOIN builds ON builds.id = runs.build_id
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE name = ?
                );
                ",
                (name,),
            )?;

            // then builds
            tx.execute(
                "
                DELETE FROM builds WHERE id IN (
                    SELECT builds.id FROM builds
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE name = ?
                );
                ",
                (name,),
            )?;

            // finally the job
            Ok(acc + tx.execute("DELETE FROM jobs WHERE name = ?", (name,))?)
        })
    }
}
