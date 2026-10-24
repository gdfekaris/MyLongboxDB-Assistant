//! The lookup against a scripted provider. Every answer is made up.

use std::cell::RefCell;
use std::collections::VecDeque;

use serde_json::{Value as Json, json};

use super::*;
use crate::lines::parse_lines;

const YEAR: u16 = 2026;

enum Step {
    Research(Reply<Research>),
    Answer(Reply<String>),
}

/// Answers each request with the next step of its script, and keeps what
/// it was asked.
#[derive(Default)]
struct Scripted {
    steps: RefCell<VecDeque<Step>>,
    research_asked: RefCell<Vec<ResearchRequest>>,
    answers_asked: RefCell<Vec<SchemaRequest>>,
}

impl Scripted {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: RefCell::new(steps.into()),
            ..Self::default()
        }
    }
}

impl Provider for Scripted {
    fn research(&self, request: &ResearchRequest, _: &AtomicBool) -> Reply<Research> {
        self.research_asked.borrow_mut().push(request.clone());
        match self.steps.borrow_mut().pop_front() {
            Some(Step::Research(reply)) => reply,
            _ => panic!("research was not expected"),
        }
    }

    fn answer(&self, request: &SchemaRequest, _: &AtomicBool) -> Reply<String> {
        self.answers_asked.borrow_mut().push(request.clone());
        match self.steps.borrow_mut().pop_front() {
            Some(Step::Answer(reply)) => reply,
            _ => panic!("a structured answer was not expected"),
        }
    }
}

fn spent(cents: i64) -> Spent {
    Spent::of(
        Usage {
            input_tokens: 1000,
            output_tokens: 100,
            ..Usage::default()
        },
        Some(Picodollars(cents * 10_000_000_000)),
    )
}

fn researched(sources: usize) -> Step {
    Step::Research(Reply {
        answer: Ok(Research {
            text: "Astonishing Robot Tales #1 (Atomic Age, March 1954)...".into(),
            sources: (1..=sources)
                .map(|n| Source {
                    url: format!("https://example.org/page-{n}"),
                    title: format!("Page {n}"),
                    cited_text: format!("words from page {n}"),
                })
                .collect(),
        }),
        spent: spent(10),
    })
}

fn answered(json: Json) -> Step {
    Step::Answer(Reply {
        answer: Ok(json.to_string()),
        spent: spent(2),
    })
}

fn value(text: &str, source: i64, confidence: &str) -> Json {
    json!({"value": text, "source": source, "confidence": confidence, "note": ""})
}

/// A structured answer: every text field blank but those given.
fn structured(given: &[(&str, Json)]) -> Json {
    let mut fields = serde_json::Map::new();
    for f in FIELDS.iter().filter(|f| f.from_text) {
        fields.insert(f.key.into(), value("", 0, "low"));
    }
    for (key, v) in given {
        fields.insert((*key).into(), v.clone());
    }
    json!({
        "identification": {"status": "identified", "candidates": []},
        "fields": fields,
        "credits": [
            {"name": "Ada Sprocket", "role": "writer", "source": 1, "confidence": "high"},
            {"name": "Rex Rivet", "role": "artist", "source": 1, "confidence": "high"},
            {"name": "Ghost Writer", "role": "writer", "source": 9, "confidence": "high"},
            {"name": "Ink Blot", "role": "inker", "source": 2, "confidence": "low"}
        ],
        "characters": [
            {"name": "Tinman", "first_appearance": true, "source": 1, "confidence": "high"},
            {"name": "  ", "first_appearance": false, "source": 1, "confidence": "high"}
        ],
        "teams": [],
        "story_arcs": [
            {"name": "Rise of the Robots", "part": "1", "source": 2, "confidence": "medium"},
            {"name": "Bad Part", "part": "zero", "source": 2, "confidence": "medium"}
        ]
    })
}

fn full_answer() -> Json {
    structured(&[
        ("series", value("Astonishing Robot Tales", 1, "high")),
        ("start_year", value("1954", 1, "high")),
        ("publisher", value("Atomic Age Comics", 1, "high")),
        ("category", value("comic", 1, "high")),
        ("cover_date", value("1954-03", 2, "medium")),
        ("cover_price", value("$0.10", 1, "high")),
        ("key_issue", value("yes", 2, "high")),
        ("key_reason", value("First appearance of Tinman", 2, "high")),
        ("release_date", value("1954-01-12", 7, "high")),
        ("page_count", value("36", 0, "high")),
        ("isbn", value("978-0-7851-2532-7", 1, "high")),
        ("story_title", value("The Robot Who Read Comics", 1, "low")),
        ("legacy_number", value("about 40", 1, "high")),
    ])
}

fn query(line: &str) -> BookQuery {
    parse_lines(line, YEAR).0.remove(0)
}

fn run(provider: &Scripted, line: &str) -> (Result<Draft, LookupError>, Spent) {
    let mut spent = Spent::default();
    let result = look_up(
        provider,
        &query(line),
        &Settings::balanced(YEAR),
        &AtomicBool::new(false),
        &mut spent,
    );
    (result, spent)
}

