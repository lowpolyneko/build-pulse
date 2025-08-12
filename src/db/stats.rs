use jenkins_api::build::BuildStatus;

use crate::{config::Severity, read_value, write_value};

/// Statistics of [Issue]s and [Run]s in [Database]
#[derive(Default)]
pub struct Statistics {
    /// Number of [BuildStatus::Success] [Job]s
    pub successful_jobs: u64,

    /// Total number of tracked [Job]s
    pub total_jobs: u64,

    /// Successful [Run]s
    pub successful: Vec<i64>,

    /// Unstable [Run]s
    pub unstable: Vec<i64>,

    /// Failed [Run]s
    pub failures: Vec<i64>,

    /// Aborted [Run]s
    pub aborted: Vec<i64>,

    /// Not built [Run]s
    pub not_built: Vec<i64>,

    /// Total [Issue]s found
    pub issues_found: u64,

    /// [Run]s with unknown issues
    pub unknown_runs: Vec<i64>,
}

/// Gets [Database]'s [Statistics]
impl Statistics {
    pub fn new(db: &super::Database) -> rusqlite::Result<Self> {
        // calculate success/failures for all runs
        let mut stats = db
            .conn
            .prepare("SELECT status, id FROM runs")?
            .query_map((), |row| Ok((read_value!(row, 0), row.get(1)?)))?
            .try_fold(Statistics::default(), |mut stats, res| {
                let (status, id) = res?;
                match status {
                    Some(BuildStatus::Aborted) => stats.aborted.push(id),
                    Some(BuildStatus::Failure) => stats.failures.push(id),
                    Some(BuildStatus::NotBuilt) => stats.not_built.push(id),
                    Some(BuildStatus::Success) => stats.successful.push(id),
                    Some(BuildStatus::Unstable) => stats.unstable.push(id),
                    _ => {}
                };

                Ok::<_, rusqlite::Error>(stats)
            })?;

        stats.successful_jobs = db
            .conn
            .prepare(
                "
                SELECT COUNT(*) FROM jobs j
                WHERE EXISTS (
                    SELECT 1 FROM builds
                    WHERE builds.job_id = j.id
                    AND status = ?
                )
                ",
            )?
            .query_one((write_value!(BuildStatus::Success),), |row| row.get(0))?;

        stats.total_jobs = db
            .conn
            .prepare("SELECT COUNT(*) FROM jobs")?
            .query_one((), |row| row.get(0))?;

        // don't count metadata issues in total
        stats.issues_found = db
            .conn
            .prepare(
                "
                SELECT COUNT(*) FROM issues
                JOIN tags ON tags.id = issues.tag_id
                WHERE tags.severity != ?
                ",
            )?
            .query_one((write_value!(Severity::Metadata),), |row| row.get(0))?;

        stats.unknown_runs = db
            .conn
            .prepare(
                "
                SELECT r.id FROM runs r
                WHERE r.status = ?
                    AND NOT EXISTS (
                        SELECT 1 FROM issues
                        JOIN tags ON tags.id = issues.tag_id
                        WHERE
                            issues.run_id = r.id
                            AND tags.severity != ?
                    )
                ",
            )?
            .query_map(
                (
                    write_value!(Some(BuildStatus::Failure)),
                    write_value!(Severity::Metadata),
                ),
                |row| row.get(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(stats)
    }
}
