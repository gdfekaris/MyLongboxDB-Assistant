//! The index of the owner's GCD copy, as a lookup reads it.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Mutex;

use assistant_core::Date;
use assistant_core::lookup::clean;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::dump::harden;
use crate::index::{INDEX_FILE, INDEX_VERSION};
use crate::keys::{barcode_key, number_key, series_key};

/// The most records a lookup is given for one book.
const MAX_RECORDS: usize = 3;
/// The most characters named in one story of a record.
const MAX_CHARACTERS: usize = 30;
/// The longest record given. Room for every edition of an issue with
/// sixty variants, as modern issues can have; a record longer still, of
/// a long anthology say, loses its last stories.
pub const MAX_RECORD: usize = 16_000;
/// The most editions of one issue listed.
const MAX_EDITIONS: usize = 120;
/// The longest record given after the first: the others are only told
/// apart from it, by their series and editions.
const MAX_OTHER_RECORD: usize = 600;
/// The most issues a looser match of a name looks through.
const MAX_CLOSE_SERIES: i64 = 2_000;

/// The GCD's names of its credit types, by id.
fn credit_type(id: Option<i64>) -> &'static str {
    match id {
        Some(1) => "script",
        Some(2) => "pencils",
        Some(3) => "inks",
        Some(4) => "colors",
        Some(5) => "letters",
        Some(6) => "editing",
        Some(7) => "pencils and inks",
        Some(8) => "pencils, inks, and colors",
        Some(9) => "painting",
        Some(10) => "script, pencils, and inks",
        Some(11) => "script, pencils, inks, and colors",
        Some(12) => "script, pencils, inks, and letters",
        Some(13) => "script, pencils, inks, colors, and letters",
        Some(14) => "pencils, inks, and letters",
        _ => "other",
    }
}

/// The GCD's names of a character's part in a story, by id.
fn character_role(id: Option<i64>) -> Option<&'static str> {
    Some(match id? {
        1 => "antagonist",
        2 => "cameo",
        3 => "featured",
        4 => "guest",
        5 => "supporting",
        6 => "villain",
        _ => return None,
    })
}

/// The GCD's names of the kinds of story the index keeps, by id.
fn story_kind(id: Option<i64>) -> &'static str {
    match id {
        Some(6) => "cover",
        Some(13) => "photo story",
        Some(21) => "text story",
        _ => "comic story",
    }
}

/// A page count as the GCD gives it, without a needless .0.
fn pages(count: f64) -> String {
    if count.fract() == 0.0 {
        format!("{count:.0}")
    } else {
        format!("{count}")
    }
}

/// Cuts `text` to at most `max` characters, marking where it was cut.
fn cut(text: &mut String, max: usize) {
    if text.chars().count() > max {
        *text = text.chars().take(max.saturating_sub(1)).collect();
        text.push('\u{2026}');
    }
}

/// What an edition says, for the issue itself always, and for a variant
/// unless it only repeats the issue.
fn own<'a, T: PartialEq>(first: bool, this: &'a Option<T>, base: &Option<T>) -> Option<&'a T> {
    this.as_ref().filter(|_| first || this != base)
}

/// One edition of an issue as the index holds it: the issue itself, or
/// one of its variants.
struct Edition {
    id: i64,
    number: Option<String>,
    variant: Option<String>,
    title: Option<String>,
    printed: Option<String>,
    key: Option<String>,
    sale: Option<String>,
    price: Option<String>,
    pages: Option<f64>,
    barcode: Option<String>,
    isbn: Option<String>,
    indicia: Option<String>,
    notes: Option<String>,
}

