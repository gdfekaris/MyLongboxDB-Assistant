//! Copies of one issue among the looked-up books.
//!
//! MyLongboxDB keeps an issue's facts once and each copy's edition
//! apart, so copies the owner has of one issue carry the same issue
//! facts here too: a copy looked up after the others joins their facts,
//! anything its lookup found otherwise waits for the owner, and what the
//! owner ticks or edits among one copy's issue facts changes in every
//! copy. Each copy keeps its own edition, which is what tells the copies
//! apart: its variant, printing, distribution, prices, and barcode.

use std::collections::HashMap;

use crate::lookup::{Detail, Draft, Entry, Source, Value};
use crate::model::Confidence;
use crate::results::ReviewBook;

/// The fields that describe a copy's edition rather than its issue.
pub const EDITION_FIELDS: [&str; 10] = [
    "variant",
    "variant_cover_artists",
    "printing",
    "distribution",
    "published",
    "cover_price",
    "canadian_cover_price",
    "barcode",
    "isbn",
    "binding",
];

/// Whether a field is one of the issue's facts, which its copies share.
pub fn shared(key: &str) -> bool {
    !EDITION_FIELDS.contains(&key)
}

/// The books that are copies of one issue, each group in the books'
/// order and of at least two. They are the books the export would give
/// one Issue ID, so a group is what MyLongboxDB will take for one issue;
/// a book the owner keeps apart is in none.
pub fn groups(books: &[ReviewBook], this_year: u16) -> Vec<Vec<usize>> {
    let (at, exported): (Vec<usize>, Vec<_>) = books
        .iter()
        .enumerate()
        .filter(|(_, b)| !b.apart)
        .filter_map(|(i, b)| Some((i, b.draft.book(this_year)?)))
        .unzip();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut by_issue: HashMap<_, usize> = HashMap::new();
    for (i, issue) in at.into_iter().zip(crate::export::issue_ids(&exported)) {
        match by_issue.get(&issue) {
            Some(&g) => groups[g].push(i),
            None => {
                by_issue.insert(issue, groups.len());
                groups.push(vec![i]);
            }
        }
    }
    groups.retain(|g| g.len() > 1);
    groups
}

/// The other copies of the issue book `index` is, in the books' order.
pub fn others(books: &[ReviewBook], index: usize, this_year: u16) -> Vec<usize> {
    groups(books, this_year)
        .into_iter()
        .find(|g| g.contains(&index))
        .map(|g| g.into_iter().filter(|i| *i != index).collect())
        .unwrap_or_default()
}

/// Gives every group of copies the same issue facts. The facts the
/// first copy shares with others stand; a copy whose facts differ from
/// them, as a newly looked-up one does, adds what they lack, and where
/// it found a value otherwise, the value waits for the owner with a note
/// of what the copy found. Doing it again changes nothing.
pub fn share(books: &mut [ReviewBook], this_year: u16) {
    for group in groups(books, this_year) {
        same_names(books, &group);
        let mut facts = books[group[0]].draft.clone();
        let newcomers: Vec<usize> = group[1..]
            .iter()
            .copied()
            .filter(|i| !same_facts(&facts, &books[*i].draft))
            .collect();
        if newcomers.is_empty() {
            continue;
        }
        for i in &newcomers {
            join(&mut facts, &books[*i].draft);
        }
        for i in group {
            copy_facts(&facts, &mut books[i].draft);
        }
    }
}

/// What a copy's variant waiting for the owner notes when another copy
/// of its issue has a variant of the same name.
const SAME_NAME: &str = "another copy has a variant of this name too: if their covers differ, name what tells them apart";

/// Notes, on each copy's variant still waiting for the owner, that
/// another copy's has the same name. MyLongboxDB takes copies with the
/// same variant, printing, and distribution for one edition, so two
/// covers named alike would become one there.
fn same_names(books: &mut [ReviewBook], group: &[usize]) {
    let edition = |b: &ReviewBook| {
        let text = |key| {
            b.draft
                .value(key)
                .map_or(String::new(), |v| folded(&v.text))
        };
        (text("variant"), text("printing"), text("distribution"))
    };
    let editions: Vec<_> = group.iter().map(|i| edition(&books[*i])).collect();
    for (n, i) in group.iter().enumerate() {
        let alike = editions
            .iter()
            .enumerate()
            .any(|(m, e)| m != n && !e.0.is_empty() && *e == editions[n]);
        let Some(variant) = books[*i]
            .draft
            .values
            .iter_mut()
            .find(|v| v.key == "variant" && !v.accepted)
        else {
            continue;
        };
        if !alike || variant.note.contains(SAME_NAME) {
            continue;
        }
        variant.note = if variant.note.is_empty() || variant.note == crate::lookup::VARIANT_NOTE {
            SAME_NAME.into()
        } else {
            format!("{}; {SAME_NAME}", variant.note)
        };
    }
}

