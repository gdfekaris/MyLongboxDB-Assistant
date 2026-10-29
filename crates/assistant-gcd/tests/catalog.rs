//! Finding a book in the index of a made-up dump, and the record a
//! lookup is given of it.

mod common;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use assistant_gcd::{Catalog, Record, import};
use common::{dump, folder};

/// More of the made-up catalogue: a later issue sharing the series'
/// barcode, as older comics' barcodes did; a variant with a cover of its
/// own, which a colorist helped with; an edition catalogued as an issue
/// of its own, and another issue's; an annual; a one-shot with a long title; a foreign edition of
/// the series; and a series a cover's logo names with more words.
const MORE: &str = "
INSERT INTO stddata_country VALUES (54, 'de', 'Germany');
INSERT INTO stddata_language VALUES (22, 'de', 'German');
INSERT INTO gcd_issue VALUES (104, 10, '2', '1', NULL, '', '', 'April 1954', '1954-04-00', '',
    '0.10 USD', 36, '07596060893600111', '', 5, '', '2026-01-01 00:00:00', 0);
INSERT INTO gcd_issue VALUES (105, 10, '1', '1', 100, 'Cover B', '', 'March 1954', '1954-03-00',
    '', '0.10 USD', 36, '', '', 5, '', '2026-01-01 00:00:00', 0);
INSERT INTO gcd_story VALUES (1004, 105, 0, 6, '', '', 1, '', '', '', '', '', '', '', 0);
INSERT INTO gcd_creator_name_detail VALUES (5, 'Vera Variant');
INSERT INTO gcd_story_credit VALUES (6, 1004, 5, 7, '', '', 0, 0);
INSERT INTO gcd_story_credit VALUES (7, 1004, 3, 4, '', '', 0, 0);
INSERT INTO gcd_issue VALUES (106, 10, '1 [Collector''s Edition]', '1', NULL, '', '',
    'March 1954', '1954-03-00', '', '0.25 USD', 52, '', '', 5, '', '2026-01-01 00:00:00', 0);
INSERT INTO gcd_story VALUES (1005, 106, 0, 6, '', '', 1, '', '', '', '', '', '', '', 0);
INSERT INTO gcd_story_credit VALUES (8, 1005, 5, 7, 'Vera Variant', '', 0, 0);
INSERT INTO gcd_issue VALUES (107, 10, '10 [UK]', '1', NULL, '', '', '', '', '', '', 36, '',
    '', 5, '', '2026-01-01 00:00:00', 0);
INSERT INTO gcd_series VALUES (20, 'The Astonishing Robot Tales Annual', '', 1955, 1956, 2, 1,
    225, 25, '', 2, 0);
INSERT INTO gcd_issue VALUES (200, 20, '3', '', NULL, '', '', '1955', '1955-00-00', '', '', 64,
    '', '', 5, '', '2026-01-01 00:00:00', 0);
INSERT INTO gcd_series VALUES (21, 'Robot Treasury Special, Giant Holiday Grab-Bag', '', 1974,
    1974, 1, 1, 225, 25, 'one-shot', 2, 0);
INSERT INTO gcd_issue VALUES (210, 21, '[nn]', '', NULL, '', '', '1974', '1974-00-00', '', '',
    100, '', '', 5, '', '2026-01-01 00:00:00', 0);
INSERT INTO gcd_series VALUES (22, 'The Astonishing Robot Tales', '', 1954, 1955, 12, 1, 54, 22,
    '', 2, 0);
INSERT INTO gcd_issue VALUES (220, 22, '1', '', NULL, '', '', '1954', '1954-00-00', '',
    '0.30 DEM', 36, '', '', NULL, '', '2026-01-01 00:00:00', 0);
INSERT INTO gcd_series VALUES (23, 'The Spectacular Robot', '', 1976, 1998, 263, 1, 225, 25, '',
    2, 0);
INSERT INTO gcd_issue VALUES (230, 23, '90', '', NULL, '', '', 'May 1984', '1984-05-00', '', '',
    36, '', '', 5, '', '2026-01-01 00:00:00', 0);
INSERT INTO gcd_series VALUES (24, 'Robot', '', 1980, 1990, 100, 1, 225, 25, '', 2, 0);
INSERT INTO gcd_issue VALUES (240, 24, '90', '', NULL, '', '', 'May 1989', '1989-05-00', '', '',
    36, '', '', 5, '', '2026-01-01 00:00:00', 0);
";

