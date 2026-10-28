//! Imports an Assistant export into an empty collection with
//! MyLongboxDB's own import, as the owner's import wizard would, and
//! fails unless every row is added, every photo is stored, and every
//! cell the Assistant wrote comes back out of MyLongboxDB's export as it
//! went in. It then imports the export again into a collection that
//! already has every name it gives, and fails unless each name joins the
//! entry the collection has rather than adding one of its own.

use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::process::ExitCode;

use mylongbox_core::{
    BookEntry, Category, CharacterId, CoverPhoto, CreatorId, Database, Date, ImportOutcome,
    ImportSettings, ImportSource, IssueNumber, LookupId, Role, SeriesChoice, StoryArcId, TeamId,
    split_list,
};

/// Columns whose cells MyLongboxDB fills itself, or writes otherwise.
const SKIPPED: [&str; 4] = ["Date Added", "Front Photo", "Back Photo", "Signature Photo"];

fn prepare(bytes: Vec<u8>) -> Result<CoverPhoto, String> {
    let decoded = mylongbox_media::decode_bytes(&bytes).map_err(|e| e.to_string())?;
    let image = mylongbox_media::upright(decoded);
    let encoded = mylongbox_media::encode(&image, mylongbox_media::Output::default())
        .map_err(|e| e.to_string())?;
    Ok(CoverPhoto {
        full_jpeg: encoded.full,
        thumb_jpeg: encoded.thumbnail,
        width: encoded.width,
        height: encoded.height,
        sha256: encoded.sha256,
    })
}

/// A CSV's rows as maps from column name to cell, keyed by Copy ID.
fn rows(csv: &[u8]) -> Result<HashMap<String, HashMap<String, String>>, String> {
    let mut reader = csv::Reader::from_reader(csv);
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| e.to_string())?
        .iter()
        .map(str::to_owned)
        .collect();
    let mut rows = HashMap::new();
    for record in reader.records() {
        let record = record.map_err(|e| e.to_string())?;
        let row: HashMap<String, String> = headers
            .iter()
            .cloned()
            .zip(record.iter().map(str::to_owned))
            .collect();
        rows.insert(row["Copy ID"].clone(), row);
    }
    Ok(rows)
}

/// Columns holding lists, which MyLongboxDB writes in its own order.
const LISTS: [&str; 16] = [
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
    "Variant Cover Artists",
];

/// The columns naming creators.
const CREATORS: [&str; 11] = [
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
    "Variant Cover Artists",
];

/// The names each list of the collection is given by the rows, as
/// MyLongboxDB's import reads them: creators, characters, teams, and
/// story arcs.
fn named(rows: &HashMap<String, HashMap<String, String>>) -> [BTreeSet<String>; 4] {
    let mut lists: [BTreeSet<String>; 4] = Default::default();
    let mut add = |list: usize, column: &str, row: &HashMap<String, String>| {
        let cell = row.get(column).map_or("", |c| c.trim());
        // The mark export puts before what a spreadsheet would take for
        // a formula, which import takes off.
        let cell = cell
            .strip_prefix('\'')
            .filter(|rest| rest.starts_with(['=', '+', '-', '@', '\t', '\r', '\'']))
            .unwrap_or(cell);
        for item in split_list(cell, ';') {
            let item = match list {
                // An arc's part is not part of its name.
                3 => item
                    .rsplit_once(" (part ")
                    .filter(|(_, n)| n.ends_with(')'))
                    .map_or(item.as_str(), |(name, _)| name)
                    .to_owned(),
                _ => item,
            };
            if !item.trim().is_empty() {
                lists[list].insert(item.trim().to_owned());
            }
        }
    };
    for row in rows.values() {
        for column in CREATORS {
            add(0, column, row);
        }
        add(1, "Characters", row);
        add(2, "Teams", row);
        add(3, "Story Arcs", row);
    }
    lists
}

/// The names of a list, folded as MyLongboxDB compares them.
fn entries<I: LookupId>(db: &Database) -> Result<BTreeSet<String>, String> {
    Ok(db
        .list_lookups::<I>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|l| l.name.to_ascii_lowercase())
        .collect())
}

