//! Reading the owner's GCD dump into the app's own index: the series,
//! issues, and the stories that make up a comic (its cover and its
//! stories, not its ads or letters pages), with their credits and
//! characters. The index is a fraction of the dump's size, has the
//! indexes a lookup needs, and holds only values already cleaned and
//! capped, so what a lookup reads from it is as checked as the dump
//! could make it.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use assistant_core::Date;
use rusqlite::types::Value;
use rusqlite::{Connection, Row, Transaction, params_from_iter};

use crate::dump::{self, amount, int, text};
use crate::keys::{number_key, series_key};

/// The index's name in the folder it is kept in.
pub const INDEX_FILE: &str = "gcd.sqlite";
/// The index's layout; an index of another is made again.
pub(crate) const INDEX_VERSION: i64 = 1;

/// The kinds of story kept, by the GCD's story type: the cover (6),
/// photo stories (13), comic stories (19), and text stories (21). Ads,
/// letters pages, and the like say nothing a collection records.
macro_rules! story_types {
    () => {
        "6, 13, 19, 21"
    };
}

/// Rows copied between looks at whether the owner cancelled, and between
/// reports of progress.
const BATCH: u64 = 10_000;

/// Why the dump could not be read into an index. Each says what the
/// owner can do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// The file could not be opened or read as an SQLite database.
    Unreadable(String),
    TooLarge,
    /// It is an SQLite database, but not the GCD's dump.
    NotGcd(String),
    Cancelled,
    /// The index could not be written to the app's folder.
    CannotWrite(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(why) => write!(f, "the file could not be read: {why}"),
            Self::TooLarge => f.write_str("the file is far larger than the GCD's dump"),
            Self::NotGcd(why) => write!(f, "the file is not the GCD's data dump: {why}"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::CannotWrite(why) => write!(f, "the index could not be saved: {why}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// A part of the import, in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Series,
    Issues,
    Stories,
    Credits,
    IssueCredits,
    Characters,
    Indexing,
}

/// What an import read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Imported {
    /// The date of the dump's latest change, which is the date the GCD
    /// made it.
    pub dated: Option<Date>,
    pub series: u64,
    pub issues: u64,
}

fn unreadable(e: rusqlite::Error) -> ImportError {
    ImportError::Unreadable(assistant_core::lookup::clean(&e.to_string(), 200))
}

fn unwritable(e: impl std::fmt::Display) -> ImportError {
    ImportError::CannotWrite(assistant_core::lookup::clean(&e.to_string(), 200))
}

const SCHEMA: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE series (
    id INTEGER PRIMARY KEY, name TEXT NOT NULL, name_key TEXT NOT NULL,
    year_began INTEGER, year_ended INTEGER, issue_count INTEGER,
    publisher TEXT, country TEXT, language TEXT, format TEXT, kind INTEGER
) STRICT;
CREATE TABLE issue (
    id INTEGER PRIMARY KEY, series_id INTEGER NOT NULL, number TEXT,
    number_key TEXT NOT NULL, variant_of INTEGER, variant_name TEXT, title TEXT,
    publication_date TEXT, key_date TEXT, on_sale_date TEXT, price TEXT,
    page_count REAL, barcode TEXT, isbn TEXT, indicia_publisher TEXT, notes TEXT
) STRICT;
CREATE TABLE story (
    id INTEGER PRIMARY KEY, issue_id INTEGER NOT NULL, seq INTEGER, kind INTEGER,
    title TEXT, feature TEXT, page_count REAL, characters TEXT, synopsis TEXT,
    legacy TEXT
) STRICT;
CREATE TABLE credit (
    story_id INTEGER, issue_id INTEGER, role INTEGER, name TEXT NOT NULL,
    credited_as TEXT, detail TEXT, uncertain INTEGER NOT NULL
) STRICT;
CREATE TABLE appearance (
    story_id INTEGER NOT NULL, name TEXT NOT NULL, role INTEGER,
    origin INTEGER NOT NULL, death INTEGER NOT NULL, notes TEXT
) STRICT;
";

const INDEXES: &str = "
CREATE INDEX series_key ON series (name_key, year_began);
CREATE INDEX issue_number ON issue (series_id, number_key);
CREATE INDEX issue_barcode ON issue (barcode) WHERE barcode IS NOT NULL;
CREATE INDEX issue_variant ON issue (variant_of) WHERE variant_of IS NOT NULL;
CREATE INDEX story_issue ON story (issue_id, seq);
CREATE INDEX credit_story ON credit (story_id) WHERE story_id IS NOT NULL;
CREATE INDEX credit_issue ON credit (issue_id) WHERE issue_id IS NOT NULL;
CREATE INDEX appearance_story ON appearance (story_id);
";

/// One table of the index, filled from one query of the dump.
struct Copy {
    step: Step,
    select: &'static str,
    insert: &'static str,
    convert: fn(&Row<'_>) -> rusqlite::Result<Option<Vec<Value>>>,
}

fn t(value: Option<String>) -> Value {
    value.map_or(Value::Null, Value::Text)
}

fn i(value: Option<i64>) -> Value {
    value.map_or(Value::Null, Value::Integer)
}

fn r(value: Option<f64>) -> Value {
    value.map_or(Value::Null, Value::Real)
}

fn flag(value: Option<i64>) -> Value {
    Value::Integer(i64::from(value.is_some_and(|n| n != 0)))
}

/// A year as the GCD gives one, if it is one a comic could have.
fn year(value: Option<i64>) -> Option<i64> {
    value.filter(|y| (1800..=2200).contains(y))
}

/// A count, if it is one.
fn count(value: Option<i64>) -> Option<i64> {
    value.filter(|n| (0..=1_000_000).contains(n))
}

/// A date the GCD keeps as text, `YYYY-MM-DD` with 00 for what it does
/// not know: kept only in that form.
fn date_text(value: Option<String>) -> Option<String> {
    let value = value?;
    let digits = value.bytes().enumerate().all(|(n, b)| match n {
        4 | 7 => b == b'-',
        _ => b.is_ascii_digit(),
    });
    (value.len() == 10 && digits).then_some(value)
}

/// The text credits a story had before the GCD kept credits by creator,
/// those it knows: "script: Stan Lee; pencils: Jack Kirby".
fn legacy(row: &Row<'_>, from: usize) -> rusqlite::Result<Option<String>> {
    let mut parts = Vec::new();
    for (n, role) in ["script", "pencils", "inks", "colors", "letters"]
        .iter()
        .enumerate()
    {
        if let Some(text) = text(row.get_ref(from + n)?, 300)
            && !text.trim_start_matches('?').trim().is_empty()
        {
            parts.push(format!("{role}: {text}"));
        }
    }
    Ok((!parts.is_empty()).then(|| parts.join("; ")))
}

const COPIES: &[Copy] = &[
    Copy {
        step: Step::Series,
        select: "SELECT s.id, s.name, s.year_began, s.year_ended, s.issue_count, p.name,
                        c.code, l.code, s.publishing_format, s.publication_type_id
                 FROM gcd_series s
                 LEFT JOIN gcd_publisher p ON p.id = s.publisher_id
                 LEFT JOIN stddata_country c ON c.id = s.country_id
                 LEFT JOIN stddata_language l ON l.id = s.language_id
                 WHERE s.deleted = 0",
        insert: "INSERT OR IGNORE INTO series VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        convert: |row| {
            let (Some(id), Some(name)) = (int(row.get_ref(0)?), text(row.get_ref(1)?, 300)) else {
                return Ok(None);
            };
            let key = series_key(&name);
            Ok(Some(vec![
                Value::Integer(id),
                Value::Text(name),
                Value::Text(key),
                i(year(int(row.get_ref(2)?))),
                i(year(int(row.get_ref(3)?))),
                i(count(int(row.get_ref(4)?))),
                t(text(row.get_ref(5)?, 300)),
                t(text(row.get_ref(6)?, 10)),
                t(text(row.get_ref(7)?, 10)),
                t(text(row.get_ref(8)?, 200)),
                i(int(row.get_ref(9)?).filter(|k| (1..=3).contains(k))),
            ]))
        },
    },
    Copy {
        step: Step::Issues,
        select: "SELECT i.id, i.series_id, i.number, i.variant_of_id, i.variant_name, i.title,
                        i.publication_date, i.key_date, i.on_sale_date, i.price, i.page_count,
                        i.barcode, i.valid_isbn, ip.name, i.notes
                 FROM gcd_issue i
                 LEFT JOIN gcd_indicia_publisher ip ON ip.id = i.indicia_publisher_id
                 WHERE i.deleted = 0",
        insert: "INSERT OR IGNORE INTO issue VALUES
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        convert: |row| {
            let (Some(id), Some(series)) = (int(row.get_ref(0)?), int(row.get_ref(1)?)) else {
                return Ok(None);
            };
            let number = text(row.get_ref(2)?, 50);
            let key = number.as_deref().map(number_key).unwrap_or_default();
            let digits = |v: Option<String>, max: usize| {
                v.filter(|d| d.len() <= max && d.bytes().all(|b| b.is_ascii_digit()))
            };
            Ok(Some(vec![
                Value::Integer(id),
                Value::Integer(series),
                t(number),
                Value::Text(key),
                i(int(row.get_ref(3)?)),
                t(text(row.get_ref(4)?, 200)),
                t(text(row.get_ref(5)?, 300)),
                t(text(row.get_ref(6)?, 100)),
                t(date_text(text(row.get_ref(7)?, 10))),
                t(date_text(text(row.get_ref(8)?, 10))),
                t(text(row.get_ref(9)?, 200)),
                r(amount(row.get_ref(10)?).filter(|p| *p <= 10_000.0)),
                t(digits(text(row.get_ref(11)?, 40), 20)),
                t(digits(text(row.get_ref(12)?, 13), 13)),
                t(text(row.get_ref(13)?, 300)),
                t(text(row.get_ref(14)?, 1_000)),
            ]))
        },
    },
    Copy {
        step: Step::Stories,
        select: concat!(
            "SELECT id, issue_id, sequence_number, type_id, title, feature, page_count,
                        characters, synopsis, script, pencils, inks, colors, letters
                 FROM gcd_story
                 WHERE deleted = 0 AND type_id IN (",
            story_types!(),
            ")"
        ),
        insert: "INSERT OR IGNORE INTO story VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        convert: |row| {
            let (Some(id), Some(issue)) = (int(row.get_ref(0)?), int(row.get_ref(1)?)) else {
                return Ok(None);
            };
            Ok(Some(vec![
                Value::Integer(id),
                Value::Integer(issue),
                i(int(row.get_ref(2)?).filter(|n| (0..=10_000).contains(n))),
                i(int(row.get_ref(3)?)),
                t(text(row.get_ref(4)?, 300)),
                t(text(row.get_ref(5)?, 300)),
                r(amount(row.get_ref(6)?).filter(|p| *p <= 10_000.0)),
                t(text(row.get_ref(7)?, 2_000)),
                t(text(row.get_ref(8)?, 1_000)),
                t(legacy(row, 9)?),
            ]))
        },
    },
    Copy {
        step: Step::Credits,
        select: concat!(
            "SELECT sc.story_id, sc.credit_type_id, n.name, sc.credited_as, sc.credit_name,
                        sc.uncertain
                 FROM gcd_story_credit sc
                 JOIN gcd_story s ON s.id = sc.story_id
                 JOIN gcd_creator_name_detail n ON n.id = sc.creator_id
                 WHERE sc.deleted = 0 AND s.deleted = 0 AND s.type_id IN (",
            story_types!(),
            ")"
        ),
        insert: "INSERT INTO credit VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6)",
        convert: credit,
    },
    Copy {
        step: Step::IssueCredits,
        select: "SELECT ic.issue_id, ic.credit_type_id, n.name, ic.credited_as, ic.credit_name,
                        ic.uncertain
                 FROM gcd_issue_credit ic
                 JOIN gcd_creator_name_detail n ON n.id = ic.creator_id
                 WHERE ic.deleted = 0",
        insert: "INSERT INTO credit VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6)",
        convert: credit,
    },
    Copy {
        step: Step::Characters,
        select: concat!(
            "SELECT sc.story_id, n.name, sc.role_id, sc.is_origin, sc.is_death, sc.notes
                 FROM gcd_story_character sc
                 JOIN gcd_story s ON s.id = sc.story_id
                 JOIN gcd_character_name_detail n ON n.id = sc.character_id
                 WHERE sc.deleted = 0 AND s.deleted = 0 AND s.type_id IN (",
            story_types!(),
            ")"
        ),
        insert: "INSERT INTO appearance VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        convert: |row| {
            let (Some(story), Some(name)) = (int(row.get_ref(0)?), text(row.get_ref(1)?, 200))
            else {
                return Ok(None);
            };
            Ok(Some(vec![
                Value::Integer(story),
                Value::Text(name),
                i(int(row.get_ref(2)?)),
                flag(int(row.get_ref(3)?)),
                flag(int(row.get_ref(4)?)),
                t(text(row.get_ref(5)?, 300)),
            ]))
        },
    },
];

/// A story's or an issue's credit: who, as what, under what name, and
/// whether the GCD is sure.
fn credit(row: &Row<'_>) -> rusqlite::Result<Option<Vec<Value>>> {
    let (Some(owner), Some(name)) = (int(row.get_ref(0)?), text(row.get_ref(2)?, 200)) else {
        return Ok(None);
    };
    Ok(Some(vec![
        Value::Integer(owner),
        i(int(row.get_ref(1)?)),
        Value::Text(name),
        t(text(row.get_ref(3)?, 200)),
        t(text(row.get_ref(4)?, 200)),
        flag(int(row.get_ref(5)?)),
    ]))
}

fn copy(
    from: &Connection,
    to: &Transaction<'_>,
    table: &Copy,
    progress: &mut dyn FnMut(Step, u64),
    cancel: &AtomicBool,
) -> Result<u64, ImportError> {
    let mut select = from.prepare(table.select).map_err(unreadable)?;
    let mut insert = to.prepare(table.insert).map_err(unwritable)?;
    let mut rows = select.query([]).map_err(unreadable)?;
    let mut copied = 0;
    while let Some(row) = rows.next().map_err(unreadable)? {
        if let Some(values) = (table.convert)(row).map_err(unreadable)? {
            insert
                .execute(params_from_iter(values))
                .map_err(unwritable)?;
            copied += 1;
            if copied % BATCH == 0 {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ImportError::Cancelled);
                }
                progress(table.step, copied);
            }
        }
    }
    progress(table.step, copied);
    Ok(copied)
}

