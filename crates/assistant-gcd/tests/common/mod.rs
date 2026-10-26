//! A made-up dump, shaped as the GCD's is, for the tests.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// A folder of its own for each test.
pub fn folder(name: &str) -> PathBuf {
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

pub fn dump(dir: &Path, extra: &str) -> PathBuf {
    let path = dir.join("2026-10-16.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(TABLES).unwrap();
    conn.execute_batch(ROWS).unwrap();
    conn.execute_batch(extra).unwrap();
    path
}
