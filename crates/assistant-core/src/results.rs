//! Looked-up books kept until they are exported, as they cost money to
//! look up again. The file holds each book's draft and whether it is
//! included, as JSON, never a key. It is read back as strictly as a
//! model's answer: every value is checked again by its field's rules, so
//! a damaged or altered file cannot put into an export what a lookup
//! could not.

use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::issue_number::IssueNumber;
use crate::lines::BookQuery;
use crate::lookup::{Candidate, Detail, Draft, Entry, Identity, Source, Value, check, spec};
use crate::model::{Confidence, Role};

/// The format written; a file of another one is not read.
const VERSION: u32 = 1;
/// A results file larger than this is not read.
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// The most books a results file may hold.
const MAX_BOOKS: usize = 10_000;
/// The longest text kept in any one place.
const MAX_TEXT: usize = 20_000;

/// A looked-up book and whether the owner includes it in the export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewBook {
    pub draft: Draft,
    pub included: bool,
}

impl ReviewBook {
    /// A new result: included if the lookup identified the issue.
    pub fn new(draft: Draft) -> Self {
        let included = draft.identity == Identity::Identified;
        Self { draft, included }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    books: Vec<Saved>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    included: bool,
    line: usize,
    series: String,
    start_year: Option<u16>,
    volume: Option<u32>,
    number: String,
    identity: String,
    candidates: Vec<SavedCandidate>,
    sources: Vec<SavedSource>,
    values: Vec<SavedValue>,
    credits: Vec<SavedEntry>,
    characters: Vec<SavedEntry>,
    teams: Vec<SavedEntry>,
    story_arcs: Vec<SavedEntry>,
    research: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedCandidate {
    series: String,
    start_year: Option<u16>,
    volume: Option<u32>,
    publisher: String,
    why: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedSource {
    url: String,
    title: String,
    cited_text: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedValue {
    key: String,
    text: String,
    source: Option<usize>,
    confidence: String,
    note: String,
    accepted: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedEntry {
    name: String,
    /// A credit's role code, a character's or team's first appearance,
    /// or an arc's part, as its list has.
    role: Option<String>,
    first: Option<bool>,
    part: Option<u32>,
    source: Option<usize>,
    confidence: String,
    note: String,
    accepted: bool,
}

fn confidence_code(c: Confidence) -> &'static str {
    match c {
        Confidence::High => "high",
        Confidence::Medium => "medium",
        Confidence::Low => "low",
    }
}

fn confidence(code: &str) -> Confidence {
    match code {
        "high" => Confidence::High,
        "medium" => Confidence::Medium,
        _ => Confidence::Low,
    }
}

fn saved_entry(e: &Entry) -> SavedEntry {
    let (role, first, part) = match e.detail {
        Detail::Role(r) => (Some(r.code().to_owned()), None, None),
        Detail::FirstAppearance(f) => (None, Some(f), None),
        Detail::Part(p) => (None, None, p),
    };
    SavedEntry {
        name: e.name.clone(),
        role,
        first,
        part,
        source: e.source,
        confidence: confidence_code(e.confidence).into(),
        note: e.note.clone(),
        accepted: e.accepted,
    }
}

/// The books as the results file's text.
pub fn to_json(books: &[ReviewBook]) -> String {
    let saved = books
        .iter()
        .map(|b| {
            let d = &b.draft;
            let (identity, candidates) = match &d.identity {
                Identity::Identified => ("identified", Vec::new()),
                Identity::Ambiguous(c) => ("ambiguous", c.clone()),
                Identity::NotFound => ("not_found", Vec::new()),
                Identity::NoSources => ("no_sources", Vec::new()),
            };
            Saved {
                included: b.included,
                line: d.query.line,
                series: d.query.series.clone(),
                start_year: d.query.start_year,
                volume: d.query.volume,
                number: d.query.number.as_str().to_owned(),
                identity: identity.into(),
                candidates: candidates
                    .into_iter()
                    .map(|c| SavedCandidate {
                        series: c.series,
                        start_year: c.start_year,
                        volume: c.volume,
                        publisher: c.publisher,
                        why: c.why,
                    })
                    .collect(),
                sources: d
                    .sources
                    .iter()
                    .map(|s| SavedSource {
                        url: s.url.clone(),
                        title: s.title.clone(),
                        cited_text: s.cited_text.clone(),
                    })
                    .collect(),
                values: d
                    .values
                    .iter()
                    .map(|v| SavedValue {
                        key: v.key.into(),
                        text: v.text.clone(),
                        source: v.source,
                        confidence: confidence_code(v.confidence).into(),
                        note: v.note.clone(),
                        accepted: v.accepted,
                    })
                    .collect(),
                credits: d.credits.iter().map(saved_entry).collect(),
                characters: d.characters.iter().map(saved_entry).collect(),
                teams: d.teams.iter().map(saved_entry).collect(),
                story_arcs: d.story_arcs.iter().map(saved_entry).collect(),
                research: d.research.clone(),
            }
        })
        .collect();
    serde_json::to_string(&File {
        version: VERSION,
        books: saved,
    })
    .unwrap_or_default()
}

/// Text from the file made safe to show, as model text is.
fn clean(text: &str) -> String {
    crate::lookup::clean(text, MAX_TEXT)
}

fn entries(
    saved: Vec<SavedEntry>,
    sources: usize,
    detail: impl Fn(&SavedEntry) -> Option<Detail>,
) -> Vec<Entry> {
    saved
        .into_iter()
        .filter_map(|e| {
            let detail = detail(&e)?;
            let name = clean(&e.name);
            (!name.is_empty()).then(|| Entry {
                name,
                detail,
                source: e.source.filter(|s| *s < sources),
                confidence: confidence(&e.confidence),
                note: clean(&e.note),
                accepted: e.accepted,
            })
        })
        .collect()
}

fn book(saved: Saved, this_year: u16) -> Option<ReviewBook> {
    let number = IssueNumber::new(&saved.number).ok()?;
    let series = clean(&saved.series);
    if series.is_empty() {
        return None;
    }
    let sources: Vec<Source> = saved
        .sources
        .into_iter()
        .map(|s| Source {
            url: clean(&s.url),
            title: clean(&s.title),
            cited_text: clean(&s.cited_text),
        })
        .collect();
    let identity = match saved.identity.as_str() {
        "identified" => Identity::Identified,
        "ambiguous" => Identity::Ambiguous(
            saved
                .candidates
                .into_iter()
                .map(|c| Candidate {
                    series: clean(&c.series),
                    start_year: c.start_year,
                    volume: c.volume,
                    publisher: clean(&c.publisher),
                    why: clean(&c.why),
                })
                .filter(|c| !c.series.is_empty())
                .collect(),
        ),
        "not_found" => Identity::NotFound,
        "no_sources" => Identity::NoSources,
        _ => return None,
    };
    let mut values = Vec::new();
    for v in saved.values {
        // A key the app does not know, or one already read, is dropped.
        let Some(field) = spec(&v.key) else {
            continue;
        };
        if values.iter().any(|seen: &Value| seen.key == field.key) {
            continue;
        }
        let mut value = Value {
            key: field.key,
            text: String::new(),
            source: v.source.filter(|s| *s < sources.len()),
            confidence: confidence(&v.confidence),
            note: clean(&v.note),
            accepted: v.accepted,
        };
        match check(field.kind, &v.text, this_year) {
            Ok(text) => value.text = text,
            Err(why) => {
                value.note = why;
                value.accepted = false;
            }
        }
        values.push(value);
    }
    let n = sources.len();
    let included = saved.included && identity == Identity::Identified;
    Some(ReviewBook {
        draft: Draft {
            query: BookQuery {
                line: saved.line,
                series,
                start_year: saved.start_year,
                volume: saved.volume,
                number,
            },
            identity,
            values,
            credits: entries(saved.credits, n, |e| {
                Role::from_code(e.role.as_deref()?).map(Detail::Role)
            }),
            characters: entries(saved.characters, n, |e| {
                e.first.map(Detail::FirstAppearance)
            }),
            teams: entries(saved.teams, n, |e| e.first.map(Detail::FirstAppearance)),
            story_arcs: entries(saved.story_arcs, n, |e| {
                Some(Detail::Part(e.part.filter(|p| *p >= 1)))
            }),
            sources,
            research: clean(&saved.research),
        },
        included,
    })
}

/// The books in a results file's text. Books that cannot be read are
/// left out; a file that cannot be read at all is an error.
pub fn from_json(text: &str, this_year: u16) -> Result<Vec<ReviewBook>, String> {
    let file: File = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if file.version != VERSION {
        return Err(format!("results of format {} cannot be read", file.version));
    }
    if file.books.len() > MAX_BOOKS {
        return Err("the results file holds too many books".into());
    }
    Ok(file
        .books
        .into_iter()
        .filter_map(|b| book(b, this_year))
        .collect())
}

/// Writes the results at `path`, readable and writable by the owner only,
/// whole or not at all. With no books, the file is removed.
pub fn save(path: &Path, books: &[ReviewBook]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    if books.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let temp = path.with_extension("tmp");
    let written = (|| {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        out.write_all(to_json(books).as_bytes())?;
        out.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// The results kept at `path`; none if there is no file.
pub fn load(path: &Path, this_year: u16) -> Result<Vec<ReviewBook>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut text = String::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
        return Err("the results file is too large".into());
    }
    from_json(&text, this_year)
}

#[cfg(test)]
mod tests;
