//! [rusqlite] based ORM to cache build results.
use std::hash::Hash;
use std::ops::{Deref, DerefMut};

use rusqlite::{Connection, Params, Result, Row};

mod build;
mod issue;
mod job;
mod run;
mod similarity;
mod stats;
mod tag;

pub use {build::*, issue::*, job::*, run::*, similarity::*, stats::*, tag::*};

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
            $crate::schema!(@create_table_col $($schema)+),
            ") STRICT"
        );
    };

    (@create_table_col $col:tt $type:tt, $($rest:tt)+) => {
        concat!(stringify!($col), " ", stringify!($type), ", ", $crate::schema!(@create_table_col $($rest)+))
    };

    (@create_table_col $col:tt $type:tt $($rest:tt)*) => {
        concat!(stringify!($col), " ", stringify!($type), $crate::schema!(@create_table_const $($rest)*))
    };

    (@create_table_const $constraint:tt, $($rest:tt)+) => {
        concat!(" ", stringify!($constraint), ", ", $crate::schema!(@create_table_col $($rest)+))
    };

    (@create_table_const $constraint:tt $($rest:tt)*) => {
        concat!(" ", stringify!($constraint), $crate::schema!(@create_table_const $($rest)*))
    };

    (@create_table_const) => { "" };

    (@insert $table:tt $($schema:tt)+) => {
        const INSERT: &'static str = concat!(
            "INSERT INTO ",
            stringify!($table),
            " (",
            $crate::schema!(@repeat_vars $($schema)+),
            ")"
        );
    };

    (@repeat_vars id INTEGER PRIMARY KEY, $($rest:tt)+) => {
        $crate::schema!(@repeat_vars $($rest)+) // skip id
    };

    (@repeat_vars $_col:tt $_type:tt $($rest:tt)*) => {
        concat!("?", $crate::schema!(@repeat_vars_const $($rest)*))
    };

    (@repeat_vars $_col:tt $_type:tt, $($rest:tt)+) => {
        concat!("?,", $crate::schema!(@repeat_vars $($rest)+))
    };

    (@repeat_vars $_col:tt $_type:tt $($rest:tt)*) => {
        concat!("?", $crate::schema!(@repeat_vars_const $($rest)*))
    };

    (@repeat_vars_const $_constraint:tt, $($rest:tt)+) => {
        concat!(",", $crate::schema!(@repeat_vars $($rest)*))
    };

    (@repeat_vars_const $_constraint:tt $($rest:tt)*) => {
        $crate::schema!(@repeat_vars_const $($rest)*)
    };

    (@repeat_vars_const) => { "" };

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

macro_rules! for_all {
    ([$type:ty, $($rest:ty),*] => $($method:tt)+) => {
        <$type>::$($method)+;
        for_all!([$($rest),*] => $($method)+)
    };

    ([$type:ty] => $($method:tt)+) => {
        <$type>::$($method)+;
    };
}

macro_rules! all_tables {
    ($($method:tt)+) => {
        for_all!([Job, JobBuild, Run, Issue, TagInfo, SimilarityInfo] => $($method)+)
    };
}

/// Database object
pub struct Database {
    /// Internal [rusqlite] connection
    conn: Connection,
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
        all_tables!(create_table(&db)?);

        Ok(db)
    }

    /// Purge all rows (but not tables) from [Database]
    pub fn purge_cache(&self) -> Result<()> {
        all_tables!(delete_all(self)?);
        Ok(())
    }
}

pub trait Schema: Sized {
    const CREATE_TABLE: &'static str;
    const INSERT: &'static str;
    const SELECT_ONE: &'static str;
    const SELECT_ALL: &'static str;
    const DELETE_ALL: &'static str;

    fn create_table(db: &Database) -> Result<usize> {
        db.conn.execute(Self::CREATE_TABLE, ())
    }
}

pub trait Queryable<I = (), E = ()>: Schema {
    fn map_row(params: I) -> impl FnMut(&Row) -> Result<InDatabase<Self>>;
    fn as_params(&self, params: E) -> Result<impl Params>;

    fn insert(self, db: &Database, params: E) -> Result<InDatabase<Self>> {
        db.conn
            .prepare_cached(Self::INSERT)?
            .execute(self.as_params(params)?)?;
        Ok(InDatabase::new(db.conn.last_insert_rowid(), self))
    }

    fn select_one(db: &Database, id: i64, params: I) -> Result<InDatabase<Self>> {
        db.conn
            .prepare_cached(Self::SELECT_ONE)?
            .query_one((id,), Self::map_row(params))
    }

    fn select_all(db: &Database, params: I) -> Result<Vec<InDatabase<Self>>> {
        db.conn
            .prepare_cached(Self::SELECT_ALL)?
            .query_map((), Self::map_row(params))?
            .collect()
    }

    fn delete_all(db: &Database) -> Result<usize> {
        db.conn.execute(Self::DELETE_ALL, ())
    }
}

pub trait Upsertable<I = (), E = ()>: Queryable<I, E> {
    fn upsert(self, db: &Database, params: E) -> Result<InDatabase<Self>>;
}
