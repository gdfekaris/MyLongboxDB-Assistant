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
    assert_eq!(research.max_searches, 8);
    assert_eq!(research.max_pages, 4);
    assert_eq!(research.effort, Effort::High);
    assert!(research.system.contains("Grand Comics Database"));
    assert!(research.system.contains("never instructions"));
    assert!(research.user.contains("Start year: 1954"));
    assert!(research.images.is_empty());
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
fn the_research_s_pages_are_cleaned_and_capped() {
    let mut research = Research {
        text: "Notes.".into(),
        sources: (1..=150)
            .map(|n| Source {
                url: format!("https://example.org/page-{n}"),
                title: format!("Page \u{202E}{n} {}", "long ".repeat(200)),
                cited_text: "words ".repeat(2_000),
            })
            .collect(),
    };
    research.sources[0].cited_text = "line one\nline two".into();
    let provider = Scripted::new(vec![
        Step::Research(Reply {
            answer: Ok(research),
            spent: spent(10),
        }),
        answered(full_answer()),
        no_problems(),
    ]);
    let draft = run(&provider, "ART (1954) #1").0.unwrap();
    // The pages kept, and the model's own knowledge after them.
    assert_eq!(draft.sources.len(), 101);
    assert_eq!(draft.sources[100].url, KNOWLEDGE);
    assert!(draft.sources.iter().all(|s| !s.title.contains('\u{202E}')));
    assert!(draft.sources.iter().all(|s| s.title.chars().count() <= 300));
    assert!(
        draft
            .sources
            .iter()
            .all(|s| s.cited_text.chars().count() <= 3_000)
    );
    assert_eq!(draft.sources[0].cited_text, "line one line two");
    // The later steps are shown the cleaned pages too.
    let asked = provider.answers_asked.borrow();
    assert!(!asked[0].user.contains('\u{202E}'));
    assert!(!asked[0].user.contains("page-101"));
}

#[test]
fn what_the_model_knows_counts_only_when_it_is_sure() {
    let mut answer = structured(&[
        ("series", value("Astonishing Robot Tales", 1, "high")),
        // Source 3 is the model's own knowledge, after the two pages.
        ("start_year", value("1954", 3, "high")),
        ("cover_date", value("1954-03", 3, "medium")),
        ("story_title", value("The Robot Who Read Comics", 3, "low")),
    ]);
    answer["credits"] = json!([
        {"name": "Ada Sprocket", "role": "cover_artist", "source": 3, "confidence": "high"},
        {"name": "Rex Rivet", "role": "inker", "source": 3, "confidence": "medium"}
    ]);
    let provider = Scripted::new(vec![researched(2), answered(answer), no_problems()]);
    let draft = run(&provider, "ART #1").0.unwrap();
    assert_eq!(draft.sources[2].url, KNOWLEDGE);
    let v = |key| draft.value(key).unwrap();
    assert_eq!(
        (v("start_year").source, v("start_year").accepted),
        (Some(2), true)
    );
    assert_eq!(v("cover_date").text, "1954-03");
    assert!(!v("cover_date").accepted, "held unless the model is sure");
    assert!(!v("story_title").accepted);
    let held: Vec<bool> = draft.credits.iter().map(|c| c.accepted).collect();
    assert_eq!(held, [true, false]);
    // The later steps are told which source it is, and that it has no
    // page to compare with.
    let asked = provider.answers_asked.borrow();
    assert!(asked[0].user.contains("[3] Your own knowledge"));
    assert!(
        asked[1]
            .user
            .contains("start_year: 1954 — source [3]: the model's own knowledge, no page")
    );
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
    // Characters wait for the owner, who keeps the notable ones.
    assert!(book.issue.characters.is_empty());
    assert!(book.problems().is_empty());
    // The owner accepts the key issue and a character, and edits the
    // title.
    draft.characters[0].accepted = true;
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
    assert!(book.issue.characters[0].first);
}

#[test]
fn a_role_credited_to_several_waits_for_the_owner() {
    let mut answer = full_answer();
    answer["credits"] = json!([
        {"name": "Ada Sprocket", "role": "writer", "source": 1, "confidence": "high"},
        {"name": "Jim Mooney", "role": "inker", "source": 1, "confidence": "high"},
        {"name": "Vince Colletta", "role": "inker", "source": 2, "confidence": "high"},
    ]);
    let provider = Scripted::new(vec![researched(2), answered(answer), no_problems()]);
    let draft = run(&provider, "ART (1954) #1").0.unwrap();
    let accepted: Vec<(&str, bool)> = draft
        .credits
        .iter()
        .map(|c| (c.name.as_str(), c.accepted))
        .collect();
    assert_eq!(
        accepted,
        [
            ("Ada Sprocket", true),
            ("Jim Mooney", false),
            ("Vince Colletta", false)
        ]
    );
    assert!(draft.credits[1].note.contains("main credit"));
}

