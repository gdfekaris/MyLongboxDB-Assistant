//! What the two providers' clients share: the parts of a request, error
//! bodies made safe to show, and the search sources an answer cites.

use crate::http::{NetError, Response};

/// The longest provider error message kept.
const MAX_MESSAGE_CHARS: usize = 300;

/// A request's text and photos, the same for either provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text(String),
    /// A JPEG, re-encoded from pixels so it carries no metadata.
    Jpeg(Vec<u8>),
}

/// A web page an answer cites, as the provider reported it. Shown to the
/// owner as text; the app never opens it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Citation {
    pub url: String,
    pub title: String,
    /// The words cited, when the provider gives them.
    pub cited_text: String,
}

/// How hard the model should think, where the provider lets the app say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    pub fn code(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// A structured answer's JSON Schema, which the provider holds the
/// answer to.
#[derive(Clone, Debug, PartialEq)]
pub struct Schema {
    /// A short name, which OpenRouter requires.
    pub name: &'static str,
    pub schema: serde_json::Value,
}

/// Text from a provider, made safe to show: the key taken out wherever it
/// appears, control characters dropped, and the length capped.
pub(crate) fn scrubbed(text: &str, key: &str) -> String {
    // Hidden characters go first, so one inside an echoed key cannot keep
    // it from being found, and then show it whole once dropped, or show
    // it as is if never dropped.
    let shown: String = text.chars().filter(|c| !hidden(*c)).collect();
    let shown = if key.is_empty() {
        shown
    } else {
        shown.replace(key, "[key]")
    };
    shown.chars().take(MAX_MESSAGE_CHARS).collect()
}

/// Control characters, and those that take up no space or change the
/// direction of text.
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{00AD}' | '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

/// The error a non-success response stands for, from the error body
/// either provider sends: `{"error": {"type" or "code", "message"}}`.
pub(crate) fn status_error(response: &Response, key: &str) -> NetError {
    let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap_or_default();
    let error = &body["error"];
    let kind = match &error["type"] {
        serde_json::Value::String(kind) => kind.clone(),
        _ => match &error["code"] {
            serde_json::Value::Number(code) => code.to_string(),
            serde_json::Value::String(code) => code.clone(),
            _ => String::new(),
        },
    };
    NetError::Status {
        status: response.status,
        kind: scrubbed(&kind, key),
        message: scrubbed(error["message"].as_str().unwrap_or(""), key),
        retry_after: response.retry_after,
    }
}

/// Reads a JSON response body into `T`.
pub(crate) fn parse<T: serde::de::DeserializeOwned>(response: &Response) -> Result<T, NetError> {
    serde_json::from_slice(&response.body).map_err(|e| {
        // serde's messages name the field and its place, not its value.
        NetError::BadResponse(e.to_string().chars().take(MAX_MESSAGE_CHARS).collect())
    })
}

/// Whether bytes start as a JPEG does.
pub(crate) fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &str) -> Response {
        Response {
            status,
            retry_after: None,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn error_bodies_of_both_providers_are_read() {
        let anthropic = response(
            429,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"Slow down"},"request_id":"req_1"}"#,
        );
        assert_eq!(
            status_error(&anthropic, "k"),
            NetError::Status {
                status: 429,
                kind: "rate_limit_error".into(),
                message: "Slow down".into(),
                retry_after: None,
            }
        );
        let openrouter = response(
            402,
            r#"{"error":{"code":402,"message":"Insufficient credits"}}"#,
        );
        assert!(matches!(
            status_error(&openrouter, "k"),
            NetError::Status { status: 402, kind, .. } if kind == "402"
        ));
        let html = response(502, "<html>Bad gateway</html>");
        assert!(matches!(
            status_error(&html, "k"),
            NetError::Status { status: 502, kind, message, .. } if kind.is_empty() && message.is_empty()
        ));
    }

    #[test]
    fn the_key_never_reaches_an_error() {
        let key = "sk-ant-api03-TESTONLY";
        let echoed = response(
            401,
            r#"{"error":{"type":"authentication_error","message":"invalid x-api-key sk-ant-api03-TESTONLY\n"}}"#,
        );
        let shown = status_error(&echoed, key).to_string();
        assert!(!shown.contains("TESTONLY"));
        assert!(shown.contains("[key]"));
        assert!(!shown.contains('\n'));
        // Broken by a character that does not show, it is still found.
        let hidden = response(
            401,
            "{\"error\":{\"message\":\"bad key sk-ant-api03-TEST\u{200B}ONLY\"}}",
        );
        let shown = status_error(&hidden, key).to_string();
        assert!(!shown.contains("TEST"), "{shown}");
        assert!(shown.contains("[key]"));
    }

    mod properties {
        use proptest::prelude::*;

        use super::*;
        use crate::arbitrary;

        const KEY: &str = "sk-ant-api03-TESTONLY";

        /// An error body that may echo the key anywhere, as a careless
        /// provider might.
        fn echoing() -> impl Strategy<Value = Vec<u8>> {
            prop_oneof![
                arbitrary::body(),
                (any::<String>(), any::<String>(), any::<String>()).prop_map(|(a, b, kind)| {
                    serde_json::to_vec(&serde_json::json!({"error": {
                        "type": format!("{kind}{KEY}"),
                        "message": format!("{a}{KEY}{b}\n{KEY}"),
                    }}))
                    .unwrap_or_default()
                }),
                // The key broken by a character that showing it would drop
                // or not show at all.
                (
                    1..KEY.len(),
                    prop::sample::select(&['\n', '\t', '\u{200B}', '\u{2060}', '\u{FEFF}'][..]),
                )
                    .prop_map(|(at, mark)| {
                        let broken = format!("{}{mark}{}", &KEY[..at], &KEY[at..]);
                        serde_json::to_vec(&serde_json::json!({"error": {"message": broken}}))
                            .unwrap_or_default()
                    }),
            ]
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(512))]

            /// What an error shows never holds the key or a control
            /// character, and its parts are short.
            #[test]
            fn errors_never_show_the_key(status in 400..600u16, body in echoing()) {
                let response = Response { status, retry_after: None, body };
                let NetError::Status { kind, message, .. } = status_error(&response, KEY) else {
                    panic!("an error status is a status error");
                };
                for part in [&kind, &message] {
                    prop_assert!(!part.contains(KEY));
                    prop_assert!(!part.chars().any(char::is_control));
                    prop_assert!(part.chars().count() <= MAX_MESSAGE_CHARS);
                }
            }
        }
    }

    #[test]
    fn long_messages_are_cut() {
        assert_eq!(scrubbed(&"a".repeat(10_000), "k").len(), MAX_MESSAGE_CHARS);
    }
}