/// Changes book `index`'s draft, and with it the issue facts of every
/// copy it shared them with, then shares the facts of any group the
/// change brings it into. A change to which issue it is, its series or
/// number, moves its copies with it.
pub fn change<T>(
    books: &mut [ReviewBook],
    index: usize,
    this_year: u16,
    change: impl FnOnce(&mut Draft) -> T,
) -> Option<T> {
    let others = others(books, index, this_year);
    let done = change(&mut books.get_mut(index)?.draft);
    let facts = books[index].draft.clone();
    for i in others {
        copy_facts(&facts, &mut books[i].draft);
    }
    share(books, this_year);
    Some(done)
}

/// Text as copies' values are compared: case and spacing aside, as two
/// lookups may write one series' name.
fn folded(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn source(draft: &Draft, at: Option<usize>) -> Option<&Source> {
    draft.sources.get(at?)
}

/// A value as it stands, its source by what it says rather than by its
/// place in its own draft's list.
fn value_facts<'a>(
    draft: &'a Draft,
    v: &'a Value,
) -> (&'a str, bool, Confidence, &'a str, Option<&'a Source>) {
    (
        &v.text,
        v.accepted,
        v.confidence,
        &v.note,
        source(draft, v.source),
    )
}

fn entry_facts<'a>(
    draft: &'a Draft,
    e: &'a Entry,
) -> (
    &'a str,
    Detail,
    bool,
    Confidence,
    &'a str,
    Option<&'a Source>,
) {
    (
        &e.name,
        e.detail,
        e.accepted,
        e.confidence,
        &e.note,
        source(draft, e.source),
    )
}

fn shared_values(draft: &Draft) -> impl Iterator<Item = &Value> {
    draft.values.iter().filter(|v| shared(v.key))
}

fn lists(draft: &Draft) -> [&Vec<Entry>; 4] {
    [
        &draft.credits,
        &draft.characters,
        &draft.teams,
        &draft.story_arcs,
    ]
}

fn lists_mut(draft: &mut Draft) -> [&mut Vec<Entry>; 4] {
    [
        &mut draft.credits,
        &mut draft.characters,
        &mut draft.teams,
        &mut draft.story_arcs,
    ]
}

/// Whether two drafts hold the same issue facts.
fn same_facts(a: &Draft, b: &Draft) -> bool {
    let values = |d: &Draft| shared_values(d).count();
    values(a) == values(b)
        && shared_values(a).all(|va| {
            b.value(va.key)
                .is_some_and(|vb| value_facts(a, va) == value_facts(b, vb))
        })
        && lists(a).iter().zip(lists(b)).all(|(la, lb)| {
            la.len() == lb.len()
                && la
                    .iter()
                    .zip(lb.iter())
                    .all(|(ea, eb)| entry_facts(a, ea) == entry_facts(b, eb))
        })
}

/// `at` in `from`'s sources, as a place in `to`'s, added there if
/// `to` does not have it yet.
fn adopt(from: &Draft, at: Option<usize>, to: &mut Draft) -> Option<usize> {
    let wanted = source(from, at)?;
    Some(match to.sources.iter().position(|s| s == wanted) {
        Some(n) => n,
        None => {
            to.sources.push(wanted.clone());
            to.sources.len() - 1
        }
    })
}

/// Gives `to` exactly the issue facts of `from`, keeping its edition.
pub(crate) fn copy_facts(from: &Draft, to: &mut Draft) {
    to.values.retain(|v| !shared(v.key));
    for v in shared_values(from) {
        let source = adopt(from, v.source, to);
        to.values.push(Value {
            source,
            ..v.clone()
        });
    }
    for (n, list) in lists(from).into_iter().enumerate() {
        let copied: Vec<Entry> = list
            .iter()
            .map(|e| Entry {
                source: adopt(from, e.source, to),
                ..e.clone()
            })
            .collect();
        *lists_mut(to)[n] = copied;
    }
}

