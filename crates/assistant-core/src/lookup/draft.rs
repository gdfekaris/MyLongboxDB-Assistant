//! A lookup's result as the owner reviews it: each field's value with its
//! source, confidence, and note, and whether the owner has accepted it.
//! Only accepted values reach the export.

use super::fields::{check, spec};
use crate::codes::{Barcode, Isbn};
use crate::collects::parse_collects;
use crate::date::{CoverDate, Date, PartialDate};
use crate::lines::BookQuery;
use crate::model::{
    Appearance, ArcPart, Binding, Book, Category, Confidence, Credit, Distribution, MagazineFormat,
    Role,
};
use crate::money::parse_cents;

/// A web page the research cited, shown as text and never opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub url: String,
    pub title: String,
    /// The words the page showed, as the provider reported them.
    pub cited_text: String,
}

/// A series or volume the book could belong to, when the research could
/// not tell which.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub series: String,
    pub start_year: Option<u16>,
    pub volume: Option<u32>,
    pub publisher: String,
    pub why: String,
}

/// Whether the lookup settled on one issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Identity {
    Identified,
    /// The owner must pick one before the lookup goes on.
    Ambiguous(Vec<Candidate>),
    NotFound,
    /// The research cited no pages, so nothing it said can be used.
    NoSources,
}

/// One field's value as gathered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    pub key: &'static str,
    /// Checked by the field's kind; empty for a blank.
    pub text: String,
    /// Its place in [`Draft::sources`].
    pub source: Option<usize>,
    pub confidence: Confidence,
    /// Why it is doubtful or blank.
    pub note: String,
    pub accepted: bool,
}

/// The one detail a list entry has, by its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    Role(Role),
    FirstAppearance(bool),
    Part(Option<u32>),
}

/// A credit, character, team, or story arc as gathered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub detail: Detail,
    pub source: Option<usize>,
    pub confidence: Confidence,
    pub note: String,
    pub accepted: bool,
}

/// One book's lookup, ready for review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub query: BookQuery,
    pub identity: Identity,
    pub sources: Vec<Source>,
    pub values: Vec<Value>,
    pub credits: Vec<Entry>,
    pub characters: Vec<Entry>,
    pub teams: Vec<Entry>,
    pub story_arcs: Vec<Entry>,
    /// The research's own words, for the owner to read.
    pub research: String,
}

impl Draft {
    /// Sets a field to the owner's own value, checked by its kind. It is
    /// theirs, so it has no source and is accepted; an empty value clears
    /// the field.
    pub fn set_value(&mut self, key: &str, text: &str, this_year: u16) -> Result<(), String> {
        let field = spec(key).ok_or_else(|| format!("there is no field {key}"))?;
        let text = check(field.kind, text, this_year)?;
        let at = match self.values.iter().position(|v| v.key == field.key) {
            Some(at) => at,
            None => {
                self.values.push(Value {
                    key: field.key,
                    text: String::new(),
                    source: None,
                    confidence: Confidence::Low,
                    note: String::new(),
                    accepted: false,
                });
                self.values.len() - 1
            }
        };
        let value = &mut self.values[at];
        if value.text != text {
            value.accepted = !text.is_empty();
            value.text = text;
            value.source = None;
            value.confidence = Confidence::High;
            value.note.clear();
        }
        Ok(())
    }

    /// How many values and list entries the owner has not accepted; they
    /// are left out of the export.
    pub fn unaccepted(&self) -> usize {
        let values = self
            .values
            .iter()
            .filter(|v| !v.text.is_empty() && !v.accepted)
            .count();
        let entries = [
            &self.credits,
            &self.characters,
            &self.teams,
            &self.story_arcs,
        ]
        .into_iter()
        .flatten()
        .filter(|e| !e.accepted)
        .count();
        values + entries
    }

    /// A field's value, if the lookup has one for it.
    pub fn value(&self, key: &str) -> Option<&Value> {
        self.values.iter().find(|v| v.key == key)
    }

    /// The accepted text of a field, empty when blank or not accepted.
    fn accepted(&self, key: &str) -> &str {
        self.value(key)
            .filter(|v| v.accepted)
            .map_or("", |v| v.text.as_str())
    }