/// One issue in the owner's GCD copy, written out for a lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub series: String,
    pub start_year: Option<u16>,
    pub number: String,
    /// The record as the lookup's steps are shown it.
    pub text: String,
}

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
    // Lookups run on several threads at once; each query takes a moment.
    conn: Mutex<Connection>,
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
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn meta(&self, key: &str) -> Option<String> {
        self.conn()
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

impl Catalog {
    /// The records of the issue a book is, the likeliest first.
    ///
    /// With a `barcode` the copy knows, the issue whose edition carries
    /// it; an older barcode names a series rather than an issue, so where
    /// a number is given too, only an issue with that number. Otherwise,
    /// those numbered `number` in the series named `series`, in
    /// `start_year` if given, those from the US and in English first. An
    /// annual's number, as in Annual 3, is looked for in the series'
    /// annual.
    ///
    /// Where no series has that very name, one whose name starts with it
    /// is taken, as Marvel Treasury Special finds Marvel Treasury
    /// Special, Giant Superhero Holiday Grab-Bag; and failing that, one
    /// whose name holds every word of it, or whose every word it holds,
    /// as a cover's logo may add a character's name.
    pub fn find(
        &self,
        series: &str,
        start_year: Option<u16>,
        number: &str,
        barcode: Option<&str>,
    ) -> Result<Vec<Record>, CatalogError> {
        let unreadable = |e: rusqlite::Error| CatalogError::Unreadable(clean(&e.to_string(), 200));
        let conn = self.conn();
        let mut name = series_key(series);
        let mut number = number_key(number);
        if let Some(rest) = number.strip_prefix("annual ")
            && !name.ends_with(" annual")
        {
            name.push_str(" annual");
            number = rest.to_owned();
        }
        // (issue, the edition whose barcode matched)
        let mut found: Vec<(i64, Option<i64>)> = Vec::new();
        if let Some(code) = barcode.map(barcode_key).filter(|c| c.len() >= 8) {
            let mut stmt = conn
                .prepare(
                    "SELECT e.id, coalesce(e.variant_of, e.id), b.number_key
                     FROM issue e JOIN issue b ON b.id = coalesce(e.variant_of, e.id)
                     WHERE e.barcode_key = ?1 ORDER BY e.id LIMIT 50",
                )
                .map_err(unreadable)?;
            let rows = stmt
                .query_map([&code], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(unreadable)?;
            for row in rows {
                let (edition, base, key) = row.map_err(unreadable)?;
                let numbered = number.is_empty() || number == "nn" || key == number;
                if numbered && !found.iter().any(|(b, _)| *b == base) {
                    found.push((base, Some(edition)));
                }
            }
        }
        if found.is_empty() {
            let order = "ORDER BY s.country = 'us' DESC, s.language = 'en' DESC,
                         s.issue_count DESC, i.id";
            let ids =
                |sql: &str, args: &[&dyn rusqlite::ToSql]| -> Result<Vec<i64>, CatalogError> {
                    let mut stmt = conn.prepare(sql).map_err(unreadable)?;
                    let rows = stmt
                        .query_map(args, |r| r.get::<_, i64>(0))
                        .map_err(unreadable)?;
                    rows.collect::<Result<_, _>>().map_err(unreadable)
                };
            let limit = MAX_RECORDS as i64;
            let mut issues = ids(
                &format!(
                    "SELECT i.id FROM series s JOIN issue i ON i.series_id = s.id
                     WHERE s.name_key = ?1 AND (?2 IS NULL OR s.year_began = ?2)
                         AND i.number_key = ?3 AND i.variant_of IS NULL {order} LIMIT ?4"
                ),
                &[&name, &start_year, &number, &limit],
            )?;
            if issues.is_empty() && !name.is_empty() {
                // The keys that start with the name and then a space, as
                // ! sorts just after the space.
                let (from, to) = (format!("{name} "), format!("{name}!"));
                issues = ids(
                    &format!(
                        "SELECT i.id FROM series s JOIN issue i ON i.series_id = s.id
                         WHERE s.name_key >= ?1 AND s.name_key < ?2
                             AND (?3 IS NULL OR s.year_began = ?3)
                             AND i.number_key = ?4 AND i.variant_of IS NULL {order} LIMIT ?5"
                    ),
                    &[&from, &to, &start_year, &number, &limit],
                )?;
            }
            let words: Vec<&str> = name.split_whitespace().collect();
            if issues.is_empty() && words.len() >= 2 {
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT s.name_key, i.id FROM series s JOIN issue i ON i.series_id = s.id
                         WHERE (?1 IS NULL OR s.year_began = ?1) AND i.number_key = ?2
                             AND i.variant_of IS NULL
                         {order} LIMIT ?3"
                    ))
                    .map_err(unreadable)?;
                let rows = stmt
                    .query_map(params![start_year, number, MAX_CLOSE_SERIES], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                    })
                    .map_err(unreadable)?;
                // A name holding every word asked for is likelier than one
                // the words hold; then the more words shared, the fewer
                // left over, and the order above.
                let mut close: Vec<((bool, usize, usize, usize), i64)> = Vec::new();
                for (n, row) in rows.enumerate() {
                    let (key, issue) = row.map_err(unreadable)?;
                    let theirs: Vec<&str> = key.split_whitespace().collect();
                    let shared = words.iter().filter(|w| theirs.contains(w)).count();
                    let holds_all = shared == words.len();
                    let held = theirs.iter().all(|w| words.contains(w));
                    if (holds_all || held) && shared >= 2 {
                        let extra = theirs.len() + words.len() - 2 * shared;
                        close.push(((!holds_all, usize::MAX - shared, extra, n), issue));
                    }
                }
                close.sort_unstable();
                issues = close
                    .into_iter()
                    .take(MAX_RECORDS)
                    .map(|(_, issue)| issue)
                    .collect();
            }
            found.extend(issues.into_iter().map(|issue| (issue, None)));
        }
        found.truncate(MAX_RECORDS);
        let mut records = Vec::new();
        for (issue, matched) in found {
            let max = if records.is_empty() {
                MAX_RECORD
            } else {
                MAX_OTHER_RECORD
            };
            if let Some(record) = record(&conn, issue, matched, max).map_err(unreadable)? {
                records.push(record);
            }
        }
        Ok(records)
    }
}

