//! Books typed one line each, such as `Amazing Spider-Man (1963) 300`,
//! `ASM #300`, `X-Men 1991 #1`, `Batman Vol. 2 #1`, or a range or list
//! such as `Amazing Spider-Man (1963) 300-305, 310`, which stands for
//! seven books. A line is a query for the lookup, not a finished record:
//! an abbreviated series such as `ASM` is left for the lookup to resolve.
//!
//! Numbers and ranges read as in a collected edition's Collects list, so
//! `Annual 1-3` and `#` work the same way in both.

use std::fmt;

use crate::collects::{find_year, number_start, parse_numbers, split_items};
use crate::issue_number::IssueNumber;

/// The most books one line may stand for, the most a run may hold.
pub const MAX_BOOKS_PER_LINE: usize = 100;
/// The earliest year a bare four-digit number is read as, rather than as
/// part of the series name: the first comic books.
const EARLIEST_YEAR: u16 = 1930;

/// One book to look up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookQuery {
    /// The line it came from, counting from 1.
    pub line: usize,
    /// The series as typed, spacing tidied.
    pub series: String,
    pub start_year: Option<u16>,
    pub volume: Option<u32>,
    pub number: IssueNumber,
}

impl fmt::Display for BookQuery {
    /// As shown back to the owner: `Amazing Spider-Man (1963) Vol. 2 #300`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.series)?;
        if let Some(year) = self.start_year {
            write!(f, " ({year})")?;
        }
        if let Some(volume) = self.volume {
            write!(f, " Vol. {volume}")?;
        }
        write!(f, " #{}", self.number)
    }
}

/// A line that could not be read, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineProblem {
    pub line: usize,
    pub text: String,
    pub problem: String,
}

/// Takes a volume out of `text`, written `Vol. 2`, `Vol 2`, `Volume 2`,
/// `vol.2`, or `v2`, in any case. Returns the text without it.
fn take_volume(text: &str) -> Result<(String, Option<u32>), String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut kept = Vec::new();
    let mut volume = None;
    let mut i = 0;
    while i < words.len() {
        let lower = words[i].to_lowercase();
        let marker = ["volume", "vol.", "vol"]
            .iter()
            .find(|m| lower.starts_with(**m))
            .copied()
            .or_else(|| {
                (lower.starts_with('v')
                    && lower.len() > 1
                    && lower[1..].bytes().all(|b| b.is_ascii_digit()))
                .then_some("v")
            });
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let found = match marker {
            Some(m) if digits(&lower[m.len()..]) => Some((lower[m.len()..].to_owned(), 1)),
            Some(m) if lower.len() == m.len() && words.get(i + 1).is_some_and(|w| digits(w)) => {
                Some((words[i + 1].to_owned(), 2))
            }
            _ => None,
        };
        match found {
            Some((n, used)) => {
                if volume.is_some() {
                    return Err("names more than one volume".into());
                }
                let n: u32 = n
                    .parse()
                    .map_err(|_| "names a volume too large".to_owned())?;
                if n == 0 {
                    return Err("volumes are numbered from 1".into());
                }
                volume = Some(n);
                i += used;
            }
            None => {
                kept.push(words[i]);
                i += 1;
            }
        }
    }
    Ok((kept.join(" "), volume))
}

/// A bare year at the end of a series name, `X-Men 1991`, if it is one:
/// four digits from [`EARLIEST_YEAR`] to next year, after at least one
/// other word, so `Spider-Man 2099` keeps its number.
fn take_bare_year(series: &str, this_year: u16) -> (String, Option<u16>) {
    if let Some((name, last)) = series.rsplit_once(' ')
        && last.len() == 4
        && let Ok(year) = last.parse::<u16>()
        && (EARLIEST_YEAR..=this_year.saturating_add(1)).contains(&year)
        && !name.trim().is_empty()
    {
        return (name.trim().to_owned(), Some(year));
    }
    (series.to_owned(), None)
}