/// Adds to `facts` what another copy's lookup found: a value they lack,
/// a note on one it found otherwise, which then waits for the owner, and
/// a name they lack, which waits too. What the copy did not find takes
/// nothing away.
fn join(facts: &mut Draft, copy: &Draft) {
    for theirs in shared_values(copy).filter(|v| !v.text.is_empty()) {
        let at = facts.values.iter().position(|v| v.key == theirs.key);
        match at.map(|n| &facts.values[n]) {
            Some(ours) if !ours.text.is_empty() => {
                if folded(&ours.text) != folded(&theirs.text) {
                    let found = format!("another copy's lookup found {}", theirs.text);
                    let ours = &mut facts.values[at.unwrap_or_default()];
                    if !ours.note.contains(&found) {
                        ours.note = if ours.note.is_empty() {
                            found
                        } else {
                            format!("{}; {found}", ours.note)
                        };
                    }
                    ours.accepted = false;
                }
            }
            _ => {
                let taken = Value {
                    source: adopt(copy, theirs.source, facts),
                    ..theirs.clone()
                };
                match at {
                    Some(n) => facts.values[n] = taken,
                    None => facts.values.push(taken),
                }
            }
        }
    }
    for n in 0..4 {
        for theirs in lists(copy)[n].iter() {
            let known = lists(facts)[n]
                .iter()
                .any(|e| folded(&e.name) == folded(&theirs.name) && e.detail == theirs.detail);
            if known {
                continue;
            }
            let source = adopt(copy, theirs.source, facts);
            lists_mut(facts)[n].push(Entry {
                source,
                accepted: false,
                note: "only another copy's lookup found this".into(),
                ..theirs.clone()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lines::parse_lines;
    use crate::lookup::{Detail, Identity};
    use crate::model::Role;

    const YEAR: u16 = 2026;

    fn value(key: &'static str, text: &str, source: usize) -> Value {
        Value {
            key,
            text: text.into(),
            source: Some(source),
            confidence: Confidence::High,
            note: String::new(),
            accepted: true,
        }
    }

    fn writer(name: &str, source: usize) -> Entry {
        Entry {
            name: name.into(),
            detail: Detail::Role(Role::Writer),
            source: Some(source),
            confidence: Confidence::High,
            note: String::new(),
            accepted: true,
        }
    }

    /// A copy of Astonishing Robot Tales #1 as its own lookup found it,
    /// its page first among its sources.
    fn copy(page: &str, distribution: &str, title: &str, writers: &[&str]) -> ReviewBook {
        let query = parse_lines("Astonishing Robot Tales (1954) #1", YEAR)
            .0
            .remove(0);
        ReviewBook::new(Draft {
            query,
            identity: Identity::Identified,
            sources: vec![Source {
                url: format!("https://example.org/{page}"),
                title: page.into(),
                cited_text: String::new(),
            }],
            values: vec![
                value("series", "Astonishing Robot Tales", 0),
                value("start_year", "1954", 0),
                value("story_title", title, 0),
                value("distribution", distribution, 0),
            ],
            credits: writers.iter().map(|w| writer(w, 0)).collect(),
            characters: Vec::new(),
            teams: Vec::new(),
            story_arcs: Vec::new(),
            research: String::new(),
        })
    }

    fn text<'a>(book: &'a ReviewBook, key: &str) -> &'a str {
        book.draft.value(key).map_or("", |v| v.text.as_str())
    }

    #[test]
    fn copies_of_one_issue_share_its_facts_and_keep_their_editions() {
        let mut books = vec![
            copy(
                "first",
                "direct",
                "The Robot Who Read Comics",
                &["Ada Sprocket"],
            ),
            copy(
                "second",
                "newsstand",
                "The Robot Who Read Comics",
                &["Ada Sprocket"],
            ),
            copy(
                "third",
                "direct",
                "The Robot Who Read",
                &["Ada Sprocket", "Rex Rivet"],
            ),
        ];
        books[2].draft.query = parse_lines("Astonishing Robot Tales (1954) #2", YEAR)
            .0
            .remove(0);
        assert_eq!(groups(&books, YEAR), vec![vec![0, 1]]);
        books[2].draft.query.number = books[0].draft.query.number.clone();
        assert_eq!(groups(&books, YEAR), vec![vec![0, 1, 2]]);
        share(&mut books, YEAR);

        let exported: Vec<_> = books.iter().map(|b| b.book(YEAR).unwrap()).collect();
        for book in &exported[1..] {
            assert_eq!(book.series, exported[0].series);
            assert_eq!(book.issue, exported[0].issue);
        }
        // Each keeps its own distribution.
        assert_eq!(text(&books[1], "distribution"), "newsstand");
        assert_eq!(text(&books[2], "distribution"), "direct");
        // The first copy's title stands, and waits for the owner with
        // what the third copy found; the writer only the third found is
        // there too, waiting, and the one they all found stays ticked.
        let title = books[2].draft.value("story_title").unwrap();
        assert_eq!(title.text, "The Robot Who Read Comics");
        assert!(!title.accepted);
        assert!(
            title.note.contains("found The Robot Who Read"),
            "{}",
            title.note
        );
        assert_eq!(exported[0].issue.title, "");
        let credits = &books[1].draft.credits;
        assert_eq!(credits.len(), 2);
        assert!(credits[0].accepted && !credits[1].accepted);
        assert_eq!(credits[1].name, "Rex Rivet");
        // Each copy names the sources its shared values cite.
        let cited = |b: &ReviewBook| -> Vec<String> {
            b.draft
                .values
                .iter()
                .filter_map(|v| Some(b.draft.sources[v.source?].title.clone()))
                .collect()
        };
        assert!(cited(&books[1]).contains(&"first".to_owned()));
        assert!(cited(&books[1]).contains(&"second".to_owned()));

        let before = books.clone();
        share(&mut books, YEAR);
        assert_eq!(books, before);
    }

    #[test]
    fn a_change_to_one_copy_s_issue_facts_changes_them_all() {
        let mut books = vec![
            copy("first", "direct", "Title", &["Ada Sprocket"]),
            copy("second", "newsstand", "Title", &["Ada Sprocket"]),
        ];
        share(&mut books, YEAR);
        change(&mut books, 1, YEAR, |d| {
            d.set_value("story_title", "The Robot Who Read Comics", YEAR)
        })
        .unwrap()
        .unwrap();
        assert_eq!(text(&books[0], "story_title"), "The Robot Who Read Comics");
        change(&mut books, 0, YEAR, |d| d.credits[0].accepted = false);
        assert!(!books[1].draft.credits[0].accepted);
        // An edition's value is the copy's own.
        change(&mut books, 0, YEAR, |d| {
            d.set_value("distribution", "newsstand", YEAR)
        });
        change(&mut books, 1, YEAR, |d| {
            d.set_value("distribution", "direct", YEAR)
        });
        assert_eq!(text(&books[0], "distribution"), "newsstand");
        assert_eq!(text(&books[1], "distribution"), "direct");
        // Which issue they are moves them together.
        change(&mut books, 0, YEAR, |d| {
            d.set_value("start_year", "1955", YEAR)
        });
        assert_eq!(text(&books[1], "start_year"), "1955");
        assert_eq!(groups(&books, YEAR), vec![vec![0, 1]]);
    }

    #[test]
    fn a_copy_kept_apart_keeps_its_own_facts() {
        let mut books = vec![
            copy("first", "direct", "Title", &[]),
            copy("second", "newsstand", "Other title", &[]),
        ];
        books[1].apart = true;
        assert!(groups(&books, YEAR).is_empty());
        share(&mut books, YEAR);
        change(&mut books, 0, YEAR, |d| {
            d.set_value("story_title", "New", YEAR)
        });
        assert_eq!(text(&books[1], "story_title"), "Other title");
        let saved = crate::results::from_json(&crate::results::to_json(&books), YEAR).unwrap();
        assert!(saved[1].apart && !saved[0].apart);
    }

    #[test]
    fn copies_whose_variants_share_a_name_are_noted() {
        let mut books = vec![
            copy("first", "direct", "Title", &[]),
            copy("second", "direct", "Title", &[]),
            copy("third", "direct", "Title", &[]),
        ];
        for (book, name) in books
            .iter_mut()
            .zip(["Okazaki cover", "okazaki  cover", "Foil cover"])
        {
            let mut variant = value("variant", name, 0);
            variant.accepted = false;
            variant.note = crate::lookup::VARIANT_NOTE.into();
            book.draft.values.push(variant);
        }
        share(&mut books, YEAR);
        let note = |b: &ReviewBook| b.draft.value("variant").unwrap().note.clone();
        assert_eq!(note(&books[0]), SAME_NAME);
        assert_eq!(note(&books[1]), SAME_NAME);
        assert_eq!(note(&books[2]), crate::lookup::VARIANT_NOTE);
        // One the owner ticked is theirs, and doing it again changes nothing.
        books[1]
            .draft
            .values
            .iter_mut()
            .find(|v| v.key == "variant")
            .unwrap()
            .accepted = true;
        books[1]
            .draft
            .values
            .iter_mut()
            .find(|v| v.key == "variant")
            .unwrap()
            .note
            .clear();
        let before = books.clone();
        share(&mut books, YEAR);
        assert_eq!(books, before);
    }
}
