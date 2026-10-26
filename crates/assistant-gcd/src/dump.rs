//! Opening the owner's GCD dump, an untrusted SQLite file, so that
//! reading it can do nothing but read the tables the index needs.

use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use assistant_core::lookup::clean;
use rusqlite::config::DbConfig;
use rusqlite::limits::Limit;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

use crate::index::ImportError;

/// A dump larger than this is refused. The dump of 2026-10-16 is 6.75 GB.
pub(crate) const MAX_DUMP_BYTES: u64 = 40 * 1024 * 1024 * 1024;
/// The longest value SQLite reads from the dump. The GCD's longest notes
/// run to a few thousand characters.
const MAX_VALUE_BYTES: i32 = 4 * 1024 * 1024;
const MAX_SQL_BYTES: i32 = 64 * 1024;

/// The tables the index is made from and the columns it reads of each.
/// Each must be a plain table, not a view.
pub(crate) const READ: &[(&str, &[&str])] = &[
    (
        "gcd_series",
        &[
            "id",
            "name",
            "year_began",
            "year_ended",
            "issue_count",
            "publisher_id",
            "country_id",
            "language_id",
            "publishing_format",
            "publication_type_id",
            "deleted",
        ],
    ),
    ("gcd_publisher", &["id", "name"]),
    ("stddata_country", &["id", "code"]),
    ("stddata_language", &["id", "code"]),
    (
        "gcd_issue",
        &[
            "id",
            "series_id",
            "number",
            "variant_of_id",
            "variant_name",
            "title",
            "publication_date",
            "key_date",
            "on_sale_date",
            "price",
            "page_count",
            "barcode",
            "valid_isbn",
            "indicia_publisher_id",
            "notes",
            "modified",
            "deleted",
        ],
    ),
    ("gcd_indicia_publisher", &["id", "name"]),
    (
        "gcd_story",
        &[
            "id",
            "issue_id",
            "sequence_number",
            "type_id",
            "title",
            "feature",
            "page_count",
            "characters",
            "synopsis",
            "script",
            "pencils",
            "inks",
            "colors",
            "letters",
            "deleted",
        ],
    ),
    (
        "gcd_story_credit",
        &[
            "story_id",
            "creator_id",
            "credit_type_id",
            "credited_as",
            "credit_name",
            "uncertain",
            "deleted",
        ],
    ),
    (
        "gcd_issue_credit",
        &[
            "issue_id",
            "creator_id",
            "credit_type_id",
            "credited_as",
            "credit_name",
            "uncertain",
            "deleted",
        ],
    ),
    ("gcd_creator_name_detail", &["id", "name"]),
    (
        "gcd_story_character",
        &[
            "story_id",
            "character_id",
            "role_id",
            "is_origin",
            "is_death",
            "notes",
            "deleted",
        ],
    ),
    ("gcd_character_name_detail", &["id", "name"]),
];

/// The file as an SQLite URI that opens it read-only and immutable: no
/// lock, journal, or other file is made beside it, and nothing in it is
/// written.
fn uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'.' | b'_' | b'~') {
            uri.push(char::from(b));
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri.push_str("?mode=ro&immutable=1");
    uri
}

/// SQLite set to read an untrusted file and do nothing else: defensive
/// mode, no functions run from the file's schema, no views or triggers,
/// no other database attached, values and statements capped, and every
/// page's cells checked as they are read.
pub(crate) fn harden(conn: &Connection) -> rusqlite::Result<()> {
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_VIEW, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_FTS3_TOKENIZER, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL, false)?;
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES)?;
    conn.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL_BYTES)?;
    conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
    conn.pragma_update(None, "cell_size_check", true)?;
    conn.pragma_update(None, "mmap_size", 0)?;
    // Sorts stay in memory: the thread reading the dump may write only
    // in the index's folder.
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "query_only", true)?;
    Ok(())
}

