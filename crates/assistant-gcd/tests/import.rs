//! Reading a made-up dump, shaped as the GCD's is, into the index: what
//! is kept, what is left out, and what is refused.

mod common;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use assistant_gcd::{About, Catalog, CatalogError, INDEX_FILE, ImportError, Step, import};
use common::{dump, folder};
use rusqlite::Connection;

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

/// The dump in a zip, as the GCD offers it for download, beside a
/// readme.
fn zipped(dir: &Path, dump: &Path, name: &str) -> std::path::PathBuf {
    use std::io::Write;
    let path = dir.join("current.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("README.txt", options).unwrap();
    zip.write_all(b"Data from the Grand Comics Database")
        .unwrap();
    zip.start_file(name, options).unwrap();
    zip.write_all(&std::fs::read(dump).unwrap()).unwrap();
    zip.finish().unwrap();
    path
}

#[test]
fn a_dump_in_the_gcd_s_zip_is_read_from_it() {
    let dir = folder("gcd-zip");
    let db = dump(&dir, "");
    let zip = zipped(&dir, &db, "2026-10-16.db");
    std::fs::remove_file(&db).unwrap();
    let mut steps = Vec::new();
    let done = import(
        &zip,
        &dir,
        &mut |s, _| steps.push(s),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(done.issues, 3);
    assert_eq!(steps[0], Step::Unpacking);
    // The dump taken out of the zip is not kept.
    let left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let mut left = left;
    left.sort();
    assert_eq!(left, ["current.zip", INDEX_FILE]);
}

#[test]
fn a_zip_without_one_dump_is_refused() {
    let dir = folder("gcd-zip-other");
    let db = dump(&dir, "");
    let zip = zipped(&dir, &db, "notes.txt");
    assert_eq!(
        import(&zip, &dir, &mut |_, _| {}, &AtomicBool::new(false)),
        Err(ImportError::NotGcd("the zip holds no .db file".into()))
    );
    assert!(!dir.join(INDEX_FILE).exists());
}
