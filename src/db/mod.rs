//! [rusqlite] based ORM to cache build results.
use std::hash::Hash;
use std::ops::{Deref, DerefMut};

use rusqlite::{Connection, Params, Result, Row};

mod artifact;
mod build;
mod issue;
mod job;
mod run;
mod similarity;
mod stats;
mod tag;

pub use {artifact::*, build::*, issue::*, job::*, run::*, similarity::*, stats::*, tag::*};

/// Read [serde] serialized value from `row` and `idx`
#[macro_export]
macro_rules! read_value {
    ($row:ident, $idx:literal) => {
        serde_json::from_value($row.get($idx)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure($idx, rusqlite::types::Type::Text, e.into())
        })?
    };
}

/// Write as [serde] serializable value
#[macro_export]
macro_rules! write_value {
    ($val:expr) => {
        serde_json::to_value($val).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?
    };
}

/// Generate [Schema] for struct
#[macro_export]
macro_rules! schema {
    ($($table:tt for $model:ident$(<$($generic:tt),+>)? {
        $($schema:tt)+
    });+) => {
        $(impl$(<$($generic),+>)? $crate::db::Schema for $model$(<$($generic),+>)? {
            $crate::schema!(@create_table $table $($schema)+);
            $crate::schema!(@insert $table $($schema)+);
            $crate::schema!(@select_one $table);
            $crate::schema!(@select_all $table);
            $crate::schema!(@delete_all $table);
        })+
    };

    (@create_table $table:tt $($schema:tt)+) => {
        const CREATE_TABLE: &'static str = concat!(
            "CREATE TABLE IF NOT EXISTS ",
            stringify!($table),
            " (",
            $crate::schema!(@column_def $($schema)+),
            ") STRICT"
        );
    };

    (@column_def $col:tt $type:tt, $($rest:tt)+) => {
        concat!(stringify!($col), " ", stringify!($type), ",", $crate::schema!(@column_def $($rest)+))
    };
    (@column_def $col:tt $type:tt $($rest:tt)*) => {
        concat!(stringify!($col), " ", stringify!($type), $crate::schema!(@column_constraint $($rest)*))
    };

    (@column_constraint $constraint:tt, $($rest:tt)+) => {
        concat!(" ", stringify!($constraint), ",", $crate::schema!(@column_def $($rest)+))
    };
    (@column_constraint $constraint:tt $($rest:tt)*) => {
        concat!(" ", stringify!($constraint), $crate::schema!(@column_constraint $($rest)*))
    };
    (@column_constraint) => { "" };

    (@insert $table:tt $($schema:tt)+) => {
        const INSERT: &'static str = concat!(
            "INSERT INTO ",
            stringify!($table),
            " (",
            $crate::schema!(@insert_col $($schema)+),
            ") VALUES (",
            $crate::schema!(@repeat_vars $($schema)+),
            ")"
        );
    };

    (@insert_col id INTEGER PRIMARY KEY, $($rest:tt)+) => {
        $crate::schema!(@insert_col $($rest)+) // skip id
    };
    (@insert_col $col:tt $_type:tt, $($rest:tt)+) => {
        concat!(stringify!($col), ",", $crate::schema!(@insert_col $($rest)+))
    };
    (@insert_col $col:tt $_type:tt $($rest:tt)*) => {
        concat!(stringify!($col), $crate::schema!(@insert_col_const $($rest)*))
    };

    (@insert_col_const $_constraint:tt, $($rest:tt)+) => {
        concat!(",", $crate::schema!(@insert_col $($rest)*))
    };
    (@insert_col_const $_constraint:tt $($rest:tt)*) => {
        $crate::schema!(@insert_col_const $($rest)*)
    };
    (@insert_col_const) => { "" };

    (@repeat_vars id INTEGER PRIMARY KEY, $($rest:tt)+) => {
        $crate::schema!(@repeat_vars $($rest)+) // skip id
    };
    (@repeat_vars $_col:tt $_type:tt, $($rest:tt)+) => {
        concat!("?,", $crate::schema!(@repeat_vars $($rest)+))
    };
    (@repeat_vars $_col:tt $_type:tt $($rest:tt)*) => {
        concat!("?", $crate::schema!(@repeat_vars_constraint $($rest)*))
    };

    (@repeat_vars_constraint $_constraint:tt, $($rest:tt)+) => {
        concat!(",", $crate::schema!(@repeat_vars $($rest)*))
    };
    (@repeat_vars_constraint $_constraint:tt $($rest:tt)*) => {
        $crate::schema!(@repeat_vars_constraint $($rest)*)
    };
    (@repeat_vars_constraint) => { "" };

    (@select_one $table:tt) => {
        const SELECT_ONE: &'static str = concat!(
            "SELECT * FROM ",
            stringify!($table),
            " WHERE id = ?"
        );
    };

    (@select_all $table:tt) => {
        const SELECT_ALL: &'static str = concat!(
            "SELECT * FROM ",
            stringify!($table)
        );
    };

    (@delete_all $table:tt) => {
        const DELETE_ALL: &'static str = concat!(
            "DELETE FROM ",
            stringify!($table)
        );
    };
}