/// The day the dump was made: the date of its latest change.
fn dated(from: &Connection) -> Result<Option<Date>, ImportError> {
    let latest: Option<String> = from
        .query_row(
            "SELECT max(modified) FROM gcd_issue WHERE deleted = 0",
            [],
            |row| Ok(text(row.get_ref(0)?, 30)),
        )
        .map_err(unreadable)?;
    Ok(latest
        .and_then(|l| l.get(..10).map(str::to_owned))
        .and_then(|d| d.parse::<Date>().ok()))
}

/// Reads the GCD dump at `dump` into a new index in `folder`, in place
/// of any index there once it is complete; an import that fails or is
/// cancelled leaves the earlier index as it was. Progress is reported as
/// each step goes, by rows copied.
///
/// The dump is untrusted, and this reads it through SQLite: call it on a
/// thread limited to reading `dump` and writing in `folder`.
pub fn import(
    dump: &Path,
    folder: &Path,
    progress: &mut dyn FnMut(Step, u64),
    cancel: &AtomicBool,
) -> Result<Imported, ImportError> {
    let from = dump::open(dump)?;
    let partial = folder.join(format!("{INDEX_FILE}.partial"));
    let result = build(&from, &partial, progress, cancel);
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
        return result;
    }
    let done = finish(&partial, &folder.join(INDEX_FILE));
    if done.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    done.and(result)
}

