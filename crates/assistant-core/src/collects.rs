//! Reading and writing a collected edition's contents as text, such as
//! `Amazing Spider-Man (1963) 1–10, Annual 1; Fantastic Four (1961) 48–50`.
//!
//! A semicolon starts a new series. Within a series, commas separate
//! single numbers and ranges. Each series begins with its name, then an
//! optional start year in parentheses, then its first number. `#` is
//! accepted anywhere and ignored.
//!
//! Copied from MyLongboxDB's core, whose import reads this column.

use crate::issue_number::IssueNumber;

/// A series named in a contents list.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SeriesRef {
    pub name: String,
    pub start_year: Option<u16>,
}

/// One reprinted issue, in reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectsEntry {
    pub series: SeriesRef,
    pub number: IssueNumber,
}

/// Longest range accepted, so a typo like `1–10000` cannot create
/// thousands of catalog entries.
pub const MAX_RANGE: u32 = 1000;

/// Most issues one list may name.
pub const MAX_ENTRIES: usize = 5000;

const DASHES: [char; 3] = ['–', '—', '-'];

/// Reads a contents list. The error says what was wrong, for the user.
pub fn parse_collects(text: &str) -> Result<Vec<CollectsEntry>, String> {
    let mut entries = Vec::new();
    for group in text.split(';') {
        let group = group.trim();
        if group.is_empty() {
            continue;
        }
        let items = split_items(group);
        let Some(head) = items.first() else {
            continue;
        };
        let (series, first) = parse_head(head)?;
        for item in std::iter::once(first.as_str()).chain(items[1..].iter().map(String::as_str)) {
            for number in parse_numbers(item)? {
                entries.push(CollectsEntry {
                    series: series.clone(),
                    number,
                });
                if entries.len() > MAX_ENTRIES {
                    return Err(format!("a book can list at most {MAX_ENTRIES} issues"));
                }
            }
        }
    }
    if entries.is_empty() {
        return Err("list at least one issue".into());
    }
    Ok(entries)
}

/// Writes entries in the form [`parse_collects`] reads, collapsing runs
/// of consecutive numbers into ranges.
pub fn format_collects(entries: &[CollectsEntry]) -> String {
    let mut groups: Vec<String> = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let series = &entries[i].series;
        let end = entries[i..]
            .iter()
            .position(|e| e.series != *series)
            .map_or(entries.len(), |n| i + n);
        let numbers: Vec<&IssueNumber> = entries[i..end].iter().map(|e| &e.number).collect();
        let head = match series.start_year {
            Some(year) => format!("{} ({year})", series.name),
            None => series.name.clone(),
        };
        groups.push(format!("{head} {}", compress(&numbers).join(", ")));
        i = end;
    }
    groups.join("; ")
}

/// Splits a series group on commas. Until the series head has its first
/// digit, pieces are rejoined, so a name like `Batman, Inc.` stays whole;
/// after that, every piece is an item.
pub(crate) fn split_items(group: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    let mut head = String::new();
    for piece in group.split(',') {
        if !items.is_empty() {
            if !piece.trim().is_empty() {
                items.push(piece.trim().to_owned());
            }
            continue;
        }
        if !head.is_empty() {
            head.push(',');
        }
        head.push_str(piece);
        if head.chars().any(|c| c.is_ascii_digit()) {
            items.push(head.trim().to_owned());
        }
    }
    if items.is_empty() && !head.trim().is_empty() {
        items.push(head.trim().to_owned());
    }
    items
}

/// Splits `Name (1963) 1–10` into the series and its first numbers.
fn parse_head(item: &str) -> Result<(SeriesRef, String), String> {
    let (name, year, rest) = match find_year(item) {
        Some((at, year)) => (&item[..at], Some(year), &item[at + 6..]),
        None => {
            let at = number_start(item)
                .ok_or_else(|| format!("\u{201c}{item}\u{201d} needs an issue number"))?;
            (&item[..at], None, &item[at..])
        }
    };
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(format!(
            "\u{201c}{}\u{201d} needs a series name before its numbers",
            item.trim()
        ));
    }
    if rest.trim().is_empty() {
        return Err(format!("{name} needs an issue number"));
    }
    Ok((
        SeriesRef {
            name,
            start_year: year,
        },
        rest.trim().to_owned(),
    ))
}

/// The first `(YYYY)` in `text`: its byte offset and the year.
pub(crate) fn find_year(text: &str) -> Option<(usize, u16)> {
    let bytes = text.as_bytes();
    (0..bytes.len().saturating_sub(5)).find_map(|i| {
        let window = &bytes[i..i + 6];
        let digits = &window[1..5];
        (window[0] == b'(' && window[5] == b')' && digits.iter().all(u8::is_ascii_digit))
            .then(|| {
                let year = digits
                    .iter()
                    .fold(0u16, |y, d| y * 10 + u16::from(d - b'0'));
                (year > 0).then_some((i, year))
            })
            .flatten()
    })
}

