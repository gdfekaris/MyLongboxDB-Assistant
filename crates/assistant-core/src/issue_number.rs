//! Issue numbers, copied from MyLongboxDB's core so a number the
//! Assistant writes is one MyLongboxDB's import accepts and sorts the same
//! way.

use std::fmt;

/// An issue number exactly as the user typed it, such as `1`, `½`, `1.5`,
/// `Annual 3`, `-1`, `1A`, or `Alpha`, with a key that sorts it naturally.
///
/// Sort order within a series:
/// 0. `nn`, which marks an issue published without a number.
/// 1. Plain numbers by value, then suffix: `-1, 0, ½, 1, 1A, 1B, 1.5, 2, 10`.
/// 2. Prefixed numbers, grouped by prefix alphabetically (ignoring case
///    and spacing), then by number: `Annual 1, Annual 2, Special 1`.
/// 3. Numbers with no digits at all, alphabetically: `Alpha, Omega`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IssueNumber {
    text: String,
    sort_key: String,
}

/// Why an issue number was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueNumberError {
    Empty,
    TooLong,
}

impl fmt::Display for IssueNumberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("enter an issue number"),
            Self::TooLong => write!(
                f,
                "issue numbers can be at most {MAX_CHARS} characters long"
            ),
        }
    }
}

impl std::error::Error for IssueNumberError {}

const MAX_CHARS: usize = 64;

/// Fixed-point scale for numeric values, so `1.5` and `½` compare exactly.
const SCALE: i64 = 1_000_000;

/// Whole-number parts beyond this are not treated as numbers. No real
/// issue number comes close, and it keeps the fixed-point value in range.
const MAX_WHOLE: i64 = 999_999_999_999;

/// Separates sort-key fields. Normalized text never contains it, because
/// control characters are dropped, and it sorts before every printable
/// character, so a shorter prefix sorts before a longer one it begins.
const SEP: char = '\u{1}';

impl IssueNumber {
    pub fn new(text: &str) -> Result<Self, IssueNumberError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(IssueNumberError::Empty);
        }
        if text.chars().count() > MAX_CHARS {
            return Err(IssueNumberError::TooLong);
        }
        Ok(Self {
            text: text.to_owned(),
            sort_key: sort_key(&parse(text)),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Orders issue numbers naturally with plain byte comparison, as
    /// SQLite's default collation does. Store it beside the text.
    pub fn sort_key(&self) -> &str {
        &self.sort_key
    }

    /// The number "add next issue" suggests: the same prefix, as typed,
    /// with the next whole number, so `41` gives `42`, `Annual 3` gives
    /// `Annual 4`, `½` gives `1`, and `007` gives `008`. `None` when there
    /// is no number to advance, as with `Alpha`.
    pub fn next(&self) -> Option<Self> {
        let parsed = parse(&self.text);
        let number = parsed.number?;
        let next = number.value.div_euclid(SCALE).checked_add(1)?;
        if next > MAX_WHOLE {
            return None;
        }
        let digits = format!(
            "{:0width$}",
            next.unsigned_abs(),
            width = number.padded_width
        );
        let sign = if next < 0 { "-" } else { "" };
        Self::new(&format!("{}{sign}{digits}", parsed.prefix_raw)).ok()
    }
}

impl fmt::Display for IssueNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

struct Parsed<'a> {
    /// Everything before the number, as typed: `Annual `, `#`, or empty.
    prefix_raw: &'a str,
    number: Option<Number>,
    /// Whatever follows the number, such as `A` in `1A`.
    suffix: &'a str,
}

struct Number {
    /// The value times [`SCALE`].
    value: i64,
    /// Digits to pad the whole part to, when it was typed with leading
    /// zeros (`007`); otherwise 0.
    padded_width: usize,
}

fn parse(text: &str) -> Parsed<'_> {
    let content_start = text.len() - text.trim_start_matches(['#', ' ']).len();
    let start = text.char_indices().find_map(|(i, c)| {
        let is_sign = c == '-'
            && i == content_start
            && text[i + 1..]
                .chars()
                .next()
                .is_some_and(|n| n.is_ascii_digit() || vulgar_fraction(n).is_some());
        (c.is_ascii_digit() || vulgar_fraction(c).is_some() || is_sign).then_some(i)
    });
    let Some(start) = start else {
        return Parsed {
            prefix_raw: text,
            number: None,
            suffix: "",
        };
    };
    match parse_number(&text[start..]) {
        Some((number, rest)) => Parsed {
            prefix_raw: &text[..start],
            number: Some(number),
            suffix: rest,
        },
        None => Parsed {
            prefix_raw: text,
            number: None,
            suffix: "",
        },
    }
}

