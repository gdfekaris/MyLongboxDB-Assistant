//! Looked-up books kept until they are exported, as they cost money to
//! look up again. The file holds each book's draft, whether it is
//! included, whether the owner has reviewed it, and the grade they gave
//! their copy, as JSON, never a key. It is read back as strictly as a
//! model's answer: every value is checked again by its field's rules, so
//! a damaged or altered file cannot put into an export what a lookup
//! could not. The owner's photos of each book are kept beside it, one
//! JPEG a file, each written once.

use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::issue_number::IssueNumber;
use crate::lines::BookQuery;
use crate::lookup::{Candidate, Detail, Draft, Entry, Identity, Source, Value, check, spec};
use crate::model::{Confidence, CopyInfo, Grade, GradeType, Jpeg, Role};

/// The format written; a file of another one is not read.
const VERSION: u32 = 1;
/// A results file larger than this is not read.
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// The most books a results file may hold.
const MAX_BOOKS: usize = 10_000;
/// The longest text kept in any one place.
const MAX_TEXT: usize = 20_000;
/// The longest certificate number kept; graders print ten digits or so.
const MAX_CERT: usize = 40;

/// A certificate number as the owner typed it, made safe to keep: hidden
/// characters dropped, spacing tidied, and its length capped.
pub fn cert_number(text: &str) -> String {
    crate::lookup::clean(text, MAX_CERT)
}
/// A kept photo larger than this is not read. The app keeps them at
/// 1,600 pixels a side, far smaller.
const MAX_PHOTO_BYTES: u64 = 16 * 1024 * 1024;

/// One of the owner's photos of a book, under the name it is kept by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Photo {
    name: String,
    pub jpeg: Jpeg,
}

impl Photo {
    /// The name it is kept by, the same for as long as it is kept.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// A photo not yet kept, given a name of its own.
    pub fn new(jpeg: Jpeg) -> Self {
        Self {
            name: uuid::Uuid::new_v4().simple().to_string(),
            jpeg,
        }
    }
}

/// A looked-up book, the owner's photos of it, whether the owner
/// includes it in the export, and whether they have reviewed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewBook {
    pub draft: Draft,
    pub included: bool,
    /// Marked by the owner once they have looked the book over, to keep
    /// track of a long review; the export does not depend on it.
    pub reviewed: bool,
    /// The owner's own grade of their copy, typed on the review page.
    pub copy: CopyInfo,
    pub front: Option<Photo>,
    pub back: Option<Photo>,
}

impl ReviewBook {
    /// A new result: included if the lookup identified the issue.
    pub fn new(draft: Draft) -> Self {
        let included = draft.identity == Identity::Identified;
        Self {
            draft,
            included,
            reviewed: false,
            copy: CopyInfo::default(),
            front: None,
            back: None,
        }
    }

    /// The book as it goes into the export: what the owner accepted, with
    /// their photos as its covers. `None` for a book not identified.
    pub fn book(&self, this_year: u16) -> Option<crate::model::Book> {
        let mut book = self.draft.book(this_year)?;
        // A collected edition has a condition in MyLongboxDB, not a grade,
        // and only a grading company issues a certificate.
        if book.category != crate::model::Category::CollectedEdition {
            book.copy = CopyInfo {
                cert_number: if self.copy.grade_type.is_some_and(GradeType::is_company) {
                    self.copy.cert_number.clone()
                } else {
                    String::new()
                },
                ..self.copy.clone()
            };
        }
        book.front = self.front.as_ref().map(|p| p.jpeg.clone());
        book.back = self.back.as_ref().map(|p| p.jpeg.clone());
        Some(book)
    }