/// Where the trailing number or range begins in a head without a year:
/// the last word, or the last three when they read `a – b`.
pub(crate) fn number_start(item: &str) -> Option<usize> {
    let words: Vec<(usize, &str)> = item
        .split_whitespace()
        .map(|w| (w.as_ptr() as usize - item.as_ptr() as usize, w))
        .collect();
    let has_digit = |w: &str| w.chars().any(|c| c.is_ascii_digit());
    let (last_at, last) = *words.last()?;
    if !has_digit(last) {
        return None;
    }
    if let [.., (first_at, a), (_, dash), _] = words.as_slice()
        && dash.chars().all(|c| DASHES.contains(&c))
        && has_digit(a)
    {
        return Some(*first_at);
    }
    Some(last_at)
}

/// Reads one item: a number such as `121` or `Annual 1`, or a range such
/// as `1–10` or `Annual 1–3`.
pub(crate) fn parse_numbers(item: &str) -> Result<Vec<IssueNumber>, String> {
    let item = item.replace('#', " ");
    let item = item.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((left, right)) = split_range(&item) {
        let start_of_digits = left
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_digit())
            .last()
            .map_or(left.len(), |(i, _)| i);
        let prefix = left[..start_of_digits].trim();
        let parse = |s: &str| {
            s.parse::<u32>()
                .map_err(|_| format!("\u{201c}{item}\u{201d} is not a range of issue numbers"))
        };
        let (from, to) = (parse(&left[start_of_digits..])?, parse(right)?);
        if from > to {
            return Err(format!("the range \u{201c}{item}\u{201d} runs backwards"));
        }
        if to - from >= MAX_RANGE {
            return Err(format!(
                "the range \u{201c}{item}\u{201d} is longer than {MAX_RANGE} issues"
            ));
        }
        return (from..=to)
            .map(|n| {
                let text = if prefix.is_empty() {
                    n.to_string()
                } else {
                    format!("{prefix} {n}")
                };
                IssueNumber::new(&text).map_err(|e| e.to_string())
            })
            .collect();
    }
    IssueNumber::new(&item)
        .map(|n| vec![n])
        .map_err(|_| format!("\u{201c}{item}\u{201d} is not an issue number"))
}

/// Splits `Annual 1–3` at its dash when both sides are whole numbers.
fn split_range(item: &str) -> Option<(&str, &str)> {
    item.char_indices()
        .filter(|(_, c)| DASHES.contains(c))
        .find_map(|(i, c)| {
            let left = item[..i].trim_end();
            let right = item[i + c.len_utf8()..].trim_start();
            let ok = left.ends_with(|c: char| c.is_ascii_digit())
                && !right.is_empty()
                && right.bytes().all(|b| b.is_ascii_digit());
            ok.then_some((left, right))
        })
}

/// A number that is a plain whole number after an optional word prefix,
/// such as `12` or `Annual 3`, written exactly as it would be rebuilt.
fn as_counted(number: &IssueNumber) -> Option<(&str, u32)> {
    let text = number.as_str();
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit())
        .last()
        .map(|(i, _)| i)?;
    let (prefix, digits) = (&text[..start], &text[start..]);
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    let n = digits.parse().ok()?;
    let prefix = match prefix {
        "" => "",
        p if p.ends_with(' ') && p.trim_end().ends_with(char::is_alphabetic) => p.trim_end(),
        _ => return None,
    };
    if !prefix.is_empty() && prefix.split_whitespace().count() != prefix.split(' ').count() {
        return None;
    }
    Some((prefix, n))
}

