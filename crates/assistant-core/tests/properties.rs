//! Property tests of every parser that reads text from outside the app:
//! the owner's typed lines, the model's answers, and the saved results.
//! Each is given arbitrary and damaged input, and must answer without
//! panicking; what it accepts must read back the same.

use std::str::FromStr;

use assistant_core::lookup::{FIELDS, Kind, Reading, check};
use assistant_core::{
    Barcode, Date, Isbn, IssueNumber, MAX_BOOKS_PER_LINE, PartialDate, Picodollars, cents_string,
    parse_cents, parse_collects, parse_lines, results,
};
use proptest::prelude::*;
use serde_json::{Value as Json, json};

const THIS_YEAR: u16 = 2026;

const KINDS: [Kind; 17] = [
    Kind::Text,
    Kind::LongText,
    Kind::Year,
    Kind::Count,
    Kind::Ordinal,
    Kind::Category,
    Kind::MagazineFormat,
    Kind::YesNo,
    Kind::CoverDate,
    Kind::Date,
    Kind::PartialDate,
    Kind::Price,
    Kind::Isbn,
    Kind::Barcode,
    Kind::Binding,
    Kind::Distribution,
    Kind::Collects,
];

/// Text of the kinds the parsers see: anything at all, or short runs of
/// the characters their formats are made of, which reach further in.
fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        "[0-9 #./,;:()\\-]{0,24}",
        "[A-Za-z0-9 #./,;()\\-]{0,40}",
        "(19|20)[0-9]{2}(-[0-1][0-9](-[0-3][0-9])?)?",
        "\\$?[0-9]{0,6}(\\.[0-9]{0,3})?",
    ]
}

/// `valid` with a few bytes changed, removed, or added, read as text.
fn damaged(valid: &'static str) -> impl Strategy<Value = String> {
    prop::collection::vec((any::<prop::sample::Index>(), any::<u8>(), 0..3u8), 1..8).prop_map(
        move |edits| {
            let mut bytes = valid.as_bytes().to_vec();
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

/// A results file as the app writes them, but with arbitrary text in
/// every place one can hold text, as an edited or damaged file might.
fn results_file() -> impl Strategy<Value = String> {
    let value = (
        prop::sample::select(FIELDS),
        text(),
        0..3usize,
        any::<bool>(),
    );
    (
        text(),
        text(),
        prop::collection::vec((text(), text(), text()), 0..4),
        prop::collection::vec(value, 0..8),
        prop::collection::vec((text(), 0..3usize), 0..4),
        text(),
    )
        .prop_map(|(series, number, sources, values, credits, research)| {
            let values: Vec<Json> = values
                .into_iter()
                .map(|(field, text, source, accepted)| {
                    json!({"key": field.key, "text": text, "source": source,
                        "confidence": "high", "note": "", "accepted": accepted})
                })
                .collect();
            let credits: Vec<Json> = credits
                .into_iter()
                .map(|(name, source)| {
                    json!({"name": name, "role": "writer", "source": source,
                        "confidence": "medium", "note": "", "accepted": true})
                })
                .collect();
            let sources: Vec<Json> = sources
                .into_iter()
                .map(|(url, title, cited)| json!({"url": url, "title": title, "cited_text": cited}))
                .collect();
            json!({"version": 1, "books": [{"included": true, "line": 1, "series": series,
                "start_year": 1954, "volume": null, "number": number, "identity": "identified",
                "candidates": [], "sources": sources, "values": values, "credits": credits,
                "characters": [], "teams": [], "story_arcs": [], "research": research}]})
            .to_string()
        })
}

/// A cover reading with arbitrary text in every item.
fn reading() -> impl Strategy<Value = String> {
    let template: Json = serde_json::from_str(READING).expect("valid");
    let keys: Vec<String> = template["items"]
        .as_object()
        .expect("items")
        .keys()
        .cloned()
        .collect();
    prop::collection::vec((text(), text()), keys.len()).prop_map(move |texts| {
        let mut answer = template.clone();
        for (key, (text, place)) in keys.iter().zip(texts) {
            answer["items"][key] = json!({"text": text, "where": place});
        }
        answer.to_string()
    })
}

const RESULTS: &str = r#"{"version":1,"books":[{"included":true,"line":1,
"series":"Astonishing Robot Tales","start_year":1954,"volume":null,"number":"1",
"identity":"identified","candidates":[],"sources":[{"url":"https://example.org/a",
"title":"A page","cited_text":"Astonishing Robot Tales #1, March 1954"}],
"values":[{"key":"cover_date","text":"1954-03","source":0,"confidence":"high",
"note":"","accepted":true},{"key":"cover_price","text":"0.10","source":0,
"confidence":"high","note":"","accepted":true}],"credits":[{"name":"Ada Sprocket",
"role":"writer","source":0,"confidence":"high","note":"","accepted":true}],
"characters":[],"teams":[],"story_arcs":[],"research":"Notes."}]}"#;

