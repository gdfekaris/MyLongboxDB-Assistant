//! The fields a lookup fills, what each holds, and how each is checked.
//! One table drives the schema the model answers to, the checks its
//! answer passes, and the review page's rows, so they cannot drift apart.

use crate::codes::{Barcode, Isbn};
use crate::collects::{format_collects, parse_collects};
use crate::date::{CoverDate, Date, PartialDate};
use crate::model::{Binding, Category, Distribution, MagazineFormat};
use crate::money::{cents_string, parse_cents};

/// What a field holds, which decides how its text is checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A name or title, on one line.
    Text,
    /// A few sentences.
    LongText,
    Year,
    /// A whole number, zero or more.
    Count,
    /// A whole number, one or more.
    Ordinal,
    Category,
    MagazineFormat,
    YesNo,
    CoverDate,
    Date,
    PartialDate,
    /// Dollars and cents, such as `0.75`.
    Price,
    Isbn,
    Barcode,
    Binding,
    Distribution,
    Collects,
}

/// One field a lookup can fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// Its name in the schema and in code.
    pub key: &'static str,
    /// Its name on the review page.
    pub label: &'static str,
    pub kind: Kind,
    /// What the model is told it holds.
    pub hint: &'static str,
    /// Whether a typed line can tell it; a line names an issue, not which
    /// printing or cover of it the owner has.
    pub from_text: bool,
    /// Whether it is a judgment the owner should look at even when the
    /// model is sure, and so starts unaccepted.
    pub judgment: bool,
}

const fn field(key: &'static str, label: &'static str, kind: Kind, hint: &'static str) -> Field {
    Field {
        key,
        label,
        kind,
        hint,
        from_text: true,
        judgment: false,
    }
}

const fn edition(key: &'static str, label: &'static str, kind: Kind, hint: &'static str) -> Field {
    Field {
        from_text: false,
        ..field(key, label, kind, hint)
    }
}

const fn judgment(key: &'static str, label: &'static str, kind: Kind, hint: &'static str) -> Field {
    Field {
        judgment: true,
        ..field(key, label, kind, hint)
    }
}

