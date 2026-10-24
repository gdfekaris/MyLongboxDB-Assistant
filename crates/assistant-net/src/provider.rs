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

/// An amount of US dollars in trillionths (picodollars), exact and in
/// whole units: per-token prices are fractions of a cent, and money is
/// never held in floating point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Picodollars(pub i64);

impl Picodollars {
    /// Reads a non-negative decimal dollar amount such as `0.000004`
    /// exactly; `None` if it has more than 12 decimal places, a sign, an
    /// exponent, or anything else.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
        let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        if whole.is_empty() && frac.is_empty() || !digits(whole) || !digits(frac) {
            return None;
        }
        let frac = frac.trim_end_matches('0');
        if frac.len() > 12 {
            return None;
        }
        let mut units: i64 = 0;
        let padded = whole
            .bytes()
            .chain(frac.bytes())
            .chain(std::iter::repeat_n(b'0', 12 - frac.len()));
        for digit in padded {
            units = units
                .checked_mul(10)?
                .checked_add(i64::from(digit - b'0'))?;
        }
        Some(Self(units))
    }
}

/// Text from a provider, made safe to show: the key taken out wherever it
/// appears, control characters dropped, and the length capped.
pub(crate) fn scrubbed(text: &str, key: &str) -> String {
    let text = if key.is_empty() {
        text.to_owned()
    } else {
        text.replace(key, "[key]")
    };
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_MESSAGE_CHARS)
        .collect()
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
    }

    #[test]
    fn prices_are_read_exactly() {
        assert_eq!(Picodollars::parse("0.000004"), Some(Picodollars(4_000_000)));
        assert_eq!(
            Picodollars::parse("10"),
            Some(Picodollars(10_000_000_000_000))
        );
        assert_eq!(Picodollars::parse("0.000000000001"), Some(Picodollars(1)));
        assert_eq!(Picodollars::parse("0.0000000000001"), None);
        for bad in ["", ".", "-1", "1e-6", "0.1.2", "$1", "NaN"] {
            assert_eq!(Picodollars::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn long_messages_are_cut() {
        assert_eq!(scrubbed(&"a".repeat(10_000), "k").len(), MAX_MESSAGE_CHARS);
    }
}
