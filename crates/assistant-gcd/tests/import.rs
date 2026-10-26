//! Reading a made-up dump, shaped as the GCD's is, into the index: what
//! is kept, what is left out, and what is refused.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use assistant_gcd::{About, Catalog, CatalogError, INDEX_FILE, ImportError, Step, import};
use rusqlite::Connection;

/// A folder of its own for each test.
fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The GCD's tables, with the columns the index reads and a few it
/// does not.
const TABLES: &str = "
CREATE TABLE gcd_publisher (id INTEGER PRIMARY KEY, name TEXT, url TEXT);
CREATE TABLE stddata_country (id INTEGER PRIMARY KEY, code TEXT, name TEXT);
CREATE TABLE stddata_language (id INTEGER PRIMARY KEY, code TEXT, name TEXT);
CREATE TABLE gcd_series (id INTEGER PRIMARY KEY, name TEXT, sort_name TEXT, year_began INTEGER,
    year_ended INTEGER, issue_count INTEGER, publisher_id INTEGER, country_id INTEGER,
    language_id INTEGER, publishing_format TEXT, publication_type_id INTEGER, deleted INTEGER);
CREATE TABLE gcd_indicia_publisher (id INTEGER PRIMARY KEY, name TEXT);
CREATE TABLE gcd_issue (id INTEGER PRIMARY KEY, series_id INTEGER, number TEXT, volume TEXT,
    variant_of_id INTEGER, variant_name TEXT, title TEXT, publication_date TEXT, key_date TEXT,
    on_sale_date TEXT, price TEXT, page_count REAL, barcode TEXT, valid_isbn TEXT,
    indicia_publisher_id INTEGER, notes TEXT, modified TEXT, deleted INTEGER);
CREATE TABLE gcd_story (id INTEGER PRIMARY KEY, issue_id INTEGER, sequence_number INTEGER,
    type_id INTEGER, title TEXT, feature TEXT, page_count REAL, characters TEXT, synopsis TEXT,
    script TEXT, pencils TEXT, inks TEXT, colors TEXT, letters TEXT, deleted INTEGER);
CREATE TABLE gcd_creator_name_detail (id INTEGER PRIMARY KEY, name TEXT);
CREATE TABLE gcd_story_credit (id INTEGER PRIMARY KEY, story_id INTEGER, creator_id INTEGER,
    credit_type_id INTEGER, credited_as TEXT, credit_name TEXT, uncertain INTEGER, deleted INTEGER);
CREATE TABLE gcd_issue_credit (id INTEGER PRIMARY KEY, issue_id INTEGER, creator_id INTEGER,
    credit_type_id INTEGER, credited_as TEXT, credit_name TEXT, uncertain INTEGER, deleted INTEGER);
CREATE TABLE gcd_character_name_detail (id INTEGER PRIMARY KEY, name TEXT);
CREATE TABLE gcd_story_character (id INTEGER PRIMARY KEY, story_id INTEGER, character_id INTEGER,
    role_id INTEGER, is_origin INTEGER, is_death INTEGER, notes TEXT, deleted INTEGER);
";

/// One made-up series with a direct issue, its newsstand variant, a
/// deleted issue, and stories of every kind that matters and one that
/// does not.
const ROWS: &str = "
INSERT INTO gcd_publisher VALUES (1, 'Atomic Age', '');
INSERT INTO stddata_country VALUES (225, 'us', 'United States');
INSERT INTO stddata_language VALUES (25, 'en', 'English');
INSERT INTO gcd_series VALUES (10, 'The Astonishing Robot Tales', 'Astonishing Robot Tales', 1954,
    1956, 24, 1, 225, 25, 'was ongoing series', 2, 0);
INSERT INTO gcd_series VALUES (11, 'Deleted Tales', '', 1960, NULL, 1, 1, 225, 25, '', 2, 1);
INSERT INTO gcd_indicia_publisher VALUES (5, 'Atomic Age Publications, Inc.');
INSERT INTO gcd_issue VALUES (100, 10, '1', '1', NULL, 'Direct', '', 'March 1954', '1954-03-00',
    '1954-01-12', '0.10 USD; 0.15 CAD', 36, '', '', 5, 'First issue.', '2026-10-16 21:58:09', 0);
INSERT INTO gcd_issue VALUES (101, 10, '1', '1', 100, 'Newsstand', '', 'March 1954',
    '1954-03-00', '1954-01-20', '0.10 USD', 36, '07596060893600111', '', 5, '',
    '2026-10-01 10:00:00', 0);
INSERT INTO gcd_issue VALUES (102, 10, '[nn]', '', NULL, '', 'Giant Robot Special', '1955',
    'March', 'soon', 'free', -4, 'not digits', '', NULL, '', '2025-01-01 00:00:00', 0);
