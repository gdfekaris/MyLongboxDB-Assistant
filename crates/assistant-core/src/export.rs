//! The export: a MyLongboxDB full export, `collection.csv` with one row
//! per book under MyLongboxDB's exact column names, the owner's photos in
//! `images/`, and a `README.txt`. MyLongboxDB's own import reads it, as
//! untrusted input, with the import wizard as a second review.
//!
//! The column list, list escaping, formula guard, and README format are
//! MyLongboxDB's, copied from its export so what is written here reads
//! back the same.

use std::collections::HashMap;
use std::fmt;
use std::io::{Seek, Write};

use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::collects::format_collects;
use crate::model::{Book, Category, Role};
use crate::money::cents_string;

/// The columns, in order. The importer recognizes them by these names.
pub const COLUMNS: &[&str] = &[
    "Copy ID",
    "Edition ID",
    "Issue ID",
    "Series ID",
    "Category",
    "Series",
    "Start Year",
    "Volume",
    "Publisher",
    "Imprint",
    "Known Issue Count",
    "Magazine Format",
    "Black and White",
    "Series Notes",
    "Series Characters",
    "Series Teams",
    "Issue Number",
    "Legacy Number",
    "Story Title",
    "Cover Date",
    "Release Date",
    "Page Count",
    "Key Issue",
    "Key Reason",
    "Synopsis",
    "Writers",
    "Artists",
    "Pencillers",
    "Inkers",
    "Colorists",
    "Letterers",
    "Cover Artists",
    "Editors",
    "Article Writers",
    "Interviews",
    "Characters",
    "First Appearances of Characters",
    "Teams",
    "First Appearances of Teams",
    "Story Arcs",
    "Collects",
    "Variant",
    "Printing",
    "Distribution",
    "Published",
    "Cover Price",
    "Cover Price Currency",
    "Canadian Cover Price",
    "Barcode",
    "ISBN",
    "Binding",
    "Dust Jacket",
    "Slipcase",
    "Variant Cover Artists",
    "Grade",
    "Grade Type",
    "Certificate Number",
    "Label",
    "Pedigree",
    "Page Quality",
    "Book Condition",
    "Dust Jacket Condition",
    "Purchase Price",
    "Purchase Currency",
    "Purchase Date",
    "Seller",
    "Storage Box",
    "Box Position",
    "Tags",
    "Signatures",
    "Notes",
    "Current Value",
    "Current Value Currency",
    "Value History",
    "Date Added",
    "Front Photo",
    "Back Photo",
    "Signature Photo",
];

/// The credit roles in the order of their columns, from Writers to
/// Interviews.
const ROLE_COLUMNS: [Role; 10] = [
    Role::Writer,
    Role::Artist,
    Role::Penciller,
    Role::Inker,
    Role::Colorist,
    Role::Letterer,
    Role::CoverArtist,
    Role::Editor,
    Role::Article,
    Role::Interview,
];

