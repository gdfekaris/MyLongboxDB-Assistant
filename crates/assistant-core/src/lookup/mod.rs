//! Looking a book up, in steps the app controls rather than in one
//! open-ended prompt:
//!
//! 0. Read, for photos: the model reports what the covers print, and
//!    that becomes the book to look up and sources the later steps can
//!    cite.
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

use crate::cost::{Estimate, Planned, Price, Usage, image_tokens};
use crate::lines::BookQuery;
use crate::model::{Category, Confidence, Jpeg, Role};
use crate::money::Picodollars;
use draft::fits_category;
pub(crate) use fields::clean;
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
    pub effort: Effort,
    /// The current year, which bounds the dates a value may have.
    pub this_year: u16,
}

impl Settings {
    /// The owner's choice of 2026-10-24: up to five searches a book, at
    /// medium effort.
    pub fn balanced(this_year: u16) -> Self {
        Self {
            max_searches: 5,
            effort: Effort::Medium,
            this_year,
        }
    }
}

/// The most a step may write. Thinking counts toward it.
const READ_TOKENS: u32 = 4_000;
const RESEARCH_TOKENS: u32 = 16_000;
const STRUCTURE_TOKENS: u32 = 16_000;
const CHECK_TOKENS: u32 = 8_000;

/// What each step is expected to use, for the estimate before a run.
/// Thinking counts as output, and each search's results become input.
/// The research's figures are from the live trial of 2026-10-24 on Opus
/// 5.5, which read about 77,000 tokens over 3 searches a book, rounded
/// up; the others' are first guesses that it found close.
pub fn planned(settings: &Settings) -> [Planned; 3] {
    let searches = u64::from(settings.max_searches);
    [
        Planned {
            input_tokens: 4_000,
            likely_output_tokens: 4_000,
            most_output_tokens: u64::from(RESEARCH_TOKENS),
            likely_searches: searches.min(3),
            most_searches: searches,
            tokens_per_search: 25_000,
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

/// The research notes and numbered sources, for the later steps.
fn notes(research: &Research) -> String {
    let mut text = format!("Research notes:\n<<<\n{}\n>>>\n\nSources:\n", research.text);
    for (n, source) in research.sources.iter().enumerate() {
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
    detail: impl Fn(T) -> Option<Detail>,
) -> Vec<Entry> {
    named
        .into_iter()
        .filter_map(|n| {
            let name = clean(&n.name, MAX_NAME);
            // A name with no cited source is not kept.
            let source = source_index(n.source, sources)?;
            let detail = detail(n.detail)?;
            if name.is_empty() {
                return None;
            }
            let confidence = Confidence::from(n.confidence);
            Some(Entry {
                name,
                detail,
                source: Some(source),
                confidence,
                note: String::new(),
                accepted: confidence != Confidence::Low,
            })
        })
        .collect()
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
    match answer.identification.status {
        Status::Identified => {}
        Status::NotFound => {
            draft.identity = Identity::NotFound;
            return Ok(draft);
        }
        Status::Ambiguous => {
            let candidates = answer
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
                            value.accepted = !field.judgment && value.confidence != Confidence::Low;
                        }
                    },
                }
            }
        }
        draft.values.push(value);
    }
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
    draft.credits = entries(answer.credits, sources, |role: String| {
        Role::from_code(&role).map(Detail::Role)
    });
    draft.characters = entries(answer.characters, sources, |first| {
        Some(Detail::FirstAppearance(first))
    });
    draft.teams = entries(answer.teams, sources, |first| {
        Some(Detail::FirstAppearance(first))
    });
    draft.story_arcs = entries(answer.story_arcs, sources, |part: String| {
        let part = part.trim();
        if part.is_empty() {
            return Some(Detail::Part(None));
        }
        part.parse::<u32>()
            .ok()
            .filter(|p| *p >= 1)
            .map(|p| Detail::Part(Some(p)))
    });
    Ok(draft)
}

/// The values the check step is shown, each with a key it can answer by.
fn checked_values(draft: &Draft) -> String {
    let source = |s: Option<usize>| match s {
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
    query: &BookQuery,
    settings: &Settings,
    cancel: &AtomicBool,
    spent: &mut Spent,
) -> Result<Draft, LookupError> {
    look_up_from(provider, query, None, None, settings, cancel, spent)
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
    covers: &Covers,
    settings: &Settings,
    cancel: &AtomicBool,
    spent: &mut Spent,
) -> Result<Draft, LookupError> {
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
    let query = match &covers.chosen {
        Some(chosen) => chosen.clone(),
        None => reading
            .query(covers.line)
            .map_err(LookupError::NoBookOnCover)?,
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(LookupError::Failed(Failure::Cancelled));
    }
    let mut draft = look_up_from(
        provider,
        &query,
        Some(&reading),
        Some(&covers.front),
        settings,
        cancel,
        spent,
    )?;
    compare_barcodes(&mut draft, &reading);
    first_printing(&mut draft, &reading);
    printed_number(&mut draft, &reading);
    undated_cover(&mut draft, &reading);
    variant_name(&mut draft);
    Ok(draft)
}

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
            value.note = "name it as your collection names variants".into();
        }
    }
}

/// A book dated nowhere, on its cover, in its indicia, or in the
/// catalogue, as many modern ones are, is dated by the month it was
/// released, as the owner records it. A date the catalogue gives is kept:
/// it is the one the indicia prints, or the month it supplies.
fn undated_cover(draft: &mut Draft, reading: &Reading) {
    if draft.identity != Identity::Identified || reading.get("cover_date").is_some() {
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
    if let Some(value) = draft
        .values
        .iter_mut()
        .find(|v| v.key == "cover_date" && v.text.is_empty())
    {
        value.text = release.text[..7].to_owned();
        value.source = release.source;
        value.confidence = release.confidence;
        value.accepted = release.accepted;
        value.note = "your cover prints no date: the month it was released".into();
    }
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

/// The steps after reading: research, structure, check. `reading` is
/// what the covers showed, for a photo lookup.
fn look_up_from(
    provider: &dyn Provider,
    query: &BookQuery,
    reading: Option<&Reading>,
    front: Option<&Jpeg>,
    settings: &Settings,
    cancel: &AtomicBool,
    spent: &mut Spent,
) -> Result<Draft, LookupError> {
    let from_text = reading.is_none();
    let (edition, cover) = match reading {
        None => (prompts::TEXT_EDITION, String::new()),
        Some(r) => (prompts::PHOTO_EDITION, format!("\n{}", r.describe())),
    };
    let research = reply(
        provider.research(
            &ResearchRequest {
                system: format!(
                    "{}{}{}{}and its credits, characters, teams, and story arcs, each with the page that says so.\n",
                    prompts::RESEARCH,
                    edition,
                    prompts::REPORT,
                    prompts::field_list(from_text)
                ),
                images: front.into_iter().cloned().collect(),
                user: format!("{}{cover}", describe(query, reading)),
                max_searches: settings.max_searches,
                effort: settings.effort,
                max_tokens: RESEARCH_TOKENS,
            },
            cancel,
        ),
        spent,
    )?;
    if research.sources.is_empty() {
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
    // their passages with; the covers' items follow.
    let research = match reading {
        None => research,
        Some(r) => Research {
            sources: research.sources.into_iter().chain(r.sources()).collect(),
            text: research.text,
        },
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
    annual_series(&mut draft);
    year_only_date(&mut draft);
    Ok(draft)
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