INSERT INTO gcd_issue VALUES (103, 10, '2', '', NULL, '', '', '', '', '', '', 36, '', '', 5, '',
    '2026-10-20 00:00:00', 1);
INSERT INTO gcd_story VALUES (1000, 100, 0, 6, '', '', 1, '', '', '', '', '', '', '', 0);
INSERT INTO gcd_story VALUES (1001, 100, 1, 19, 'The Robot Who Read Comics', 'Tinman', 30,
    'Tinman (introduction)', 'Tinman learns to read.', '?', '', '', '?', 'typeset', 0);
INSERT INTO gcd_story VALUES (1002, 100, 2, 2, 'Sea-Monkeys', '', 1, '', '', '?', '?', '?', '?',
    '?', 0);
INSERT INTO gcd_story VALUES (1003, 101, 0, 6, '', '', 1, '', '', '', '', '', '', '', 0);
INSERT INTO gcd_creator_name_detail VALUES (1, 'Ada Sprocket');
INSERT INTO gcd_creator_name_detail VALUES (2, 'Rex Rivet');
INSERT INTO gcd_creator_name_detail VALUES (3, 'Ed Itor');
INSERT INTO gcd_creator_name_detail VALUES (4, 'Big Chief');
INSERT INTO gcd_story_credit VALUES (1, 1000, 2, 7, '', '', 0, 0);
INSERT INTO gcd_story_credit VALUES (2, 1001, 1, 1, 'A. Sprocket', '', 0, 0);
INSERT INTO gcd_story_credit VALUES (3, 1001, 2, 2, '', '', 1, 0);
INSERT INTO gcd_story_credit VALUES (4, 1002, 2, 2, '', '', 0, 0);
INSERT INTO gcd_story_credit VALUES (5, 1001, 1, 3, '', '', 0, 1);
INSERT INTO gcd_issue_credit VALUES (1, 100, 3, 6, '', 'editor', 0, 0);
INSERT INTO gcd_issue_credit VALUES (2, 100, 4, 6, '', 'editor-in-chief', 0, 0);
INSERT INTO gcd_character_name_detail VALUES (1, 'Tinman');
INSERT INTO gcd_character_name_detail VALUES (2, 'Professor Gears');
INSERT INTO gcd_story_character VALUES (1, 1001, 1, 3, 1, 0, 'first appearance', 0);
INSERT INTO gcd_story_character VALUES (2, 1001, 2, 6, 0, 1, '', 0);
INSERT INTO gcd_story_character VALUES (3, 1002, 1, 3, 0, 0, '', 0);
";

