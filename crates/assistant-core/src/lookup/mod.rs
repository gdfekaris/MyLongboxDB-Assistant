//! Looking a book up, in steps the app controls rather than in one
//! open-ended prompt:
//!
//! 0. Read, for photos: the model reports what the covers print, and
//!    that becomes the book to look up and sources the later steps can
//!    cite. Then the owner's own copy of the Grand Comics Database, if
//!    they added one, is asked for the issue's records, which become
//!    sources too.
//! 1. Research: with web search, find which issue it is and what reliable
//!    pages say about it. The provider reports which pages were cited.
//! 2. Structure: without tools, turn the notes into the fields under a
//!    schema. A value names its source by number from the research's
//!    cited pages, so a source can only ever be a page actually cited.
//! 3. Check: a separate request looks for values their sources do not
//!    support; those are cleared or marked for the owner.
//! 4. Validate: code checks every value by its field's rules, clears what
//!    fails, and leaves doubtful values unaccepted for review.
//!
//! The provider is behind [`Provider`], so this crate makes no
//! connections; the network crate supplies the real ones.

mod draft;
mod fields;
mod prompts;
mod read;
mod schema;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};

pub use draft::{Candidate, Detail, Draft, Entry, Identity, Source, Value};
pub use fields::{FIELDS, Field, Kind, check, spec};
pub use prompts::PROMPT_VERSION;
pub use read::{COVER_PHOTO, Reading, Seen};

use crate::codes::Barcode;
use crate::cost::{Estimate, Planned, Price, Usage, image_tokens};
use crate::lines::BookQuery;
use crate::model::{Category, Confidence, Jpeg, Role};
use crate::money::Picodollars;
use draft::fits_category;
pub(crate) use draft::{NameList, split_names, split_note, without_repeats};
pub use fields::clean;
use schema::{Action, Checked, FieldAnswer, MAX_LIST, Named, Status, Structured};

/// How hard the model should think.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effort {
    Low,
    Medium,
    High,
}

/// How much a lookup may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The most web searches the research may make.
    pub max_searches: u32,
    /// The most pages the research may read whole.
    pub max_pages: u32,
    /// The most searches and pages for a further copy of an issue already
    /// looked up, whose research is only of which edition it is.
    pub copy_searches: u32,
    pub copy_pages: u32,
    pub effort: Effort,
    /// The current year, which bounds the dates a value may have.
    pub this_year: u16,
}

impl Settings {
    /// The owner's choice of 2026-10-26, for accuracy near what the same
    /// model gives with a photo pasted into a chat: up to eight searches
    /// and four pages read whole a book, at high effort.
    pub fn balanced(this_year: u16) -> Self {
        Self {
            max_searches: 8,
            max_pages: 4,
            copy_searches: 3,
            copy_pages: 2,
            effort: Effort::High,
            this_year,
        }
    }
}

/// The most a step may write. Thinking counts toward it, and at high
/// effort the research and structuring think at length.
const READ_TOKENS: u32 = 4_000;
const RESEARCH_TOKENS: u32 = 32_000;
const STRUCTURE_TOKENS: u32 = 24_000;
const CHECK_TOKENS: u32 = 12_000;

/// What each step is expected to use, for the estimate before a run.
/// Thinking counts as output, and each search's results become input.
/// The research's search figures are from the live trial of 2026-10-24
/// on Opus 5.5, which read about 77,000 tokens over 3 searches a book,
/// rounded up, with its thinking doubled for high effort; the others' are
/// first guesses that it found close.
pub fn planned(settings: &Settings) -> [Planned; 3] {
    let searches = u64::from(settings.max_searches);
    let pages = u64::from(settings.max_pages);
    [
        Planned {
            input_tokens: 4_000,
            likely_output_tokens: 8_000,
            most_output_tokens: u64::from(RESEARCH_TOKENS),
            likely_searches: searches.min(4),
            most_searches: searches,
            tokens_per_search: 25_000,
            likely_pages: pages.min(2),
            most_pages: pages,
            // A page is kept to 10,000 tokens, and each later turn of the
            // research reads it again.
            tokens_per_page: 20_000,
            images: 0,
        },
        Planned {
            input_tokens: 7_000,
            likely_output_tokens: 4_000,
            most_output_tokens: u64::from(STRUCTURE_TOKENS),
            ..Planned::default()
        },
        Planned {
            input_tokens: 7_000,
            likely_output_tokens: 2_000,
            most_output_tokens: u64::from(CHECK_TOKENS),
            ..Planned::default()
        },
    ]
}

/// What the read step is expected to use for photos of these sizes, in
/// pixels.
pub fn planned_read(photos: &[(u32, u32)]) -> Planned {
    let images: u64 = photos.iter().map(|(w, h)| image_tokens(*w, *h)).sum();
    Planned {
        input_tokens: 1_500 + images,
        likely_output_tokens: 1_500,
        most_output_tokens: u64::from(READ_TOKENS),
        images: photos.len() as u64,
        ..Planned::default()
    }
}

/// What one book's lookup from a typed line should and at most can cost.
pub fn estimate_book(price: &Price, settings: &Settings) -> Estimate {
    planned(settings)
        .iter()
        .fold(Estimate::default(), |sum, step| {
            sum.plus(step.estimate(price))
        })
}

/// What one book's lookup from photos of these sizes should and at most
/// can cost: the read step, then the same steps as a typed line, with the
/// front photo read again by the research before it starts and after
/// each search.
pub fn estimate_photo_book(price: &Price, settings: &Settings, photos: &[(u32, u32)]) -> Estimate {
    let front = photos.first().map_or(0, |(w, h)| image_tokens(*w, *h));
    let [research, structure, check] = planned(settings);
    let research = Planned {
        input_tokens: research.input_tokens + front,
        tokens_per_search: research.tokens_per_search + front,
        images: research.images + u64::from(front > 0),
        ..research
    };
    [research, structure, check, planned_read(photos)]
        .iter()
        .fold(Estimate::default(), |sum, step| {
            sum.plus(step.estimate(price))
        })
}

/// The query for the series the owner picked from an unsettled lookup's
/// candidates, to look the book up again.
pub fn choose(query: &BookQuery, candidate: &Candidate) -> BookQuery {
    BookQuery {
        line: query.line,
        series: candidate.series.clone(),
        start_year: candidate.start_year.or(query.start_year),
        volume: candidate.volume,
        number: query.number.clone(),
    }
}

/// A request with web search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchRequest {
    pub system: String,
    /// The owner's front cover, sent before the text, so the research can
    /// tell which of an issue's covers and printings it shows.
    pub images: Vec<Jpeg>,
    pub user: String,
    pub max_searches: u32,
    /// The most pages it may read whole, from the sites the provider
    /// names; none for a provider that cannot.
    pub max_pages: u32,
    pub effort: Effort,
    pub max_tokens: u32,
}

/// A request without tools whose answer must follow a JSON Schema.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaRequest {
    pub name: &'static str,
    pub system: String,
    /// Photos, sent before the text.
    pub images: Vec<Jpeg>,
    pub user: String,
    pub schema: serde_json::Value,
    pub effort: Effort,
    pub max_tokens: u32,
}