/// One line read: its series and the numbers it names.
struct Line {
    series: String,
    start_year: Option<u16>,
    volume: Option<u32>,
    numbers: Vec<IssueNumber>,
}

/// Reads one line into its series and the numbers it names.
fn read_line(text: &str, this_year: u16) -> Result<Line, String> {
    let (text, volume) = take_volume(text)?;
    let items = split_items(&text);
    let head = items.first().ok_or("is empty")?;
    // The series ends at the first `#`, at a start year in parentheses, or
    // where the trailing number or range begins.
    let (series, mut year, first) = if let Some(at) = head.find('#') {
        let number = &head[at..];
        // A start year written after the number: `#300 (1963)`.
        match find_year(number) {
            Some((year_at, year)) => (
                head[..at].to_owned(),
                Some(year),
                format!("{} {}", &number[..year_at], &number[year_at + 6..]),
            ),
            None => (head[..at].to_owned(), None, number.to_owned()),
        }
    } else if let Some((at, year)) = find_year(head) {
        (head[..at].to_owned(), Some(year), head[at + 6..].to_owned())
    } else {
        let at = number_start(head).ok_or("needs an issue number")?;
        (head[..at].to_owned(), None, head[at..].to_owned())
    };
    let mut series = series.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((at, paren)) = find_year(&series) {
        // A start year written before a `#`.
        year = Some(paren);
        series = format!("{} {}", &series[..at], &series[at + 6..]);
        series = series.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    if year.is_none() {
        (series, year) = take_bare_year(&series, this_year);
    }
    let series = series
        .trim_end_matches([',', ':', '-', '–'])
        .trim()
        .to_owned();
    if series.is_empty() {
        return Err("needs a series before its issue number".into());
    }
    if first.trim().trim_matches('#').trim().is_empty() {
        return Err("needs an issue number".into());
    }
    let mut numbers = Vec::new();
    for item in std::iter::once(first.as_str()).chain(items[1..].iter().map(String::as_str)) {
        numbers.extend(parse_numbers(item)?);
        if numbers.len() > MAX_BOOKS_PER_LINE {
            return Err(format!(
                "stands for more than {MAX_BOOKS_PER_LINE} books; split it up"
            ));
        }
    }
    Ok(Line {
        series,
        start_year: year,
        volume,
        numbers,
    })
}

/// Reads the owner's lines, skipping blank ones. `this_year` bounds what a
/// bare four-digit number can be read as.
pub fn parse_lines(text: &str, this_year: u16) -> (Vec<BookQuery>, Vec<LineProblem>) {
    let mut books = Vec::new();
    let mut problems = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        match read_line(line, this_year) {
            Ok(read) => {
                books.extend(read.numbers.into_iter().map(|number| BookQuery {
                    line: n + 1,
                    series: read.series.clone(),
                    start_year: read.start_year,
                    volume: read.volume,
                    number,
                }));
            }
            Err(problem) => problems.push(LineProblem {
                line: n + 1,
                text: line.to_owned(),
                problem,
            }),
        }
    }
    (books, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    const THIS_YEAR: u16 = 2026;

    /// Each book a line stands for, as shown back.
    fn shown(text: &str) -> Vec<String> {
        let (books, problems) = parse_lines(text, THIS_YEAR);
        assert!(problems.is_empty(), "{problems:?}");
        books.iter().map(ToString::to_string).collect()
    }

    fn problem(text: &str) -> String {
        let (books, problems) = parse_lines(text, THIS_YEAR);
        assert!(books.is_empty(), "{books:?}");
        assert_eq!(problems.len(), 1);
        problems[0].problem.clone()
    }

    #[test]
    fn the_common_forms_read_alike() {
        let asm = ["Amazing Spider-Man (1963) #300"];
        assert_eq!(shown("Amazing Spider-Man (1963) 300"), asm);
        assert_eq!(shown("Amazing Spider-Man (1963) #300"), asm);
        assert_eq!(shown("Amazing Spider-Man #300 (1963)"), asm);
        assert_eq!(shown("  Amazing   Spider-Man 1963 #300 "), asm);
        assert_eq!(shown("Amazing Spider-Man 1963 300"), asm);
        assert_eq!(shown("ASM #300"), ["ASM #300"]);
        assert_eq!(shown("ASM 300"), ["ASM #300"]);
        assert_eq!(shown("X-Men 1991 #1"), ["X-Men (1991) #1"]);
    }

    #[test]
    fn a_number_in_the_name_is_not_taken_for_a_year() {
        assert_eq!(shown("Spider-Man 2099 #1"), ["Spider-Man 2099 #1"]);
        assert_eq!(shown("2000 AD 1"), ["2000 AD #1"]);
        assert_eq!(
            shown("Spider-Man 2099 (1992) 1"),
            ["Spider-Man 2099 (1992) #1"]
        );
        assert_eq!(shown("Batman 1989 1"), ["Batman (1989) #1"]);
    }

    #[test]
    fn volumes_are_read_in_their_usual_spellings() {
        for line in [
            "Batman Vol. 2 #1",
            "Batman vol 2 1",
            "Batman Volume 2 #1",
            "Batman v2 #1",
            "Batman VOL.2 #1",
        ] {
            assert_eq!(shown(line), ["Batman Vol. 2 #1"], "{line}");
        }
        assert_eq!(shown("Batman (2011) Vol 2 #1"), ["Batman (2011) Vol. 2 #1"]);
        assert_eq!(shown("Vampirella 1"), ["Vampirella #1"]);
    }

    #[test]
    fn ranges_and_lists_stand_for_several_books() {
        assert_eq!(shown("Amazing Spider-Man (1963) 300-305").len(), 6);
        assert_eq!(
            shown("ASM 300 – 302, 310, Annual 21"),
            [
                "ASM #300",
                "ASM #301",
                "ASM #302",
                "ASM #310",
                "ASM #Annual 21"
            ]
        );
        assert_eq!(
            shown("Amazing Spider-Man (1963) Annual 1-2"),
            [
                "Amazing Spider-Man (1963) #Annual 1",
                "Amazing Spider-Man (1963) #Annual 2"
            ]
        );
    }

    #[test]
    fn unusual_numbers_keep_their_form() {
        assert_eq!(shown("Batman #-1"), ["Batman #-1"]);
        assert_eq!(shown("Batman #½"), ["Batman #½"]);
        assert_eq!(shown("Batman 1A"), ["Batman #1A"]);
        assert_eq!(shown("Watchmen #Alpha"), ["Watchmen #Alpha"]);
        assert_eq!(shown("Batman, Inc. 1"), ["Batman, Inc. #1"]);
    }

    #[test]
    fn lines_remember_where_they_came_from() {
        let (books, problems) = parse_lines("ASM 1\n\n  \nThor 337-338\nnonsense", THIS_YEAR);
        let lines: Vec<usize> = books.iter().map(|b| b.line).collect();
        assert_eq!(lines, [1, 4, 4]);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].line, 5);
        assert_eq!(problems[0].text, "nonsense");
    }

    #[test]
    fn unreadable_lines_say_why() {
        assert!(problem("Amazing Spider-Man").contains("issue number"));
        assert!(problem("#300").contains("series"));
        assert!(problem("ASM #").contains("issue number"));
        assert!(problem("ASM 305-300").contains("backwards"));
        assert!(problem("ASM 1-101").contains("more than 100"));
        assert!(problem("Batman Vol. 0 #1").contains("from 1"));
        assert!(problem("Batman Vol 2 Vol 3 #1").contains("more than one volume"));
    }

    #[test]
    fn a_line_of_exactly_the_most_books_is_fine() {
        assert_eq!(shown("ASM 1-100").len(), MAX_BOOKS_PER_LINE);
    }
}
