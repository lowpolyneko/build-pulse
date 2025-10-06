use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
};

use arcstr::Substr;
use futures::{TryFutureExt, TryStreamExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::{
    config::Field,
    db::{InDatabase, IssueInfo, Queryable, TagInfo},
    schema,
};

pub struct SimilarityInfo {
    pub similarity_hash: u64,
    pub issue_id: i64,
}

/// List of similar [Run]s by [TagInfo] in [super::Database]
pub struct Similarity {
    pub tag: InDatabase<TagInfo>,
    pub related: HashSet<i64>,
    pub example: Substr,
}

schema! {
    similarities for SimilarityInfo {
        id              INTEGER PRIMARY KEY,
        similarity_hash INTEGER NOT NULL,
        issue_id        INTEGER NOT NULL REFERENCES issues(id)
    }
}

impl Queryable<'_> for SimilarityInfo {
    fn map_row(row: &rusqlite::Row) -> rusqlite::Result<InDatabase<Self>> {
        Ok(InDatabase::new(
            row.get(0)?,
            Self {
                similarity_hash: row.get(1).map(i64::cast_unsigned)?,
                issue_id: row.get(2)?,
            },
        ))
    }

    fn as_params(&self) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((self.similarity_hash.cast_signed(), self.issue_id))
    }
}

impl Similarity {
    /// Get all similarities by [crate::parse::Tag] in [super::Database]
    pub async fn query_all(db: &super::Database) -> rusqlite::Result<impl Iterator<Item = Self>> {
        struct InfoSet {
            similarity: SimilarityInfo,
            tag_id: i64,
            run_id: i64,
        }

        db.call(|conn| -> rusqlite::Result<UnboundedReceiverStream<_>> {
            let (tx, rx) = mpsc::unbounded_channel();
            conn.prepare_cached(
                "
                SELECT DISTINCT
                    s.similarity_hash,
                    s.issue_id,
                    i.tag_id,
                    i.run_id
                FROM similarities s
                JOIN issues i ON i.id = s.issue_id
                WHERE EXISTS (
                        SELECT 1 FROM similarities
                        JOIN issues ON issues.id = similarities.issue_id
                        JOIN runs ON runs.id = issues.run_id
                        WHERE similarity_hash = s.similarity_hash
                            AND build_id IN (
                                    SELECT id FROM builds
                                    GROUP BY job_id
                                    HAVING MAX(number)
                                )
                    )
                ",
            )?
            .query_and_then((), |row| {
                Ok(InfoSet {
                    similarity: SimilarityInfo {
                        similarity_hash: row.get(0).map(i64::cast_unsigned)?,
                        issue_id: row.get(1)?,
                    },
                    tag_id: row.get(2)?,
                    run_id: row.get(3)?,
                })
            })?
            .try_for_each(|info| {
                tx.send(info)
                    .map_err(|e| rusqlite::Error::UserFunctionError(e.into()))
            })?;

            Ok(rx.into())
        })
        .await?
        .try_fold(HashMap::new(), |hm, info| async {
            let tag = TagInfo::select_one(db, info.tag_id).await?;
            hm.entry(info.similarity.similarity_hash)
                .or_insert({
                    Self {
                        tag,
                        related: HashSet::new(),
                        example: match tag.field {
                            Field::Artifact => {}
                            Field::RunName => {}
                            Field::Console => {}
                        },
                        // example: IssueInfo::select_one(db, info.similarity.issue_id)
                        //     .await?
                        //     .into_issue(),
                    }
                })
                .related
                .insert(info.run_id);

            Ok::<_, rusqlite::Error>(hm)
        })
        .map_ok(|hm| hm.into_values())
        .await

        // let mut similarities: Vec<_> = hm
        //     .into_values()
        //     // ignore similarities within the same run
        //     .filter(|s| s.related.len() > 1)
        //     .collect();
        //
        // similarities.sort_by_cached_key(|s| Reverse(s.related.len()));
        //
        // Ok(similarities)
    }
}