/// The research's answer: its notes and the pages it cited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Research {
    pub text: String,
    pub sources: Vec<Source>,
}

/// Why a step's request gave no usable answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The model declined, with the provider's category if it gave one.
    Declined(Option<String>),
    /// The answer was cut off at its length limit.
    Cut,
    /// The request failed: a network or provider error, said plainly.
    Failed(String),
    /// The provider refused the key or the account, such as a revoked
    /// key or no credit left, so every other request would fail too.
    Refused(String),
    Cancelled,
}

/// What requests used and cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spent {
    pub usage: Usage,
    pub cost: Picodollars,
    /// Whether the cost of every request is known; a model with no price
    /// leaves it unknown.
    pub cost_known: bool,
}

impl Spent {
    /// A request's spending, its cost known or not.
    pub fn of(usage: Usage, cost: Option<Picodollars>) -> Self {
        Self {
            usage,
            cost: cost.unwrap_or_default(),
            cost_known: cost.is_some(),
        }
    }

    /// Both spendings together.
    pub fn add(&mut self, other: Self) {
        let first = *self == Self::default();
        self.usage = self.usage.plus(other.usage);
        self.cost = Picodollars(self.cost.0.saturating_add(other.cost.0));
        self.cost_known = (first || self.cost_known) && other.cost_known;
    }
}

/// A request's answer and what it cost, which is spent whether or not
/// the answer can be used.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply<T> {
    pub answer: Result<T, Failure>,
    pub spent: Spent,
}

/// A model provider, as the lookup needs it.
pub trait Provider {
    fn research(&self, request: &ResearchRequest, cancel: &AtomicBool) -> Reply<Research>;
    /// The answer's JSON text.
    fn answer(&self, request: &SchemaRequest, cancel: &AtomicBool) -> Reply<String>;
}

/// What the owner's own copy of a catalogue records of an issue,
/// written out for the lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRecord {
    /// Which catalogue and issue, such as Grand Comics Database (your
    /// copy, dated 2026-10-16): The Amazing Spider-Man (1963) #300.
    pub title: String,
    pub text: String,
}

/// The owner's own copy of a catalogue, the Grand Comics Database's,
/// asked before the web. Lookups run on several threads at once.
pub trait Catalog: Sync {
    /// The records of the issue `query` names, or whose edition carries
    /// `barcode`, the likeliest first; none where the copy has none.
    fn records(&self, query: &BookQuery, barcode: Option<&Barcode>) -> Vec<CatalogRecord>;
}

/// The address given to sources from the owner's catalogue copy. It has
/// a space, so no web page's address can be the same.
pub const CATALOG_COPY: &str = "catalogue copy";

/// The most records a lookup takes from the catalogue copy, and the
/// longest each may be.
const MAX_RECORDS: usize = 3;
const MAX_RECORD: usize = 16_000;

/// The catalogue copy's records of the book, cleaned and capped as any
/// text from outside is.
fn catalogued(
    catalog: Option<&dyn Catalog>,
    query: &BookQuery,
    reading: Option<&Reading>,
) -> Vec<Source> {
    let Some(catalog) = catalog else {
        return Vec::new();
    };
    let barcode = reading.and_then(Reading::barcode);
    catalog
        .records(query, barcode.as_ref())
        .into_iter()
        .take(MAX_RECORDS)
        .map(|r| Source {
            url: CATALOG_COPY.into(),
            title: clean(&r.title, MAX_TITLE),
            cited_text: r
                .text
                .lines()
                .map(|line| clean(line, MAX_RECORD))
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
                .chars()
                .take(MAX_RECORD)
                .collect(),
        })
        .collect()
}

/// The catalogue copy's records, for the research to start from.
fn records_for_research(records: &[Source]) -> String {
    if records.is_empty() {
        return String::new();
    }
    let mut text =
        String::from("\nThe owner's copy of the Grand Comics Database has these records:\n");
    for (n, r) in records.iter().enumerate() {
        text.push_str(&format!(
            "[GCD {}] {}\n<<<\n{}\n>>>\n",
            n + 1,
            r.title,
            r.cited_text
        ));
    }
    text
}

/// Why a lookup produced nothing to review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookupError {
    Failed(Failure),
    /// The model's structured answer could not be read.
    Unreadable(String),
    /// The covers did not show a book to look up, and why.
    NoBookOnCover(String),
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(Failure::Declined(category)) => {
                f.write_str("the model declined to look this book up")?;
                match category {
                    Some(c) => write!(f, " ({c})"),
                    None => Ok(()),
                }
            }
            Self::Failed(Failure::Cut) => f.write_str("the model's answer was cut off"),
            Self::Failed(Failure::Failed(why)) => f.write_str(why),
            Self::Failed(Failure::Refused(why)) => {
                write!(f, "the provider refused the key or the account: {why}")
            }
            Self::Failed(Failure::Cancelled) => f.write_str("cancelled"),
            Self::Unreadable(why) => write!(f, "the model's answer could not be read: {why}"),
            Self::NoBookOnCover(why) => write!(f, "nothing was looked up: {why}"),
        }
    }
}

impl std::error::Error for LookupError {}

fn reply<T>(reply: Reply<T>, spent: &mut Spent) -> Result<T, LookupError> {
    spent.add(reply.spent);
    reply.answer.map_err(LookupError::Failed)
}

/// The owner's book as the steps are told it, with what was read from its
/// covers if it came as photos.
fn describe(query: &BookQuery, reading: Option<&Reading>) -> String {
    let unknown = "not given";
    // A cover's title is often not the series' catalogued name, so it is
    // not given as a name the owner chose.
    let (named, series) = if reading.is_some() {
        (
            "The owner's cover shows this book",
            "Title, as the cover prints it",
        )
    } else {
        ("The owner named this book", "Series, as they wrote it")
    };
    let number = match reading {
        Some(r) if r.printed_number().is_none() => {
            "not printed on the cover: find it from the other details, such as a grading \
             label, the date, or the price; a book published without a number has none, nn"
                .to_owned()
        }
        _ => query.number.to_string(),
    };
    format!(
        "{named}: {query}\n{series}: {}\nStart year: {}\nVolume: {}\nIssue number: {number}\n",
        query.series,
        query.start_year.map_or(unknown.into(), |y| y.to_string()),
        query.volume.map_or(unknown.into(), |v| v.to_string()),
    )
}

/// The address given to the source that stands for what the model knows
/// without a page. It has a space, so no web page's address can be the
/// same.
pub const KNOWLEDGE: &str = "model knowledge";

/// The source cited for what the model knows of an issue without a page
/// saying it, as it knows who drew a famous cover.
fn knowledge() -> Source {
    Source {
        url: KNOWLEDGE.into(),
        title: "The model's own knowledge".into(),
        cited_text: String::new(),
    }
}

/// Whether a value from this source can go into the export without the
/// owner's look: one from a page or the cover unless the model doubts
/// it, one from its own knowledge only when it is sure.
fn trusted(source: &Source, confidence: Confidence) -> bool {
    if source.url == KNOWLEDGE {
        confidence == Confidence::High
    } else {
        confidence != Confidence::Low
    }
}

