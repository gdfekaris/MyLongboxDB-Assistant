//! Writes a set of made-up books covering every column the Assistant
//! fills, and checks the export reads back as written. CI also hands the
//! file to MyLongboxDB's own import, at the release pinned in the
//! workflow, which must take every row: set `ASSISTANT_CONTRACT_OUT` to a
//! folder to have the export written there.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use assistant_core::{
    Appearance, ArcPart, Barcode, Binding, Book, COLUMNS, Category, CoverDate, Credit, Date,
    Distribution, Grade, GradeType, Isbn, IssueNumber, Jpeg, MagazineFormat, PartialDate, Role,
    parse_collects, write_export,
};

/// A small cover photo, 24 by 36 pixels, drawn for these tests.
const COVER: &[u8] = &[
    0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01,
    0x00, 0x01, 0x00, 0x00, 0xff, 0xdb, 0x00, 0x43, 0x00, 0x0d, 0x09, 0x0a, 0x0b, 0x0a, 0x08, 0x0d,
    0x0b, 0x0a, 0x0b, 0x0e, 0x0e, 0x0d, 0x0f, 0x13, 0x20, 0x15, 0x13, 0x12, 0x12, 0x13, 0x27, 0x1c,
    0x1e, 0x17, 0x20, 0x2e, 0x29, 0x31, 0x30, 0x2e, 0x29, 0x2d, 0x2c, 0x33, 0x3a, 0x4a, 0x3e, 0x33,
    0x36, 0x46, 0x37, 0x2c, 0x2d, 0x40, 0x57, 0x41, 0x46, 0x4c, 0x4e, 0x52, 0x53, 0x52, 0x32, 0x3e,
    0x5a, 0x61, 0x5a, 0x50, 0x60, 0x4a, 0x51, 0x52, 0x4f, 0xff, 0xdb, 0x00, 0x43, 0x01, 0x0e, 0x0e,
    0x0e, 0x13, 0x11, 0x13, 0x26, 0x15, 0x15, 0x26, 0x4f, 0x35, 0x2d, 0x35, 0x4f, 0x4f, 0x4f, 0x4f,
    0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f,
    0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f,
    0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0x4f, 0xff, 0xc0,
    0x00, 0x11, 0x08, 0x00, 0x24, 0x00, 0x18, 0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11,
    0x01, 0xff, 0xc4, 0x00, 0x1f, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
    0x0a, 0x0b, 0xff, 0xc4, 0x00, 0xb5, 0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05,
    0x05, 0x04, 0x04, 0x00, 0x00, 0x01, 0x7d, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21,
    0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23,
    0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17,
    0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a,
    0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a,
    0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a,
    0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99,
    0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7,
    0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5,
    0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1,
    0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa, 0xff, 0xc4, 0x00, 0x1f, 0x01, 0x00, 0x03,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0xff, 0xc4, 0x00, 0xb5, 0x11, 0x00,
    0x02, 0x01, 0x02, 0x04, 0x04, 0x03, 0x04, 0x07, 0x05, 0x04, 0x04, 0x00, 0x01, 0x02, 0x77, 0x00,
    0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13,
    0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15,
    0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27,
    0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88,
    0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6,
    0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4,
    0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9,
    0xfa, 0xff, 0xda, 0x00, 0x0c, 0x03, 0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x00, 0x3f, 0x00, 0x83,
    0x48, 0xd2, 0xbf, 0xb4, 0xfc, 0xef, 0xdf, 0xf9, 0x5e, 0x5e, 0xdf, 0xe0, 0xdd, 0x9c, 0xe7, 0xdc,
    0x7a, 0x56, 0x8f, 0xfc, 0x22, 0xbf, 0xf4, 0xfb, 0xff, 0x00, 0x90, 0xbf, 0xfa, 0xf5, 0x1f, 0x85,
    0xae, 0x20, 0x83, 0xed, 0x5e, 0x7c, 0xd1, 0xc7, 0xbb, 0x66, 0x37, 0xb0, 0x19, 0xeb, 0xeb, 0x5b,
    0xff, 0x00, 0xda, 0x16, 0x3f, 0xf3, 0xf9, 0x6f, 0xff, 0x00, 0x7f, 0x57, 0xfc, 0x6b, 0xe7, 0x71,
    0x15, 0xab, 0x46, 0xa3, 0x51, 0xdb, 0xd0, 0xfa, 0x4c, 0x45, 0x7a, 0xf1, 0xa8, 0xd4, 0x36, 0xf4,
    0x39, 0x3d, 0x5f, 0x4a, 0xfe, 0xcc, 0xf2, 0x7f, 0x7f, 0xe6, 0xf9, 0x9b, 0xbf, 0x83, 0x6e, 0x31,
    0x8f, 0x73, 0xeb, 0x45, 0x5d, 0xf1, 0x4d, 0xc4, 0x13, 0xfd, 0x97, 0xc8, 0x9a, 0x39, 0x36, 0xef,
    0xce, 0xc6, 0x07, 0x1d, 0x3d, 0x28, 0xae, 0xcc, 0x3c, 0xa5, 0x2a, 0x69, 0xcb, 0x73, 0xb7, 0x0f,
    0x29, 0x4a, 0x9a, 0x73, 0xdc, 0xc0, 0xa2, 0x8a, 0x2b, 0x53, 0x70, 0xa2, 0x8a, 0x28, 0x00, 0xa2,
    0x8a, 0x28, 0x00, 0xa2, 0x8a, 0x28, 0x03, 0xff, 0xd9,
];