    /// A new result looked up from these photos.
    pub fn with_photos(draft: Draft, front: Photo, back: Option<Photo>) -> Self {
        Self {
            front: Some(front),
            back,
            ..Self::new(draft)
        }
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
    /// Files from before books could be marked reviewed have none.
    #[serde(default)]
    reviewed: bool,
    /// Files from before copies could be graded have none of these.
    #[serde(default)]
    grade: Option<String>,
    #[serde(default)]
    grade_type: Option<String>,
    #[serde(default)]
    cert_number: String,
    /// The names the book's photos are kept by; files from before photos
    /// were kept have none.
    #[serde(default)]
    front: Option<String>,
    #[serde(default)]
    back: Option<String>,
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
                reviewed: b.reviewed,
                grade: b.copy.grade.map(|g| g.to_string()),
                grade_type: b.copy.grade_type.map(|g| g.code().to_owned()),
                cert_number: b.copy.cert_number.clone(),
                front: b.front.as_ref().map(|p| p.name.clone()),
                back: b.back.as_ref().map(|p| p.name.clone()),
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

fn book(saved: Saved, this_year: u16, photo: &dyn Fn(&str) -> Option<Photo>) -> Option<ReviewBook> {
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
        reviewed: saved.reviewed,
        // Checked as the review page checks what the owner types; a value
        // that does not read is dropped.
        copy: CopyInfo {
            grade: saved.grade.as_deref().and_then(Grade::parse),
            grade_type: saved.grade_type.as_deref().and_then(GradeType::from_code),
            cert_number: cert_number(&saved.cert_number),
        },
        front: saved.front.as_deref().and_then(photo),
        back: saved.back.as_deref().and_then(photo),
    })
}

/// The books in a results file's text, without their photos. Books that
/// cannot be read are left out; a file that cannot be read at all is an
/// error.
pub fn from_json(text: &str, this_year: u16) -> Result<Vec<ReviewBook>, String> {
    read(text, this_year, &|_| None)
}

fn read(
    text: &str,
    this_year: u16,
    photo: &dyn Fn(&str) -> Option<Photo>,
) -> Result<Vec<ReviewBook>, String> {
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
        .filter_map(|b| book(b, this_year, photo))
        .collect())
}

/// Where the photos of the results at `path` are kept.
fn photo_dir(path: &Path) -> std::path::PathBuf {
    path.with_extension("photos")
}

/// Whether `name` is one [`Photo::new`] could have given, so a name in
/// an altered file cannot reach outside the photo folder.
fn photo_name(name: &str) -> bool {
    name.len() == 32
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A kept photo, if it is there and starts as a JPEG does. It is decoded
/// within limits wherever it is shown, and again by MyLongboxDB.
fn read_photo(dir: &Path, name: &str) -> Option<Photo> {
    if !photo_name(name) {
        return None;
    }
    let file = std::fs::File::open(dir.join(format!("{name}.jpg"))).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_PHOTO_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PHOTO_BYTES {
        return None;
    }
    Some(Photo {
        name: name.to_owned(),
        jpeg: Jpeg::new(bytes)?,
    })
}

/// Writes `bytes` to `path`, readable and writable by the owner only,
/// whole or not at all.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let temp = path.with_extension("tmp");
    let written = (|| {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        out.write_all(bytes)?;
        out.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Writes the photos not yet kept; a kept photo never changes.
fn save_photos(dir: &Path, books: &[ReviewBook]) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let photos: Vec<&Photo> = books
        .iter()
        .flat_map(|b| b.front.iter().chain(b.back.iter()))
        .collect();
    if photos.is_empty() {
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    for photo in photos {
        let path = dir.join(format!("{}.jpg", photo.name));
        if !path.exists() {
            write_private(&path, photo.jpeg.bytes())?;
        }
    }
    Ok(())
}

/// Removes the kept photos no book has any more.
fn forget_photos(dir: &Path, books: &[ReviewBook]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let kept = file
            .to_str()
            .and_then(|f| f.strip_suffix(".jpg"))
            .is_some_and(|name| {
                books.iter().any(|b| {
                    [&b.front, &b.back]
                        .into_iter()
                        .flatten()
                        .any(|p| p.name == name)
                })
            });
        if !kept {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// Writes the results at `path`, readable and writable by the owner only,
/// whole or not at all, and their photos beside it. With no books, the
/// file and photos are removed.
pub fn save(path: &Path, books: &[ReviewBook]) -> std::io::Result<()> {
    let photos = photo_dir(path);
    if books.is_empty() {
        let removed = match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
        forget_photos(&photos, books);
        return removed;
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    // Photos first, so the file never names one that is not there; the
    // ones it no longer names go only once it is written.
    save_photos(&photos, books)?;
    write_private(path, to_json(books).as_bytes())?;
    forget_photos(&photos, books);
    Ok(())
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
    let photos = photo_dir(path);
    read(&text, this_year, &|name| read_photo(&photos, name))
}

#[cfg(test)]
mod tests;