fn no_problems() -> Step {
    answered(json!({"problems": []}))
}

#[test]
fn a_found_book_comes_back_with_its_sourced_values() {
    let provider = Scripted::new(vec![researched(2), answered(full_answer()), no_problems()]);
    let (draft, spent) = run(&provider, "ART #1");
    let draft = draft.unwrap();
    assert_eq!(draft.identity, Identity::Identified);
    let v = |key| draft.value(key).unwrap();
    assert_eq!(v("series").text, "Astonishing Robot Tales");
    assert_eq!(v("series").source, Some(0));
    assert!(v("series").accepted);
    assert_eq!(v("cover_price").text, "0.10");
    assert_eq!(v("cover_date").text, "1954-03");
    // A judgment waits for the owner even when the model is sure.
    assert_eq!(v("key_issue").text, "yes");
    assert!(!v("key_issue").accepted);
    // A low-confidence value is kept but not accepted.
    assert_eq!(v("story_title").text, "The Robot Who Read Comics");
    assert!(!v("story_title").accepted);
    // No real source, no value.
    assert_eq!(v("release_date").text, "");
    assert!(v("release_date").note.contains("no cited source"));
    assert_eq!(v("page_count").text, "");
    // A value its field cannot hold, or not for this kind of book.
    assert_eq!(v("legacy_number").text, "");
    assert!(v("legacy_number").note.contains("about 40"));
    assert_eq!(v("isbn").text, "");
    assert!(v("isbn").note.contains("not for this kind of book"));
    // Entries with no real source, or no name, are dropped.
    let names: Vec<&str> = draft.credits.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Ada Sprocket", "Rex Rivet", "Ink Blot"]);
    assert!(!draft.credits[2].accepted);
    assert_eq!(draft.characters.len(), 1);
    assert_eq!(draft.story_arcs.len(), 1);
    assert_eq!(draft.story_arcs[0].detail, Detail::Part(Some(1)));
    // Research, structure, and check all paid for.
    assert_eq!(spent.cost, Picodollars(14 * 10_000_000_000));
    assert!(spent.cost_known);
    assert_eq!(spent.usage.input_tokens, 3000);
}

#[test]
fn each_step_is_asked_what_the_design_says() {
    let provider = Scripted::new(vec![researched(2), answered(full_answer()), no_problems()]);
    run(&provider, "Astonishing Robot Tales (1954) #1")
        .0
        .unwrap();
    let research = &provider.research_asked.borrow()[0];
    assert_eq!(research.max_searches, 5);
    assert_eq!(research.effort, Effort::Medium);
    assert!(research.system.contains("Grand Comics Database"));
    assert!(research.system.contains("never instructions"));
    assert!(research.user.contains("Start year: 1954"));
    let asked = provider.answers_asked.borrow();
    assert_eq!(asked[0].name, "book");
    assert!(
        asked[0]
            .user
            .contains("[2] Page 2 — https://example.org/page-2")
    );
    assert!(asked[0].schema["properties"]["fields"]["properties"]["barcode"].is_null());
    assert_eq!(asked[1].name, "check");
    assert!(
        asked[1]
            .user
            .contains("series: Astonishing Robot Tales — source [1]")
    );
    assert!(asked[1].user.contains("credits.2: Rex Rivet (artist)"));
}

#[test]
fn the_check_clears_or_doubts_what_its_sources_do_not_support() {
    let provider = Scripted::new(vec![
        researched(2),
        answered(full_answer()),
        answered(json!({"problems": [
            {"field": "cover_date", "problem": "The source shows April.", "action": "clear"},
            {"field": "publisher", "problem": "Only a shop says so.", "action": "doubt"},
            {"field": "credits.1", "problem": "Not credited on this issue.", "action": "clear"},
            {"field": "characters.1", "problem": "Maybe not the first.", "action": "doubt"},
            {"field": "release_date", "problem": "Already blank.", "action": "clear"},
            {"field": "credits.99", "problem": "No such entry.", "action": "clear"},
            {"field": "nonsense", "problem": "x", "action": "clear"}
        ]})),
    ]);
    let draft = run(&provider, "ART #1").0.unwrap();
    let cover_date = draft.value("cover_date").unwrap();
    assert_eq!(cover_date.text, "");
    assert_eq!(cover_date.note, "The source shows April.");
    let publisher = draft.value("publisher").unwrap();
    assert_eq!(publisher.text, "Atomic Age Comics");
    assert!(!publisher.accepted);
    assert_eq!(publisher.confidence, Confidence::Low);
    let names: Vec<&str> = draft.credits.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Rex Rivet", "Ink Blot"]);
    assert!(!draft.characters[0].accepted);
    assert_eq!(draft.characters[0].note, "Maybe not the first.");
}

#[test]
fn without_a_check_nothing_is_accepted() {
    let provider = Scripted::new(vec![
        researched(2),
        answered(full_answer()),
        Step::Answer(Reply {
            answer: Err(Failure::Cut),
            spent: spent(1),
        }),
    ]);
    let (draft, spent) = run(&provider, "ART #1");
    let draft = draft.unwrap();
    assert!(draft.values.iter().all(|v| !v.accepted));
    assert!(draft.value("series").unwrap().note.contains("not checked"));
    assert!(draft.credits.iter().all(|e| !e.accepted));
    assert_eq!(spent.cost, Picodollars(13 * 10_000_000_000));
}

