//! The first step of a photo lookup: the model reads what is printed on
//! the owner's covers, and nothing more. What it reads becomes the query
//! the later steps look up, and each item becomes a source they can cite,
//! so a value taken from the cover shows that it was, and the check step
//! holds it to the words read.

use serde::Deserialize;
use serde_json::{Value, json};

use super::draft::Source;
use super::fields::clean;
use crate::codes::Barcode;
use crate::issue_number::IssueNumber;
use crate::lines::BookQuery;

/// The address given to sources read from the owner's cover photos. It
/// has a space, so no web page's address can be the same.
pub const COVER_PHOTO: &str = "cover photo";

/// The longest text kept from one item.
const MAX_ITEM: usize = 200;

/// The things the read step looks for, with their labels on the review
/// page and what the model is told each holds.
const ITEMS: &[(&str, &str, &str)] = &[
    (
        "title",
        "title",
        "The series title as printed, usually the logo, without the issue number.",
    ),
    (
        "issue_number",
        "issue number",
        "The issue number as printed, such as 300, 1/2, or Annual 3, without # or No.",
    ),
    (
        "volume",
        "volume",
        "The volume number where the cover prints one, such as Vol. 2.",
    ),
    (
        "cover_date",
        "cover date",
        "The month and year as printed, such as March 1954 or Mar 1954.",
    ),
    (
        "us_price",
        "US price",
        "The US price as printed, with its mark, such as 35¢ or $3.99.",
    ),
    (
        "canadian_price",
        "Canadian price",
        "The Canadian price as printed, such as CAN $4.99 or 45¢ Canada.",
    ),
    (
        "other_price",
        "other price",
        "Any other price as printed, such as £1.25.",
    ),
    (
        "publisher",
        "publisher",
        "The publisher's name or mark, such as Marvel or DC.",
    ),
    (
        "imprint",
        "imprint",
        "An imprint's name or mark, such as Vertigo or Epic.",
    ),
    (
        "barcode",
        "barcode digits",
        "Every digit printed with the barcode, the small add-on included, in order; blank unless all are legible.",
    ),
    (
        "barcode_box",
        "barcode box",
        "What the barcode box shows: plain barcode when it holds a barcode with no words naming an edition; the words in or beside it, such as Direct Edition, Direct Sales, or Newsstand; or what is pictured in place of a barcode. no barcode when the cover has no barcode box at all. Blank only when the photos do not show the part of the cover where it would be.",
    ),
    (
        "variant",
        "variant marks",
        "Words marking a variant cover, such as Variant Edition, Cover B, or 1:25.",
    ),
    (
        "printing",
        "printing marks",
        "Words naming a later printing, such as Second Printing or 2nd Print.",
    ),
    ("isbn", "ISBN", "The ISBN as printed, usually on the back."),
    (
        "grading_label",
        "grading label",
        "Everything printed on a grading company's label, such as CGC or CBCS, if the book is in a graded case: title, number, date, printing, and notes; blank if there is none.",
    ),
    (
        "cover_artist",
        "cover artist's signature",
        "A signature legible on the cover art.",
    ),
];

/// What the read step answers.
pub fn schema() -> Value {
    let item = json!({
        "type": "object",
        "properties": {
            "text": {"type": "string"},
            "where": {"type": "string"},
        },
        "required": ["text", "where"],
        "additionalProperties": false,
    });
    let keys: Vec<&str> = ITEMS.iter().map(|(k, _, _)| *k).collect();
    let items: serde_json::Map<String, Value> = keys
        .iter()
        .map(|k| ((*k).to_owned(), json!({"$ref": "#/$defs/item"})))
        .collect();
    json!({
        "type": "object",
        "properties": {
            "is_cover": {"type": "boolean"},
            "kind": {"type": "string", "enum": ["comic", "magazine", "collected", "unsure"]},
            "items": {
                "type": "object",
                "properties": items,
                "required": keys,
                "additionalProperties": false,
            },
        },
        "required": ["is_cover", "kind", "items"],
        "additionalProperties": false,
        "$defs": {"item": item},
    })
}

/// The read step's instructions, with the items it looks for.
pub fn prompt() -> String {
    let mut text = String::from(super::prompts::READ);
    for (key, _, hint) in ITEMS {
        text.push_str(&format!("- {key}: {hint}\n"));
    }
    text
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    is_cover: bool,
    kind: String,
    items: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    text: String,
    #[serde(rename = "where")]
    place: String,
}

/// One thing read from the covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub key: &'static str,
    pub label: &'static str,
    pub text: String,
    pub place: String,
}

/// What the read step found on the covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    pub is_cover: bool,
    /// comic, magazine, or collected, or empty when unsure.
    pub kind: String,
    /// The items read, blank ones left out.
    pub seen: Vec<Seen>,
}

impl Reading {
    /// The read step's answer, read as strictly as any model answer.
    pub fn parse(json: &str) -> Result<Self, String> {
        let answer: Answer = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut seen = Vec::new();
        for (key, label, _) in ITEMS {
            let Some(item) = answer.items.get(*key) else {
                return Err(format!("the answer has no {key}"));
            };
            let item: Item = serde_json::from_value(item.clone()).map_err(|e| e.to_string())?;
            let text = clean(&item.text, MAX_ITEM);
            if !text.is_empty() {
                seen.push(Seen {
                    key,
                    label,
                    text,
                    place: clean(&item.place, MAX_ITEM),
                });
            }
        }
        if answer.items.len() != ITEMS.len() {
            return Err("the answer has items the schema does not name".into());
        }
        let kind = match answer.kind.as_str() {
            k @ ("comic" | "magazine" | "collected") => k.to_owned(),
            _ => String::new(),
        };
        Ok(Self {
            is_cover: answer.is_cover,
            kind,
            seen,
        })
    }

