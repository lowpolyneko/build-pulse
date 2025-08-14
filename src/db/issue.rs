use crate::{
    config::{Field, Severity},
    db::{Queryable, Run, TagInfo},
    schema, write_value,
};

/// [Issue] stored in [Database]
#[derive(PartialEq, Eq, Hash)]
pub struct Issue<'a> {
    /// String snippet from [Run]
    pub snippet: &'a str,

    /// [Tag] associated with [Issue]
    pub tag_id: i64,

    /// Number of duplicate emits in the same [Run]
    pub duplicates: u64,
}

schema! {
    issues for Issue<'a> {
        id              INTEGER PRIMARY KEY,
        snippet_start   INTEGER NOT NULL,
        snippet_end     INTEGER NOT NULL,
        run_id          INTEGER NOT NULL REFERENCES runs(id),
        tag_id          INTEGER NOT NULL REFERENCES tags(id),
        duplicates      INTEGER NOT NULL
    }
}

impl<'a>
    Queryable<
        (&super::Database, &'a super::InDatabase<Run>),
        (&super::Database, &'a super::InDatabase<Run>),
    > for Issue<'a>
{
    fn map_row(
        params: (&super::Database, &'a super::InDatabase<Run>),
    ) -> impl FnMut(&rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        let (db, run) = params;
        move |row| {
            let tag = TagInfo::select_one(db, row.get(4)?, ())?;
            Ok(super::InDatabase::new(
                row.get(0)?,
                Self {
                    snippet: match tag.field {
                        Field::Console => run.log.as_ref().ok_or(rusqlite::Error::InvalidQuery)?,
                        Field::RunName => &run.display_name,
                    }
                    .get(row.get(1)?..row.get(2)?)
                    .ok_or(rusqlite::Error::InvalidQuery)?,
                    tag_id: tag.id,
                    duplicates: row.get(5).map(i64::cast_unsigned)?,
                },
            ))
        }
    }

    fn as_params(
        &self,
        params: (&super::Database, &super::InDatabase<Run>),
    ) -> rusqlite::Result<impl rusqlite::Params> {
        let (db, run) = params;
        let log = match TagInfo::select_one(db, self.tag_id, ())?.field {
            Field::Console => run.log.as_ref().ok_or(rusqlite::Error::InvalidQuery)?,
            Field::RunName => &run.display_name,
        }
        .as_ptr();
        let start = unsafe {
            // SAFETY: [Run] owns all underlying [Issue]s
            self.snippet.as_ptr().offset_from_unsigned(log)
        };
        let end = start + self.snippet.len();

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
        params: (&super::Database, &'a super::InDatabase<Run>),
    ) -> rusqlite::Result<Vec<super::InDatabase<Self>>> {
        let (_, run) = params;
        db.conn
            .prepare_cached(
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

impl<'a> Issue<'a> {
    /// Get all [Issue]s from [super::Database] that aren't [Severity::Metadata]
    pub fn select_all_not_metadata(
        db: &super::Database,
        params: (&super::Database, &'a super::InDatabase<Run>),
    ) -> rusqlite::Result<Vec<super::InDatabase<Self>>> {
        let (_, run) = params;
        db.conn
            .prepare_cached(
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
        let mut tx = db.conn.transaction()?;
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