fn catalog(name: &str) -> Catalog {
    let dir = folder(name);
    dump(&dir, MORE);
    import(
        &dir.join("2026-10-16.db"),
        &dir,
        &mut |_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    Catalog::open(Path::new(&dir)).unwrap()
}

fn found(catalog: &Catalog, series: &str, year: Option<u16>, number: &str) -> Vec<Record> {
    catalog.find(series, year, number, None).unwrap()
}

#[test]
fn a_book_is_found_by_its_series_whatever_the_case_and_the() {
    let catalog = catalog("gcd-find-name");
    let records = found(&catalog, "astonishing robot tales", Some(1954), "#1");
    assert_eq!(records.len(), 2);
    // The US series comes before the German one of the same name.
    assert_eq!(records[0].series, "The Astonishing Robot Tales");
    assert_eq!(records[0].start_year, Some(1954));
    assert!(records[1].text.contains("de/de"), "{}", records[1].text);
    // A start year that fits no series finds nothing.
    assert!(found(&catalog, "Astonishing Robot Tales", Some(1999), "1").is_empty());
    assert!(found(&catalog, "Astonishing Robot Tales", None, "99").is_empty());
}

#[test]
fn the_record_says_what_the_lookup_needs() {
    let catalog = catalog("gcd-record");
    let text = found(&catalog, "Astonishing Robot Tales", Some(1954), "1")
        .remove(0)
        .text;
    for said in [
        "Series: The Astonishing Robot Tales (1954\u{2013}1956), Atomic Age",
        "Issue: #1",
        "Edition Direct (the issue itself): cover date as printed March 1954; catalogue date 1954-03; on sale 1954-01-12; price 0.10 USD; 0.15 CAD; 36 pages",
        "Edition Newsstand:",
        "barcode 07596060893600111",
        "Edition Cover B:",
        "Issue credits: editing: Ed Itor (editor); editing: Big Chief (editor-in-chief)",
        "36 pages; indicia publisher Atomic Age Publications, Inc.; cover: pencils and inks: Rex Rivet\n",
        "Story 1 (comic story) \u{201c}The Robot Who Read Comics\u{201d}, feature Tinman, 30 pages: script: Ada Sprocket (credited as A. Sprocket); pencils: Rex Rivet (uncertain)",
        "Characters: Tinman (featured, origin, first appearance); Professor Gears (villain, death)",
        "Synopsis: Tinman learns to read.",
        "Edition Cover B: price 0.10 USD; cover: pencils and inks: Vera Variant\n",
        "Notes: First issue.",
        "Edition Collector's Edition (catalogued as an issue of its own, #1 [Collector's Edition]): price 0.25 USD; 52 pages; cover: pencils and inks: Vera Variant\n",
    ] {
        assert!(text.contains(said), "{said}\n\n{text}");
    }
    // The catalogue's count of the series' issues takes in its specials,
    // so it is not given as the run's length.
    assert!(!text.contains("24 issues"), "{text}");
    // Ads and deleted credits are not in it.
    assert!(!text.contains("Sea-Monkeys"), "{text}");
    assert!(!text.contains("inks: Ada Sprocket"), "{text}");
    // A cover's colorist is not one of its artists, and #10's UK
    // edition is not #1's.
    assert!(!text.contains("colors"), "{text}");
    assert!(!text.contains("UK"), "{text}");
    // A variant does not repeat the dates it shares with the issue.
    assert_eq!(text.matches("cover date as printed").count(), 1, "{text}");
}

/// An issue with seventy variants, each with a cover of its own, and
/// more stories than a record has room for.
fn crowded() -> String {
    let mut sql = String::from(
        "INSERT INTO gcd_series VALUES (30, 'Variant Robot', '', 2019, 2019, 7, 1, 225, 25, '',
             2, 0);
         INSERT INTO gcd_issue VALUES (300, 30, '1', '', NULL, '', '', 'March 2019',
             '2019-03-00', '2019-01-23', '4.99 USD', 40, '', '', 5, '', '2026-01-01', 0);
         INSERT INTO gcd_story VALUES (3000, 300, 0, 6, '', '', 1, '', '', '', '', '', '', '',
             0);
         INSERT INTO gcd_story_credit VALUES (300, 3000, 1, 7, '', '', 0, 0);",
    );
    for n in 1..=70 {
        let (issue, story, creator) = (300 + n, 3000 + n, 100 + n);
        sql.push_str(&format!(
            "INSERT INTO gcd_issue VALUES ({issue}, 30, '1', '', 300, 'Store {n} Exclusive',
                 '', 'March 2019', '2019-03-00', '2019-01-23', '4.99 USD', 40, '', '', 5, '',
                 '2026-01-01', 0);
             INSERT INTO gcd_story VALUES ({story}, {issue}, 0, 6, '', '', 1, '', '', '', '',
                 '', '', '', 0);
             INSERT INTO gcd_creator_name_detail VALUES ({creator}, 'Artist Number {n}');
             INSERT INTO gcd_story_credit VALUES ({story}, {story}, {creator}, 7, '', '', 0,
                 0);"
        ));
    }
    let synopsis = "The robot walks on. ".repeat(40);
    for n in 1..=40 {
        let story = 4000 + n;
        sql.push_str(&format!(
            "INSERT INTO gcd_story VALUES ({story}, 300, {n}, 19, 'Chapter {n}', '', 6, '',
                 '{synopsis}', '', '', '', '', '', 0);"
        ));
    }
    sql
}

