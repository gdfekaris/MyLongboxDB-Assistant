//! Cover prices in dollars and cents, held as whole cents so no amount is
//! ever rounded. Read and written as MyLongboxDB reads and writes US
//! dollar amounts.

use std::fmt;

/// Why a price could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParsePriceError {
    Empty,
    /// Anything other than digits and at most one decimal point.
    Invalid,
    Negative,
    /// More than two decimal places.
    TooPrecise,
    TooLarge,
}

impl fmt::Display for ParsePriceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "enter a price",
            Self::Invalid => "prices may contain only digits and one decimal point",
            Self::Negative => "prices cannot be negative",
            Self::TooPrecise => "prices have at most two decimal places",
            Self::TooLarge => "that price is too large",
        })
    }
}

impl std::error::Error for ParsePriceError {}

/// Reads a price such as `0.75` or `12` as whole cents, exactly.
pub fn parse_cents(text: &str) -> Result<i64, ParsePriceError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ParsePriceError::Empty);
    }
    if text.starts_with('-') {
        return Err(ParsePriceError::Negative);
    }
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if (whole.is_empty() && frac.is_empty()) || !all_digits(whole) || !all_digits(frac) {
        return Err(ParsePriceError::Invalid);
    }
    let frac = frac.trim_end_matches('0');
    if frac.len() > 2 {
        return Err(ParsePriceError::TooPrecise);
    }
    let digits = whole
        .bytes()
        .chain(frac.bytes())
        .chain(std::iter::repeat_n(b'0', 2 - frac.len()));
    let mut cents: i64 = 0;
    for digit in digits {
        cents = cents
            .checked_mul(10)
            .and_then(|c| c.checked_add(i64::from(digit - b'0')))
            .ok_or(ParsePriceError::TooLarge)?;
    }
    Ok(cents)
}

/// Whole cents as dollars and cents, such as `0.75`; [`parse_cents`]
/// reads it back.
pub fn cents_string(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let magnitude = cents.unsigned_abs();
    format!("{sign}{}.{:02}", magnitude / 100, magnitude % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_prices_exactly() {
        assert_eq!(parse_cents("0.75"), Ok(75));
        assert_eq!(parse_cents("12"), Ok(1200));
        assert_eq!(parse_cents(" 1.5 "), Ok(150));
        assert_eq!(parse_cents(".99"), Ok(99));
        assert_eq!(parse_cents("3.990"), Ok(399));
    }

    #[test]
    fn refuses_what_is_not_a_price() {
        assert_eq!(parse_cents(""), Err(ParsePriceError::Empty));
        assert_eq!(parse_cents("-1"), Err(ParsePriceError::Negative));
        assert_eq!(parse_cents("$1"), Err(ParsePriceError::Invalid));
        assert_eq!(parse_cents("1,25"), Err(ParsePriceError::Invalid));
        assert_eq!(parse_cents("."), Err(ParsePriceError::Invalid));
        assert_eq!(parse_cents("1.255"), Err(ParsePriceError::TooPrecise));
        assert_eq!(
            parse_cents("99999999999999999999"),
            Err(ParsePriceError::TooLarge)
        );
    }

    #[test]
    fn writes_what_it_reads() {
        for cents in [0, 5, 75, 100, 399, 12_345] {
            assert_eq!(parse_cents(&cents_string(cents)), Ok(cents));
        }
        assert_eq!(cents_string(75), "0.75");
        assert_eq!(cents_string(1200), "12.00");
    }
}