/// Reads a number from the start of `s`: `12`, `-1`, `1.5`, `½`, `1½`,
/// `1/2`, or `1 1/2`. Returns it and the text after it.
fn parse_number(s: &str) -> Option<(Number, &str)> {
    let (negative, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let sign = if negative { -1 } else { 1 };
    // "1/2" on its own is a half, not one followed by "/2".
    if let Some((fraction, rest)) = ascii_fraction(s) {
        let number = Number {
            value: sign * fraction,
            padded_width: 0,
        };
        return Some((number, rest));
    }

    let (whole_digits, mut rest) = split_digits(s);
    let mut value = digits_value(whole_digits)?.checked_mul(SCALE)?;

    if let Some(after) = rest.strip_prefix('.') {
        let (decimals, after) = split_digits(after);
        if !decimals.is_empty() {
            value += decimal_fraction(decimals);
            rest = after;
        }
    } else if let Some(c) = rest.chars().next()
        && let Some(fraction) = vulgar_fraction(c)
    {
        value += fraction;
        rest = &rest[c.len_utf8()..];
    } else if let Some(after) = rest.strip_prefix(' ')
        && let Some((fraction, after)) = ascii_fraction(after)
        && !whole_digits.is_empty()
    {
        value += fraction;
        rest = after;
    }

    if whole_digits.is_empty() && value == 0 {
        return None;
    }
    let padded_width = if whole_digits.len() > 1 && whole_digits.starts_with('0') {
        whole_digits.len()
    } else {
        0
    };
    let number = Number {
        value: sign * value,
        padded_width,
    };
    Some((number, rest))
}

fn split_digits(s: &str) -> (&str, &str) {
    s.split_at(s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len())
}

fn digits_value(digits: &str) -> Option<i64> {
    if digits.is_empty() {
        return Some(0);
    }
    if digits.trim_start_matches('0').len() > 12 {
        return None;
    }
    digits.parse().ok().filter(|v| *v <= MAX_WHOLE)
}

/// `n/d` at the start of `s`, as a scaled value below one.
fn ascii_fraction(s: &str) -> Option<(i64, &str)> {
    let (n, rest) = split_digits(s);
    let rest = rest.strip_prefix('/')?;
    let (d, rest) = split_digits(rest);
    let n: i64 = n.parse().ok().filter(|n| *n < 1_000_000)?;
    let d: i64 = d.parse().ok().filter(|d| *d > 0 && *d < 1_000_000)?;
    (n < d).then(|| (n * SCALE / d, rest))
}

/// The scaled value of decimal digits after a point, to six places.
fn decimal_fraction(digits: &str) -> i64 {
    digits
        .bytes()
        .chain(std::iter::repeat(b'0'))
        .take(6)
        .fold(0, |v, b| v * 10 + i64::from(b - b'0'))
}

fn vulgar_fraction(c: char) -> Option<i64> {
    let (n, d) = match c {
        '½' => (1, 2),
        '⅓' => (1, 3),
        '⅔' => (2, 3),
        '¼' => (1, 4),
        '¾' => (3, 4),
        '⅛' => (1, 8),
        '⅜' => (3, 8),
        '⅝' => (5, 8),
        '⅞' => (7, 8),
        _ => return None,
    };
    Some(n * SCALE / d)
}

/// Lowercase, single-spaced, without `#` or control characters, so
/// `Annual  3` and `annual 3` group together.
fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() && *c != '#')
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn sort_key(parsed: &Parsed<'_>) -> String {
    let Some(number) = &parsed.number else {
        let words = normalize(parsed.prefix_raw);
        // `!` sorts before the digit that starts every other group.
        let group = if words == "nn" { '!' } else { '2' };
        return format!("{group}{words}");
    };
    // Flipping the sign bit makes signed values sort as unsigned hex.
    let value = format!("{:016x}", (number.value as u64) ^ (1 << 63));
    let suffix = normalize(parsed.suffix.trim_start_matches(['-', '.', ' ']));
    let prefix = normalize(parsed.prefix_raw);
    if prefix.is_empty() {
        format!("0{value}{SEP}{suffix}")
    } else {
        format!("1{prefix}{SEP}{value}{SEP}{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(text: &str) -> IssueNumber {
        IssueNumber::new(text).unwrap()
    }

    fn key(text: &str) -> String {
        issue(text).sort_key().to_owned()
    }

    fn next(text: &str) -> Option<String> {
        issue(text).next().map(|n| n.as_str().to_owned())
    }

    #[test]
    fn keeps_the_text_as_typed() {
        assert_eq!(issue("  Annual  3 ").as_str(), "Annual  3");
        assert_eq!(issue("½").to_string(), "½");
    }

    #[test]
    fn rejects_empty_and_overlong_text() {
        assert_eq!(IssueNumber::new("  "), Err(IssueNumberError::Empty));
        assert_eq!(
            IssueNumber::new(&"9".repeat(65)),
            Err(IssueNumberError::TooLong)
        );
    }

    #[test]
    fn sorts_a_mixed_series_naturally() {
        let expected = [
            "nn",
            "-1",
            "0",
            "½",
            "1",
            "1A",
            "1b",
            "1 1/2",
            "1¾",
            "2",
            "02.5",
            "10",
            "100",
            "#101",
            "Annual 1",
            "annual 2",
            "ANNUAL 10",
            "Annual (Giant-Size) 1",
            "Annual Special 1",
            "Special 1",
            "Alpha",
            "Omega",
        ];
        let mut shuffled = expected;
        shuffled.reverse();
        shuffled.swap(3, 15);
        shuffled.sort_by_key(|s| key(s));
        assert_eq!(shuffled, expected);
    }

    #[test]
    fn equivalent_spellings_share_a_key() {
        assert_eq!(key("Annual 3"), key("annual   3"));
        assert_eq!(key("#5"), key("5"));
        assert_eq!(key("1.5"), key("1½"));
        assert_eq!(key("1.5"), key("1 1/2"));
        assert_eq!(key("½"), key("1/2"));
        assert_eq!(key("-½"), key("-0.5"));
        assert_eq!(key("-1/2"), key("-0.5"));
        assert_eq!(key("0.50"), key("½"));
        assert_eq!(key("007"), key("7"));
    }

    #[test]
    fn next_issue_follows_the_owner_rules() {
        for (from, to) in [
            ("41", "42"),
            ("Annual 3", "Annual 4"),
            ("annual  3", "annual  4"),
            ("#99", "#100"),
            ("½", "1"),
            ("1.5", "2"),
            ("1 1/2", "2"),
            ("1A", "2"),
            ("Annual 3B", "Annual 4"),
            ("-1", "0"),
            ("-½", "0"),
            ("0", "1"),
            ("007", "008"),
            ("099", "100"),
        ] {
            assert_eq!(next(from).as_deref(), Some(to), "{from}");
        }
    }

    #[test]
    fn no_next_issue_without_digits() {
        assert_eq!(next("Alpha"), None);
        assert_eq!(next("nn"), None);
    }

    #[test]
    fn huge_numbers_are_treated_as_text() {
        let huge = "9".repeat(30);
        assert!(key(&huge).starts_with('2'));
        assert_eq!(next(&huge), None);
        assert_eq!(next("999999999999"), None);
    }

    #[test]
    fn hyphens_inside_prefixes_are_not_signs() {
        assert!(key("X-Men 1").starts_with('1'));
        assert_eq!(next("X-Men 1").as_deref(), Some("X-Men 2"));
    }

    /// The number after `text`, if it has one, sorts after it and in its
    /// group.
    fn next_sorts_later_in_its_group(text: &str) {
        if let Ok(current) = IssueNumber::new(text)
            && let Some(following) = current.next()
        {
            assert!(
                following.sort_key() > current.sort_key(),
                "{:?} -> {:?}",
                current.as_str(),
                following.as_str()
            );
            let group = |k: &str| k.split(SEP).next().unwrap_or("").to_owned();
            if current.sort_key().starts_with('1') {
                assert_eq!(group(current.sort_key()), group(following.sort_key()));
            } else {
                assert_eq!(&current.sort_key()[..1], &following.sort_key()[..1]);
            }
        }
    }

    // Cases MyLongboxDB's property tests once found, kept so the copy
    // here meets them too.
    #[test]
    fn next_sorts_later_for_cases_once_found() {
        next_sorts_later_in_its_group("-\u{bd}");
    }
}