#[test]
fn a_preview_issue_s_series_waits_for_the_owner() {
    let provider = Scripted::new(vec![researched(2), answered(full_answer()), no_problems()]);
    let draft = run(&provider, "ART (1954) #1/2").0.unwrap();
    for key in ["series", "start_year"] {
        let value = draft.value(key).unwrap();
        assert!(!value.accepted, "{key}");
        assert!(value.note.contains("preview"), "{key}");
    }
    let provider = Scripted::new(vec![researched(2), answered(full_answer()), no_problems()]);
    let draft = run(&provider, "ART (1954) #1").0.unwrap();
    assert!(draft.value("series").unwrap().accepted);
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

#[test]
fn a_book_is_estimated_at_about_a_dollar_on_opus_at_high_effort() {
    let opus = crate::cost::anthropic_price("claude-opus-5-5").unwrap();
    let estimate = estimate_book(&opus, &Settings::balanced(YEAR));
    assert!(estimate.likely < estimate.most);
    assert_eq!(crate::cost::dollars(estimate.likely), "$0.96");
    assert_eq!(crate::cost::dollars(estimate.most), "$2.64");
}

#[test]
fn a_chosen_candidate_replaces_the_series_but_keeps_the_number() {
    let q = query("X-Men #1");
    let chosen = choose(
        &q,
        &Candidate {
            series: "X-Men".into(),
            start_year: Some(1991),
            volume: Some(2),
            publisher: "Marvel".into(),
            why: "the relaunch".into(),
        },
    );
    assert_eq!(chosen.to_string(), "X-Men (1991) Vol. 2 #1");
    assert_eq!(chosen.line, q.line);
}

// Photo lookups: the read step first, then the same steps.

/// A cover's items, every one blank but those given.
fn read_answer(is_cover: bool, given: &[(&str, &str, &str)]) -> Json {
    let keys = read::schema()["properties"]["items"]["required"].clone();
    let mut items = serde_json::Map::new();
    for key in keys.as_array().unwrap() {
        items.insert(
            key.as_str().unwrap().into(),
            json!({"text": "", "where": ""}),
        );
    }
    for (key, text, place) in given {
        items.insert((*key).into(), json!({"text": text, "where": place}));
    }
    json!({"is_cover": is_cover, "kind": "comic", "items": items})
}

const BARCODE: &str = "7 59606 08936 9 00111";

fn robot_cover() -> Json {
    read_answer(
        true,
        &[
            ("title", "Astonishing Robot Tales", "front, top"),
            ("issue_number", "#1", "front, top left"),
            ("cover_date", "March 1954", "front, top left"),
            ("us_price", "10¢", "front, top left"),
            ("barcode", BARCODE, "front, bottom left"),
            ("barcode_box", "Direct Edition", "front, bottom left"),
        ],
    )
}

fn covers(back: bool) -> Covers {
    let jpeg = |b: u8| Jpeg::new(vec![0xFF, 0xD8, 0xFF, b]).unwrap();
    Covers {
        line: 4,
        front: jpeg(1),
        back: back.then(|| jpeg(2)),
        chosen: None,
    }
}

fn run_covers(provider: &Scripted, back: bool) -> (Result<Draft, LookupError>, Spent) {
    let mut spent = Spent::default();
    let result = look_up_covers(
        provider,
        &covers(back),
        &Settings::balanced(YEAR),
        &AtomicBool::new(false),
        &mut spent,
    );
    (result, spent)
}

/// A structured answer for a photo lookup: every field blank but those
/// given, edition fields included. The research's two pages are sources
/// 1 and 2, and the cover's items 3 to 8.
fn photo_structured(given: &[(&str, Json)]) -> Json {
    let mut answer = structured(given);
    let fields = answer["fields"].as_object_mut().unwrap();
    for f in FIELDS.iter().filter(|f| !f.from_text) {
        fields.entry(f.key).or_insert(value("", 0, "low"));
    }
    for (key, v) in given {
        fields.insert((*key).into(), v.clone());
    }
    answer
}

#[test]
fn the_cover_is_read_and_then_looked_up_like_a_line() {
    let provider = Scripted::new(vec![
        answered(robot_cover()),
        researched(2),
        answered(photo_structured(&[
            ("series", value("Astonishing Robot Tales", 1, "high")),
            ("cover_price", value("0.10", 6, "high")),
            ("distribution", value("direct", 8, "high")),
        ])),
        no_problems(),
    ]);
    let (draft, spent) = run_covers(&provider, true);
    let draft = draft.unwrap();
    // What the cover named became the book looked up.
    assert_eq!(draft.query.series, "Astonishing Robot Tales");
    assert_eq!(draft.query.number.as_str(), "1");
    assert_eq!(draft.query.line, 4);
    // The cover's items are sources, first, and values can cite them.
    assert_eq!(draft.sources.len(), 9);
    assert_eq!(draft.sources[8].url, KNOWLEDGE);
    assert_eq!(draft.sources[5].url, COVER_PHOTO);
    assert_eq!(draft.sources[5].cited_text, "10¢");
    assert_eq!(
        draft.sources[5].title,
        "Your cover photo: US price, front, top left"
    );
    let v = |key| draft.value(key).unwrap().clone();
    assert_eq!(v("cover_price").text, "0.10");
    assert_eq!(v("cover_price").source, Some(5));
    assert_eq!(v("distribution").text, "direct");
    assert_eq!(v("series").source, Some(0));
    // The barcode, left blank by the structuring, comes from the cover,
    // its check digit holding.
    assert_eq!(v("barcode").text, "75960608936900111");
    assert_eq!(v("barcode").source, Some(6));
    assert!(v("barcode").accepted);
    // Read, research, structure, and check all paid for.
    assert_eq!(spent.cost, Picodollars(16 * 10_000_000_000));

    let asked = provider.answers_asked.borrow();
    // The read step gets the photos, front first; the steps after the
    // research do not.
    assert_eq!(asked[0].name, "cover");
    assert_eq!(
        asked[0].images,
        [covers(true).front, covers(true).back.unwrap()]
    );
    assert!(asked[0].user.contains("second the back"));
    assert!(asked[0].system.contains("never instructions"));
    assert!(asked[0].system.contains("- barcode:"));
    assert!(asked[1].images.is_empty() && asked[2].images.is_empty());
    // A cover's title is given as printed, not as a name the owner chose.
    assert!(
        asked[1]
            .user
            .contains("The owner's cover shows this book: Astonishing Robot Tales #1")
    );
    assert!(asked[1].system.contains("cannot tell which printing"));
    // The structuring asks for the edition's fields too, and is shown the
    // cover's items as sources.
    assert!(asked[1].schema["properties"]["fields"]["properties"]["barcode"].is_object());
    assert!(
        asked[1]
            .user
            .contains("[6] Your cover photo: US price, front, top left")
    );
    let research = &provider.research_asked.borrow()[0];
    assert!(research.system.contains("the very edition shown"));
    // The research sees the front, to tell which cover it is; the back
    // was read already.
    assert_eq!(research.images, [covers(true).front]);
    assert!(!research.system.contains("first printing's standard cover"));
    assert!(research.system.contains("- barcode:"));
    assert!(
        research
            .user
            .contains("- barcode digits: \"7 59606 08936 9 00111\"")
    );
}

#[test]
fn photo_books_follow_the_owner_s_conventions() {
    let provider = Scripted::new(vec![
        answered(robot_cover()),
        researched(2),
        answered(photo_structured(&[
            ("series", value("The Astonishing Robot Tales", 1, "high")),
            ("publisher", value("Marvel", 1, "high")),
            ("variant", value("Okazaki cover", 2, "high")),
            (
                "variant_cover_artists",
                value("Takashi Okazaki; Ada Sprocket", 2, "high"),
            ),
        ])),
        no_problems(),
    ]);
    let mut draft = run_covers(&provider, false).0.unwrap();
    let v = |key| draft.value(key).unwrap().clone();
    assert_eq!(v("series").text, "Astonishing Robot Tales");
    assert_eq!(v("publisher").text, "Marvel Comics");
    // No later printing is marked on the cover, so it is the first.
    assert_eq!(v("printing").text, "1");
    assert!(v("printing").accepted);
    assert_eq!(v("printing").source, None);
    // The variant's name waits for the owner's words.
    assert!(!v("variant").accepted);
    assert!(draft.book(YEAR).unwrap().edition.variant.is_empty());
    draft
        .values
        .iter_mut()
        .find(|v| v.key == "variant")
        .unwrap()
        .accepted = true;
    let book = draft.book(YEAR).unwrap();
    assert_eq!(book.series.name, "Astonishing Robot Tales");
    assert_eq!(book.edition.printing, Some(1));
    assert_eq!(
        book.edition.cover_artists,
        ["Takashi Okazaki", "Ada Sprocket"]
    );
}

#[test]
fn a_series_the_owner_chose_is_looked_up_with_the_covers() {
    let provider = Scripted::new(vec![
        answered(robot_cover()),
        researched(2),
        answered(photo_structured(&[])),
        no_problems(),
    ]);
    let mut covers = covers(false);
    covers.chosen = Some(query("Robot Tales (1960) #1"));
    let draft = look_up_covers(
        &provider,
        &covers,
        &Settings::balanced(YEAR),
        &AtomicBool::new(false),
        &mut Spent::default(),
    )
    .unwrap();
    assert_eq!(draft.query.series, "Robot Tales");
    assert_eq!(draft.query.start_year, Some(1960));
    // The cover is still read, and its items are still sources.
    assert_eq!(provider.answers_asked.borrow()[0].name, "cover");
    assert!(draft.sources.iter().any(|s| s.url == COVER_PHOTO));
}

#[test]
fn a_printing_marked_on_the_cover_is_not_taken_as_the_first() {
    let second = read_answer(
        true,
        &[
            ("title", "Astonishing Robot Tales", "front"),
            ("issue_number", "1", "front"),
            ("printing", "2nd Printing", "front, top"),
        ],
    );
    let provider = Scripted::new(vec![
        answered(second),
        researched(1),
        answered(photo_structured(&[])),
        no_problems(),
    ]);
    let draft = run_covers(&provider, false).0.unwrap();
    assert_eq!(draft.value("printing").unwrap().text, "");
}

#[test]
fn a_barcode_found_online_that_differs_from_the_cover_is_doubted() {
    let provider = Scripted::new(vec![
        answered(robot_cover()),
        researched(2),
        answered(photo_structured(&[(
            "barcode",
            value("75960608936900211", 1, "high"),
        )])),
        no_problems(),
    ]);
    let draft = run_covers(&provider, false).0.unwrap();
    let barcode = draft.value("barcode").unwrap();
    assert_eq!(barcode.text, "75960608936900211");
    assert!(!barcode.accepted);
    assert!(barcode.note.contains("75960608936900111"));
    assert!(
        provider.answers_asked.borrow()[0]
            .user
            .contains("front cover")
    );
}

#[test]
fn a_misread_barcode_is_not_taken_from_the_cover() {
    let misread = read_answer(
        true,
        &[
            ("title", "Astonishing Robot Tales", "front"),
            ("issue_number", "1", "front"),
            ("barcode", "7 59606 08936 8 00111", "front"),
        ],
    );
    let provider = Scripted::new(vec![
        answered(misread),
        researched(1),
        answered(photo_structured(&[])),
        no_problems(),
    ]);
    let draft = run_covers(&provider, false).0.unwrap();
    let barcode = draft.value("barcode").unwrap();
    assert_eq!(barcode.text, "");
    assert!(barcode.note.contains("check digit"));
}

#[test]
fn covers_that_name_no_book_go_no_further() {
    for (reading, why) in [
        (read_answer(false, &[]), "does not show a book"),
        (
            read_answer(true, &[("issue_number", "1", "front")]),
            "no title",
        ),
    ] {
        let provider = Scripted::new(vec![answered(reading)]);
        let (result, spent) = run_covers(&provider, false);
        match result {
            Err(LookupError::NoBookOnCover(text)) => assert!(text.contains(why), "{text}"),
            other => panic!("{other:?}"),
        }
        // The read was still paid for, and nothing else was asked.
        assert_eq!(spent.cost, Picodollars(2 * 10_000_000_000));
        assert!(provider.research_asked.borrow().is_empty());
    }
}

#[test]
fn a_cover_without_a_number_is_looked_up_by_its_other_details() {
    let unnumbered = read_answer(
        true,
        &[
            ("title", "Astonishing Robot Tales", "front"),
            ("cover_date", "March 1954", "front"),
        ],
    );
    let provider = Scripted::new(vec![
        answered(unnumbered),
        researched(2),
        answered(photo_structured(&[("issue_number", value("4", 1, "high"))])),
        no_problems(),
    ]);
    let draft = run_covers(&provider, false).0.unwrap();
    assert_eq!(draft.query.number.as_str(), "nn");
    // The research is told the number is not printed.
    let research = &provider.research_asked.borrow()[0];
    assert!(research.user.contains("not printed on the cover"));
    // The number found waits for the owner, and is the export's once
    // they accept it.
    let found = draft.value("issue_number").unwrap();
    assert_eq!(found.text, "4");
    assert!(!found.accepted);
    assert!(found.note.contains("not printed on your cover"));
    assert_eq!(
        draft.book(YEAR).unwrap().issue.number.unwrap().as_str(),
        "nn"
    );
    let mut accepted = draft.clone();
    // The owner ticks it.
    for v in accepted
        .values
        .iter_mut()
        .filter(|v| v.key == "issue_number")
    {
        v.accepted = true;
    }
    assert_eq!(
        accepted.book(YEAR).unwrap().issue.number.unwrap().as_str(),
        "4"
    );
}

#[test]
fn a_series_is_named_without_the_year_in_its_title() {
    let series = |name: &str, start: &str| {
        let provider = Scripted::new(vec![
            answered(robot_cover()),
            researched(2),
            answered(photo_structured(&[
                ("series", value(name, 1, "high")),
                ("start_year", value(start, 1, "high")),
            ])),
            no_problems(),
        ]);
        let draft = run_covers(&provider, false).0.unwrap();
        draft.value("series").unwrap().text.clone()
    };
    assert_eq!(
        series(
            "Free Comic Book Day 2024: Ultimate Universe/Spider-Man",
            "2024"
        ),
        "Free Comic Book Day: Ultimate Universe/Spider-Man"
    );
    assert_eq!(series("Robot Tales 1954", "1954"), "Robot Tales");
    // A number in the title that is not its year stays.
    assert_eq!(series("Spider-Man 2099", "1992"), "Spider-Man 2099");
    assert_eq!(series("1954", "1954"), "1954");
}

#[test]
fn a_facsimile_keeps_only_its_own_dates() {
    let facsimile = |release: &str| {
        let cover = read_answer(
            true,
            &[
                ("title", "Astonishing Robot Tales", "front"),
                ("issue_number", "1", "front"),
                ("cover_date", "MARCH 1954", "front"),
                ("variant", "Facsimile Edition", "front, top"),
            ],
        );
        let provider = Scripted::new(vec![
            answered(cover),
            researched(2),
            answered(photo_structured(&[
                ("start_year", value("1954", 1, "high")),
                ("cover_date", value("1954-03", 1, "high")),
                ("cover_price", value("0.10", 1, "high")),
                ("release_date", value(release, 2, "high")),
                ("published", value("1992", 2, "high")),
                ("key_issue", value("yes", 1, "high")),
                ("key_reason", value("1st appearance of Tinman.", 1, "high")),
                ("variant", value("Facsimile", 1, "high")),
            ])),
            no_problems(),
        ]);
        run_covers(&provider, false).0.unwrap()
    };
    let draft = facsimile("1954-01-12");
    let v = |key| draft.value(key).unwrap().clone();
    // Its cover only reproduces the original's date and price.
    assert_eq!(v("cover_date").text, "");
    assert_eq!(v("cover_price").text, "");
    assert_eq!(v("key_issue").text, "no");
    assert_eq!(v("key_reason").text, "");
    // The original's release date is no use; the facsimile's waits.
    assert_eq!(v("release_date").text, "");
    assert!(v("release_date").note.contains("original"));
    assert_eq!(v("published").text, "1992");
    assert!(!v("published").accepted);
    let book = draft.book(YEAR).unwrap();
    assert!(!book.issue.is_key);
    assert_eq!(book.edition.cover_price_cents, None);

    let draft = facsimile("1992-06-01");
    let release = draft.value("release_date").unwrap();
    assert_eq!(release.text, "1992-06-01");
    assert!(!release.accepted);
    assert!(release.note.contains("facsimile's own"));
}

#[test]
fn an_annual_is_its_own_series() {
    let annual = read_answer(
        true,
        &[
            ("title", "Astonishing Robot Tales", "front"),
            ("issue_number", "Annual 2", "front, top left"),
        ],
    );
    let provider = Scripted::new(vec![
        answered(annual),
        researched(2),
        answered(photo_structured(&[(
            "series",
            value("Astonishing Robot Tales", 1, "high"),
        )])),
        no_problems(),
    ]);
    let draft = run_covers(&provider, false).0.unwrap();
    assert_eq!(draft.query.number.as_str(), "2");
    let book = draft.book(YEAR).unwrap();
    assert_eq!(book.series.name, "Astonishing Robot Tales Annual");
    assert_eq!(book.issue.number.unwrap().as_str(), "2");
}

/// A lookup of a cover that prints `printed` as its date, where the
/// catalogue gives `catalogue` and the book went on sale on `sale`.
fn dated(printed: &str, catalogue: &str, sale: &str) -> Value {
    let mut items = vec![
        ("title", "Astonishing Robot Tales", "front"),
        ("issue_number", "1", "front"),
    ];
    if !printed.is_empty() {
        items.push(("cover_date", printed, "front, top right"));
    }
    let provider = Scripted::new(vec![
        answered(read_answer(true, &items)),
        researched(2),
        answered(photo_structured(&[
            ("cover_date", value(catalogue, 1, "high")),
            ("release_date", value(sale, 2, "high")),
        ])),
        no_problems(),
    ]);
    let draft = run_covers(&provider, false).0.unwrap();
    draft.value("cover_date").unwrap().clone()
}

#[test]
fn a_cover_date_is_the_month_printed_or_else_a_modern_book_s_sale_month() {
    // A modern cover without a date: the month it went on sale, even
    // where the indicia gives a later one. The sale date's source is the
    // answer's second, at index 1.
    let date = dated("", "2024-05", "2024-03-06");
    assert_eq!(date.text, "2024-03");
    assert_eq!(date.source, Some(1));
    assert!(date.note.contains("went on sale"));
    assert_eq!(dated("", "", "2024-03-06").text, "2024-03");

    // An older one keeps the indicia's or the catalogue's month, and
    // takes the sale month only where no source dates it.
    assert_eq!(dated("", "1985-02", "1985-03-20").text, "1985-02");
    assert_eq!(dated("1983", "1983-08", "1983-05-10").text, "1983-08");
    assert_eq!(dated("", "", "1994-03-17").text, "1994-03");

    // A month the cover prints is kept.
    assert_eq!(dated("MAY", "2024-05", "2024-03-06").text, "2024-05");
    assert_eq!(dated("Mar 1954", "1954-03", "1954-01-12").text, "1954-03");
}

#[test]
fn a_cover_date_of_only_a_year_waits_for_the_owner() {
    let provider = Scripted::new(vec![
        answered(robot_cover()),
        researched(2),
        answered(photo_structured(&[(
            "cover_date",
            value("1954", 1, "high"),
        )])),
        no_problems(),
    ]);
    let draft = run_covers(&provider, false).0.unwrap();
    let date = draft.value("cover_date").unwrap();
    assert_eq!(date.text, "1954");
    assert!(!date.accepted);
    assert!(date.note.contains("month"));
}

#[test]
fn a_grading_label_gives_the_number_the_cover_does_not() {
    let slabbed = read_answer(
        true,
        &[
            ("title", "Astonishing Robot Tales", "front"),
            (
                "grading_label",
                "CGC 9.2 Astonishing Robot Tales #3 Atomic Age, 5/54",
                "front, label",
            ),
        ],
    );
    let provider = Scripted::new(vec![
        answered(slabbed),
        researched(2),
        // A number the research gives anyway is not wanted.
        answered(photo_structured(&[("issue_number", value("4", 1, "high"))])),
        no_problems(),
    ]);
    let draft = run_covers(&provider, false).0.unwrap();
    assert_eq!(draft.query.number.as_str(), "3");
    assert_eq!(draft.value("issue_number").unwrap().text, "");
    assert_eq!(
        draft.book(YEAR).unwrap().issue.number.unwrap().as_str(),
        "3"
    );
}

/// The distribution a photo lookup gives, with its note, for a cover
/// whose barcode box shows `shown`, dated `date`, where the research said
/// `said`.
fn distributed(shown: &str, date: &str, said: &str) -> Value {
    let provider = Scripted::new(vec![
        answered(read_answer(
            true,
            &[
                ("title", "Astonishing Robot Tales", "front"),
                ("issue_number", "1", "front"),
                ("barcode_box", shown, "front, bottom left"),
            ],
        )),
        researched(2),
        answered(photo_structured(&[
            ("cover_date", value(date, 1, "high")),
            ("distribution", value(said, 1, "medium")),
        ])),
        no_problems(),
    ]);
    let draft = run_covers(&provider, false).0.unwrap();
    draft.value("distribution").unwrap().clone()
}

#[test]
fn the_barcode_box_on_the_cover_tells_direct_from_newsstand() {
    let plain = distributed("plain barcode", "1979-03", "");
    assert_eq!(plain.text, "newsstand");
    assert!(!plain.accepted);
    assert!(plain.note.contains("barcode box"));
    // Its source is what the cover showed.
    assert!(plain.source.is_some());
    assert_eq!(distributed("Direct Edition", "1992-11", "").text, "direct");
    assert_eq!(distributed("NEWSSTAND", "2015-04", "").text, "newsstand");

    // A plain barcode tells nothing from 2000 on, nor a box not shown.
    assert_eq!(distributed("plain barcode", "2015-04", "").text, "");
    assert_eq!(distributed("", "1979-03", "").text, "");

    // Where the research says otherwise, the owner looks.
    let other = distributed("plain barcode", "1979-03", "direct");
    assert_eq!(other.text, "direct");
    assert!(!other.accepted);
    assert!(other.note.contains("newsstand"));
}

#[test]
fn characters_go_by_the_names_they_are_known_by() {
    let mut answer = full_answer();
    answer["characters"] = json!([
        {"name": "Tinman (Tom Tinsley)", "first_appearance": true, "source": 1, "confidence": "high"},
        {"name": "Tinman", "first_appearance": false, "source": 2, "confidence": "high"},
        {"name": "Professor (Ret.) Gear", "first_appearance": false, "source": 1, "confidence": "high"}
    ]);
    answer["teams"] = json!([
        {"name": "Robot Rangers (Earth-54)", "first_appearance": true, "source": 1, "confidence": "high"}
    ]);
    let provider = Scripted::new(vec![researched(2), answered(answer), no_problems()]);
    let draft = run(&provider, "Astonishing Robot Tales (1954) #1")
        .0
        .unwrap();
    let names = |list: &[Entry]| list.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
    assert_eq!(
        names(&draft.characters),
        ["Tinman", "Professor (Ret.) Gear"]
    );
    assert_eq!(draft.characters[0].detail, Detail::FirstAppearance(true));
    assert_eq!(names(&draft.teams), ["Robot Rangers"]);
}

#[test]
fn a_reading_that_does_not_follow_the_schema_is_refused() {
    let mut extra = robot_cover();
    extra["items"]["secret"] = json!({"text": "x", "where": "y"});
    let mut missing = robot_cover();
    missing["items"].as_object_mut().unwrap().remove("isbn");
    let mut odd = robot_cover();
    odd["mood"] = json!("cheerful");
    for answer in [extra, missing, odd] {
        let provider = Scripted::new(vec![answered(answer)]);
        assert!(matches!(
            run_covers(&provider, false).0,
            Err(LookupError::Unreadable(_))
        ));
    }
    // Text read from a cover is made safe to show, like any answer.
    let reading = Reading::parse(
        &read_answer(true, &[("title", "Robot\u{202e} Tales", "front")]).to_string(),
    )
    .unwrap();
    assert_eq!(reading.get("title").unwrap().text, "Robot Tales");
}

#[test]
fn a_photo_book_costs_its_reading_more_than_a_line() {
    let opus = crate::cost::anthropic_price("claude-opus-5-5").unwrap();
    let settings = Settings::balanced(YEAR);
    let line = estimate_book(&opus, &settings);
    let photo = estimate_photo_book(&opus, &settings, &[(1600, 2400), (1600, 2400)]);
    assert!(photo.likely > line.likely);
    assert!(photo.most > line.most);
    // Two photos at most 4,784 tokens each, and the read step's text.
    let read = planned_read(&[(1600, 2400), (1600, 2400)]);
    assert!(read.input_tokens <= 1_500 + 2 * 4_784);
    assert_eq!(read.images, 2);
    // The research reads the front again before it starts and after each
    // search, and that is counted too.
    let read_only = line.plus(read.estimate(&opus));
    assert!(photo.likely > read_only.likely);
    assert!(photo.most > read_only.most);
}

mod properties {
    //! Whatever the model sends, what reaches the draft has passed its
    //! field's check, and every name is clean and cites a real source.

    use proptest::prelude::*;

    use super::*;

    fn text() -> impl Strategy<Value = String> {
        prop_oneof![
            any::<String>(),
            "[A-Za-z0-9 #$./,;:()\\-]{0,40}",
            "(19|20)[0-9]{2}(-[0-1][0-9](-[0-3][0-9])?)?",
        ]
    }

    /// A structured answer of the right shape, with arbitrary values,
    /// sources, confidences, and lists.
    fn shaped() -> impl Strategy<Value = String> {
        let value = (
            text(),
            -2..5i64,
            prop::sample::select(&["high", "medium", "low"][..]),
        );
        let named = (text(), -2..5i64, any::<prop::sample::Index>());
        (
            prop::collection::vec(value, FIELDS.len()),
            prop::collection::vec(named, 0..12),
            prop::collection::vec((text(), -2..5i64, any::<bool>()), 0..12),
            prop::sample::select(&["identified", "ambiguous", "not_found"][..]),
        )
            .prop_map(|(values, credits, characters, status)| {
                let mut answer = structured(&[]);
                answer["identification"]["status"] = json!(status);
                for (field, (text, source, confidence)) in FIELDS.iter().zip(values) {
                    answer["fields"][field.key] = json!({"value": text, "source": source,
                        "confidence": confidence, "note": ""});
                }
                answer["credits"] = credits
                    .into_iter()
                    .map(|(name, source, role)| {
                        json!({"name": name, "role": role.get(Role::ALL).code(),
                            "source": source, "confidence": "high"})
                    })
                    .collect();
                answer["characters"] = characters
                    .into_iter()
                    .map(|(name, source, first)| {
                        json!({"name": name, "first_appearance": first,
                            "source": source, "confidence": "medium"})
                    })
                    .collect();
                answer.to_string()
            })
    }

    /// The full answer with a few bytes changed, removed, or added.
    fn damaged() -> impl Strategy<Value = String> {
        let valid = full_answer().to_string();
        prop::collection::vec((any::<prop::sample::Index>(), any::<u8>(), 0..3u8), 1..8).prop_map(
            move |edits| {
                let mut bytes = valid.clone().into_bytes();
                for (at, byte, how) in edits {
                    let i = at.index(bytes.len() + 1);
                    match how {
                        0 if i < bytes.len() => bytes[i] = byte,
                        1 if i < bytes.len() => {
                            bytes.remove(i);
                        }
                        _ => bytes.insert(i, byte),
                    }
                }
                String::from_utf8_lossy(&bytes).into_owned()
            },
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn only_checked_values_and_clean_names_reach_the_draft(
            answer in prop_oneof![shaped(), damaged(), any::<String>()],
        ) {
            let provider = Scripted::new(vec![
                researched(2),
                Step::Answer(Reply { answer: Ok(answer), spent: spent(2) }),
                no_problems(),
            ]);
            let Ok(draft) = run(&provider, "ART (1954) #1").0 else {
                return Ok(());
            };
            for value in &draft.values {
                let field = spec(value.key).expect("a known field");
                if !value.text.is_empty() {
                    prop_assert_eq!(
                        check(field.kind, &value.text, YEAR),
                        Ok(value.text.clone()),
                        "{}", value.key
                    );
                }
                prop_assert!(value.source.is_none_or(|s| s < draft.sources.len()));
                prop_assert_eq!(&clean(&value.note, 1_000), &value.note);
            }
            for entry in draft.credits.iter().chain(&draft.characters) {
                prop_assert!(!entry.name.is_empty());
                prop_assert_eq!(&clean(&entry.name, 200), &entry.name);
                prop_assert!(entry.source.is_some_and(|s| s < draft.sources.len()));
            }
        }
    }
}

#[test]
fn a_series_chosen_from_several_waits_for_the_owner() {
    let mut answer = structured(&[
        ("series", value("Astonishing Robot Tales", 1, "high")),
        ("start_year", value("1954", 1, "high")),
        ("publisher", value("Atomic Age", 1, "high")),
    ]);
    answer["identification"]["candidates"] = json!([
        {"series": "Astonishing Robot Tales", "start_year": "1954", "volume": "", "publisher": "Atomic Age", "why": "the original"},
        {"series": "Astonishing Robot Tales", "start_year": "2011", "volume": "", "publisher": "Atomic Age", "why": "a relaunch"}
    ]);
    let provider = Scripted::new(vec![researched(2), answered(answer), no_problems()]);
    let draft = run(&provider, "ART #1").0.unwrap();
    assert_eq!(draft.identity, Identity::Identified);
    let v = |key| draft.value(key).unwrap();
    assert!(!v("series").accepted);
    assert!(!v("start_year").accepted);
    assert!(
        v("start_year")
            .note
            .contains("Astonishing Robot Tales (2011)"),
        "{}",
        v("start_year").note
    );
    assert!(!v("start_year").note.contains("(1954)"));
    assert!(v("publisher").accepted);
}

#[test]
fn a_typed_modern_book_is_dated_by_its_sale_month_for_the_owner_to_check() {
    let answer = structured(&[
        ("series", value("Astonishing Robot Tales", 1, "high")),
        ("cover_date", value("2022-06", 1, "high")),
        ("release_date", value("2022-04-13", 2, "high")),
    ]);
    let provider = Scripted::new(vec![researched(2), answered(answer), no_problems()]);
    let draft = run(&provider, "ART #1").0.unwrap();
    let date = draft.value("cover_date").unwrap();
    assert_eq!(date.text, "2022-04");
    assert!(!date.accepted);
    // An older book keeps the catalogue's cover date.
    let answer = structured(&[
        ("cover_date", value("1965-12", 1, "high")),
        ("release_date", value("1965-10-05", 2, "high")),
    ]);
    let provider = Scripted::new(vec![researched(2), answered(answer), no_problems()]);
    let draft = run(&provider, "ART #1").0.unwrap();
    let date = draft.value("cover_date").unwrap();
    assert_eq!((date.text.as_str(), date.accepted), ("1965-12", true));
}

#[test]
fn an_editor_waits_for_the_owner() {
    let mut answer = structured(&[("series", value("Astonishing Robot Tales", 1, "high"))]);
    answer["credits"] = json!([
        {"name": "Ada Sprocket", "role": "writer", "source": 1, "confidence": "high"},
        {"name": "Rex Rivet", "role": "editor", "source": 1, "confidence": "high"}
    ]);
    let provider = Scripted::new(vec![researched(2), answered(answer), no_problems()]);
    let draft = run(&provider, "ART #1").0.unwrap();
    let held: Vec<(&str, bool)> = draft
        .credits
        .iter()
        .map(|c| (c.name.as_str(), c.accepted))
        .collect();
    assert_eq!(held, [("Ada Sprocket", true), ("Rex Rivet", false)]);
}