fn compress(numbers: &[&IssueNumber]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < numbers.len() {
        let mut end = i + 1;
        if let Some((prefix, first)) = as_counted(numbers[i]) {
            // A run stops at the largest number, which nothing follows.
            let mut last = first;
            while end < numbers.len()
                && let Some(next) = last.checked_add(1)
                && as_counted(numbers[end]) == Some((prefix, next))
            {
                last = next;
                end += 1;
            }
            if last > first {
                let space = if prefix.is_empty() { "" } else { " " };
                out.push(format!("{prefix}{space}{first}–{last}"));
                i = end;
                continue;
            }
        }
        out.push(numbers[i].as_str().to_owned());
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, year: Option<u16>, number: &str) -> CollectsEntry {
        CollectsEntry {
            series: SeriesRef {
                name: name.into(),
                start_year: year,
            },
            number: IssueNumber::new(number).unwrap(),
        }
    }

    fn numbers(text: &str) -> Vec<(String, Option<u16>, String)> {
        parse_collects(text)
            .unwrap()
            .into_iter()
            .map(|e| {
                (
                    e.series.name,
                    e.series.start_year,
                    e.number.as_str().to_owned(),
                )
            })
            .collect()
    }

    fn asm(n: &str) -> (String, Option<u16>, String) {
        ("Amazing Spider-Man".into(), Some(1963), n.into())
    }

    #[test]
    fn reads_the_spec_example() {
        let mut expected: Vec<_> = (1..=10).map(|n| asm(&n.to_string())).collect();
        expected.push(asm("Annual 1"));
        assert_eq!(
            numbers("Amazing Spider-Man (1963) 1–10, Annual 1"),
            expected
        );
    }

    #[test]
    fn reads_several_series_and_spellings() {
        let ff = |n: &str| ("Fantastic Four".to_owned(), Some(1961), n.to_owned());
        assert_eq!(
            numbers("Amazing Spider-Man (1963) #121 - 122 ; Fantastic Four (1961) #48-50, ½, 1A"),
            [
                asm("121"),
                asm("122"),
                ff("48"),
                ff("49"),
                ff("50"),
                ff("½"),
                ff("1A")
            ]
        );
        assert_eq!(
            numbers("Amazing Spider-Man (1963) Annual #1—3"),
            [asm("Annual 1"), asm("Annual 2"), asm("Annual 3")]
        );
    }

    #[test]
    fn a_year_is_optional() {
        let w = |n: &str| ("Watchmen".to_owned(), None, n.to_owned());
        assert_eq!(numbers("Watchmen 1 – 3"), [w("1"), w("2"), w("3")]);
        let s = ("Spider-Man 2099".to_owned(), None, "1".to_owned());
        assert_eq!(numbers("Spider-Man 2099 1"), [s]);
    }

    #[test]
    fn commas_inside_series_names_survive() {
        assert_eq!(
            numbers("Batman, Inc. (2011) 1-2"),
            [
                ("Batman, Inc.".to_owned(), Some(2011), "1".to_owned()),
                ("Batman, Inc.".to_owned(), Some(2011), "2".to_owned()),
            ]
        );
    }

    #[test]
    fn negative_and_fractional_numbers_are_not_ranges() {
        assert_eq!(
            numbers("X-Men (1991) -1"),
            [("X-Men".to_owned(), Some(1991), "-1".to_owned())]
        );
    }

    #[test]
    fn explains_what_is_wrong() {
        for (text, expected) in [
            ("", "list at least one issue"),
            ("Watchmen", "needs an issue number"),
            ("(1963) 1-3", "needs a series name"),
            ("Amazing Spider-Man (1963)", "needs an issue number"),
            ("Watchmen 12-1", "runs backwards"),
            ("Watchmen 1-5000", "longer than 1000"),
        ] {
            let error = parse_collects(text).unwrap_err();
            assert!(error.contains(expected), "{text:?}: {error}");
        }
    }

    #[test]
    fn writes_ranges_and_series_groups() {
        let mut entries: Vec<_> = (1..=10)
            .map(|n| entry("Amazing Spider-Man", Some(1963), &n.to_string()))
            .collect();
        entries.push(entry("Amazing Spider-Man", Some(1963), "Annual 1"));
        entries.push(entry("Fantastic Four", Some(1961), "48"));
        entries.push(entry("Fantastic Four", Some(1961), "50"));
        entries.push(entry("Fantastic Four", Some(1961), "½"));
        assert_eq!(
            format_collects(&entries),
            "Amazing Spider-Man (1963) 1–10, Annual 1; Fantastic Four (1961) 48, 50, ½"
        );
    }

    #[test]
    fn the_largest_numbers_end_a_range_rather_than_overflow() {
        let entries = [
            entry("Zot!", Some(1984), "4294967294"),
            entry("Zot!", Some(1984), "4294967295"),
            entry("Zot!", Some(1984), "Annual 1"),
        ];
        assert_eq!(
            format_collects(&entries),
            "Zot! (1984) 4294967294–4294967295, Annual 1"
        );
        let back = parse_collects(&format_collects(&entries)).unwrap();
        assert_eq!(back, entries);
    }

    #[test]
    fn padded_and_suffixed_numbers_are_written_as_typed() {
        let entries = [
            entry("Zot!", Some(1984), "01"),
            entry("Zot!", Some(1984), "02"),
            entry("Zot!", Some(1984), "2A"),
        ];
        assert_eq!(format_collects(&entries), "Zot! (1984) 01, 02, 2A");
        let back = parse_collects(&format_collects(&entries)).unwrap();
        assert_eq!(back, entries);
    }

    /// Writing `entries` and reading the text gives them back.
    fn reads_back(entries: &[CollectsEntry]) {
        let text = format_collects(entries);
        assert_eq!(parse_collects(&text).as_deref(), Ok(entries), "{text}");
    }

    // Cases MyLongboxDB's property tests once found, kept so the copy
    // here meets them too.
    #[test]
    fn cases_once_found_read_back() {
        let aa = |n: &str| entry("Aa", Some(1900), n);
        reads_back(&[aa("1"), aa("\u{bd}"), aa("1")]);
    }

    #[test]
    fn cases_once_found_do_not_panic() {
        let _ = parse_collects(",");
    }
}
