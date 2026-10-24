//! The numbers printed in a book's barcodes: ISBNs on collected editions,
//! and UPC or EAN barcodes on comics and magazines. Both carry a check
//! digit, so a misread or made-up number is caught before it reaches the
//! export.

use std::fmt;

/// Digits with the hyphens and spaces printed between them taken out, or
/// `None` if anything else is there.
fn digits(text: &str) -> Option<String> {
    let digits: String = text.chars().filter(|c| !matches!(c, '-' | ' ')).collect();
    digits.bytes().all(|b| b.is_ascii_digit()).then_some(digits)
}

/// Whether 13 digits end in the right EAN-13 check digit, as both
/// ISBN-13s and EAN barcodes do.
fn ean13_checks(digits: &str) -> bool {
    let sum: u32 = digits
        .bytes()
        .enumerate()
        .map(|(i, b)| u32::from(b - b'0') * if i % 2 == 0 { 1 } else { 3 })
        .sum();
    digits.len() == 13 && sum.is_multiple_of(10)
}

/// A 13-digit ISBN with a valid check digit, held without hyphens. The
/// same rule as MyLongboxDB's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Isbn(String);

impl Isbn {
    /// Accepts hyphens and spaces between digits, as printed on books.
    pub fn parse(text: &str) -> Option<Self> {
        let digits = digits(text)?;
        ean13_checks(&digits).then_some(Self(digits))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Isbn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The number under a cover's barcode: a 12-digit UPC-A or a 13-digit
/// EAN-13, whose check digit must hold, and then, on most comics and
/// many magazines, a 2- or 5-digit add-on that tells issues apart, which
/// has no check digit of its own. Held as one run of digits.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Barcode(String);

impl Barcode {
    /// Accepts hyphens and spaces between digits, as barcodes print them.
    /// The lengths are unambiguous: 12 or 13 digits, then 0, 2, or 5.
    pub fn parse(text: &str) -> Option<Self> {
        let digits = digits(text)?;
        let main = match digits.len() {
            12 | 14 | 17 => 12,
            13 | 15 | 18 => 13,
            _ => return None,
        };
        // A UPC-A is an EAN-13 with a leading zero.
        let as_ean = if main == 12 {
            format!("0{}", &digits[..12])
        } else {
            digits[..13].to_owned()
        };
        ean13_checks(&as_ean).then_some(Self(digits))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Barcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isbns_need_their_check_digit() {
        assert_eq!(
            Isbn::parse("978-0-7851-2532-7").unwrap().as_str(),
            "9780785125327"
        );
        assert!(Isbn::parse("978-0-7851-2532-8").is_none());
        assert!(Isbn::parse("0785125329").is_none());
        assert!(Isbn::parse("978O785125327").is_none());
    }

    #[test]
    fn barcodes_need_their_check_digit() {
        // A UPC-A alone, with a 5-digit add-on, and with a 2-digit one.
        assert!(Barcode::parse("759606089369").is_some());
        assert!(Barcode::parse("759606089368").is_none());
        assert_eq!(
            Barcode::parse("7 59606 08936 9 00111").unwrap().as_str(),
            "75960608936900111"
        );
        assert!(Barcode::parse("75960608936901").is_some());
        assert!(Barcode::parse("75960608936800111").is_none());
        // An EAN-13, alone and with add-ons.
        assert!(Barcode::parse("9771234567003").is_some());
        assert!(Barcode::parse("977123456700312").is_some());
        assert!(Barcode::parse("977123456700312345").is_some());
        assert!(Barcode::parse("9771234567004").is_none());
    }

    #[test]
    fn barcodes_of_other_lengths_are_refused() {
        for text in [
            "",
            "12345678",
            "75960608936",
            "7596060893690011",
            "75960608936x",
        ] {
            assert!(Barcode::parse(text).is_none(), "{text}");
        }
    }
}