/// The research notes and numbered sources, for the later steps.
fn notes(research: &Research) -> String {
    let mut text = format!("Research notes:\n<<<\n{}\n>>>\n\nSources:\n", research.text);
    let mut records = 0;
    for (n, source) in research.sources.iter().enumerate() {
        if source.url == CATALOG_COPY {
            records += 1;
            text.push_str(&format!(
                "[{}] {}, the owner's own copy, which the notes call [GCD {records}]\n    shows:\n<<<\n{}\n>>>\n",
                n + 1,
                source.title,
                source.cited_text
            ));
            continue;
        }
        if source.url == KNOWLEDGE {
            text.push_str(&format!(
                "[{}] Your own knowledge, for what the notes mark [knowledge]\n",
                n + 1
            ));
            continue;
        }
        text.push_str(&format!(
            "[{}] {} — {}\n    shows: \"{}\"\n",
            n + 1,
            source.title,
            source.url,
            source.cited_text
        ));
    }
    text
}

/// The most pages kept from one research answer. Lookups cite a few
/// dozen at most.
const MAX_SOURCES: usize = 100;
const MAX_URL: usize = 2_000;
const MAX_TITLE: usize = 300;
/// Room for the dozen short passages kept from a page.
const MAX_CITED: usize = 3_000;

/// The research's pages as the provider reported them are untrusted
/// text: cleaned and capped as the model's own answers are, before they
/// are shown, copied into the later steps' prompts, or saved.
fn cleaned(research: Research) -> Research {
    Research {
        sources: research
            .sources
            .into_iter()
            .take(MAX_SOURCES)
            .map(|s| Source {
                url: clean(&s.url, MAX_URL),
                title: clean(&s.title, MAX_TITLE),
                cited_text: clean(&s.cited_text, MAX_CITED),
            })
            .collect(),
        text: research.text,
    }
}

/// A source's number from an answer: its place in the list, or `None`.
fn source_index(number: i64, sources: &[Source]) -> Option<usize> {
    usize::try_from(number)
        .ok()
        .filter(|n| (1..=sources.len()).contains(n))
        .map(|n| n - 1)
}

const MAX_NAME: usize = 200;

