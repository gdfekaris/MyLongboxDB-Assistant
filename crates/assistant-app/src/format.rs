//! Values written as MyLongboxDB shows them, and read back as people
//! type them. The results file and the export keep the plain forms
//! MyLongboxDB imports, such as `1990-04`; only what is shown differs.

use assistant_core::CoverDate;

use crate::i18n::{gettext_f, pgettext};

/// The English month names, which cover dates are typed in.
const FULL_MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

/// A month's short name, as shown, for months numbered 1 to 12.
fn month_name(month: u8) -> String {
    let context = "month abbreviation";
    match month {
        1 => pgettext(context, "Jan"),
        2 => pgettext(context, "Feb"),
        3 => pgettext(context, "Mar"),
        4 => pgettext(context, "Apr"),
        5 => pgettext(context, "May"),
        6 => pgettext(context, "Jun"),
        7 => pgettext(context, "Jul"),
        8 => pgettext(context, "Aug"),
        9 => pgettext(context, "Sep"),
        10 => pgettext(context, "Oct"),
        11 => pgettext(context, "Nov"),
        _ => pgettext(context, "Dec"),
    }
}

/// e.g. `Mar 1975`, or `1975` when the month is unknown.
pub fn cover_date(date: CoverDate) -> String {
    match date.month() {
        // Translators: a cover date such as "Mar 1975".
        Some(m) => gettext_f(
            "{month} {year}",
            &[
                ("month", &month_name(m)),
                ("year", &date.year().to_string()),
            ],
        ),
        None => date.year().to_string(),
    }
}

/// A cover date as kept, `1975-03` or `1975`, as it is shown; text that
/// is not one is shown as it is.
pub fn shown_cover_date(kept: &str) -> String {
    parse_typed_cover_date(kept)
        .ok()
        .flatten()
        .map_or_else(|| kept.to_owned(), cover_date)
}

/// Reads a cover date as it is shown, or as people write it: `1975`,
/// `1975-03`, `3/1975`, `Mar 1975`, or `March 1975`. Empty text is no
/// date; `Err` means the text is not a date.
pub fn parse_cover_date(text: &str) -> Result<Option<CoverDate>, ()> {
    parse_cover_date_shown(text, &cover_date)
}

/// [`parse_cover_date`], trying first the way `shown` writes a cover
/// date, as a translation may name months and order the words its own
/// way, and the edit box must read back what it shows.
fn parse_cover_date_shown(
    text: &str,
    shown: &dyn Fn(CoverDate) -> String,
) -> Result<Option<CoverDate>, ()> {
    match as_shown(text.trim(), shown) {
        Some(date) => Ok(Some(date)),
        None => parse_typed_cover_date(text),
    }
}

/// The cover date that `shown` writes as `text`, if any. Its year is
/// the first run of exactly four digits, and each month of that year is
/// tried.
fn as_shown(text: &str, shown: &dyn Fn(CoverDate) -> String) -> Option<CoverDate> {
    let bytes = text.as_bytes();
    let digit = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_digit);
    let year: u16 = (0..bytes.len())
        .find(|&i| (i..i + 4).all(digit) && !digit(i + 4) && (i == 0 || !digit(i - 1)))
        .and_then(|i| text.get(i..i + 4))?
        .parse()
        .ok()?;
    let text = text.to_lowercase();
    (1..=12)
        .filter_map(|month| CoverDate::new(year, Some(month)))
        .find(|date| shown(*date).to_lowercase() == text)
}

/// Reads a cover date as people type it, with English month names.
fn parse_typed_cover_date(text: &str) -> Result<Option<CoverDate>, ()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let year = |t: &str| -> Result<u16, ()> {
        if t.len() == 4 && t.bytes().all(|b| b.is_ascii_digit()) {
            t.parse().map_err(|_| ())
        } else {
            Err(())
        }
    };
    let month = |t: &str| -> Result<u8, ()> {
        if let Ok(m) = t.parse::<u8>() {
            return Ok(m);
        }
        let t = t.to_lowercase();
        let t = t.trim_end_matches('.');
        if t.len() < 3 {
            return Err(());
        }
        let found = FULL_MONTHS
            .iter()
            .position(|m| m.starts_with(t))
            .ok_or(())?;
        u8::try_from(found + 1).map_err(|_| ())
    };
    let date = if let Some((y, m)) = text.split_once('-') {
        CoverDate::new(year(y)?, Some(month(m)?))
    } else if let Some((m, y)) = text.split_once('/') {
        CoverDate::new(year(y)?, Some(month(m)?))
    } else if let Some((m, y)) = text.split_once(' ') {
        CoverDate::new(year(y.trim())?, Some(month(m)?))
    } else {
        CoverDate::new(year(text)?, None)
    };
    date.map(Some).ok_or(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    #[test]
    fn cover_dates_are_shown_as_mylongboxdb_shows_them() {
        assert_eq!(shown_cover_date("1990-04"), "Apr 1990");
        assert_eq!(shown_cover_date("1975"), "1975");
        assert_eq!(shown_cover_date("not a date"), "not a date");
        for (month, name) in MONTHS.iter().enumerate() {
            let date = CoverDate::new(1975, Some(month as u8 + 1)).unwrap();
            assert_eq!(cover_date(date), format!("{name} 1975"));
        }
    }

    #[test]
    fn cover_dates_parse_as_people_write_them() {
        let mar = CoverDate::new(1975, Some(3));
        for text in [
            "1975-03",
            "1975-3",
            "3/1975",
            "Mar 1975",
            "march 1975",
            "Mar. 1975",
            " Mar 1975 ",
        ] {
            assert_eq!(parse_cover_date(text), Ok(mar), "{text}");
        }
        assert_eq!(parse_cover_date("1975"), Ok(CoverDate::new(1975, None)));
        assert_eq!(parse_cover_date("  "), Ok(None));
        for bad in [
            "75",
            "13/1975",
            "Marz 1975",
            "Ma 1975",
            "1975-00",
            "spring",
            "1975 03 01",
        ] {
            assert_eq!(parse_cover_date(bad), Err(()), "{bad}");
        }
        // What the review page shows reads back as the same date.
        for month in 1..=12 {
            let date = CoverDate::new(1990, Some(month)).unwrap();
            assert_eq!(parse_cover_date(&cover_date(date)), Ok(Some(date)));
        }
    }

    #[test]
    fn cover_dates_read_back_as_a_translation_shows_them() {
        const FRENCH: [&str; 12] = [
            "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.",
            "nov.", "déc.",
        ];
        let french = |d: CoverDate| match d.month() {
            Some(m) => format!("{} {}", FRENCH[usize::from(m) - 1], d.year()),
            None => d.year().to_string(),
        };
        // A translation may put the year first, with no space.
        let year_first = |d: CoverDate| match d.month() {
            Some(m) => format!("{}年{}月", d.year(), m),
            None => d.year().to_string(),
        };
        for shown in [&french as &dyn Fn(CoverDate) -> String, &year_first] {
            for month in 1..=12 {
                let date = CoverDate::new(1975, Some(month));
                let text = shown(date.unwrap());
                assert_eq!(parse_cover_date_shown(&text, shown), Ok(date), "{text}");
            }
        }
    }
}