/// Whether each name the export gives is one name, and joins the entry
/// a collection that already has it keeps. The collection is made as an
/// owner typing one name to a row of MyLongboxDB's form would make it:
/// each name in capitals, so only a match that ignores case finds it,
/// and the first creator only as another creator's alternate name.
fn names_join(
    path: &std::path::Path,
    settings: &ImportSettings,
    written: &HashMap<String, HashMap<String, String>>,
) -> Result<Vec<String>, String> {
    let [creators, characters, teams, arcs] = named(written);
    let mut problems = Vec::new();
    for name in creators.iter().chain(&characters).chain(&teams).chain(&arcs) {
        if name.contains([';', '|']) {
            problems.push(format!("{name:?} is one name holding a separator"));
        }
    }

    let db = Database::open_in_memory().map_err(|e| e.to_string())?;
    let upper = |names: &BTreeSet<String>| -> Vec<String> {
        names.iter().map(|n| n.to_ascii_uppercase()).collect()
    };
    let aliased = creators.iter().next().cloned();
    let mut seed = BookEntry::new(
        Category::Comic,
        SeriesChoice::New {
            name: "Contract Seed".into(),
            start_year: Some(1900),
            volume: None,
            publisher: None,
            imprint: String::new(),
            magazine_format: None,
            black_and_white: None,
        },
        Some(IssueNumber::new("1").map_err(|e| e.to_string())?),
    );
    seed.credits = upper(&creators)
        .into_iter()
        .skip(usize::from(aliased.is_some()))
        .map(|n| (n, Role::Writer))
        .collect();
    seed.credits.push(("Contract Seed Creator".into(), Role::Editor));
    seed.characters = upper(&characters).into_iter().map(|n| (n, false)).collect();
    seed.teams = upper(&teams).into_iter().map(|n| (n, false)).collect();
    seed.story_arcs = upper(&arcs).into_iter().map(|n| (n, None)).collect();
    db.add_book(&seed).map_err(|e| e.to_string())?;
    if let Some(alias) = &aliased {
        let target = db
            .list_lookups::<CreatorId>()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|l| l.name == "Contract Seed Creator")
            .ok_or("the seeded creator is missing")?;
        db.add_alias(target.id, alias).map_err(|e| e.to_string())?;
    }
    let before = [
        entries::<CreatorId>(&db)?,
        entries::<CharacterId>(&db)?,
        entries::<TeamId>(&db)?,
        entries::<StoryArcId>(&db)?,
    ];

    let mut source = ImportSource::open(path).map_err(|e| e.to_string())?;
    let report = db
        .import(&mut source, settings, &prepare)
        .map_err(|e| e.to_string())?;
    for row in &report.rows {
        if row.outcome != ImportOutcome::Added {
            problems.push(format!(
                "into a collection with its names, line {} {}: {:?}",
                row.line, row.label, row.outcome
            ));
        }
    }
    let after = [
        entries::<CreatorId>(&db)?,
        entries::<CharacterId>(&db)?,
        entries::<TeamId>(&db)?,
        entries::<StoryArcId>(&db)?,
    ];
    for ((list, before), after) in ["creator", "character", "team", "story arc"]
        .iter()
        .zip(&before)
        .zip(&after)
    {
        for added in after.difference(before) {
            problems.push(format!(
                "the import added the {list} {added:?}, which the collection already had"
            ));
        }
    }
    Ok(problems)
}

/// A list cell's items, in order, with escapes kept so an escaped
/// separator is not split on.
fn items(cell: &str) -> Vec<String> {
    let mut items = vec![String::new()];
    let mut chars = cell.chars();
    while let Some(c) = chars.next() {
        let item = items.last_mut().expect("never empty");
        if c == '\\' {
            item.push(c);
            item.extend(chars.next());
        } else if c == ';' {
            items.push(String::new());
        } else {
            item.push(c);
        }
    }
    let mut items: Vec<String> = items.iter().map(|i| i.trim().to_owned()).collect();
    items.sort();
    items
}