fn build(
    from: &Connection,
    partial: &Path,
    progress: &mut dyn FnMut(Step, u64),
    cancel: &AtomicBool,
) -> Result<Imported, ImportError> {
    match std::fs::remove_file(partial) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(unwritable(e)),
        _ => {}
    }
    let dated = dated(from)?;
    let mut to = Connection::open(partial).map_err(unwritable)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(partial, std::fs::Permissions::from_mode(0o600))
            .map_err(unwritable)?;
    }
    // Made in one go and thrown away if anything fails, so no journal is
    // needed.
    to.pragma_update(None, "journal_mode", "OFF")
        .map_err(unwritable)?;
    to.pragma_update(None, "synchronous", "OFF")
        .map_err(unwritable)?;
    to.pragma_update(None, "temp_store", "MEMORY")
        .map_err(unwritable)?;
    to.execute_batch(SCHEMA).map_err(unwritable)?;
    let tx = to.transaction().map_err(unwritable)?;
    let mut counts = Vec::new();
    for table in COPIES {
        if cancel.load(Ordering::Relaxed) {
            return Err(ImportError::Cancelled);
        }
        counts.push((table.step, copy(from, &tx, table, progress, cancel)?));
    }
    progress(Step::Indexing, 0);
    tx.execute_batch(INDEXES).map_err(unwritable)?;
    let counted = |step: Step| {
        counts
            .iter()
            .find(|(s, _)| *s == step)
            .map_or(0, |(_, n)| *n)
    };
    let imported = Imported {
        dated,
        series: counted(Step::Series),
        issues: counted(Step::Issues),
    };
    for (key, value) in [
        ("dated", dated.map(|d| d.to_string()).unwrap_or_default()),
        ("series", imported.series.to_string()),
        ("issues", imported.issues.to_string()),
    ] {
        tx.execute("INSERT INTO meta VALUES (?1, ?2)", [key, &value])
            .map_err(unwritable)?;
    }
    tx.pragma_update(None, "user_version", INDEX_VERSION)
        .map_err(unwritable)?;
    tx.commit().map_err(unwritable)?;
    to.close().map_err(|(_, e)| unwritable(e))?;
    Ok(imported)
}

/// Puts the finished index in place of the old, so a reader sees one or
/// the other whole.
fn finish(partial: &Path, index: &Path) -> Result<(), ImportError> {
    std::fs::File::open(partial)
        .and_then(|f| f.sync_all())
        .map_err(unwritable)?;
    std::fs::rename(partial, index).map_err(unwritable)?;
    if let Some(folder) = index.parent() {
        std::fs::File::open(folder)
            .and_then(|f| f.sync_all())
            .map_err(unwritable)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_dates_in_the_gcd_s_form_are_kept() {
        assert_eq!(
            date_text(Some("1988-05-00".into())).as_deref(),
            Some("1988-05-00")
        );
        assert_eq!(date_text(Some("May 1988".into())), None);
        assert_eq!(date_text(Some("1988-5-1".into())), None);
        assert_eq!(date_text(Some("1988-05-001".into())), None);
    }
}