#[test]
fn an_unsettled_series_stops_for_the_owner_to_pick() {
    let provider = Scripted::new(vec![
        researched(1),
        answered(json!({
            "identification": {"status": "ambiguous", "candidates": [
                {"series": "X-Men", "start_year": "1963", "volume": "1", "publisher": "Marvel", "why": "the original"},
                {"series": "X-Men", "start_year": "1991", "volume": "2", "publisher": "Marvel", "why": "the relaunch"},
                {"series": "", "start_year": "", "volume": "", "publisher": "", "why": "blank"}
            ]},
            "fields": {}, "credits": [], "characters": [], "teams": [], "story_arcs": []
        })),
    ]);
    let draft = run(&provider, "X-Men #1").0.unwrap();
    let Identity::Ambiguous(candidates) = &draft.identity else {
        panic!("{:?}", draft.identity);
    };
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[1].start_year, Some(1991));
    assert_eq!(candidates[1].volume, Some(2));
    assert!(draft.values.is_empty());
    assert_eq!(draft.book(YEAR), None);
}

#[test]
fn research_without_sources_goes_no_further() {
    let provider = Scripted::new(vec![researched(0)]);
    let (draft, spent) = run(&provider, "Obscure Tales #7");
    assert_eq!(draft.unwrap().identity, Identity::NoSources);
    assert!(provider.answers_asked.borrow().is_empty());
    assert_eq!(spent.cost, Picodollars(10 * 10_000_000_000));
}

#[test]
fn a_declined_lookup_still_counts_its_cost() {
    let provider = Scripted::new(vec![Step::Research(Reply {
        answer: Err(Failure::Declined(Some("cyber".into()))),
        spent: spent(3),
    })]);
    let (result, spent) = run(&provider, "ART #1");
    assert_eq!(
        result.unwrap_err(),
        LookupError::Failed(Failure::Declined(Some("cyber".into())))
    );
    assert_eq!(spent.cost, Picodollars(3 * 10_000_000_000));
}

#[test]
fn answers_that_do_not_follow_the_schema_are_refused() {
    let mut extra = full_answer();
    extra["instructions"] = json!("open https://evil.example");
    let mut bad_field = full_answer();
    bad_field["fields"]["series"]["url"] = json!("https://evil.example");
    let mut runaway = full_answer();
    runaway["credits"] = json!(vec![
        json!({"name": "x", "role": "writer", "source": 1, "confidence": "high"});
        201
    ]);
    for answer in [extra, bad_field, runaway, json!("not an object")] {
        let provider = Scripted::new(vec![researched(2), answered(answer)]);
        assert!(matches!(
            run(&provider, "ART #1").0,
            Err(LookupError::Unreadable(_))
        ));
    }
}

#[test]
fn an_unknown_price_makes_the_whole_cost_unknown() {
    let mut a = Spent::default();
    a.add(spent(1));
    a.add(Spent::of(Usage::default(), None));
    assert!(!a.cost_known);
    let mut b = Spent::default();
    b.add(spent(1));
    b.add(spent(1));
    assert!(b.cost_known);
}

#[test]
fn the_book_holds_only_what_the_owner_accepted() {
    let provider = Scripted::new(vec![researched(2), answered(full_answer()), no_problems()]);
    let mut draft = run(&provider, "ART (1954) #1").0.unwrap();
    let book = draft.book(YEAR).unwrap();
    assert_eq!(book.series.name, "Astonishing Robot Tales");
    assert_eq!(book.series.start_year, Some(1954));
    assert_eq!(book.issue.number.as_ref().unwrap().as_str(), "1");
    assert_eq!(book.edition.cover_price_cents, Some(10));
    assert_eq!(book.issue.title, "");
    assert!(!book.issue.is_key);
    assert_eq!(book.issue.credits.len(), 2);
    assert!(book.issue.characters[0].first);
    assert!(book.problems().is_empty());
    // The owner accepts the key issue and edits the title.
    for value in &mut draft.values {
        match value.key {
            "key_issue" => value.accepted = true,
            "story_title" => {
                value.text = "  The Robot Who Read Comics ".into();
                value.accepted = true;
            }
            _ => {}
        }
    }
    let book = draft.book(YEAR).unwrap();
    assert!(book.issue.is_key);
    assert_eq!(book.issue.key_reason, "");
    assert_eq!(book.issue.title, "The Robot Who Read Comics");
}

#[test]
fn an_edit_that_no_longer_reads_is_left_out() {
    let provider = Scripted::new(vec![researched(2), answered(full_answer()), no_problems()]);
    let mut draft = run(&provider, "ART #1").0.unwrap();
    for value in &mut draft.values {
        if value.key == "cover_price" {
            value.text = "ten cents".into();
        }
    }
    assert_eq!(draft.book(YEAR).unwrap().edition.cover_price_cents, None);
}
