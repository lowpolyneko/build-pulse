use std::collections::HashMap;

use crate::{
    db::{InDatabase, Issue, Queryable, Run, TagInfo},
    schema,
};

pub struct SimilarityInfo {
    pub similarity_hash: u64,
    pub issue_id: i64,
}

/// List of similar [Run]s by [TagInfo] in [super::Database]
pub struct Similarity {
    pub tag: InDatabase<TagInfo>,
    pub related: Vec<i64>,
    pub example: String,
}

schema! {
    similarities for SimilarityInfo {
        id              INTEGER PRIMARY KEY,
        similarity_hash INTEGER NOT NULL,
        issue_id        INTEGER NOT NULL REFERENCES issues(id)
    }
}

impl Queryable for SimilarityInfo {
    fn map_row(_: ()) -> impl FnMut(&rusqlite::Row) -> rusqlite::Result<InDatabase<Self>> {
        |row| {
            Ok(InDatabase::new(
                row.get(0)?,
                Self {
                    similarity_hash: row.get(1).map(i64::cast_unsigned)?,
                    issue_id: row.get(2)?,
                },
            ))
        }
    }

    fn as_params(&self, _: ()) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((self.similarity_hash.cast_signed(), self.issue_id))
    }
}

impl Similarity {
    /// Get all similarities by [crate::parse::Tag] in [super::Database]
    pub fn query_all(db: &super::Database, _: ()) -> rusqlite::Result<Vec<Self>> {
        let mut hm: HashMap<u64, Self> = HashMap::new();
        db.prepare_cached(
            "
                SELECT DISTINCT
                    similarity_hash,
                    tag_id,
                    run_id,
                    issue_id
                FROM similarities
                JOIN issues ON issues.id = similarities.issue_id
                ",
        )?
        .query_map((), |row| {
            Ok((
                row.get(0).map(i64::cast_unsigned)?,
                TagInfo::select_one(db, row.get(1)?, ())?,
                row.get(2)?,
                row.get(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .try_for_each(|(hash, tag, run_id, issue_id)| {
            hm.entry(hash)
                .or_insert({
                    Self {
                        tag,
                        related: Vec::new(),
                        example: Issue::select_one(
                            db,
                            issue_id,
                            (db, &Run::select_one(db, run_id, ())?),
                        )?
                        .snippet
                        .to_string(),
                    }
                })
                .related
                .push(run_id);

            Ok::<_, rusqlite::Error>(())
        })?;

        Ok(hm.into_values().collect())
    }
}