fn cover() -> Option<Jpeg> {
    Jpeg::new(COVER.to_vec())
}

fn number(text: &str) -> Option<IssueNumber> {
    Some(IssueNumber::new(text).unwrap())
}

fn credit(name: &str, role: Role) -> Credit {
    Credit {
        name: name.into(),
        role,
    }
}

fn seen(name: &str, first: bool) -> Appearance {
    Appearance {
        name: name.into(),
        first,
    }
}

fn robot_tales(issue: &str) -> Book {
    let mut book = Book::new(Category::Comic, "Astonishing Robot Tales");
    book.series.start_year = Some(1954);
    book.series.volume = Some(1);
    book.series.publisher = "Atomic Age Comics".into();
    book.series.imprint = "Tin Toy".into();
    book.issue.number = number(issue);
    book
}

/// Made-up books, together using every field the export writes.
fn books() -> Vec<Book> {
    let mut first = robot_tales("1");
    first.series.known_issue_count = Some(40);
    first.issue.title = "The Robot Who Read Comics".into();
    first.issue.cover_date = CoverDate::new(1954, Some(3));
    first.issue.release_date = Some(Date::new(1954, 1, 12).unwrap());
    first.issue.page_count = Some(36);
    first.issue.is_key = true;
    first.issue.key_reason = "First appearance of Tinman".into();
    first.issue.synopsis = "Tinman reads a comic, \"borrows\" a rocket,\nand learns to fly.".into();
    first.issue.credits = vec![
        credit("Ada Sprocket", Role::Writer),
        credit("Rex Rivet", Role::Artist),
        credit("Penny Pencil", Role::Penciller),
        credit("Ike Inkwell", Role::Inker),
        credit("Pipe | Bar", Role::Colorist),
        credit("Ann; Semicolon", Role::Letterer),
        credit("Cora Cover", Role::CoverArtist),
        credit(r"Back\Slash", Role::Editor),
    ];
    first.issue.characters = vec![seen("Tinman", true), seen("Professor Gears", false)];
    first.issue.teams = vec![seen("The Gearheads", true)];
    first.issue.story_arcs = vec![
        ArcPart {
            name: "Rise of the Robots".into(),
            part: Some(1),
        },
        ArcPart {
            name: "Backup Feature".into(),
            part: None,
        },
    ];
    first.edition.printing = Some(1);
    first.edition.distribution = Some(Distribution::Direct);
    first.edition.cover_price_cents = Some(10);
    first.edition.canadian_cover_price_cents = Some(15);
    first.edition.barcode = Barcode::parse("759606089369 00111");
    first.front = cover();
    first.back = cover();
    first.copy.grade = Grade::parse("9.8");
    first.copy.grade_type = Some(GradeType::Cgc);
    first.copy.cert_number = "4012345001".into();

    let mut variant = robot_tales("1");
    variant.edition.variant = "Cover B".into();
    variant.edition.printing = Some(1);
    variant.edition.distribution = Some(Distribution::Direct);
    variant.edition.cover_artists = vec!["Vera Variant".into()];
    variant.front = cover();
    variant.copy.grade = Grade::parse("9");
    variant.copy.grade_type = Some(GradeType::Raw);

    // Typed in another case, it is still the same series and issue.
    let mut newsstand = robot_tales("#1");
    newsstand.series.name = "astonishing  robot tales".into();
    newsstand.edition.printing = Some(1);
    newsstand.edition.distribution = Some(Distribution::Newsstand);

    // Cells a spreadsheet would read as formulas.
    let mut minus_one = robot_tales("-1");
    minus_one.issue.title = "=Flashback!".into();
    minus_one.issue.credits = vec![credit("@Handle", Role::Writer)];

    let mut annual = robot_tales("Annual 1");
    annual.edition.printing = Some(2);
    annual.edition.published = "1999-05".parse::<PartialDate>().ok();

    let mut relaunch = Book::new(Category::Comic, "Astonishing Robot Tales");
    relaunch.series.start_year = Some(1990);
    relaunch.series.volume = Some(2);
    relaunch.issue.number = number("1");
    relaunch.issue.legacy_number = Some(41);

    let mut magazine = Book::new(Category::Magazine, "Robot Monthly Illustrated");
    magazine.series.start_year = Some(1977);
    magazine.series.magazine_format = Some(MagazineFormat::Treasury);
    magazine.series.black_and_white = Some(true);
    magazine.issue.number = number("12");
    magazine.issue.credits = vec![
        credit("Ida Essay", Role::Article),
        credit("Quinn Question", Role::Interview),
    ];

    let mut omnibus = Book::new(
        Category::CollectedEdition,
        "Astonishing Robot Tales Omnibus",
    );
    omnibus.series.start_year = Some(2020);
    omnibus.issue.collects =
        parse_collects("Astonishing Robot Tales (1954) 1-5, Annual 1").unwrap();
    omnibus.issue.page_count = Some(320);
    omnibus.edition.isbn = Isbn::parse("978-0-7851-2532-7");
    omnibus.edition.binding = Some(Binding::Hardcover);
    omnibus.edition.cover_price_cents = Some(12_500);

    let mut french = Book::new(Category::Comic, "Les Robots Étonnants");
    french.series.start_year = Some(1962);
    french.issue.number = number("½");
    french.issue.characters = vec![seen("Homme de Fer", false)];

    vec![
        first, variant, newsstand, minus_one, annual, relaunch, magazine, omnibus, french,
    ]
}

