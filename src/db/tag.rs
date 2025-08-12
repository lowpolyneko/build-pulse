use crate::{
    config::{Field, Severity},
    db::{Queryable, Run, Upsertable},
    parse::{Tag, TagSet},
    read_value, schema, write_value,
};

#[derive(PartialEq, Eq, Hash)]
pub struct TagInfo {
    /// Name of [Tag]
    pub name: String,

    /// Description of [Tag]
    pub desc: String,

    /// Field of [Tag]
    pub field: Field,

    /// Severity of [Tag]
    pub severity: Severity,
}

impl From<&Tag<'_>> for TagInfo {
    fn from(value: &Tag<'_>) -> Self {
        TagInfo {
            name: value.name.to_string(),
            desc: value.desc.to_string(),
            field: *value.from,
            severity: *value.severity,
        }
    }
}

schema! {
    tags for TagInfo {
        id              INTEGER PRIMARY KEY,
        name            TEXT NOT NULL UNIQUE,
        desc            TEXT NOT NULL,
        field           TEXT NOT NULL,
        severity        TEXT NOT NULL
    }
}

impl Queryable for TagInfo {
    fn map_row(_: ()) -> impl FnMut(&rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        |row| {
            Ok(super::InDatabase::new(
                row.get(0)?,
                Self {
                    name: row.get(1)?,
                    desc: row.get(2)?,
                    severity: read_value!(row, 3),
                    field: read_value!(row, 4),
                },
            ))
        }
    }

    fn as_params(&self, _: ()) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((
            &self.name,
            &self.desc,
            write_value!(self.field),
            write_value!(self.severity),
        ))
    }
}

impl Upsertable for TagInfo {
    fn upsert(self, db: &super::Database, params: ()) -> rusqlite::Result<super::InDatabase<Self>> {
        db.conn
            .prepare_cached(
                "
            INSERT INTO tags (name, desc, field, severity) VALUES (?, ?, ?, ?)
                ON CONFLICT(name) DO UPDATE SET
                    desc = excluded.desc,
                    field = excluded.field,
                    severity = excluded.severity
            ",
            )?
            .execute(self.as_params(params)?)?;

        Self::select_by_name(db, &self.name, ())
    }
}

impl TagInfo {
    /// Get all [TagInfo]s from [super::Database] by name
    pub fn select_by_name(
        db: &super::Database,
        name: &str,
        params: (),
    ) -> rusqlite::Result<super::InDatabase<Self>> {
        db.conn
            .prepare_cached("SELECT id, name, desc, severity, field FROM tags WHERE name = ?")?
            .query_one((name,), Self::map_row(params))
    }

    /// Get all [TagInfo]s from [Run]
    pub fn select_by_run(
        db: &super::Database,
        run: &super::InDatabase<Run>,
        params: (),
    ) -> rusqlite::Result<Vec<super::InDatabase<Self>>> {
        db.conn
            .prepare_cached(
                "
                SELECT DISTINCT tags.id, name, desc, field, severity FROM tags
                JOIN issues ON tags.id = issues.tag_id
                WHERE issues.run_id = ?
                ",
            )?
            .query_map((run.id,), Self::map_row(params))?
            .collect()
    }

    /// Upsert a [TagSet] into [super::Database]
    pub fn upsert_tag_set<'a>(
        db: &super::Database,
        tags: TagSet<Tag<'a>>,
        params: (),
    ) -> rusqlite::Result<TagSet<super::InDatabase<Tag<'a>>>> {
        tags.try_swap_tags(|t| {
            Ok(super::InDatabase::new(
                TagInfo::from(&t).upsert(db, params)?.id,
                t,
            ))
        })
    }

    /// Remove all [Tag]s which aren't referenced by [super::Issue]s from [super::Database]
    pub fn purge_orphans(db: &super::Database) -> rusqlite::Result<usize> {
        db.conn.execute(
            "
            DELETE FROM tags WHERE NOT EXISTS (
                SELECT 1 FROM issues
                WHERE tags.id = issues.tag_id
            )
            ",
            (),
        )
    }
}
