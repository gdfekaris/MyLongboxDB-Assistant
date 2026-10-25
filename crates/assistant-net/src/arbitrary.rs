//! Arbitrary JSON for the property tests of the response readers: any
//! shape, with the keys the providers' answers use more often than
//! chance would give them.

use proptest::prelude::*;
use serde_json::{Map, Value};

const KEYS: [&str; 16] = [
    "type",
    "text",
    "citations",
    "url",
    "title",
    "cited_text",
    "content",
    "error",
    "message",
    "code",
    "choices",
    "annotations",
    "url_citation",
    "usage",
    "stop_reason",
    "error_code",
];

fn key() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(&KEYS[..]).prop_map(str::to_owned),
        any::<String>(),
    ]
}

fn leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<f64>().prop_map(Value::from),
        any::<String>().prop_map(Value::from),
        prop::sample::select(&KEYS[..]).prop_map(Value::from),
    ]
}

/// Any JSON value, a few levels deep.
pub(crate) fn json() -> impl Strategy<Value = Value> {
    leaf().prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::vec((key(), inner), 0..8)
                .prop_map(|pairs| Value::Object(pairs.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

/// Bytes a response body might hold: arbitrary JSON, or arbitrary bytes.
pub(crate) fn body() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        json().prop_map(|v| serde_json::to_vec(&v).unwrap_or_default()),
        prop::collection::vec(any::<u8>(), 0..256),
    ]
}