/// Convenience macro to run method on all types
macro_rules! for_all {
    ([$type:ty, $($rest:ty),*] => $($method:tt)+) => {
        <$type>::$($method)+;
        for_all!([$($rest),*] => $($method)+)
    };

    ([$type:ty] => $($method:tt)+) => {
        <$type>::$($method)+;
    };

    ($($method:tt)+) => {
        for_all!([SimilarityInfo, Issue, Artifact, Run, JobBuild, Job, TagInfo] => $($method)+)
    };
}

/// Database object
pub struct Database {
    /// Internal [rusqlite] connection
    conn: Connection,
}

/// Implicit deref to [Connection] from [Database]
impl Deref for Database {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

/// Implicit deref_mut to [Connection] from [Database]
impl DerefMut for Database {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}

/// Represents an item `T` in [Database]
pub struct InDatabase<T> {
    /// Row ID of `item`
    pub id: i64,

    /// Item itself
    item: T,
}

impl<T> InDatabase<T> {
    /// Wrap item in [InDatabase] with new `id` from [Database]
    fn new(id: i64, item: T) -> Self {
        InDatabase { id, item }
    }

    pub fn item(self) -> T {
        self.item
    }
}

// Hash only considers the id property for [InDatabase]
impl<T> Hash for InDatabase<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

// Implicit deref to `T` from [InDatabase]
impl<T> Deref for InDatabase<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

// Implicit deref_mut to `T` from [InDatabase]
impl<T> DerefMut for InDatabase<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.item
    }
}

// Establish ordering by the `id` primary key
impl<T> Ord for InDatabase<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
    }
}

impl<T> PartialOrd for InDatabase<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> PartialEq for InDatabase<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T> Eq for InDatabase<T> {}

impl Database {
    /// Open or create an `sqlite3` database at `path` returning [Database]
    pub fn open(path: &str) -> Result<Database> {
        // Enable REGEXP
        rusqlite_regex::enable_auto_extension()?;

        // try to open existing, otherwise create a new one
        let db = Database {
            conn: Connection::open(path)?,
        };

        // create the necessary tables
        for_all!(create_table(&db)?);

        Ok(db)
    }

    /// Purge all rows (but not tables) from [Database]
    pub fn purge_cache(&self) -> Result<()> {
        for_all!(delete_all(self)?);
        Ok(())
    }
}

pub trait Schema: Sized {
    const CREATE_TABLE: &'static str;
    const INSERT: &'static str;
    const SELECT_ONE: &'static str;
    const SELECT_ALL: &'static str;
    const DELETE_ALL: &'static str;

    /// Creates the table in [Database]
    fn create_table(db: &Database) -> Result<usize> {
        db.execute(Self::CREATE_TABLE, ())
    }
}

pub trait Queryable<I = (), E = ()>: Schema {
    /// Convert [Row] to `Self` with params
    fn map_row(params: I) -> impl FnMut(&Row) -> Result<InDatabase<Self>>;

    /// Convert `self` to [Params] with `params`
    fn as_params(&self, params: E) -> Result<impl Params>;

    /// Insert `self` to [Database] with `params`
    fn insert(self, db: &Database, params: E) -> Result<InDatabase<Self>> {
        db.prepare_cached(Self::INSERT)?
            .execute(self.as_params(params)?)?;
        Ok(InDatabase::new(db.last_insert_rowid(), self))
    }

    /// Select one of `Self` from [Database] by `id` with `params`
    fn select_one(db: &Database, id: i64, params: I) -> Result<InDatabase<Self>> {
        db.prepare_cached(Self::SELECT_ONE)?
            .query_one((id,), Self::map_row(params))
    }

    /// Select all of `Self` from [Database] with `params`
    fn select_all(db: &Database, params: I) -> Result<Vec<InDatabase<Self>>> {
        db.prepare_cached(Self::SELECT_ALL)?
            .query_map((), Self::map_row(params))?
            .collect()
    }

    /// Delete all of `Self` from [Database]
    fn delete_all(db: &Database) -> Result<usize> {
        db.execute(Self::DELETE_ALL, ())
    }
}

pub trait Upsertable<I = (), E = ()>: Queryable<I, E> {
    /// Upsert `self` to [Database] with `params`
    fn upsert(self, db: &Database, params: E) -> Result<InDatabase<Self>>;
}