fn dump(dir: &Path, extra: &str) -> PathBuf {
    let path = dir.join("2026-10-16.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(TABLES).unwrap();
    conn.execute_batch(ROWS).unwrap();
    conn.execute_batch(extra).unwrap();
    path
}

fn imported(dir: &Path) -> Result<assistant_gcd::Imported, ImportError> {
    let mut steps = Vec::new();
    let result = import(
        &dir.join("2026-10-16.db"),
        dir,
        &mut |step, _| steps.push(step),
        &AtomicBool::new(false),
    );
    if result.is_ok() {
        assert_eq!(steps.first(), Some(&Step::Series));
        assert_eq!(steps.last(), Some(&Step::Indexing));
    }
    result
}

/// The index, opened as SQLite to look inside it.
fn index(dir: &Path) -> Connection {
    Connection::open(dir.join(INDEX_FILE)).unwrap()
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn the_dump_becomes_an_index_of_what_a_lookup_needs() {
    let dir = folder("gcd-import");
    dump(&dir, "");
    let done = imported(&dir).unwrap();
    assert_eq!(done.series, 1);
    assert_eq!(done.issues, 3);
    assert_eq!(done.dated.unwrap().to_string(), "2026-10-16");
    let about = Catalog::open(&dir).unwrap().about();
    assert_eq!(
        about,
        About {
            dated: done.dated,
            series: 1,
            issues: 3
        }
    );
    let conn = index(&dir);
    // Deleted rows and ads are left out, with their credits and
    // characters.
    assert_eq!(count(&conn, "SELECT count(*) FROM series"), 1);
    assert_eq!(count(&conn, "SELECT count(*) FROM issue WHERE id = 103"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM story"), 3);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM credit WHERE story_id = 1002"),
        0
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM credit WHERE story_id IS NOT NULL"
        ),
        3
    );
    assert_eq!(
        count(&conn, "SELECT count(*) FROM credit WHERE issue_id = 100"),
        2
    );
    assert_eq!(count(&conn, "SELECT count(*) FROM appearance"), 2);
    let (key, publisher, country): (String, String, String) = conn
        .query_row(
            "SELECT name_key, publisher, country FROM series WHERE id = 10",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (key.as_str(), publisher.as_str(), country.as_str()),
        ("astonishing robot tales", "Atomic Age", "us")
    );
    let barcode: String = conn
        .query_row(
            "SELECT barcode FROM issue WHERE variant_of = 100",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(barcode, "07596060893600111");
    // Values not in the forms a lookup can use are left out.
    let odd: (String, Option<String>, Option<String>, Option<f64>, Option<String>) = conn
        .query_row(
            "SELECT number_key, key_date, on_sale_date, page_count, barcode FROM issue WHERE id = 102",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(odd, ("nn".into(), None, None, None, None));
    // The old text credits are kept where they say something.
    let legacy: Option<String> = conn
        .query_row("SELECT legacy FROM story WHERE id = 1001", [], |r| r.get(0))
        .unwrap();
    assert_eq!(legacy.as_deref(), Some("letters: typeset"));
    assert!(!dir.join(format!("{INDEX_FILE}.partial")).exists());
}

#[test]
fn text_from_the_dump_is_cleaned_and_capped() {
    let dir = folder("gcd-hostile");
    dump(
        &dir,
        &format!(
            "UPDATE gcd_series SET name = 'Robot\u{202e} Tales\u{0007}' WHERE id = 10;
             UPDATE gcd_issue SET notes = '{}' WHERE id = 100;
             UPDATE gcd_story SET synopsis = x'00ff41' WHERE id = 1001;",
            "word ".repeat(1000)
        ),
    );
    imported(&dir).unwrap();
    let conn = index(&dir);
    let name: String = conn
        .query_row("SELECT name FROM series", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "Robot Tales");
    let notes: String = conn
        .query_row("SELECT notes FROM issue WHERE id = 100", [], |r| r.get(0))
        .unwrap();
    assert_eq!(notes.chars().count(), 1_000);
    // A value stored as bytes is not text and is left out.
    let synopsis: Option<String> = conn
        .query_row("SELECT synopsis FROM story WHERE id = 1001", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(synopsis, None);
}

#[test]
fn a_file_that_is_not_the_gcd_s_dump_is_refused() {
    let dir = folder("gcd-not-sqlite");
    std::fs::write(dir.join("2026-10-16.db"), b"PK\x03\x04 this is a zip, say").unwrap();
    assert!(matches!(imported(&dir), Err(ImportError::Unreadable(_))));

    let dir = folder("gcd-missing-table");
    dump(&dir, "DROP TABLE gcd_story_character;");
    assert_eq!(
        imported(&dir),
        Err(ImportError::NotGcd(
            "it has no gcd_story_character table".into()
        ))
    );

    let dir = folder("gcd-missing-column");
    dump(&dir, "ALTER TABLE gcd_issue DROP COLUMN barcode;");
    assert_eq!(
        imported(&dir),
        Err(ImportError::NotGcd(
            "its gcd_issue table has no barcode column".into()
        ))
    );

    // A view standing in for a table could make any query of the dump
    // run whatever it holds.
    let dir = folder("gcd-view");
    dump(
        &dir,
        "ALTER TABLE gcd_publisher RENAME TO publishers;
         CREATE VIEW gcd_publisher AS SELECT * FROM publishers;",
    );
    assert_eq!(
        imported(&dir),
        Err(ImportError::NotGcd("it has no gcd_publisher table".into()))
    );
    assert!(!dir.join(INDEX_FILE).exists());
}

#[test]
fn a_cancelled_or_failed_import_keeps_the_index_there_was() {
    let dir = folder("gcd-cancel");
    dump(&dir, "");
    imported(&dir).unwrap();
    let before = std::fs::read(dir.join(INDEX_FILE)).unwrap();
    let cancelled = import(
        &dir.join("2026-10-16.db"),
        &dir,
        &mut |_, _| {},
        &AtomicBool::new(true),
    );
    assert_eq!(cancelled, Err(ImportError::Cancelled));
    assert_eq!(std::fs::read(dir.join(INDEX_FILE)).unwrap(), before);
    assert!(!dir.join(format!("{INDEX_FILE}.partial")).exists());

    std::fs::write(dir.join("2026-10-16.db"), b"not a database").unwrap();
    assert!(imported(&dir).is_err());
    assert_eq!(std::fs::read(dir.join(INDEX_FILE)).unwrap(), before);
}

#[test]
fn the_index_is_private_and_checked_when_opened() {
    use std::os::unix::fs::PermissionsExt;
    let dir = folder("gcd-open");
    assert_eq!(Catalog::open(&dir).unwrap_err(), CatalogError::Missing);
    dump(&dir, "");
    imported(&dir).unwrap();
    let mode = std::fs::metadata(dir.join(INDEX_FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    index(&dir).pragma_update(None, "user_version", 99).unwrap();
    assert_eq!(Catalog::open(&dir).unwrap_err(), CatalogError::Outdated);
}