/// Whether a cell written by the Assistant arrived. Series are matched
/// as MyLongboxDB matches them, ignoring case and spacing; lists may come
/// back in another order; and a "no" for a key issue says nothing an
/// earlier copy's "yes" should not override.
fn arrived(column: &str, written: &str, exported: &str) -> bool {
    let fold = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase()
    };
    match column {
        "Series" => fold(written) == fold(exported),
        "Key Issue" if written == "no" => true,
        c if LISTS.contains(&c) => items(written) == items(exported),
        _ => written == exported,
    }
}

fn check(path: &std::path::Path) -> Result<usize, Vec<String>> {
    let fail = |e: &dyn std::fmt::Display| vec![format!("{}: {e}", path.display())];
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).map_err(|e| fail(&e))?)
        .map_err(|e| fail(&e))?;
    let mut written_csv = Vec::new();
    archive
        .by_name("collection.csv")
        .map_err(|e| fail(&e))?
        .read_to_end(&mut written_csv)
        .map_err(|e| fail(&e))?;
    let written = rows(&written_csv).map_err(|e| fail(&e))?;

    let db = Database::open_in_memory().map_err(|e| fail(&e))?;
    let mut source = ImportSource::open(path).map_err(|e| fail(&e))?;
    if !source.is_own_format() {
        return Err(fail(&"not read as MyLongboxDB's own format"));
    }
    let settings = ImportSettings {
        mapping: mylongbox_core::auto_mapping(source.headers()),
        separator: ';',
        today: Date::new(2026, 1, 1).map_err(|e| fail(&e))?,
        skip_likely_duplicates: false,
    };
    let report = db
        .import(&mut source, &settings, &prepare)
        .map_err(|e| fail(&e))?;

    let mut problems = Vec::new();
    for row in &report.rows {
        if row.outcome != ImportOutcome::Added {
            problems.push(format!(
                "line {} {}: {:?}",
                row.line, row.label, row.outcome
            ));
        }
    }
    problems.extend(report.photo_problems.iter().map(|p| format!("photo {p}")));
    if report.rows.len() != written.len() {
        problems.push(format!(
            "{} rows written, {} read",
            written.len(),
            report.rows.len()
        ));
    }

    let mut exported_csv = Vec::new();
    db.export_csv(None, &mut exported_csv, false)
        .map_err(|e| fail(&e))?;
    let exported = rows(&exported_csv).map_err(|e| fail(&e))?;
    for (copy, row) in &written {
        let Some(back) = exported.get(copy) else {
            problems.push(format!("copy {copy} is missing after the import"));
            continue;
        };
        for (column, cell) in row {
            if cell.is_empty() || SKIPPED.contains(&column.as_str()) {
                continue;
            }
            let out = back.get(column).map_or("", String::as_str);
            if !arrived(column, cell, out) {
                problems.push(format!(
                    "copy {copy}, {column}: wrote {cell:?}, MyLongboxDB has {out:?}"
                ));
            }
        }
    }
    problems.extend(names_join(path, &settings, &written).unwrap_or_else(|e| vec![e]));
    let photos = archive
        .file_names()
        .filter(|n| n.as_ref().is_ok_and(|n| n.starts_with("images/")))
        .count();
    let stored = written
        .values()
        .flat_map(|r| [&r["Front Photo"], &r["Back Photo"]])
        .filter(|p| !p.is_empty())
        .count();
    if photos != stored {
        problems.push(format!(
            "{photos} photos in the archive, {stored} named in the CSV"
        ));
    }
    if problems.is_empty() {
        Ok(written.len())
    } else {
        Err(problems)
    }
}

fn main() -> ExitCode {
    let paths: Vec<_> = std::env::args_os().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: assistant-contract EXPORT.zip...");
        return ExitCode::FAILURE;
    }
    let mut failed = false;
    for path in paths {
        let path = std::path::Path::new(&path);
        match check(path) {
            Ok(n) => println!("{}: all {n} rows imported as written", path.display()),
            Err(problems) => {
                failed = true;
                for p in problems {
                    eprintln!("{}: {p}", path.display());
                }
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