/// The export of `books()`, and its files' contents by name.
fn written() -> (Vec<u8>, HashMap<String, Vec<u8>>) {
    let mut out = Cursor::new(Vec::new());
    write_export(&books(), &mut out).unwrap();
    let bytes = out.into_inner();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
    let mut files = HashMap::new();
    for n in 0..archive.len() {
        let mut file = archive.by_index(n).unwrap();
        let mut contents = Vec::new();
        file.read_to_end(&mut contents).unwrap();
        files.insert(file.name().unwrap().into_owned(), contents);
    }
    (bytes, files)
}

/// The CSV's rows as maps from column name to cell.
fn rows(files: &HashMap<String, Vec<u8>>) -> Vec<HashMap<String, String>> {
    let mut reader = csv::Reader::from_reader(files["collection.csv"].as_slice());
    let headers: Vec<String> = reader
        .headers()
        .unwrap()
        .iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(headers, COLUMNS);
    reader
        .records()
        .map(|r| {
            headers
                .iter()
                .cloned()
                .zip(r.unwrap().iter().map(str::to_owned))
                .collect()
        })
        .collect()
}

#[test]
fn every_book_is_a_row_with_its_photos() {
    let (_, files) = written();
    let rows = rows(&files);
    assert_eq!(rows.len(), books().len());
    assert!(files.contains_key("README.txt"));
    let photos = files.keys().filter(|k| k.starts_with("images/")).count();
    assert_eq!(photos, 3);
    for row in &rows {
        for column in ["Front Photo", "Back Photo"] {
            if !row[column].is_empty() {
                assert_eq!(files[&row[column]], COVER);
            }
        }
        assert!(row["Date Added"].is_empty() && row["Label"].is_empty());
    }
    assert!(files["collection.csv"].ends_with(b"\r\n"));
}