const READING: &str = r##"{"is_cover":true,"kind":"comic","items":{
"title":{"text":"Astonishing Robot Tales","where":"front"},
"issue_number":{"text":"#1","where":"front"},"volume":{"text":"","where":""},
"cover_date":{"text":"MAR 1954","where":"front"},"us_price":{"text":"10¢","where":"front"},
"canadian_price":{"text":"","where":""},"other_price":{"text":"","where":""},
"publisher":{"text":"Atomic Age","where":"front"},"imprint":{"text":"","where":""},
"barcode":{"text":"","where":""},"barcode_box":{"text":"","where":""},
"variant":{"text":"","where":""},"printing":{"text":"","where":""},
"isbn":{"text":"","where":""},"grading_label":{"text":"","where":""},
"cover_artist":{"text":"","where":""}}}"##;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// A value the checks accept is written as they would write it, so
    /// the results file, checked again when read, never refuses it.
    #[test]
    fn checked_values_pass_their_check_again(kind in prop::sample::select(&KINDS[..]), text in text()) {
        if let Ok(written) = check(kind, &text, THIS_YEAR) {
            prop_assert_eq!(check(kind, &written, THIS_YEAR), Ok(written.clone()), "{:?}", kind);
        }
    }

    #[test]
    fn typed_lines_never_panic_and_stay_bounded(text in text()) {
        let (books, problems) = parse_lines(&text, THIS_YEAR);
        let lines = text.lines().count().max(1);
        prop_assert!(books.len() <= lines * MAX_BOOKS_PER_LINE);
        prop_assert!(problems.len() <= lines);
    }

    #[test]
    fn issue_numbers_read_back_the_same(text in text()) {
        if let Ok(number) = IssueNumber::new(&text) {
            prop_assert!(!number.as_str().is_empty());
            prop_assert_eq!(IssueNumber::new(number.as_str()), Ok(number.clone()));
        }
    }

    #[test]
    fn dates_read_back_the_same(text in text()) {
        if let Ok(date) = Date::from_str(&text) {
            prop_assert_eq!(Date::from_str(&date.to_string()), Ok(date));
        }
        if let Ok(date) = PartialDate::from_str(&text) {
            prop_assert_eq!(PartialDate::from_str(&date.to_string()), Ok(date));
        }
    }

    #[test]
    fn prices_read_back_the_same(text in text()) {
        if let Ok(cents) = parse_cents(&text) {
            prop_assert!(cents >= 0);
            prop_assert_eq!(parse_cents(&cents_string(cents)), Ok(cents));
        }
        if let Some(amount) = Picodollars::parse(&text) {
            prop_assert!(amount.0 >= 0);
        }
    }

    #[test]
    fn codes_are_digits_with_a_true_check_digit(text in text()) {
        if let Some(barcode) = Barcode::parse(&text) {
            prop_assert!(barcode.as_str().bytes().all(|b| b.is_ascii_digit()));
            prop_assert_eq!(Barcode::parse(barcode.as_str()), Some(barcode.clone()));
        }
        if let Some(isbn) = Isbn::parse(&text) {
            prop_assert_eq!(Isbn::parse(isbn.as_str()), Some(isbn.clone()));
        }
    }

    #[test]
    fn collects_lists_never_panic(text in text()) {
        let _ = parse_collects(&text);
    }

    /// A damaged results file is refused or read, never a crash, and what
    /// is read is saved and read again unchanged.
    #[test]
    fn results_files_read_back_the_same(
        file in prop_oneof![damaged(RESULTS), results_file(), any::<String>()],
    ) {
        if let Ok(books) = results::from_json(&file, THIS_YEAR) {
            let again = results::from_json(&results::to_json(&books), THIS_YEAR);
            prop_assert_eq!(again, Ok(books));
        }
    }

    #[test]
    fn cover_readings_never_panic(
        answer in prop_oneof![damaged(READING), reading(), any::<String>()],
    ) {
        if let Ok(reading) = Reading::parse(&answer) {
            let _ = reading.query(1);
            let _ = reading.barcode();
            let _ = reading.describe();
        }
    }
}

#[test]
fn the_valid_inputs_used_above_are_read() {
    assert_eq!(results::from_json(RESULTS, THIS_YEAR).unwrap().len(), 1);
    let reading = Reading::parse(READING).unwrap();
    assert_eq!(reading.query(1).unwrap().number.as_str(), "1");
}