/// One item of a list, with the separators in it kept literal.
fn escape(item: &str) -> String {
    let mut out = String::with_capacity(item.len());
    for c in item.chars() {
        if matches!(c, '\\' | ';' | '|') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Items joined as a list cell.
fn join<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> String {
    items
        .into_iter()
        .map(|i| escape(i.as_ref()))
        .collect::<Vec<_>>()
        .join("; ")
}

/// What a cell may start with that a spreadsheet would take for a
/// formula, and the mark that guards against it.
pub const FORMULA_STARTS: [char; 7] = ['=', '+', '-', '@', '\t', '\r', '\''];

/// A cell a spreadsheet would read as a formula written after a `'`,
/// which spreadsheets show as text and import takes off again. A name or
/// note from a collection file made elsewhere could otherwise run a
/// formula in the spreadsheet the export is opened in. A cell already
/// starting with `'` gets one too, so it comes back as it was.
fn guarded(cell: String) -> String {
    if cell.starts_with(FORMULA_STARTS) {
        format!("'{cell}")
    } else {
        cell
    }
}

/// What the export's `README.txt` says, so the archive explains itself to
/// anyone opening it without MyLongboxDB. The format part is MyLongboxDB's
/// own.
pub const README: &str = "\
MyLongboxDB full export
=======================

This archive holds books looked up by MyLongboxDB Assistant, written as
a MyLongboxDB full export, in plain formats any spreadsheet or image
viewer can open. Copy details such as grade, price paid, and storage box
are left for you to add in MyLongboxDB.

collection.csv
  One row per physical copy, UTF-8, comma-separated (RFC 4180), with a
  header row naming each column. Every field the app keeps for a copy,
  its edition, its issue, and its series is there.

  IDs: the Copy, Edition, Issue, and Series ID columns are permanent
  identifiers. Rows with the same Issue ID are copies of one issue.

  Lists: a cell holding several names separates them with \"; \", for
  example \"Wolverine; Hulk\". Signatures and value history entries have
  parts separated by \" | \":
    Signatures:     name | verification | certificate | date | place |
                    sketch | placement
    Value History:  date | amount | currency | source
  A backslash before ; or | or \\ means that character is part of a
  name rather than a separator.

  A cell that starts with = + - @ or ' has a ' added in front, so a
  spreadsheet shows it as text rather than running it as a formula;
  \"-1\" is written \"'-1\". MyLongboxDB takes the added ' off again on
  import.

  Story arcs give their part as \"Name (part 3)\". Characters and teams
  that make their first appearance are also listed in the First
  Appearances columns.

  Series Characters and Series Teams are those in every issue of the
  series, such as Spider-Man in Amazing Spider-Man; the Characters and
  Teams columns list only those added to the issue itself.

  Amounts are a number and a currency code in separate columns:
    USD  US dollars
    XAU  troy ounces of gold
    BTC  bitcoin
  Each amount is exactly as it was entered.

  Dates are YYYY-MM-DD. A cover date is YYYY-MM, or YYYY when only the
  year is known, and Published, when an edition came out, is YYYY,
  YYYY-MM, or YYYY-MM-DD, as far as it is known. Date Added is a UTC
  time.

  Fixed choices are written as codes:
    Category: comic, magazine, collected
    Grade Type: raw, cgc, cbcs, pgx
    Page Quality, best to worst: white, off_white_to_white, off_white,
      cream_to_off_white, cream, light_tan_to_off_white,
      light_tan_to_cream, light_tan, tan_to_off_white, tan_to_cream,
      tan, dark_tan_to_off_white, dark_tan, brown_to_off_white,
      brown_to_tan, brown, brown_brittle
    Book and dust jacket condition: sealed, new, like_new, very_good,
      good, fair, poor
    Binding: hardcover, softcover, oversized_hardcover
    Distribution: direct, newsstand
    Magazine Format: standard, treasury, digest
    Signature verification: unverified, witnessed, coa, jsa, cgc_ss,
      cgc_jsa, cbcs_witnessed, cbcs, pgx
    Signature placement: front_cover, inside_front_cover, first_page,
      interior_page, inside_back_cover, back_cover
  Yes or no columns are \"yes\", \"no\", or empty when not recorded.

  Collects, for collected editions, lists the issues the book reprints.
  Commas separate the issues of one series and semicolons separate
  series, for example:
    Amazing Spider-Man (1963) 1-10, Annual 1; Fantastic Four (1961) 48-50

images/
  The photos, as JPEG files named after the copy:
    <Copy ID>_front.jpg
    <Copy ID>_back.jpg
    <Copy ID>_signature.jpg   a signature inside the book
  The Front Photo, Back Photo, and Signature Photo columns name each
  copy's files.

Import this archive with MyLongboxDB's Import from Spreadsheet, which
shows every row before anything is added.
";

/// Why an export could not be written.
#[derive(Debug)]
pub enum ExportError {
    /// Books MyLongboxDB would refuse, by their place in the list, with
    /// what is wrong with each.
    Refused(Vec<(usize, Vec<String>)>),
    Io(std::io::Error),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(books) => write!(
                f,
                "{} of the books would not import into MyLongboxDB",
                books.len()
            ),
            Self::Io(e) => write!(f, "the export could not be written: {e}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

fn zip_error(e: zip::result::ZipError) -> ExportError {
    match e {
        zip::result::ZipError::Io(e) => ExportError::Io(e),
        other => ExportError::Io(std::io::Error::other(other)),
    }
}

fn csv_error(e: csv::Error) -> ExportError {
    ExportError::Io(std::io::Error::other(e))
}

/// A name as MyLongboxDB compares series names: spacing collapsed and
/// ASCII letters in either case alike, as SQLite's NOCASE does.
fn folded(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// What makes two books the same series, issue, or edition in
/// MyLongboxDB, so books that share one share its id in the export.
type SeriesKey = (String, Option<u16>, Option<u32>);
type IssueKey = (SeriesKey, String);
type EditionKey = (IssueKey, String, Option<u32>, Option<&'static str>);

/// The ids of each book's row: copy, edition, issue, and series.
///
/// A book with no volume shares the series of the others of its name
/// and start year when they give one volume between them. MyLongboxDB
/// keeps apart series a file gives different ids and volumes, as its own
/// export of two lookalike series does, so with an id of its own the
/// book would start a series without a volume rather than join the one
/// the owner has. Its Volume cell stays blank: the volume is not guessed.
fn ids(books: &[Book]) -> Vec<[Uuid; 4]> {
    let mut volumes: HashMap<(String, Option<u16>), Vec<u32>> = HashMap::new();
    for book in books {
        if let Some(volume) = book.series.volume {
            let stated = volumes
                .entry((folded(&book.series.name), book.series.start_year))
                .or_default();
            if !stated.contains(&volume) {
                stated.push(volume);
            }
        }
    }
    let mut series: HashMap<SeriesKey, Uuid> = HashMap::new();
    let mut issues: HashMap<IssueKey, Uuid> = HashMap::new();
    let mut editions: HashMap<EditionKey, Uuid> = HashMap::new();
    books
        .iter()
        .map(|book| {
            let (name, start_year) = (folded(&book.series.name), book.series.start_year);
            let volume = book.series.volume.or_else(|| {
                match volumes.get(&(name.clone(), start_year)).map(Vec::as_slice) {
                    Some(&[only]) => Some(only),
                    _ => None,
                }
            });
            let series_key = (name, start_year, volume);
            let series_id = *series
                .entry(series_key.clone())
                .or_insert_with(Uuid::new_v4);
            // A book with no number is an issue of its own, as MyLongboxDB
            // never matches one to another.
            let Some(number) = &book.issue.number else {
                return [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), series_id];
            };
            let issue_key = (series_key, number.sort_key().to_owned());
            let issue_id = *issues.entry(issue_key.clone()).or_insert_with(Uuid::new_v4);
            let edition_key = (
                issue_key,
                book.edition.variant.trim().to_ascii_lowercase(),
                book.edition.printing,
                book.edition.distribution.map(|d| d.code()),
            );
            let edition_id = *editions.entry(edition_key).or_insert_with(Uuid::new_v4);
            [Uuid::new_v4(), edition_id, issue_id, series_id]
        })
        .collect()
}

fn text<T: ToString>(value: Option<T>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

fn yes_no(flag: Option<bool>) -> String {
    match flag {
        Some(true) => "yes".into(),
        Some(false) => "no".into(),
        None => String::new(),
    }
}

/// A book's row, in the order of [`COLUMNS`]. Copy details other than
/// the grade, and the time it was added, are left blank for MyLongboxDB
/// to fill.
fn row(book: &Book, [copy, edition, issue, series]: [Uuid; 4]) -> Vec<String> {
    let (s, i, e) = (&book.series, &book.issue, &book.edition);
    let names = |list: &[crate::model::Appearance], first_only: bool| {
        join(
            list.iter()
                .filter(|a| !first_only || a.first)
                .map(|a| a.name.trim()),
        )
    };
    let photo = |has: bool, side: &str| {
        if has {
            format!("images/{copy}_{side}.jpg")
        } else {
            String::new()
        }
    };
    let mut row = vec![
        copy.to_string(),
        edition.to_string(),
        issue.to_string(),
        series.to_string(),
        book.category.code().to_owned(),
        s.name.split_whitespace().collect::<Vec<_>>().join(" "),
        text(s.start_year),
        text(s.volume),
        s.publisher.trim().to_owned(),
        s.imprint.trim().to_owned(),
        text(s.known_issue_count),
        text(s.magazine_format.map(|f| f.code())),
        yes_no(s.black_and_white),
        String::new(),
        String::new(),
        String::new(),
        // MyLongboxDB's import takes a leading `#` off.
        text(
            i.number
                .as_ref()
                .map(|n| n.as_str().trim_start_matches('#').trim()),
        ),
        text(i.legacy_number),
        i.title.trim().to_owned(),
        text(i.cover_date),
        text(i.release_date),
        text(i.page_count),
        yes_no(Some(i.is_key)),
        i.key_reason.trim().to_owned(),
        i.synopsis.trim().to_owned(),
    ];
    row.extend(ROLE_COLUMNS.map(|role| {
        join(
            i.credits
                .iter()
                .filter(|c| c.role == role)
                .map(|c| c.name.trim()),
        )
    }));
    row.extend([
        names(&i.characters, false),
        names(&i.characters, true),
        names(&i.teams, false),
        names(&i.teams, true),
        join(i.story_arcs.iter().map(|a| match a.part {
            Some(n) => format!("{} (part {n})", a.name.trim()),
            None => a.name.trim().to_owned(),
        })),
        if book.category == Category::CollectedEdition && !i.collects.is_empty() {
            format_collects(&i.collects)
        } else {
            String::new()
        },
        e.variant.trim().to_owned(),
        text(e.printing),
        text(e.distribution.map(|d| d.code())),
        text(e.published),
        text(e.cover_price_cents.map(cents_string)),
        if e.cover_price_cents.is_some() {
            "USD".to_owned()
        } else {
            String::new()
        },
        text(e.canadian_cover_price_cents.map(cents_string)),
        text(e.barcode.as_ref()),
        text(e.isbn.as_ref()),
        text(e.binding.map(|b| b.code())),
        String::new(),
        String::new(),
        join(e.cover_artists.iter().map(|a| a.trim())),
    ]);
    let c = &book.copy;
    row.extend([
        text(c.grade),
        text(c.grade_type.map(|g| g.code())),
        c.cert_number.trim().to_owned(),
    ]);
    // Label through Value History, and Date Added: the owner's, or
    // MyLongboxDB's to fill.
    row.extend(std::iter::repeat_n(String::new(), 18));
    row.extend([
        photo(book.front.is_some(), "front"),
        photo(book.back.is_some(), "back"),
        String::new(),
    ]);
    debug_assert_eq!(row.len(), COLUMNS.len());
    row
}

/// Writes `books` as a MyLongboxDB full export to `out`. Nothing is
/// written if any book would be refused by MyLongboxDB's import.
pub fn write_export(books: &[Book], out: impl Write + Seek) -> Result<(), ExportError> {
    let refused: Vec<(usize, Vec<String>)> = books
        .iter()
        .enumerate()
        .map(|(n, book)| (n, book.problems()))
        .filter(|(_, problems)| !problems.is_empty())
        .collect();
    if !refused.is_empty() {
        return Err(ExportError::Refused(refused));
    }
    let ids = ids(books);
    let mut zip = ZipWriter::new(out);
    let text_file = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    // Photos are JPEGs already; squeezing them again gains nothing.
    let photo_file = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);
    zip.start_file("README.txt", text_file).map_err(zip_error)?;
    zip.write_all(README.as_bytes())?;
    zip.start_file("collection.csv", text_file)
        .map_err(zip_error)?;
    {
        let mut csv = csv::WriterBuilder::new()
            .terminator(csv::Terminator::CRLF)
            .from_writer(&mut zip);
        csv.write_record(COLUMNS).map_err(csv_error)?;
        for (book, ids) in books.iter().zip(&ids) {
            csv.write_record(row(book, *ids).into_iter().map(guarded))
                .map_err(csv_error)?;
        }
        csv.flush()?;
    }
    for (book, [copy, ..]) in books.iter().zip(&ids) {
        for (photo, side) in [(&book.front, "front"), (&book.back, "back")] {
            if let Some(jpeg) = photo {
                zip.start_file(format!("images/{copy}_{side}.jpg"), photo_file)
                    .map_err(zip_error)?;
                zip.write_all(jpeg.bytes())?;
            }
        }
    }
    zip.finish().map_err(zip_error)?;
    Ok(())
}