    /// The book as the owner accepted it. Values are re-read by their
    /// field's kind, so a value edited on the review page is checked as
    /// strictly as one the model gave; one that no longer reads is left
    /// out. `None` for a book that was not identified.
    pub fn book(&self, this_year: u16) -> Option<Book> {
        if self.identity != Identity::Identified {
            return None;
        }
        let get = |key: &str| -> Option<String> {
            let text = self.accepted(key);
            let kind = spec(key)?.kind;
            check(kind, text, this_year).ok().filter(|t| !t.is_empty())
        };
        let number = |key: &str| get(key).and_then(|t| t.parse::<u32>().ok());
        let category = get("category")
            .and_then(|c| Category::from_code(&c))
            .unwrap_or(Category::Comic);
        let series = get("series")
            .unwrap_or_else(|| super::fields::house_style("series", &self.query.series));
        let mut book = Book::new(category, series);
        book.series.start_year = get("start_year")
            .and_then(|y| y.parse().ok())
            .or(self.query.start_year);
        book.series.volume = number("volume").or(self.query.volume);
        book.series.publisher = get("publisher").unwrap_or_default();
        book.series.imprint = get("imprint").unwrap_or_default();
        book.series.known_issue_count = number("known_issue_count");
        if category == Category::Magazine {
            book.series.magazine_format =
                get("magazine_format").and_then(|f| MagazineFormat::from_code(&f));
            book.series.black_and_white = get("black_and_white").map(|b| b == "yes");
        }
        // A number found for a book whose covers print none, once the
        // owner accepts it; otherwise the number the book was looked up by.
        book.issue.number = Some(
            get("issue_number")
                .and_then(|n| crate::issue_number::IssueNumber::new(&n).ok())
                .unwrap_or_else(|| self.query.number.clone()),
        );
        book.issue.legacy_number = number("legacy_number");
        book.issue.title = get("story_title").unwrap_or_default();
        book.issue.cover_date = get("cover_date").and_then(|d| {
            let (year, month) = d
                .split_once('-')
                .map_or((d.as_str(), None), |(y, m)| (y, Some(m)));
            CoverDate::new(
                year.parse().ok()?,
                month.map(|m| m.parse().ok()).unwrap_or(None),
            )
        });
        book.issue.release_date = get("release_date").and_then(|d| d.parse::<Date>().ok());
        book.issue.page_count = number("page_count");
        book.issue.synopsis = get("synopsis").unwrap_or_default();
        book.issue.is_key = get("key_issue").is_some_and(|k| k == "yes");
        if book.issue.is_key {
            book.issue.key_reason = get("key_reason").unwrap_or_default();
        }
        if category == Category::CollectedEdition {
            book.issue.collects = get("collects")
                .and_then(|c| parse_collects(&c).ok())
                .unwrap_or_default();
            book.edition.isbn = get("isbn").and_then(|i| Isbn::parse(&i));
            book.edition.binding = get("binding").and_then(|b| Binding::from_code(&b));
        }
        book.edition.cover_price_cents = get("cover_price").and_then(|p| parse_cents(&p).ok());
        book.edition.canadian_cover_price_cents =
            get("canadian_cover_price").and_then(|p| parse_cents(&p).ok());
        book.edition.variant = get("variant").unwrap_or_default();
        if !book.edition.variant.is_empty() {
            book.edition.cover_artists = get("variant_cover_artists")
                .map(|names| {
                    names
                        .split(';')
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
        }
        book.edition.printing = number("printing");
        book.edition.distribution = get("distribution").and_then(|d| Distribution::from_code(&d));
        book.edition.published = get("published").and_then(|p| p.parse::<PartialDate>().ok());
        book.edition.barcode = get("barcode").and_then(|b| Barcode::parse(&b));

        let accepted = |list: &[Entry]| -> Vec<Entry> {
            list.iter()
                .filter(|e| e.accepted && !e.name.trim().is_empty())
                .cloned()
                .collect()
        };
        for entry in accepted(&self.credits) {
            if let Detail::Role(role) = entry.detail {
                book.issue.credits.push(Credit {
                    name: entry.name,
                    role,
                });
            }
        }
        let appearances = |list: &[Entry]| {
            accepted(list)
                .into_iter()
                .map(|e| Appearance {
                    first: matches!(e.detail, Detail::FirstAppearance(true)),
                    name: e.name,
                })
                .collect()
        };
        book.issue.characters = appearances(&self.characters);
        book.issue.teams = appearances(&self.teams);
        book.issue.story_arcs = accepted(&self.story_arcs)
            .into_iter()
            .map(|e| ArcPart {
                part: match e.detail {
                    Detail::Part(part) => part,
                    _ => None,
                },
                name: e.name,
            })
            .collect();
        Some(book)
    }
}

/// Whether a field means anything for a book of this category, so a
/// value for another kind of book is set aside.
pub(crate) fn fits_category(key: &str, category: Category) -> bool {
    match key {
        "magazine_format" | "black_and_white" => category == Category::Magazine,
        "isbn" | "binding" | "collects" => category == Category::CollectedEdition,
        _ => true,
    }
}
