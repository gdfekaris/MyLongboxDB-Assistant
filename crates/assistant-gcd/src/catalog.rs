//! The index of the owner's GCD copy, as a lookup reads it.

use std::path::Path;

use assistant_core::Date;
use rusqlite::{Connection, OpenFlags};

use crate::dump::harden;
use crate::index::{INDEX_FILE, INDEX_VERSION};

/// Why the index could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogError {
    /// There is no index: the owner has not picked a GCD dump.
    Missing,
    /// The index was made by another version of the app, or is damaged;
    /// the dump must be read in again.
    Outdated,
    Unreadable(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("no copy of the Grand Comics Database has been added"),
            Self::Outdated => f.write_str(
                "the copy of the Grand Comics Database must be added again for this version",
            ),
            Self::Unreadable(why) => {
                write!(
                    f,
                    "the copy of the Grand Comics Database could not be read: {why}"
                )
            }
        }
    }
}

impl std::error::Error for CatalogError {}

/// What the index holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct About {
    /// The day the GCD made the dump it was read from.
    pub dated: Option<Date>,
    pub series: u64,
    pub issues: u64,
}

/// The owner's GCD copy, opened to read.
pub struct Catalog {
    conn: Connection,
}

impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Catalog")
    }
}

impl Catalog {
    /// The index in `folder`, opened read-only.
    pub fn open(folder: &Path) -> Result<Self, CatalogError> {
        let path = folder.join(INDEX_FILE);
        if !path.is_file() {
            return Err(CatalogError::Missing);
        }
        let unreadable = |e: rusqlite::Error| {
            CatalogError::Unreadable(assistant_core::lookup::clean(&e.to_string(), 200))
        };
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(unreadable)?;
        harden(&conn).map_err(unreadable)?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(unreadable)?;
        if version != INDEX_VERSION {
            return Err(CatalogError::Outdated);
        }
        Ok(Self { conn })
    }

    fn meta(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .ok()
    }

    /// The dump's date and how many series and issues it holds.
    pub fn about(&self) -> About {
        let number = |key: &str| self.meta(key).and_then(|n| n.parse().ok()).unwrap_or(0);
        About {
            dated: self.meta("dated").and_then(|d| d.parse().ok()),
            series: number("series"),
            issues: number("issues"),
        }
    }
}