#[test]
fn every_edition_and_its_cover_artist_is_given_however_long_the_record() {
    let dir = folder("gcd-crowded");
    dump(&dir, &format!("{MORE}{}", crowded()));
    import(
        &dir.join("2026-10-16.db"),
        &dir,
        &mut |_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    let catalog = Catalog::open(Path::new(&dir)).unwrap();
    let text = found(&catalog, "Variant Robot", Some(2019), "1")
        .remove(0)
        .text;
    assert!(text.chars().count() <= assistant_gcd::MAX_RECORD);
    assert!(text.contains("Edition: the issue itself"), "{text}");
    for n in 1..=70 {
        let line =
            format!("Edition Store {n} Exclusive: cover: pencils and inks: Artist Number {n}\n");
        assert!(text.contains(&line), "{line}\n\n{text}");
    }
    // The stories fill what room is left, and the last are cut.
    assert!(
        text.contains("Story 1 (comic story) \u{201c}Chapter 1\u{201d}"),
        "{text}"
    );
    assert!(!text.contains("Chapter 40"), "{text}");
    assert!(text.ends_with('\u{2026}'), "{text}");
}

#[test]
fn a_barcode_finds_its_edition_among_issues_that_share_it() {
    let catalog = catalog("gcd-barcode");
    // The series' barcode is on #1's newsstand edition and on #2.
    let both = catalog
        .find("", None, "", Some("7 59606 08936 0 0111"))
        .unwrap();
    assert_eq!(both.len(), 2);
    let first = catalog
        .find(
            "Whatever the cover says",
            None,
            "1",
            Some("759606089360 0111"),
        )
        .unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].number, "1");
    assert!(
        first[0]
            .text
            .contains("Edition Newsstand: on sale 1954-01-20; price 0.10 USD; barcode 07596060893600111; its barcode is the one on the owner's cover\n"),
        "{}",
        first[0].text
    );
    // A barcode the copy does not know leaves the name to find it.
    let named = catalog
        .find(
            "Astonishing Robot Tales",
            Some(1954),
            "2",
            Some("012345678905"),
        )
        .unwrap();
    assert_eq!(named[0].number, "2");
}

#[test]
fn an_annual_is_found_in_the_series_annual() {
    let catalog = catalog("gcd-annual");
    let records = found(&catalog, "Astonishing Robot Tales", None, "Annual 3");
    assert_eq!(records[0].series, "The Astonishing Robot Tales Annual");
    assert_eq!(records[0].number, "3");
}

#[test]
fn a_name_found_nowhere_whole_is_looked_for_more_loosely() {
    let catalog = catalog("gcd-close");
    // A shortened title finds the one-shot whose name it starts.
    let shortened = found(&catalog, "Robot Treasury Special", Some(1974), "nn");
    assert_eq!(
        shortened[0].series,
        "Robot Treasury Special, Giant Holiday Grab-Bag"
    );
    // Its words in another order and with other punctuation.
    let reordered = found(
        &catalog,
        "Giant Holiday Robot Treasury Special",
        Some(1974),
        "nn",
    );
    assert_eq!(reordered.len(), 1);
    // A logo that adds a name finds the series whose words it holds, the
    // one sharing the most of them first.
    let logo = found(&catalog, "Peter Parker, the Spectacular Robot", None, "90");
    assert_eq!(logo[0].series, "The Spectacular Robot");
    // Robot shares a single word with it, which is not enough.
    assert!(logo.iter().all(|r| r.series != "Robot"));
    // One word in common is not enough.
    assert!(found(&catalog, "Robot Wars", None, "90").is_empty());
}

#[test]
fn a_lookup_is_given_titled_records() {
    use assistant_core::lookup::Catalog as _;
    let catalog = catalog("gcd-records");
    let query = assistant_core::parse_lines("Astonishing Robot Tales (1954) #1", 2026)
        .0
        .remove(0);
    let records = catalog.records(&query, None);
    assert_eq!(
        records[0].title,
        "Grand Comics Database (your copy, dated 2026-10-16): The Astonishing Robot Tales (1954) #1"
    );
    assert!(
        records[0]
            .text
            .starts_with("Series: The Astonishing Robot Tales")
    );
}