/// Opens the dump at `path` to read, once it is known to be a file of a
/// size a dump can be, an SQLite database, and one with every table and
/// column the index reads.
pub(crate) fn open(path: &Path) -> Result<Connection, ImportError> {
    let unreadable =
        |e: &dyn std::fmt::Display| ImportError::Unreadable(clean(&e.to_string(), 200));
    let meta = std::fs::metadata(path).map_err(|e| unreadable(&e))?;
    if !meta.is_file() {
        return Err(ImportError::Unreadable("it is not a file".into()));
    }
    if meta.len() > MAX_DUMP_BYTES {
        return Err(ImportError::TooLarge);
    }
    let conn = Connection::open_with_flags(
        uri(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| unreadable(&e))?;
    harden(&conn).map_err(|e| unreadable(&e))?;
    check_schema(&conn)?;
    Ok(conn)
}

/// Whether the dump has every table the index reads, each a plain table
/// with every column read from it.
fn check_schema(conn: &Connection) -> Result<(), ImportError> {
    let unreadable = |e: rusqlite::Error| ImportError::Unreadable(clean(&e.to_string(), 200));
    for (table, columns) in READ {
        let kind: Option<String> = conn
            .query_row(
                "SELECT type FROM sqlite_schema WHERE name = ?1",
                [table],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })
            .map_err(unreadable)?;
        if kind.as_deref() != Some("table") {
            return Err(ImportError::NotGcd(format!("it has no {table} table")));
        }
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info(?1)")
            .map_err(unreadable)?;
        let have: Vec<String> = stmt
            .query_map([table], |r| r.get(0))
            .map_err(unreadable)?
            .collect::<Result<_, _>>()
            .map_err(unreadable)?;
        if let Some(missing) = columns.iter().find(|c| !have.iter().any(|h| h == *c)) {
            return Err(ImportError::NotGcd(format!(
                "its {table} table has no {missing} column"
            )));
        }
    }
    Ok(())
}

/// A value from the dump as text, cleaned of hidden characters and
/// capped, whatever SQLite type it was stored as; `None` when blank.
pub(crate) fn text(value: ValueRef<'_>, max: usize) -> Option<String> {
    let text = match value {
        ValueRef::Text(bytes) => clean(&String::from_utf8_lossy(bytes), max),
        ValueRef::Integer(n) => n.to_string(),
        ValueRef::Real(x) => x.to_string(),
        ValueRef::Null | ValueRef::Blob(_) => return None,
    };
    (!text.is_empty()).then_some(text)
}

/// A value from the dump as a whole number, from an integer or a text
/// that is one; `None` otherwise.
pub(crate) fn int(value: ValueRef<'_>) -> Option<i64> {
    match value {
        ValueRef::Integer(n) => Some(n),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok()?.trim().parse().ok(),
        _ => None,
    }
}

/// A value from the dump as a number of pages or the like: finite and
/// not negative.
pub(crate) fn amount(value: ValueRef<'_>) -> Option<f64> {
    let x = match value {
        ValueRef::Integer(n) => n as f64,
        ValueRef::Real(x) => x,
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok()?.trim().parse().ok()?,
        _ => return None,
    };
    (x.is_finite() && x >= 0.0).then_some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_becomes_a_uri_that_cannot_add_options() {
        assert_eq!(
            uri(Path::new("/run/user/1000/doc/ab12/2026-10-16.db")),
            "file:/run/user/1000/doc/ab12/2026-10-16.db?mode=ro&immutable=1"
        );
        let odd = uri(Path::new("/tmp/a?mode=rwc&b#c%d é"));
        assert_eq!(
            odd,
            "file:/tmp/a%3Fmode%3Drwc%26b%23c%25d%20%C3%A9?mode=ro&immutable=1"
        );
    }

    #[test]
    fn values_are_read_whatever_their_stored_type() {
        assert_eq!(
            text(
                ValueRef::Text("  The\u{202e} Amazing \n Spider-Man ".as_bytes()),
                100
            )
            .as_deref(),
            Some("The Amazing Spider-Man")
        );
        assert_eq!(text(ValueRef::Integer(1963), 100).as_deref(), Some("1963"));
        assert_eq!(text(ValueRef::Text(b"   "), 100), None);
        assert_eq!(text(ValueRef::Blob(b"x"), 100), None);
        assert_eq!(text(ValueRef::Text(&[b'a'; 500]), 10).unwrap().len(), 10);
        assert_eq!(int(ValueRef::Text(b" 42 ")), Some(42));
        assert_eq!(int(ValueRef::Real(4.2)), None);
        assert_eq!(amount(ValueRef::Real(52.0)), Some(52.0));
        assert_eq!(amount(ValueRef::Real(f64::NAN)), None);
        assert_eq!(amount(ValueRef::Integer(-3)), None);
    }
}