    pub fn get(&self, key: &str) -> Option<&Seen> {
        self.seen.iter().find(|s| s.key == key)
    }

    /// The issue number the covers print, or failing that a grading
    /// label's, which the grading company printed from the book itself.
    pub fn printed_number(&self) -> Option<IssueNumber> {
        let printed = self
            .get("issue_number")
            .map(|s| issue_number(&s.text))
            .filter(|n| !n.is_empty())
            .or_else(|| {
                self.get("grading_label")
                    .and_then(|s| label_number(&s.text))
            })?;
        IssueNumber::new(&printed).ok()
    }

    /// The book to look up, or why the covers do not name one. A book
    /// whose number is printed nowhere is looked up by its other details,
    /// as nn until the research finds its number.
    pub fn query(&self, line: usize) -> Result<BookQuery, String> {
        if !self.is_cover {
            return Err("the photo does not show a book's cover".into());
        }
        let series = self
            .get("title")
            .map(|s| s.text.clone())
            .ok_or("no title could be read on the cover")?;
        let number = match self.printed_number() {
            Some(number) => number,
            None => IssueNumber::new("nn").map_err(|e| e.to_string())?,
        };
        let volume = self.get("volume").and_then(|s| {
            let digits: String = s.text.chars().filter(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok().filter(|v| *v >= 1)
        });
        Ok(BookQuery {
            line,
            series,
            start_year: None,
            volume,
            number,
        })
    }

    /// The barcode read, if every digit was and its check digit holds.
    pub fn barcode(&self) -> Option<Barcode> {
        self.get("barcode").and_then(|s| Barcode::parse(&s.text))
    }

    /// Each item as a source the later steps can cite.
    pub fn sources(&self) -> Vec<Source> {
        self.seen
            .iter()
            .map(|s| Source {
                url: COVER_PHOTO.into(),
                title: if s.place.is_empty() {
                    format!("Your cover photo: {}", s.label)
                } else {
                    format!("Your cover photo: {}, {}", s.label, s.place)
                },
                cited_text: s.text.clone(),
            })
            .collect()
    }

    /// What was read, for the research step.
    pub fn describe(&self) -> String {
        let mut text = String::from("Read from the owner's cover photos:\n");
        if !self.kind.is_empty() {
            text.push_str(&format!("- kind of book: {}\n", self.kind));
        }
        for s in &self.seen {
            text.push_str(&format!("- {}: \"{}\" ({})\n", s.label, s.text, s.place));
        }
        text
    }
}

/// The number a grading label gives after its title, as in
/// "Teenage Mutant Ninja Turtles #1 Mirage Studios, 2/85".
fn label_number(label: &str) -> Option<String> {
    let after = label.split_once('#')?.1;
    let number: String = after
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ',')
        .collect();
    (!number.is_empty()).then_some(number)
}

/// Numbers a cover may spell out, as Teenage Mutant Ninja Turtles: The
/// Last Ronin prints ONE.
const SPELLED: [&str; 20] = [
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
];

/// An issue number as printed, without the marks that only label it,
/// and as it is catalogued: in digits, without the leading zeros some
/// covers print, as 010 for #10.
fn issue_number(printed: &str) -> String {
    let mut text = printed.trim();
    for mark in ["#", "No.", "No ", "Number "] {
        if let Some(rest) = text.strip_prefix(mark) {
            text = rest.trim_start();
        }
    }
    if let Some(n) = SPELLED.iter().position(|w| w.eq_ignore_ascii_case(text)) {
        return (n + 1).to_string();
    }
    if text.len() > 1 && text.bytes().all(|b| b.is_ascii_digit()) {
        let digits = text.trim_start_matches('0');
        return if digits.is_empty() { "0" } else { digits }.to_owned();
    }
    text.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printed_marks_are_not_part_of_the_number() {
        assert_eq!(issue_number("#300"), "300");
        assert_eq!(issue_number("No. 7"), "7");
        assert_eq!(issue_number("Annual 3"), "Annual 3");
        assert_eq!(issue_number(" # 1/2 "), "1/2");
        assert_eq!(issue_number("010"), "10");
        assert_eq!(issue_number("00"), "0");
        assert_eq!(issue_number("0"), "0");
        assert_eq!(issue_number("ONE"), "1");
        assert_eq!(issue_number("Twelve"), "12");
        assert_eq!(issue_number("One-Shot"), "One-Shot");
    }

    #[test]
    fn a_grading_label_gives_the_number_after_the_title() {
        assert_eq!(
            label_number("Teenage Mutant Ninja Turtles #1 Mirage Studios, 2/85").as_deref(),
            Some("1")
        );
        assert_eq!(
            label_number("Spider-Man #1/2, 12/98").as_deref(),
            Some("1/2")
        );
        assert_eq!(label_number("Marvel Treasury Special nn"), None);
    }
}