fn entries<T>(
    named: Vec<Named<T>>,
    sources: &[Source],
    kind: NameList,
    detail: impl Fn(T) -> Option<Detail>,
) -> Vec<Entry> {
    let mut list: Vec<Entry> = named
        .into_iter()
        .filter_map(|n| {
            // A name with no cited source is not kept.
            let source = source_index(n.source, sources)?;
            let detail = detail(n.detail)?;
            let confidence = Confidence::from(n.confidence);
            let text = clean(&n.name, MAX_NAME);
            let (names, doubtful) = split_names(&text, kind);
            let note = if doubtful {
                split_note(&text, kind, names.len())
            } else {
                String::new()
            };
            Some(
                names
                    .into_iter()
                    .map(|name| Entry {
                        name,
                        detail,
                        source: Some(source),
                        confidence,
                        note: note.clone(),
                        accepted: !doubtful && trusted(&sources[source], confidence),
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect();
    without_repeats(&mut list);
    list
}

fn read_structured(
    json: &str,
    research: &Research,
    query: &BookQuery,
    settings: &Settings,
    from_text: bool,
) -> Result<Draft, LookupError> {
    let unreadable =
        |e: &dyn std::fmt::Display| LookupError::Unreadable(clean(&e.to_string(), 300));
    let answer: Structured = serde_json::from_str(json).map_err(|e| unreadable(&e))?;
    let lists = [
        answer.credits.len(),
        answer.characters.len(),
        answer.teams.len(),
        answer.story_arcs.len(),
        answer.identification.candidates.len(),
    ];
    if lists.iter().any(|n| *n > MAX_LIST) {
        return Err(unreadable(&"a list in it is far too long"));
    }
    let sources = &research.sources;
    let mut draft = Draft {
        query: query.clone(),
        identity: Identity::Identified,
        sources: sources.clone(),
        values: Vec::new(),
        credits: Vec::new(),
        characters: Vec::new(),
        teams: Vec::new(),
        story_arcs: Vec::new(),
        research: clean(&research.text, 20_000),
    };
    let candidates: Vec<Candidate> = answer
        .identification
        .candidates
        .into_iter()
        .take(10)
        .map(|c| Candidate {
            series: clean(&c.series, MAX_NAME),
            start_year: check(Kind::Year, &c.start_year, settings.this_year)
                .ok()
                .and_then(|y| y.parse().ok()),
            volume: check(Kind::Ordinal, &c.volume, settings.this_year)
                .ok()
                .and_then(|v| v.parse().ok()),
            publisher: clean(&c.publisher, MAX_NAME),
            why: clean(&c.why, 500),
        })
        .filter(|c| !c.series.is_empty())
        .collect();
    match answer.identification.status {
        Status::Identified => {}
        Status::NotFound => {
            draft.identity = Identity::NotFound;
            return Ok(draft);
        }
        Status::Ambiguous => {
            draft.identity = Identity::Ambiguous(candidates);
            return Ok(draft);
        }
    }

    for field in FIELDS.iter().filter(|f| f.from_text || !from_text) {
        let mut value = Value {
            key: field.key,
            text: String::new(),
            source: None,
            confidence: Confidence::Low,
            note: String::new(),
            accepted: false,
        };
        let given = answer.fields.get(field.key).cloned();
        match given.map(serde_json::from_value::<FieldAnswer>) {
            None => value.note = "not answered".into(),
            Some(Err(e)) => return Err(unreadable(&e)),
            Some(Ok(given)) => {
                value.confidence = given.confidence.into();
                value.note = clean(&given.note, 500);
                match check(field.kind, &given.value, settings.this_year) {
                    Err(why) => {
                        value.note = why;
                    }
                    Ok(text) if text.is_empty() => {}
                    Ok(text) => match source_index(given.source, sources) {
                        None => value.note = "left out: no cited source states it".into(),
                        Some(source) => {
                            value.text = text;
                            value.source = Some(source);
                            value.accepted =
                                !field.judgment && trusted(&sources[source], value.confidence);
                        }
                    },
                }
            }
        }
        draft.values.push(value);
    }
    chosen_series(&mut draft, &candidates);
    let category = draft
        .value("category")
        .and_then(|v| Category::from_code(&v.text))
        .unwrap_or(Category::Comic);
    for value in &mut draft.values {
        if !value.text.is_empty() && !fits_category(value.key, category) {
            value.text.clear();
            value.source = None;
            value.accepted = false;
            value.note = "left out: not for this kind of book".into();
        }
    }
    draft.credits = entries(
        answer.credits,
        sources,
        NameList::Creators,
        |role: String| Role::from_code(&role).map(Detail::Role),
    );
    draft.characters = entries(answer.characters, sources, NameList::Characters, |first| {
        Some(Detail::FirstAppearance(first))
    });
    draft.teams = entries(answer.teams, sources, NameList::Teams, |first| {
        Some(Detail::FirstAppearance(first))
    });
    draft.story_arcs = entries(
        answer.story_arcs,
        sources,
        NameList::StoryArcs,
        |part: String| {
            let part = part.trim();
            if part.is_empty() {
                return Some(Detail::Part(None));
            }
            part.parse::<u32>()
                .ok()
                .filter(|p| *p >= 1)
                .map(|p| Detail::Part(Some(p)))
        },
    );
    Ok(draft)
}

/// A book the research found in the likeliest of several series, as a
/// line without a year often is, waits for the owner to confirm the
/// series, named with the others it could have been.
fn chosen_series(draft: &mut Draft, candidates: &[Candidate]) {
    let year = draft
        .value("start_year")
        .and_then(|v| v.text.parse::<u16>().ok());
    let others: Vec<String> = candidates
        .iter()
        .filter(|c| c.start_year != year || year.is_none())
        .map(|c| match c.start_year {
            Some(y) => format!("{} ({y})", c.series),
            None => c.series.clone(),
        })
        .collect();
    if others.is_empty() {
        return;
    }
    let note = format!(
        "the likeliest of several series; it could also be {}",
        others.join(", ")
    );
    for value in draft
        .values
        .iter_mut()
        .filter(|v| matches!(v.key, "series" | "start_year" | "volume") && !v.text.is_empty())
    {
        value.accepted = false;
        value.note = note.clone();
    }
}

/// The values the check step is shown, each with a key it can answer by.
fn checked_values(draft: &Draft) -> String {
    let source = |s: Option<usize>| match s {
        Some(i) if draft.sources[i].url == KNOWLEDGE => {
            format!("source [{}]: the model's own knowledge, no page", i + 1)
        }
        // A record runs to pages: it is shown once, with the sources.
        Some(i) if draft.sources[i].url == CATALOG_COPY => {
            format!(
                "source [{}]: the owner's catalogue record, shown below",
                i + 1
            )
        }
        Some(i) => format!("source [{}]: \"{}\"", i + 1, draft.sources[i].cited_text),
        None => "no source".into(),
    };
    let mut text = String::from("Values:\n");
    for v in draft.values.iter().filter(|v| !v.text.is_empty()) {
        text.push_str(&format!("{}: {} — {}\n", v.key, v.text, source(v.source)));
    }
    let lists = [
        ("credits", &draft.credits),
        ("characters", &draft.characters),
        ("teams", &draft.teams),
        ("story_arcs", &draft.story_arcs),
    ];
    for (name, list) in lists {
        for (n, e) in list.iter().enumerate() {
            let detail = match e.detail {
                Detail::Role(role) => role.code().to_owned(),
                Detail::FirstAppearance(true) => "first appearance".into(),
                Detail::FirstAppearance(false) => String::new(),
                Detail::Part(Some(p)) => format!("part {p}"),
                Detail::Part(None) => String::new(),
            };
            text.push_str(&format!(
                "{name}.{}: {} ({detail}) — {}\n",
                n + 1,
                e.name,
                source(e.source)
            ));
        }
    }
    text
}

/// Applies the check's findings: cleared values lose their text, doubted
/// ones are left for the owner, and each says why.
fn apply_check(draft: &mut Draft, checked: Checked) {
    for problem in checked.problems.into_iter().take(MAX_LIST) {
        let note = clean(&problem.problem, 500);
        let key = problem.field.trim();
        if let Some(value) = draft.values.iter_mut().find(|v| v.key == key) {
            if value.text.is_empty() {
                continue;
            }
            value.accepted = false;
            value.note = note;
            match problem.action {
                Action::Clear => {
                    value.text.clear();
                    value.source = None;
                }
                Action::Doubt => value.confidence = Confidence::Low,
            }
            continue;
        }
        let Some((list, n)) = key.split_once('.') else {
            continue;
        };
        let list = match list {
            "credits" => &mut draft.credits,
            "characters" => &mut draft.characters,
            "teams" => &mut draft.teams,
            "story_arcs" => &mut draft.story_arcs,
            _ => continue,
        };
        let Some(entry) = n
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| list.get_mut(i))
        else {
            continue;
        };
        entry.accepted = false;
        entry.note = note;
        match problem.action {
            // Marked here and removed below, so later numbers still
            // point at the entries they name.
            Action::Clear => entry.name.clear(),
            Action::Doubt => entry.confidence = Confidence::Low,
        }
    }
    for list in [
        &mut draft.credits,
        &mut draft.characters,
        &mut draft.teams,
        &mut draft.story_arcs,
    ] {
        list.retain(|e| !e.name.is_empty());
    }
}

fn effort_note(failure: &Failure) -> String {
    match failure {
        Failure::Declined(_) => "the check declined".into(),
        Failure::Cut => "the check's answer was cut off".into(),
        Failure::Failed(why) | Failure::Refused(why) => format!("the check failed: {why}"),
        Failure::Cancelled => "cancelled".into(),
    }
}

/// Looks one book up from a typed line. What every request spent is
/// added to `spent`, even when the lookup fails.
pub fn look_up(
    provider: &dyn Provider,
    catalog: Option<&dyn Catalog>,
    query: &BookQuery,
    settings: &Settings,
    cancel: &AtomicBool,
    spent: &mut Spent,
) -> Result<Draft, LookupError> {
    look_up_from(
        provider, catalog, query, None, None, settings, cancel, spent,
    )
}

/// The owner's photos of one book: the front cover, and the back if they
/// took it. Each is a JPEG re-encoded from pixels alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Covers {
    /// The book's place in the run, as a typed book's line.
    pub line: usize,
    pub front: Jpeg,
    pub back: Option<Jpeg>,
    /// The series the owner picked from an earlier lookup's candidates,
    /// looked up in place of the book the cover's title names.
    pub chosen: Option<BookQuery>,
}

/// Looks one book up from its cover photos: reads them, then looks up
/// the book they show, as for a typed line. What every request spent is
/// added to `spent`, even when the lookup fails.
pub fn look_up_covers(
    provider: &dyn Provider,
    catalog: Option<&dyn Catalog>,
    covers: &Covers,
    settings: &Settings,
    cancel: &AtomicBool,
    spent: &mut Spent,
) -> Result<Draft, LookupError> {
    let reading = read_covers(provider, covers, settings, cancel, spent)?;
    look_up_read(provider, catalog, covers, &reading, settings, cancel, spent)
}

/// Reads what the covers print: the first step of a photo lookup.
pub fn read_covers(
    provider: &dyn Provider,
    covers: &Covers,
    settings: &Settings,
    cancel: &AtomicBool,
    spent: &mut Spent,
) -> Result<Reading, LookupError> {
    let json = reply(
        provider.answer(
            &SchemaRequest {
                name: "cover",
                system: read::prompt(),
                images: std::iter::once(covers.front.clone())
                    .chain(covers.back.clone())
                    .collect(),
                user: if covers.back.is_some() {
                    "The first photo is the front cover, the second the back.".into()
                } else {
                    "The photo is the front cover.".into()
                },
                schema: read::schema(),
                effort: settings.effort,
                max_tokens: READ_TOKENS,
            },
            cancel,
        ),
        spent,
    )?;
    let reading = Reading::parse(&json).map_err(|why| LookupError::Unreadable(clean(&why, 300)))?;
    if cancel.load(Ordering::Relaxed) {
        return Err(LookupError::Failed(Failure::Cancelled));
    }
    Ok(reading)
}

/// The book photos show, as they are looked up: the series the owner
/// picked from an earlier lookup's candidates, or what the covers print.
pub fn covers_query(covers: &Covers, reading: &Reading) -> Result<BookQuery, LookupError> {
    match &covers.chosen {
        Some(chosen) => Ok(chosen.clone()),
        None => reading
            .query(covers.line)
            .map_err(LookupError::NoBookOnCover),
    }
}

/// What tells copies of one issue apart from other books before they are
/// looked up: the title as their covers print it, or as the owner typed
/// it, and the number. Unlike a title compared with a catalogue's, an
/// annual's keeps its Annual.
pub fn copy_key(query: &BookQuery) -> String {
    let lower = query.series.to_lowercase().replace('&', " and ");
    let spaced: String = lower
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let mut words: Vec<&str> = spaced.split_whitespace().collect();
    if words.len() > 1 && words[0] == "the" {
        words.remove(0);
    }
    format!("{} #{}", words.join(" "), query.number.sort_key())
}

/// Looks up the book whose covers were read as `reading`, as for a
/// typed line, then holds what the covers show against what was found.
pub fn look_up_read(
    provider: &dyn Provider,
    catalog: Option<&dyn Catalog>,
    covers: &Covers,
    reading: &Reading,
    settings: &Settings,
    cancel: &AtomicBool,
    spent: &mut Spent,
) -> Result<Draft, LookupError> {
    let query = covers_query(covers, reading)?;
    let draft = look_up_from(
        provider,
        catalog,
        &query,
        Some((reading, &covers.front)),
        None,
        settings,
        cancel,
        spent,
    )?;
    Ok(from_covers(draft, covers, reading))
}

/// Looks up a further copy of an issue already looked up, as `known`:
/// its research is only of which edition the covers show, and the
/// issue's facts are `known`'s. Where the research finds the covers are
/// of another issue, the copy is looked up in full as any book is.
#[allow(clippy::too_many_arguments)]
pub fn look_up_copy(
    provider: &dyn Provider,
    catalog: Option<&dyn Catalog>,
    covers: &Covers,
    reading: &Reading,
    known: &Draft,
    settings: &Settings,
    cancel: &AtomicBool,
    spent: &mut Spent,
) -> Result<Draft, LookupError> {
    let query = covers_query(covers, reading)?;
    let draft = look_up_from(
        provider,
        catalog,
        &query,
        Some((reading, &covers.front)),
        Some(known),
        settings,
        cancel,
        spent,
    )?;
    let mut draft = from_covers(draft, covers, reading);
    // Which issue each is, as found, ticked or not: the copy's series may
    // wait for the owner because its cover prints another title.
    let issue = |d: &Draft| {
        let text = |key: &str| d.value(key).map_or("", |v| v.text.as_str()).to_lowercase();
        let number = match text("issue_number") {
            n if n.is_empty() => d.query.number.sort_key().to_owned(),
            n => crate::IssueNumber::new(&n).map_or(n, |n| n.sort_key().to_owned()),
        };
        (text("series"), text("start_year"), number)
    };
    let found = issue(&draft);
    if draft.identity != Identity::Identified || found.0.is_empty() || found != issue(known) {
        if cancel.load(Ordering::Relaxed) {
            return Err(LookupError::Failed(Failure::Cancelled));
        }
        return look_up_read(provider, catalog, covers, reading, settings, cancel, spent);
    }
    crate::copies::copy_facts(known, &mut draft);
    Ok(draft)
}

/// A photo lookup's draft, held against what the covers show.
fn from_covers(mut draft: Draft, covers: &Covers, reading: &Reading) -> Draft {
    compare_barcodes(&mut draft, reading);
    distribution_from_box(&mut draft, reading);
    first_printing(&mut draft, reading);
    printed_number(&mut draft, reading);
    undated_cover(&mut draft, reading);
    facsimile(&mut draft, reading);
    variant_name(&mut draft);
    if covers.chosen.is_some() && draft.identity == Identity::Identified {
        given_volume(&mut draft);
    } else {
        series_from_cover(&mut draft, reading);
    }
    draft
}

/// A facsimile is catalogued as the issue it reprints, with the dates of
/// its own release: the owner leaves its cover date and cover prices
/// blank, as its cover only reproduces the original's, and it is never a
/// key. A release date no later than the original's year is the
/// original's, so it is cleared; any other waits for the owner, as
/// sources often give the original's.
fn facsimile(draft: &mut Draft, reading: &Reading) {
    let says = |text: &str| text.to_lowercase().contains("facsimile");
    let marked = draft
        .values
        .iter()
        .any(|v| v.key == "variant" && says(&v.text))
        || reading.seen.iter().any(|s| says(&s.text));
    if draft.identity != Identity::Identified || !marked {
        return;
    }
    let year = |key: &str| {
        draft
            .values
            .iter()
            .find(|v| v.key == key)
            .and_then(|v| v.text.get(..4))
            .and_then(|y| y.parse::<u16>().ok())
    };
    let original = year("cover_date").or(year("start_year"));
    for value in &mut draft.values {
        match value.key {
            "cover_date" | "cover_price" | "canadian_cover_price" if !value.text.is_empty() => {
                value.text.clear();
                value.source = None;
                value.accepted = true;
                value.note =
                    "a facsimile: left blank, as its cover only reproduces the original's".into();
            }
            "key_issue" => {
                value.text = "no".into();
                value.source = None;
                value.accepted = true;
                value.note = "a facsimile is never a key".into();
            }
            "key_reason" => {
                value.text.clear();
                value.source = None;
                value.accepted = true;
                value.note.clear();
            }
            "release_date" | "published" if !value.text.is_empty() => {
                let own_year = value.text.get(..4).and_then(|y| y.parse::<u16>().ok());
                if own_year
                    .zip(original)
                    .is_some_and(|(own, first)| own <= first)
                {
                    value.text.clear();
                    value.source = None;
                    value.note = "the original's date, not the facsimile's".into();
                } else {
                    value.note =
                        "check this is the facsimile's own date, not the original's".into();
                }
                value.accepted = false;
            }
            _ => {}
        }
    }
}

/// A title reduced to what tells titles apart: lower case, & read as and,
/// punctuation as spaces, and without a leading The or a trailing
/// Annual, which the collection adds to an annual's series.
fn title_key(title: &str) -> String {
    let lower = fields::spelled_out(title)
        .to_lowercase()
        .replace('&', " and ");
    let spaced: String = lower
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let mut words: Vec<&str> = spaced.split_whitespace().collect();
    if words.len() > 1 && words[0] == "the" {
        words.remove(0);
    }
    if words.len() > 1 && words.last() == Some(&"annual") {
        words.pop();
    }
    words.join(" ")
}

/// A series named otherwise than the owner's cover prints its title waits
/// for the owner's word. Catalogues and pages file many books under a
/// name the cover does not print, a translated edition under its
/// original's, a logo under its plain series, and only the owner knows
/// which name their collection keeps; a wrong one would file the book in
/// another series.
fn series_from_cover(draft: &mut Draft, reading: &Reading) {
    let Some(title) = reading.get("title").map(|t| t.text.clone()) else {
        return;
    };
    if draft.identity != Identity::Identified {
        return;
    }
    if let Some(value) = draft
        .values
        .iter_mut()
        .find(|v| v.key == "series" && !v.text.is_empty())
        && title_key(&value.text) != title_key(&title)
    {
        value.accepted = false;
        if value.note.is_empty() {
            value.note = format!(
                "your cover's title reads \u{201c}{}\u{201d}: keep the name your collection files it under",
                clean(&title, MAX_NAME)
            );
        }
    }
}

/// What a variant found by a lookup notes as it waits for the owner.
pub(crate) const VARIANT_NOTE: &str = "name it as your collection names variants";

/// Variant names are the collection's own, more a habit than a rule, so
/// one found waits for the owner to put it in their words.
fn variant_name(draft: &mut Draft) {
    if let Some(value) = draft
        .values
        .iter_mut()
        .find(|v| v.key == "variant" && !v.text.is_empty())
    {
        value.accepted = false;
        if value.note.is_empty() {
            value.note = VARIANT_NOTE.into();
        }
    }
}

/// The year from which a book whose cover prints no month is dated by
/// the month it went on sale. Older books carried their date in the
/// indicia, and the collection takes that, or the catalogue's month.
const SALE_DATED_FROM: u16 = 2000;

/// A cover date as the owner records it: the month the cover prints, or
/// where it prints none, the month a modern book went on sale. An older
/// book keeps the catalogue's date, and takes the sale month only when
/// no source dates it at all.
fn undated_cover(draft: &mut Draft, reading: &Reading) {
    if draft.identity != Identity::Identified
        || reading
            .get("cover_date")
            .is_some_and(|s| prints_month(&s.text))
    {
        return;
    }
    let Some(release) = draft
        .values
        .iter()
        .find(|v| v.key == "release_date" && v.text.len() >= 7)
        .cloned()
    else {
        return;
    };
    let modern = release
        .text
        .get(..4)
        .and_then(|y| y.parse::<u16>().ok())
        .is_some_and(|y| y >= SALE_DATED_FROM);
    if let Some(value) = draft
        .values
        .iter_mut()
        .find(|v| v.key == "cover_date" && (modern || v.text.is_empty()))
    {
        value.text = release.text[..7].to_owned();
        value.source = release.source;
        value.confidence = release.confidence;
        value.accepted = release.accepted;
        value.note = "your cover prints no month: the month it went on sale".into();
    }
}

/// For a typed line, whose cover is not seen: most covers from 2000 on
/// print no month, and the collection then dates them by the month they
/// went on sale. The owner looks, as their cover may print one.
fn sale_month(draft: &mut Draft) {
    if draft.identity != Identity::Identified {
        return;
    }
    let Some(release) = draft
        .values
        .iter()
        .find(|v| v.key == "release_date" && v.text.len() >= 7)
        .cloned()
    else {
        return;
    };
    let modern = release
        .text
        .get(..4)
        .and_then(|y| y.parse::<u16>().ok())
        .is_some_and(|y| y >= SALE_DATED_FROM);
    let month = &release.text[..7];
    let Some(value) = draft
        .values
        .iter_mut()
        .find(|v| v.key == "cover_date" && v.text != month)
    else {
        return;
    };
    if modern {
        value.text = month.to_owned();
        value.source = release.source;
        value.confidence = release.confidence;
        value.accepted = false;
        value.note = "the month it went on sale, as most covers from 2000 on print none: \
                      use the month on your cover if it prints one"
            .into();
    }
}

/// Whether a date read from a cover gives its month, as in Mar 1954 or
/// 3/54, not only a year.
fn prints_month(printed: &str) -> bool {
    printed.chars().any(|c| c.is_alphabetic())
        || printed.chars().filter(char::is_ascii_digit).count() > 4
        || printed.contains('/')
}

/// The issue number the research found is wanted only where the covers
/// print none; a number they print is the book's. A found number says
/// which book this is, so the owner confirms it before it is exported.
fn printed_number(draft: &mut Draft, reading: &Reading) {
    if reading.printed_number().is_none() {
        if let Some(value) = draft
            .values
            .iter_mut()
            .find(|v| v.key == "issue_number" && !v.text.is_empty())
        {
            value.accepted = false;
            value.note = format!(
                "not printed on your cover; {}",
                if value.note.is_empty() {
                    "check it before the export"
                } else {
                    &value.note
                }
            );
        }
        return;
    }
    if let Some(value) = draft.values.iter_mut().find(|v| v.key == "issue_number") {
        value.text.clear();
        value.source = None;
        value.accepted = false;
        value.note = "printed on your cover".into();
    }
}

/// A cover that marks no later printing is a first printing, as the
/// owner records it, unless the research found otherwise.
fn first_printing(draft: &mut Draft, reading: &Reading) {
    if draft.identity != Identity::Identified || reading.get("printing").is_some() {
        return;
    }
    if let Some(value) = draft
        .values
        .iter_mut()
        .find(|v| v.key == "printing" && v.text.is_empty())
    {
        value.text = "1".into();
        value.source = None;
        value.confidence = Confidence::Medium;
        value.accepted = true;
        value.note = "your cover marks no later printing".into();
    }
}

/// Holds the barcode to the one read from the cover, whose check digit
/// shows it was read right: a different one found online is doubted,
/// and a missing one is taken from the cover.
fn compare_barcodes(draft: &mut Draft, reading: &Reading) {
    if draft.identity != Identity::Identified {
        return;
    }
    let read = reading.barcode();
    let cover_source = draft.sources.iter().position(|s| {
        s.url == COVER_PHOTO
            && reading
                .get("barcode")
                .is_some_and(|b| b.text == s.cited_text)
    });
    let Some(value) = draft.values.iter_mut().find(|v| v.key == "barcode") else {
        return;
    };
    match read {
        Some(read) if value.text.is_empty() => {
            value.text = read.as_str().to_owned();
            value.source = cover_source;
            value.confidence = Confidence::High;
            value.accepted = true;
            value.note.clear();
        }
        Some(read) if value.text != read.as_str() => {
            value.accepted = false;
            value.note = format!("differs from the barcode on your cover, {}", read.as_str());
        }
        Some(_) => {}
        None if reading.get("barcode").is_some() && value.text.is_empty() => {
            value.note = "the digits read from your cover fail the check digit".into();
        }
        None => {}
    }
}

/// The year until which direct copies of US comics marked their barcode
/// box, so that a plain barcode meant a newsstand copy.
const MARKED_DIRECT_UNTIL: u16 = 2000;

/// Direct or newsstand, as the barcode box the owner's cover shows tells
/// it, where the lookup left it blank or said otherwise: a box that names
/// its edition says which, and before about 2000 a US comic with a plain
/// barcode was a newsstand copy, as direct copies marked the box.
fn distribution_from_box(draft: &mut Draft, reading: &Reading) {
    if draft.identity != Identity::Identified {
        return;
    }
    let Some(seen) = reading.get("barcode_box") else {
        return;
    };
    let shown = seen.text.trim().trim_end_matches('.').to_lowercase();
    let year = draft
        .values
        .iter()
        .filter(|v| matches!(v.key, "cover_date" | "release_date"))
        .find_map(|v| v.text.get(..4)?.parse::<u16>().ok());
    let distribution = if shown.contains("direct") {
        "direct"
    } else if shown.contains("newsstand")
        || (shown == "plain barcode" && year.is_some_and(|y| y < MARKED_DIRECT_UNTIL))
    {
        "newsstand"
    } else {
        return;
    };
    let source = draft
        .sources
        .iter()
        .position(|s| s.url == COVER_PHOTO && s.cited_text == seen.text);
    let Some(value) = draft.values.iter_mut().find(|v| v.key == "distribution") else {
        return;
    };
    if value.text.is_empty() {
        value.text = distribution.into();
        value.source = source;
        value.confidence = Confidence::High;
        value.accepted = false;
        value.note = "from the barcode box on your cover".into();
    } else if value.text != distribution {
        value.accepted = false;
        value.note = format!("your cover's barcode box suggests {distribution}");
    }
}

/// The characters an issue is known for: where it has first
/// appearances, those alone, as the collection keeps them, not the
/// title character beside them.
fn known_for(draft: &mut Draft) {
    let first = |e: &Entry| e.detail == Detail::FirstAppearance(true);
    if draft.characters.iter().any(first) {
        draft.characters.retain(first);
    }
}

/// Characters and teams by the names they go by, as the collection
/// keeps them: Elektra, not Elektra (Elektra Natchios). Applied after the
/// check, which compares names with their sources' own words.
fn known_names(draft: &mut Draft) {
    for list in [&mut draft.characters, &mut draft.teams] {
        for entry in list.iter_mut() {
            if let Some(open) = entry.name.find(" (")
                && entry.name.ends_with(')')
                && open > 0
            {
                entry.name.truncate(open);
            }
        }
        let mut seen = Vec::new();
        list.retain(|e| {
            let fresh = !seen.contains(&e.name);
            if fresh {
                seen.push(e.name.clone());
            }
            fresh
        });
    }
}

/// The steps after reading: research, structure, check. `photo` is what
/// the covers showed and the front cover, for a photo lookup.
/// `copy_of`, for a further copy of an issue already looked up, limits
/// the research to which edition the covers show.
#[allow(clippy::too_many_arguments)]
fn look_up_from(
    provider: &dyn Provider,
    catalog: Option<&dyn Catalog>,
    query: &BookQuery,
    photo: Option<(&Reading, &Jpeg)>,
    copy_of: Option<&Draft>,
    settings: &Settings,
    cancel: &AtomicBool,
    spent: &mut Spent,
) -> Result<Draft, LookupError> {
    let (reading, front) = (photo.map(|p| p.0), photo.map(|p| p.1));
    let from_text = reading.is_none();
    let (edition, cover) = match reading {
        None => (prompts::TEXT_EDITION, String::new()),
        Some(r) => (prompts::PHOTO_EDITION, format!("\n{}", r.describe())),
    };
    let records = catalogued(catalog, query, reading);
    let catalogue_rule = if records.is_empty() {
        ""
    } else {
        prompts::CATALOGUE
    };
    let (report, known, max_searches, max_pages) = match copy_of {
        None => (
            format!(
                "{}{}and its credits, characters, teams, and story arcs, each with the page that says so, marked [knowledge], or marked [GCD 1] and so on.\n",
                prompts::REPORT,
                prompts::field_list(from_text)
            ),
            String::new(),
            settings.max_searches,
            settings.max_pages,
        ),
        Some(known) => (
            format!(
                "{}{}{}",
                prompts::COPY,
                prompts::REPORT,
                prompts::copy_field_list()
            ),
            format!("\n{}", known_issue(known, settings.this_year)),
            settings.copy_searches,
            settings.copy_pages,
        ),
    };
    let research = reply(
        provider.research(
            &ResearchRequest {
                system: format!("{}{catalogue_rule}{}{report}", prompts::RESEARCH, edition,),
                images: front.into_iter().cloned().collect(),
                user: format!(
                    "{}{cover}{}{known}",
                    describe(query, reading),
                    records_for_research(&records)
                ),
                max_searches,
                max_pages,
                effort: settings.effort,
                max_tokens: RESEARCH_TOKENS,
            },
            cancel,
        ),
        spent,
    )?;
    let research = cleaned(research);
    if research.sources.is_empty() && records.is_empty() {
        return Ok(Draft {
            query: query.clone(),
            identity: Identity::NoSources,
            sources: Vec::new(),
            values: Vec::new(),
            credits: Vec::new(),
            characters: Vec::new(),
            teams: Vec::new(),
            story_arcs: Vec::new(),
            research: clean(&research.text, 20_000),
        });
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(LookupError::Failed(Failure::Cancelled));
    }
    // The research's pages keep the first numbers, which its notes mark
    // their passages with; the catalogue copy's records follow, then the
    // covers' items, then the model's own knowledge.
    let research = Research {
        sources: research
            .sources
            .into_iter()
            .chain(records)
            .chain(reading.map(Reading::sources).unwrap_or_default())
            .chain(std::iter::once(knowledge()))
            .collect(),
        text: research.text,
    };
    let json = reply(
        provider.answer(
            &SchemaRequest {
                name: "book",
                system: format!("{}{}", prompts::STRUCTURE, prompts::field_list(from_text)),
                images: Vec::new(),
                user: format!("{}\n{}", describe(query, reading), notes(&research)),
                schema: schema::structure(from_text),
                effort: settings.effort,
                max_tokens: STRUCTURE_TOKENS,
            },
            cancel,
        ),
        spent,
    )?;
    let mut draft = read_structured(&json, &research, query, settings, from_text)?;
    if draft.identity != Identity::Identified {
        return Ok(draft);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(LookupError::Failed(Failure::Cancelled));
    }
    let checked = provider.answer(
        &SchemaRequest {
            name: "check",
            system: prompts::CHECK.to_owned(),
            images: Vec::new(),
            user: format!(
                "{}\n{}\n{}",
                describe(query, reading),
                checked_values(&draft),
                notes(&research)
            ),
            schema: schema::check(),
            effort: settings.effort,
            max_tokens: CHECK_TOKENS,
        },
        cancel,
    );
    spent.add(checked.spent);
    let checked = match checked.answer {
        Ok(json) => serde_json::from_str::<Checked>(&json).map_err(|e| e.to_string()),
        Err(Failure::Cancelled) => return Err(LookupError::Failed(Failure::Cancelled)),
        Err(failure) => Err(effort_note(&failure)),
    };
    match checked {
        Ok(checked) => apply_check(&mut draft, checked),
        // Without a check, nothing is taken on trust: every value waits
        // for the owner.
        Err(why) => {
            for value in draft.values.iter_mut().filter(|v| !v.text.is_empty()) {
                value.accepted = false;
                value.note = format!("not checked ({why})");
            }
            for list in [
                &mut draft.credits,
                &mut draft.characters,
                &mut draft.teams,
                &mut draft.story_arcs,
            ] {
                for entry in list.iter_mut() {
                    entry.accepted = false;
                }
            }
        }
    }
    for value in &mut draft.values {
        value.text = fields::house_style(value.key, &value.text);
    }
    known_names(&mut draft);
    known_for(&mut draft);
    annual_series(&mut draft);
    if from_text {
        sale_month(&mut draft);
        given_volume(&mut draft);
    }
    year_only_date(&mut draft);
    titled_year(&mut draft);
    judgments(&mut draft);
    Ok(draft)
}

/// The issue a further copy is of, as the research is told it.
fn known_issue(known: &Draft, this_year: u16) -> String {
    let Some(book) = known.book(this_year) else {
        return String::new();
    };
    let mut text = format!(
        "The owner has already catalogued this issue from another copy: {}",
        book.series.name
    );
    if let Some(year) = book.series.start_year {
        text.push_str(&format!(" ({year})"));
    }
    if let Some(volume) = book.series.volume {
        text.push_str(&format!(" Vol. {volume}"));
    }
    if let Some(number) = &book.issue.number {
        text.push_str(&format!(" #{}", number.as_str()));
    }
    if !book.series.publisher.is_empty() {
        text.push_str(&format!(", {}", book.series.publisher));
    }
    text.push('\n');
    if let Some(variant) = known.value("variant").filter(|v| !v.text.is_empty()) {
        text.push_str(&format!(
            "That copy's variant is {}. Where this copy's cover differs from it, \
             its variant has another name.\n",
            variant.text
        ));
    }
    text
}

/// A volume the owner gave, typed as in Batman Vol. 2 #1 or with the
/// series they picked from the candidates, is theirs. Where the lookup
/// found none, it becomes the book's volume, shown and ticked as their
/// own value, so unticking it leaves it out; where the lookup found
/// another, that waits for the owner. A volume read from a cover is not
/// one the owner gave.
fn given_volume(draft: &mut Draft) {
    let Some(typed) = draft.query.volume else {
        return;
    };
    let Some(value) = draft.values.iter_mut().find(|v| v.key == "volume") else {
        return;
    };
    if value.text.is_empty() {
        value.text = typed.to_string();
        value.source = None;
        value.confidence = Confidence::High;
        value.accepted = true;
        value.note = "the volume you gave".into();
    } else if value.text != typed.to_string() {
        value.accepted = false;
        value.note = format!("you gave Vol. {typed}");
    }
}

/// The collection dates books by month. A cover date of only a year is
/// usually a month the sources left out, not a book without one, so the
/// owner looks before it is exported.
fn year_only_date(draft: &mut Draft) {
    if let Some(value) = draft
        .values
        .iter_mut()
        .find(|v| v.key == "cover_date" && v.text.len() == 4)
    {
        value.accepted = false;
        if value.note.is_empty() {
            value.note = "only the year was found: add the month if the book gives one".into();
        }
    }
}

/// What the collection records by judgment waits for the owner: a role
/// the sources credit to several people, where the collection keeps the
/// main one, the editor, and the characters, of whom it keeps the
/// notable ones. A book numbered as a preview or promotion, such as 1/2
/// or 0, waits for the owner's word on its series and start year too,
/// which the sources often place differently.
fn judgments(draft: &mut Draft) {
    let roles: Vec<Role> = draft
        .credits
        .iter()
        .filter_map(|c| match c.detail {
            Detail::Role(role) => Some(role),
            _ => None,
        })
        .collect();
    for credit in &mut draft.credits {
        let Detail::Role(role) = credit.detail else {
            continue;
        };
        if roles.iter().filter(|r| **r == role).count() > 1 {
            credit.accepted = false;
            if credit.note.is_empty() {
                credit.note = "one of several in this role: keep the main credit".into();
            }
        } else if role == Role::Editor {
            // Sources name editors loosely: an issue's own, its group's,
            // or an assistant, and collections keep them differently.
            credit.accepted = false;
            if credit.note.is_empty() {
                credit.note = "check the editor your collection records".into();
            }
        }
    }
    // Which characters to keep is the owner's choice every time, so each
    // waits for them; a note saying so on every one would only be noise.
    for character in &mut draft.characters {
        character.accepted = false;
    }
    let number = draft.query.number.as_str();
    if number.contains(['/', '½']) || number == "0" {
        for value in draft
            .values
            .iter_mut()
            .filter(|v| matches!(v.key, "series" | "start_year") && !v.text.is_empty())
        {
            value.accepted = false;
            if value.note.is_empty() {
                value.note =
                    "a preview or promotional issue: check where your collection files it".into();
            }
        }
    }
}

/// A series titled with its year, as Free Comic Book Day 2024 is, is
/// named without it in the collection, whose start year holds it.
fn titled_year(draft: &mut Draft) {
    let Some(year) = draft
        .values
        .iter()
        .find(|v| v.key == "start_year" && v.text.len() == 4)
        .map(|v| v.text.clone())
    else {
        return;
    };
    let Some(series) = draft.values.iter_mut().find(|v| v.key == "series") else {
        return;
    };
    let mut words: Vec<String> = Vec::new();
    for word in series.text.split_whitespace() {
        match word.strip_prefix(year.as_str()) {
            Some(rest) if rest.is_empty() || rest == ":" || rest == "," => {
                if let Some(last) = words.last_mut() {
                    last.push_str(rest);
                } else {
                    words.push(word.to_owned());
                }
            }
            _ => words.push(word.to_owned()),
        }
    }
    let named = words.join(" ");
    if named.chars().any(char::is_alphabetic) {
        series.text = named;
    }
}

/// An annual is its own series in the collection: Amazing Spider-Man
/// Annual #1, not Amazing Spider-Man #Annual 1.
fn annual_series(draft: &mut Draft) {
    fn rest(number: &str) -> Option<&str> {
        let lower = number.to_ascii_lowercase();
        let rest = lower.strip_prefix("annual")?;
        let rest = number[number.len() - rest.len()..].trim();
        (!rest.is_empty()).then_some(rest)
    }
    fn annual(series: &str) -> String {
        if series.to_ascii_lowercase().ends_with(" annual") {
            series.to_owned()
        } else {
            format!("{series} Annual")
        }
    }
    let found = draft
        .values
        .iter()
        .find(|v| v.key == "issue_number")
        .map(|v| v.text.clone())
        .unwrap_or_default();
    let printed = rest(draft.query.number.as_str())
        .and_then(|r| crate::issue_number::IssueNumber::new(r).ok());
    if let Some(number) = printed {
        draft.query.number = number;
    } else if let Some(r) = rest(&found).map(str::to_owned) {
        if let Some(v) = draft.values.iter_mut().find(|v| v.key == "issue_number") {
            v.text = r;
        }
    } else {
        return;
    }
    draft.query.series = annual(&draft.query.series);
    if let Some(v) = draft
        .values
        .iter_mut()
        .find(|v| v.key == "series" && !v.text.is_empty())
    {
        v.text = annual(&v.text);
    }
}