/// The record of the issue `id`, with its editions and stories, noting
/// the edition whose barcode the owner's cover has.
fn record(
    conn: &Connection,
    id: i64,
    matched: Option<i64>,
    max: usize,
) -> rusqlite::Result<Option<Record>> {
    type SeriesRow = (
        String,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    // The catalogue's count of a series' issues takes in its specials,
    // such as a #0, so it is not the run's length and is not given.
    let series: Option<SeriesRow> = conn
        .query_row(
            "SELECT s.name, s.year_began, s.year_ended, s.publisher, s.country, s.language,
                    s.format
             FROM issue i JOIN series s ON s.id = i.series_id WHERE i.id = ?1",
            [id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .optional()?;
    let Some((name, began, ended, publisher, country, language, format)) = series else {
        return Ok(None);
    };
    let mut text = String::new();
    let _ = write!(text, "Series: {name}");
    if let Some(began) = began {
        let ended = ended.map_or_else(|| "ongoing".to_owned(), |e| e.to_string());
        let _ = write!(text, " ({began}\u{2013}{ended})");
    }
    for part in [publisher, format].into_iter().flatten() {
        let _ = write!(text, ", {part}");
    }
    let place: Vec<String> = [country, language].into_iter().flatten().collect();
    if !place.is_empty() {
        let _ = write!(text, ", {}", place.join("/"));
    }
    text.push('\n');

    // A story's credits, or with `of_issue` an issue's own; with
    // `art_only`, its colorists and letterers left out.
    let credits = |owner: i64, of_issue: bool, art_only: bool| -> rusqlite::Result<Vec<String>> {
        let mut stmt = conn.prepare_cached(if of_issue {
            "SELECT role, name, credited_as, detail, uncertain FROM credit
             WHERE issue_id = ?1 ORDER BY role, rowid LIMIT 60"
        } else {
            "SELECT role, name, credited_as, detail, uncertain FROM credit
             WHERE story_id = ?1 ORDER BY role, rowid LIMIT 60"
        })?;
        let rows = stmt.query_map([owner], |r| {
            Ok((
                r.get::<_, Option<i64>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, bool>(4)?,
            ))
        })?;
        let mut parts = Vec::new();
        for row in rows {
            let (role, name, credited, detail, uncertain) = row?;
            if art_only && matches!(role, Some(4 | 5)) {
                continue;
            }
            let mut part = format!("{}: {name}", credit_type(role));
            let mut notes = Vec::new();
            if let Some(credited) = credited.filter(|c| *c != name) {
                notes.push(format!("credited as {credited}"));
            }
            if let Some(detail) = detail {
                notes.push(detail);
            }
            if uncertain {
                notes.push("uncertain".to_owned());
            }
            if !notes.is_empty() {
                let _ = write!(part, " ({})", notes.join(", "));
            }
            parts.push(part);
        }
        Ok(parts)
    };
    // Who drew an edition's cover, as the GCD records a variant's own
    // cover as a story of that edition. Its colorist and letterer are
    // not its artists, and named beside them were taken for one.
    let cover = |edition: i64| -> rusqlite::Result<Option<String>> {
        let story: Option<(i64, Option<String>)> = conn
            .prepare_cached(
                "SELECT id, legacy FROM story WHERE issue_id = ?1 AND kind = 6
                 ORDER BY seq, id LIMIT 1",
            )?
            .query_row([edition], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((story, legacy)) = story else {
            return Ok(None);
        };
        let parts = credits(story, false, true)?;
        Ok(if parts.is_empty() {
            legacy
        } else {
            Some(parts.join(", "))
        })
    };

    // The issue and each of its editions, the issue itself first. Every
    // edition is listed, however many variants the issue has, as which
    // one the owner's copy is matters more than a long record's last
    // stories; a variant repeats only what it does not share with the
    // issue. The GCD files some editions as issues of their own, numbered
    // as 82 [Collector's Edition] beside #82, and those are listed too.
    let (series_id, key): (i64, String) = conn.query_row(
        "SELECT series_id, number_key FROM issue WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    // The keys that are this one, a space, and a bracket, as a backslash
    // sorts just after the bracket.
    let (from, to) = (format!("{key} ["), format!("{key} \\"));
    let mut stmt = conn.prepare(
        "SELECT id, number, variant_name, title, publication_date, key_date, on_sale_date,
                price, page_count, barcode, isbn, indicia_publisher, notes
         FROM issue
         WHERE id IN (SELECT ?1 UNION ALL SELECT id FROM issue WHERE variant_of = ?1
             UNION ALL SELECT id FROM issue WHERE series_id = ?2 AND number_key >= ?3
                 AND number_key < ?4 AND variant_of IS NULL)
         ORDER BY id != ?1, variant_of IS NULL, id LIMIT ?5",
    )?;
    let editions = stmt
        .query_map(params![id, series_id, from, to, MAX_EDITIONS as i64], |r| {
            Ok(Edition {
                id: r.get(0)?,
                number: r.get(1)?,
                variant: r.get(2)?,
                title: r.get(3)?,
                printed: r.get(4)?,
                key: r.get(5)?,
                sale: r.get(6)?,
                price: r.get(7)?,
                pages: r.get(8)?,
                barcode: r.get(9)?,
                isbn: r.get(10)?,
                indicia: r.get(11)?,
                notes: r.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut number = String::new();
    let mut notes = String::new();
    for (n, e) in editions.iter().enumerate() {
        let base = &editions[0];
        let first = n == 0;
        let mut line = Vec::new();
        if first {
            number = e.number.clone().unwrap_or_default();
            let mut head = format!("Issue: #{number}");
            if let Some(title) = &e.title {
                let _ = write!(head, ", titled \u{201c}{title}\u{201d}");
            }
            text.push_str(&head);
            text.push('\n');
            if let Some(note) = &e.notes {
                notes.clone_from(note);
            }
        }
        let label = match (first, &e.variant) {
            (true, Some(v)) => format!("Edition {v} (the issue itself)"),
            (true, None) => "Edition: the issue itself".to_owned(),
            (false, _) if e.number != base.number => {
                let number = e.number.as_deref().unwrap_or_default();
                let name = e.variant.as_deref().or_else(|| {
                    number
                        .split_once(" [")
                        .and_then(|(_, rest)| rest.strip_suffix(']'))
                });
                match name {
                    Some(v) => {
                        format!("Edition {v} (catalogued as an issue of its own, #{number})")
                    }
                    None => {
                        format!("Another edition (catalogued as an issue of its own, #{number})")
                    }
                }
            }
            (false, Some(v)) => format!("Edition {v}"),
            (false, None) => "Another edition".to_owned(),
        };
        if let Some(printed) = own(first, &e.printed, &base.printed) {
            line.push(format!("cover date as printed {printed}"));
        }
        if let Some(key) = own(first, &e.key, &base.key) {
            line.push(format!("catalogue date {}", key.trim_end_matches("-00")));
        }
        if let Some(sale) = own(first, &e.sale, &base.sale) {
            line.push(format!("on sale {}", sale.trim_end_matches("-00")));
        }
        if let Some(price) = own(first, &e.price, &base.price) {
            line.push(format!("price {price}"));
        }
        if let Some(count) = own(first, &e.pages, &base.pages) {
            line.push(format!("{} pages", pages(*count)));
        }
        if let Some(code) = &e.barcode {
            line.push(format!("barcode {code}"));
        }
        if let Some(isbn) = &e.isbn {
            line.push(format!("ISBN {isbn}"));
        }
        if let Some(indicia) = own(first, &e.indicia, &base.indicia) {
            line.push(format!("indicia publisher {indicia}"));
        }
        if let Some(artists) = cover(e.id)? {
            line.push(format!("cover: {artists}"));
        }
        if matched == Some(e.id) {
            line.push("its barcode is the one on the owner's cover".to_owned());
        }
        if line.is_empty() {
            let _ = writeln!(text, "{label}");
        } else {
            let _ = writeln!(text, "{label}: {}", line.join("; "));
        }
    }

    let issue_credits = credits(id, true, false)?;
    if !issue_credits.is_empty() {
        let _ = writeln!(text, "Issue credits: {}", issue_credits.join("; "));
    }
    if !notes.is_empty() {
        let _ = writeln!(text, "Notes: {notes}");
    }

    // The issue's stories, its cover aside, which its edition gives.
    let mut stories_text = String::new();
    let mut stmt = conn.prepare(
        "SELECT id, kind, title, feature, page_count, characters, synopsis, legacy
         FROM story WHERE issue_id = ?1 AND (kind IS NULL OR kind != 6)
         ORDER BY seq, id LIMIT 40",
    )?;
    let stories = stmt
        .query_map([id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<f64>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (n, (sid, kind, title, feature, count, characters, synopsis, legacy)) in
        stories.into_iter().enumerate()
    {
        let mut head = format!("Story {} ({})", n + 1, story_kind(kind));
        if let Some(title) = title {
            let _ = write!(head, " \u{201c}{title}\u{201d}");
        }
        if let Some(feature) = feature {
            let _ = write!(head, ", feature {feature}");
        }
        if let Some(count) = count {
            let _ = write!(head, ", {} pages", pages(count));
        }
        let story_credits = credits(sid, false, false)?;
        let _ = write!(stories_text, "{head}:");
        if !story_credits.is_empty() {
            let _ = write!(stories_text, " {}", story_credits.join("; "));
        } else if let Some(legacy) = legacy {
            let _ = write!(stories_text, " {legacy}");
        }
        stories_text.push('\n');
        let mut cast = conn.prepare_cached(
            "SELECT name, role, origin, death, notes FROM appearance WHERE story_id = ?1
             ORDER BY notes IS NULL, role IS NULL, role, rowid LIMIT ?2",
        )?;
        let names: Vec<String> = cast
            .query_map(params![sid, MAX_CHARACTERS as i64], |r| {
                let mut what = Vec::new();
                if let Some(role) = character_role(r.get(1)?) {
                    what.push(role.to_owned());
                }
                if r.get::<_, bool>(2)? {
                    what.push("origin".to_owned());
                }
                if r.get::<_, bool>(3)? {
                    what.push("death".to_owned());
                }
                if let Some(note) = r.get::<_, Option<String>>(4)? {
                    what.push(note);
                }
                let name: String = r.get(0)?;
                Ok(if what.is_empty() {
                    name
                } else {
                    format!("{name} ({})", what.join(", "))
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        if !names.is_empty() {
            let _ = writeln!(stories_text, "  Characters: {}", names.join("; "));
        } else if let Some(characters) = characters {
            let _ = writeln!(stories_text, "  Characters: {characters}");
        }
        if let Some(synopsis) = synopsis {
            let _ = writeln!(stories_text, "  Synopsis: {synopsis}");
        }
    }
    // A long record loses its last stories, not its editions.
    cut(&mut text, max);
    let room = max - text.chars().count();
    if room > 1 {
        cut(&mut stories_text, room);
        text.push_str(&stories_text);
    }
    Ok(Some(Record {
        series: name,
        start_year: began.and_then(|y| u16::try_from(y).ok()),
        number,
        text,
    }))
}

impl assistant_core::lookup::Catalog for Catalog {
    /// The records `find` gives, each titled with the copy's date and the
    /// issue it is, for the lookup to cite. A copy that cannot be read
    /// has no records; the lookup goes to the web as it would without.
    fn records(
        &self,
        query: &assistant_core::BookQuery,
        barcode: Option<&assistant_core::Barcode>,
    ) -> Vec<assistant_core::lookup::CatalogRecord> {
        let dated = self
            .about()
            .dated
            .map_or_else(|| "undated".to_owned(), |d| format!("dated {d}"));
        let found = self
            .find(
                &query.series,
                query.start_year,
                query.number.as_str(),
                barcode.map(assistant_core::Barcode::as_str),
            )
            .unwrap_or_default();
        found
            .into_iter()
            .map(|r| {
                let year = r.start_year.map(|y| format!(" ({y})")).unwrap_or_default();
                assistant_core::lookup::CatalogRecord {
                    title: format!(
                        "Grand Comics Database (your copy, {dated}): {}{year} #{}",
                        r.series, r.number
                    ),
                    text: r.text,
                }
            })
            .collect()
    }
}