/// Every single-valued field, in the order the review page shows them.
pub const FIELDS: &[Field] = &[
    field(
        "series",
        "Series",
        Kind::Text,
        "The series' name as the Grand Comics Database or the publisher gives it, without its year or volume, and without a leading The.",
    ),
    field(
        "start_year",
        "Start year",
        Kind::Year,
        "The year the series' first issue came out, as in 'Amazing Spider-Man (1963)'.",
    ),
    // Asked only of a book from photos, and kept only where its covers
    // print no number.
    edition(
        "issue_number",
        "Issue number",
        Kind::Text,
        "Only when the cover prints no number: the issue's number as the catalogue gives it, without # or No., such as 485 or 1/2; nn for a book published without one.",
    ),
    field(
        "volume",
        "Volume",
        Kind::Ordinal,
        "The volume number, only where the publisher itself numbers the series as a volume, such as Vol. 2 in the indicia; blank otherwise, as the start year tells series apart.",
    ),
    field(
        "publisher",
        "Publisher",
        Kind::Text,
        "The publisher by its short name: Marvel Comics, Archie Comics, and otherwise the brand alone, such as DC, Image, Dark Horse, or IDW.",
    ),
    field(
        "imprint",
        "Imprint",
        Kind::Text,
        "The imprint, such as Vertigo or Epic; blank if none.",
    ),
    field(
        "category",
        "Kind",
        Kind::Category,
        "comic, magazine, or collected (a trade paperback, hardcover, or other book reprinting issues).",
    ),
    field(
        "magazine_format",
        "Magazine format",
        Kind::MagazineFormat,
        "Magazines only: standard, treasury, or digest.",
    ),
    field(
        "black_and_white",
        "Black and white",
        Kind::YesNo,
        "Magazines only: yes if its interior is black and white.",
    ),
    field(
        "known_issue_count",
        "Issues in the series",
        Kind::Count,
        "How many issues the series ran to, only for a series that has ended.",
    ),
    field(
        "legacy_number",
        "Legacy number",
        Kind::Count,
        "The issue's number counted across the title's relaunches, where the publisher printed one.",
    ),
    field(
        "story_title",
        "Story title",
        Kind::Text,
        "The title of the issue's main story; blank if it has none.",
    ),
    field(
        "cover_date",
        "Cover date",
        Kind::CoverDate,
        "The date printed on the cover, as YYYY-MM, or YYYY when it gives no month.",
    ),
    field(
        "release_date",
        "Release date",
        Kind::Date,
        "The day it went on sale, as YYYY-MM-DD.",
    ),
    field(
        "page_count",
        "Pages",
        Kind::Count,
        "The number of pages, including covers, as the reference source gives it.",
    ),
    field(
        "synopsis",
        "Synopsis",
        Kind::LongText,
        "A line or two in your own words on the main story, as short phrases or sentences each ending with a period, such as: Secret Wars II tie-in. Spider-Man in the black costume vs. the Puma, with the Beyonder. Never a semicolon, and a comma only where grammar needs one, never to join phrases.",
    ),
    judgment(
        "key_issue",
        "Key issue",
        Kind::YesNo,
        "yes only if reference sources call it a key issue, such as for a first appearance or death.",
    ),
    judgment(
        "key_reason",
        "Why it is a key",
        Kind::Text,
        "Why it is a key issue, if it is one, as short phrases each ending with a period, writing 1st, 2nd for first and second and giving a character's real name in brackets where the sources do, such as: 1st appearance & origin of the Scorpion (Mac Gargan). 1st appearance of Betty Brant. Never a semicolon, and a comma only where grammar needs one, never to join phrases.",
    ),
    field(
        "cover_price",
        "Cover price (US)",
        Kind::Price,
        "The US price printed on the standard cover in dollars, such as 0.75; blank if it was not priced in US dollars.",
    ),
    field(
        "canadian_cover_price",
        "Cover price (Canada)",
        Kind::Price,
        "The Canadian price printed beside it in dollars, such as 0.95; blank if none.",
    ),
    field(
        "isbn",
        "ISBN",
        Kind::Isbn,
        "Collected editions only: the 13-digit ISBN.",
    ),
    field(
        "binding",
        "Binding",
        Kind::Binding,
        "Collected editions only: hardcover, softcover, or oversized_hardcover.",
    ),
    field(
        "collects",
        "Collects",
        Kind::Collects,
        "Collected editions only: the issues it reprints, as 'Series (start year) 1-10, Annual 1; Other Series (start year) 5'.",
    ),
    edition(
        "variant",
        "Variant",
        Kind::Text,
        "The cover variant's name. A named edition comes first: an event, store, or convention exclusive such as SDCC 1995 Convention variant or FCBD 2024, a finish such as platinum cover or foil, a ratio such as 1:25 ratio variant, or a lettered cover such as Cover B. Otherwise, for a cover by another artist, the artist's surname and cover, such as Okazaki cover. Blank for the standard cover.",
    ),
    edition(
        "variant_cover_artists",
        "Variant cover artists",
        Kind::Text,
        "For a variant cover only: who drew it, full names separated by semicolons; blank for the standard cover.",
    ),
    edition(
        "printing",
        "Printing",
        Kind::Ordinal,
        "Which printing, such as 2 for a second printing, or 1 for the first.",
    ),
    edition(
        "distribution",
        "Distribution",
        Kind::Distribution,
        "direct or newsstand. A picture, logo, or Direct Edition in the barcode box means direct; a plain barcode on a US comic from before about 2000 means newsstand; later copies are direct unless marked newsstand.",
    ),
    edition(
        "published",
        "Published",
        Kind::PartialDate,
        "When this printing came out, for one later than the first, as YYYY, YYYY-MM, or YYYY-MM-DD.",
    ),
    edition(
        "barcode",
        "Barcode",
        Kind::Barcode,
        "The digits under the barcode, add-on included.",
    ),
];

/// A value as the owner's collection writes it: series without a leading
/// The, and publishers by the names the collection already uses. Applied
/// after the check, which compares values with their sources' own words.
pub(crate) fn house_style(key: &str, text: &str) -> String {
    let text = text.trim();
    match key {
        "series" => match text.get(..4) {
            Some(the) if the.eq_ignore_ascii_case("the ") && text.len() > 4 => {
                text[4..].trim_start().to_owned()
            }
            _ => text.to_owned(),
        },
        "publisher" => match text.to_lowercase().as_str() {
            "marvel" | "marvel comics" | "marvel comics group" => "Marvel Comics".into(),
            "dc comics" => "DC".into(),
            "image comics" => "Image".into(),
            "dark horse comics" => "Dark Horse".into(),
            "idw publishing" => "IDW".into(),
            "archie" | "archie comic publications" => "Archie Comics".into(),
            _ => text.to_owned(),
        },
        "key_reason" | "synopsis" => full_stops(text),
        _ => text.to_owned(),
    }
}

/// Prose with semicolons that join phrases turned into full stops, the
/// next phrase starting with a capital, as the collection writes it.
fn full_stops(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut capital = false;
    for part in text.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if !out.is_empty() {
            if !out.ends_with(['.', '!', '?']) {
                out.push('.');
            }
            out.push(' ');
            capital = true;
        }
        let mut chars = part.chars();
        if let (true, Some(first)) = (capital, chars.next()) {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        } else {
            out.push_str(part);
        }
    }
    out
}

