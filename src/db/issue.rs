use arcstr::Substr;

use crate::{
    config::{Field, Severity},
    db::{Queryable, Run, TagInfo},
    schema, write_value,
};

/// [Issue] stored in [super::Database]
#[derive(PartialEq, Eq, Hash)]
pub struct Issue {
    /// String snippet from [Run]
    pub snippet: Substr,

    /// [crate::parse::Tag] associated with [Issue]
    pub tag_id: i64,

    /// Number of duplicate emits in the same [Run]
    pub duplicates: u64,
}

schema! {
    issues for Issue {
        id              INTEGER PRIMARY KEY,
        snippet_start   INTEGER NOT NULL,
        snippet_end     INTEGER NOT NULL,
        run_id          INTEGER NOT NULL REFERENCES runs(id),
        tag_id          INTEGER NOT NULL REFERENCES tags(id),
        duplicates      INTEGER NOT NULL
    }
}

impl Queryable<(&super::Database, &super::InDatabase<Run>), (&super::InDatabase<Run>,)> for Issue {
    fn map_row(
        params: (&super::Database, &super::InDatabase<Run>),
    ) -> impl FnMut(&rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        let (db, run) = params;
        |row| {
            let tag_id = row.get(4)?;
            Ok(super::InDatabase::new(
                row.get(0)?,
                Self {
                    snippet: match TagInfo::select_one(db, tag_id, ())?.field {
                        Field::Console => run.log.as_ref().ok_or(rusqlite::Error::InvalidQuery)?,
                        Field::RunName => &run.display_name,
                    }
                    .substr(row.get::<_, usize>(1)?..row.get::<_, usize>(2)?),
                    tag_id,
                    duplicates: row.get(5).map(i64::cast_unsigned)?,
                },
            ))
        }
    }

    fn as_params(
        &self,
        params: (&super::InDatabase<Run>,),
    ) -> rusqlite::Result<impl rusqlite::Params> {
        let (run,) = params;
        let core::ops::Range::<_> { start, end } = self.snippet.range();
        Ok((
            start,
            end,
            run.id,
            self.tag_id,
            self.duplicates.cast_signed(),
        ))
    }

    fn select_all(
        db: &super::Database,
        params: (&super::Database, &super::InDatabase<Run>),
    ) -> rusqlite::Result<Vec<super::InDatabase<Self>>> {
        let (_, run) = params;
        db.prepare_cached(
            "
                SELECT
                    issues.id,
                    snippet_start,
                    snippet_end,
                    run_id,
                    tag_id,
                    duplicates
                FROM issues
                JOIN tags ON tags.id = issues.tag_id
                WHERE issues.run_id = ?
                ",
        )?
        .query_map((run.id,), Self::map_row(params))?
        .collect()
    }
}

impl Issue {
    /// Get all [Issue]s from [super::Database] that aren't [Severity::Metadata]
    pub fn select_all_not_metadata(
        db: &super::Database,
        params: (&super::Database, &super::InDatabase<Run>),
    ) -> rusqlite::Result<Vec<super::InDatabase<Self>>> {
        let (_, run) = params;
        db.prepare_cached(
            "
                SELECT
                    issues.id,
                    snippet_start,
                    snippet_end,
                    run_id,
                    tag_id,
                    duplicates
                FROM issues
                JOIN tags ON tags.id = issues.tag_id
                WHERE issues.run_id = ?
                AND tags.severity != ?
                ",
        )?
        .query_map(
            (run.id, write_value!(Severity::Metadata)),
            Self::map_row(params),
        )?
        .collect()
    }

    /// Remove all [Issue]s with an outdated [crate::parse::TagSet] schema from [super::Database]
    pub fn delete_all_invalid_by_tag_schema(
        db: &mut super::Database,
        current_schema: u64,
    ) -> rusqlite::Result<usize> {
        let mut tx = db.transaction()?;
        tx.set_drop_behavior(rusqlite::DropBehavior::Commit);

        // delete similarities first
        tx.execute(
            "
            DELETE FROM similarities WHERE similarity_hash IN (
                SELECT DISTINCT similarities.similarity_hash FROM similarities
                JOIN issues ON issues.id = similarities.issue_id
                JOIN runs ON runs.id = issues.run_id
                WHERE runs.tag_schema != ?
            )
            ",
            (current_schema.cast_signed(),),
        )?;

        // then issues
        tx.execute(
            "
            DELETE FROM issues WHERE id IN (
                SELECT i.id FROM issues i
                JOIN runs r ON i.run_id = r.id
                WHERE r.tag_schema != ?
            )
            ",
            (current_schema.cast_signed(),),
        )?;

        // also set the run tag_schema to NULL to indicate an unparsed run
        tx.execute(
            "UPDATE runs SET tag_schema = NULL WHERE tag_schema != ?",
            (current_schema.cast_signed(),),
        )
    }
}
