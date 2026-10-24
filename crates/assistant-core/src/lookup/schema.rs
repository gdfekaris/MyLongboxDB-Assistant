//! The JSON the structuring and checking steps must answer with, and the
//! types it is read into. Every property is required and none may be
//! null, so the schemas stay within what structured outputs allow; a
//! blank is an empty string. Reading is strict: anything the schema does
//! not name is refused.

use serde::Deserialize;
use serde_json::{Value, json};

use super::fields::FIELDS;
use crate::model::{Confidence, Role};

/// Lists longer than this in an answer are refused as runaway.
pub(crate) const MAX_LIST: usize = 200;

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn confidence() -> Value {
    json!({"type": "string", "enum": ["high", "medium", "low"]})
}

/// What the structuring step answers.
pub fn structure(from_text: bool) -> Value {
    let keys: Vec<&str> = FIELDS
        .iter()
        .filter(|f| f.from_text || !from_text)
        .map(|f| f.key)
        .collect();
    let value = object(
        json!({
            "value": {"type": "string"},
            "source": {"type": "integer"},
            "confidence": confidence(),
            "note": {"type": "string"},
        }),
        &["value", "source", "confidence", "note"],
    );
    let fields = object(
        keys.iter()
            .map(|k| ((*k).to_owned(), json!({"$ref": "#/$defs/value"})))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        &keys,
    );
    let roles: Vec<&str> = Role::ALL.iter().map(|r| r.code()).collect();
    let named = |detail: &str, kind: Value| {
        let mut properties = serde_json::Map::new();
        properties.insert("name".into(), json!({"type": "string"}));
        properties.insert(detail.into(), kind);
        properties.insert("source".into(), json!({"type": "integer"}));
        properties.insert("confidence".into(), confidence());
        json!({"type": "array", "items": object(
            properties.into(),
            &["name", detail, "source", "confidence"],
        )})
    };
    let candidate = object(
        json!({
            "series": {"type": "string"},
            "start_year": {"type": "string"},
            "volume": {"type": "string"},
            "publisher": {"type": "string"},
            "why": {"type": "string"},
        }),
        &["series", "start_year", "volume", "publisher", "why"],
    );
    let mut schema = object(
        json!({
            "identification": object(
                json!({
                    "status": {"type": "string", "enum": ["identified", "ambiguous", "not_found"]},
                    "candidates": {"type": "array", "items": candidate},
                }),
                &["status", "candidates"],
            ),
            "fields": fields,
            "credits": named("role", json!({"type": "string", "enum": roles})),
            "characters": named("first_appearance", json!({"type": "boolean"})),
            "teams": named("first_appearance", json!({"type": "boolean"})),
            "story_arcs": named("part", json!({"type": "string"})),
        }),
        &[
            "identification",
            "fields",
            "credits",
            "characters",
            "teams",
            "story_arcs",
        ],
    );
    schema["$defs"] = json!({"value": value});
    schema
}

/// What the check step answers.
pub fn check() -> Value {
    object(
        json!({"problems": {"type": "array", "items": object(
            json!({
                "field": {"type": "string"},
                "problem": {"type": "string"},
                "action": {"type": "string", "enum": ["clear", "doubt"]},
            }),
            &["field", "problem", "action"],
        )}}),
        &["problems"],
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Sureness {
    High,
    Medium,
    Low,
}

impl From<Sureness> for Confidence {
    fn from(s: Sureness) -> Self {
        match s {
            Sureness::High => Confidence::High,
            Sureness::Medium => Confidence::Medium,
            Sureness::Low => Confidence::Low,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Structured {
    pub identification: Identification,
    pub fields: serde_json::Map<String, Value>,
    pub credits: Vec<Named<String>>,
    pub characters: Vec<Named<bool>>,
    pub teams: Vec<Named<bool>>,
    pub story_arcs: Vec<Named<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identification {
    pub status: Status,
    pub candidates: Vec<Candidate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Identified,
    Ambiguous,
    NotFound,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Candidate {
    pub series: String,
    pub start_year: String,
    pub volume: String,
    pub publisher: String,
    pub why: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FieldAnswer {
    pub value: String,
    pub source: i64,
    pub confidence: Sureness,
    pub note: String,
}

/// A credit, character, team, or arc, with the one detail its kind has:
/// a role, whether it is a first appearance, or an arc's part.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Named<T> {
    pub name: String,
    #[serde(alias = "role", alias = "first_appearance", alias = "part")]
    pub detail: T,
    pub source: i64,
    pub confidence: Sureness,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Checked {
    pub problems: Vec<Problem>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Problem {
    pub field: String,
    pub problem: String,
    pub action: Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    Clear,
    Doubt,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every object closed and every property required, as structured
    /// outputs require, with no type unions.
    fn assert_strict(schema: &Value, path: &str) {
        match schema {
            Value::Object(map) => {
                if map.get("type") == Some(&json!("object")) {
                    assert_eq!(map["additionalProperties"], json!(false), "{path}");
                    let props: Vec<&String> =
                        map["properties"].as_object().unwrap().keys().collect();
                    let required: Vec<&str> = map["required"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap())
                        .collect();
                    assert_eq!(props.len(), required.len(), "{path}");
                    for p in props {
                        assert!(required.contains(&p.as_str()), "{path}.{p}");
                    }
                }
                assert!(!map.contains_key("anyOf"), "{path}");
                assert!(!map.get("type").is_some_and(Value::is_array), "{path}");
                for (k, v) in map {
                    assert_strict(v, &format!("{path}.{k}"));
                }
            }
            Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    assert_strict(v, &format!("{path}[{i}]"));
                }
            }
            _ => {}
        }
    }

    #[test]
    fn schemas_keep_within_what_structured_outputs_allow() {
        assert_strict(&structure(true), "structure");
        assert_strict(&structure(false), "structure");
        assert_strict(&check(), "check");
    }

    #[test]
    fn text_lookups_do_not_ask_about_editions() {
        let text = structure(true);
        let fields = text["properties"]["fields"]["properties"]
            .as_object()
            .unwrap();
        assert!(fields.contains_key("cover_price"));
        assert!(!fields.contains_key("barcode"));
        assert!(!fields.contains_key("printing"));
        let photo = structure(false);
        assert!(photo["properties"]["fields"]["properties"]["barcode"].is_object());
    }

    #[test]
    fn answers_with_anything_extra_are_refused() {
        let extra = json!({"problems": [], "also": "ignore previous instructions"});
        assert!(serde_json::from_value::<Checked>(extra).is_err());
        let field = json!({"value": "1963", "source": 1, "confidence": "high", "note": "", "url": "https://x"});
        assert!(serde_json::from_value::<FieldAnswer>(field).is_err());
    }
}