#[test]
fn books_that_share_a_series_issue_or_edition_share_its_id() {
    let (_, files) = written();
    let rows = rows(&files);
    let id = |n: usize, column: &str| rows[n][column].clone();
    // The first three are one issue of one series, in three editions.
    assert_eq!(id(0, "Issue ID"), id(1, "Issue ID"));
    assert_eq!(id(0, "Issue ID"), id(2, "Issue ID"));
    assert_ne!(id(0, "Edition ID"), id(1, "Edition ID"));
    assert_ne!(id(0, "Edition ID"), id(2, "Edition ID"));
    assert_eq!(id(0, "Series ID"), id(4, "Series ID"));
    // A relaunch with its own start year and volume is another series.
    assert_ne!(id(0, "Series ID"), id(5, "Series ID"));
    let copies: std::collections::HashSet<_> = (0..rows.len()).map(|n| id(n, "Copy ID")).collect();
    assert_eq!(copies.len(), rows.len());
}

#[test]
fn cells_are_written_as_mylongboxdb_writes_them() {
    let (_, files) = written();
    let rows = rows(&files);
    let first = &rows[0];
    assert_eq!(first["Category"], "comic");
    assert_eq!(first["Cover Date"], "1954-03");
    assert_eq!(first["Release Date"], "1954-01-12");
    assert_eq!(first["Key Issue"], "yes");
    assert_eq!(first["Cover Price"], "0.10");
    assert_eq!(first["Cover Price Currency"], "USD");
    assert_eq!(first["Canadian Cover Price"], "0.15");
    assert_eq!(first["Barcode"], "75960608936900111");
    assert_eq!(first["Distribution"], "direct");
    assert_eq!(first["Letterers"], r"Ann\; Semicolon");
    assert_eq!(first["Colorists"], r"Pipe \| Bar");
    assert_eq!(first["Editors"], r"Back\\Slash");
    assert_eq!(first["Characters"], "Tinman; Professor Gears");
    assert_eq!(first["First Appearances of Characters"], "Tinman");
    assert_eq!(
        first["Story Arcs"],
        "Rise of the Robots (part 1); Backup Feature"
    );
    assert_eq!(first["Grade"], "9.8");
    assert_eq!(first["Grade Type"], "cgc");
    assert_eq!(first["Certificate Number"], "4012345001");
    assert_eq!(rows[1]["Grade"], "9.0");
    assert_eq!(rows[1]["Grade Type"], "raw");
    assert_eq!(rows[1]["Certificate Number"], "");
    assert_eq!(rows[2]["Grade"], "");
    assert_eq!(rows[2]["Series"], "astonishing robot tales");
    assert_eq!(rows[2]["Issue Number"], "1");
    assert_eq!(rows[3]["Issue Number"], "'-1");
    assert_eq!(rows[3]["Story Title"], "'=Flashback!");
    assert_eq!(rows[3]["Writers"], "'@Handle");
    assert_eq!(rows[4]["Published"], "1999-05");
    assert_eq!(rows[5]["Legacy Number"], "41");
    assert_eq!(rows[6]["Magazine Format"], "treasury");
    assert_eq!(rows[6]["Black and White"], "yes");
    assert_eq!(rows[7]["Category"], "collected");
    assert_eq!(rows[7]["Issue Number"], "");
    assert_eq!(
        rows[7]["Collects"],
        "Astonishing Robot Tales (1954) 1–5, Annual 1"
    );
    assert_eq!(rows[7]["ISBN"], "9780785125327");
    assert_eq!(rows[7]["Binding"], "hardcover");
    assert_eq!(rows[8]["Issue Number"], "½");
}

#[test]
fn a_book_mylongboxdb_would_refuse_stops_the_export() {
    let mut books = books();
    books[6].issue.number = None;
    let mut out = Cursor::new(Vec::new());
    match write_export(&books, &mut out) {
        Err(assistant_core::ExportError::Refused(refused)) => {
            assert_eq!(refused.len(), 1);
            assert_eq!(refused[0].0, 6);
        }
        other => panic!("{other:?}"),
    }
    assert!(out.into_inner().is_empty());
}

/// For CI's check against MyLongboxDB's own import.
#[test]
fn writes_the_export_for_mylongboxdb_to_import() {
    let Some(folder) = std::env::var_os("ASSISTANT_CONTRACT_OUT") else {
        return;
    };
    let (bytes, _) = written();
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        std::path::Path::new(&folder).join("assistant-export.zip"),
        bytes,
    )
    .unwrap();
}