/// The field with this key.
pub fn spec(key: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.key == key)
}

/// The longest text kept in a field, so a runaway answer cannot fill the
/// review page.
const MAX_TEXT: usize = 300;
const MAX_LONG_TEXT: usize = 2000;

/// Whether a character changes how the text around it is shown rather
/// than being part of it: the marks and overrides that can make shown
/// text read differently from what it says.
fn invisible(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{061C}' | '\u{FEFF}')
}

/// Text from the model made safe to keep and show: control and
/// direction-changing characters dropped, spacing tidied, and its length
/// capped.
pub(crate) fn clean(text: &str, max: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !invisible(*c))
        .take(max)
        .collect()
}

/// A field's text checked by its kind and written as MyLongboxDB reads
/// it, or why it cannot be. Empty text is a blank field and always fine.
pub fn check(kind: Kind, text: &str, this_year: u16) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(String::new());
    }
    let year_ok = |y: u16| (1800..=this_year.saturating_add(1)).contains(&y);
    let not = |what: &str| Err(format!("\u{201c}{text}\u{201d} is not {what}"));
    match kind {
        Kind::Text => Ok(clean(text, MAX_TEXT)),
        Kind::LongText => Ok(clean(text, MAX_LONG_TEXT)),
        Kind::Year => match text.parse::<u16>() {
            Ok(y) if year_ok(y) => Ok(y.to_string()),
            _ => not("a year"),
        },
        Kind::Count => match text.parse::<u32>() {
            Ok(n) => Ok(n.to_string()),
            _ => not("a whole number"),
        },
        Kind::Ordinal => match text.parse::<u32>() {
            Ok(n) if n >= 1 => Ok(n.to_string()),
            _ => not("a number from 1"),
        },
        Kind::Category => match text.to_lowercase().as_str() {
            "collected edition" => Ok("collected".into()),
            lower => Category::from_code(lower)
                .map(|c| c.code().to_owned())
                .map_or_else(|| not("comic, magazine, or collected"), Ok),
        },
        Kind::MagazineFormat => MagazineFormat::from_code(&text.to_lowercase())
            .map(|f| f.code().to_owned())
            .map_or_else(|| not("standard, treasury, or digest"), Ok),
        Kind::YesNo => match text.to_lowercase().as_str() {
            "yes" | "true" => Ok("yes".into()),
            "no" | "false" => Ok("no".into()),
            _ => not("yes or no"),
        },
        Kind::CoverDate => {
            let mut parts = text.split('-');
            let year = parts.next().and_then(|y| y.parse::<u16>().ok());
            let month = parts.next().map(|m| m.parse::<u8>().ok());
            match (year, month, parts.next()) {
                (Some(y), None, None) if year_ok(y) => CoverDate::new(y, None)
                    .map_or_else(|| not("a cover date"), |d| Ok(d.to_string())),
                (Some(y), Some(Some(m)), None) if year_ok(y) => CoverDate::new(y, Some(m))
                    .map_or_else(|| not("a cover date"), |d| Ok(d.to_string())),
                _ => not("a cover date as YYYY-MM"),
            }
        }
        Kind::Date => match text.parse::<Date>() {
            Ok(d) if year_ok(d.year()) => Ok(d.to_string()),
            _ => not("a date as YYYY-MM-DD"),
        },
        Kind::PartialDate => match text.parse::<PartialDate>() {
            Ok(d) if year_ok(d.year()) => Ok(d.to_string()),
            _ => not("a date"),
        },
        Kind::Price => {
            let digits = text.trim_start_matches('$').trim();
            match parse_cents(digits) {
                // No cover was ever priced at a thousand dollars.
                Ok(cents) if cents < 100_000 => Ok(cents_string(cents)),
                _ => not("a price in dollars and cents"),
            }
        }
        Kind::Isbn => Isbn::parse(text).map_or_else(
            || not("an ISBN-13 with a valid check digit"),
            |i| Ok(i.to_string()),
        ),
        Kind::Barcode => Barcode::parse(text).map_or_else(
            || not("a barcode with a valid check digit"),
            |b| Ok(b.to_string()),
        ),
        Kind::Binding => Binding::from_code(&text.to_lowercase().replace([' ', '-'], "_"))
            .map(|b| b.code().to_owned())
            .map_or_else(|| not("hardcover, softcover, or oversized_hardcover"), Ok),
        Kind::Distribution => Distribution::from_code(&text.to_lowercase())
            .map(|d| d.code().to_owned())
            .map_or_else(|| not("direct or newsstand"), Ok),
        Kind::Collects => parse_collects(text)
            .map(|entries| format_collects(&entries))
            .map_err(|why| format!("the Collects list cannot be read: {why}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YEAR: u16 = 2026;

    #[test]
    fn values_are_named_as_the_owner_s_collection_names_them() {
        assert_eq!(house_style("series", "The Transformers"), "Transformers");
        assert_eq!(
            house_style("series", "the Spectacular Spider-Men"),
            "Spectacular Spider-Men"
        );
        assert_eq!(house_style("series", "Theatre of Fear"), "Theatre of Fear");
        assert_eq!(house_style("series", "The"), "The");
        assert_eq!(house_style("publisher", "Marvel"), "Marvel Comics");
        assert_eq!(
            house_style("publisher", "Marvel Comics Group"),
            "Marvel Comics"
        );
        assert_eq!(house_style("publisher", "DC Comics"), "DC");
        assert_eq!(house_style("publisher", "Image Comics"), "Image");
        assert_eq!(house_style("publisher", "Fantagraphics"), "Fantagraphics");
        assert_eq!(house_style("story_title", "The End"), "The End");
        assert_eq!(
            house_style(
                "key_reason",
                "1st appearance of Venom; origin of the symbiote; 1st meeting."
            ),
            "1st appearance of Venom. Origin of the symbiote. 1st meeting."
        );
        assert_eq!(
            house_style(
                "synopsis",
                "Spider-Man vs. the Lizard;  the Torment arc begins"
            ),
            "Spider-Man vs. the Lizard. The Torment arc begins"
        );
        assert_eq!(
            house_style(
                "key_reason",
                "1st appearance of Kraven, with Mary Jane mentioned."
            ),
            "1st appearance of Kraven, with Mary Jane mentioned."
        );
    }

    #[test]
    fn keys_are_unique() {
        for (i, f) in FIELDS.iter().enumerate() {
            assert!(FIELDS[i + 1..].iter().all(|g| g.key != f.key), "{}", f.key);
        }
    }

    #[test]
    fn good_values_are_written_as_mylongboxdb_reads_them() {
        let ok = |kind, text: &str| check(kind, text, YEAR).unwrap();
        assert_eq!(
            ok(Kind::Text, "  Amazing \n Spider-Man "),
            "Amazing Spider-Man"
        );
        assert_eq!(ok(Kind::Year, "1963"), "1963");
        assert_eq!(ok(Kind::Category, "Collected Edition"), "collected");
        assert_eq!(ok(Kind::YesNo, "Yes"), "yes");
        assert_eq!(ok(Kind::CoverDate, "1988-5"), "1988-05");
        assert_eq!(ok(Kind::CoverDate, "1988"), "1988");
        assert_eq!(ok(Kind::Date, "1988-02-09"), "1988-02-09");
        assert_eq!(ok(Kind::Price, "$1.5"), "1.50");
        assert_eq!(ok(Kind::Isbn, "978-0-7851-2532-7"), "9780785125327");
        assert_eq!(
            ok(Kind::Binding, "Oversized Hardcover"),
            "oversized_hardcover"
        );
        assert_eq!(
            ok(Kind::Collects, "Watchmen (1986) 1-12"),
            "Watchmen (1986) 1–12"
        );
        assert_eq!(ok(Kind::Count, ""), "");
    }

    #[test]
    fn bad_values_are_refused_with_a_reason() {
        for (kind, text) in [
            (Kind::Year, "1963?"),
            (Kind::Year, "2099"),
            (Kind::Ordinal, "0"),
            (Kind::Count, "-1"),
            (Kind::Category, "graphic novel"),
            (Kind::YesNo, "probably"),
            (Kind::CoverDate, "March 1988"),
            (Kind::CoverDate, "1988-13"),
            (Kind::Date, "1988-02-30"),
            (Kind::Price, "1,50"),
            (Kind::Price, "£1.50"),
            (Kind::Price, "1500"),
            (Kind::Isbn, "9780785125328"),
            (Kind::Barcode, "759606089368"),
            (Kind::Collects, "the first twelve issues"),
        ] {
            let why = check(kind, text, YEAR).unwrap_err();
            assert!(!why.is_empty(), "{kind:?} {text}");
        }
    }

    #[test]
    fn runaway_text_is_cut_and_cleaned() {
        let long = check(Kind::LongText, &"word ".repeat(1000), YEAR).unwrap();
        assert_eq!(long.chars().count(), 2000);
        assert_eq!(
            check(Kind::Text, "a\u{7}b\u{202e}c\u{200b}d", YEAR).unwrap(),
            "abcd"
        );
    }
}
