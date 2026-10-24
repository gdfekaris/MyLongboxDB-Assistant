//! The book record and the checks MyLongboxDB's import applies. No
//! interface and no network.

#![forbid(unsafe_code)]

pub mod codes;
pub mod collects;
pub mod date;
pub mod issue_number;
pub mod model;
pub mod money;

pub use codes::{Barcode, Isbn};
pub use collects::{CollectsEntry, SeriesRef, format_collects, parse_collects};
pub use date::{CoverDate, Date, DateError, PartialDate};
pub use issue_number::{IssueNumber, IssueNumberError};
pub use model::{
    Appearance, ArcPart, Binding, Book, Category, Confidence, Credit, Distribution, EditionInfo,
    Gathered, IssueInfo, Jpeg, MagazineFormat, Role, SeriesInfo,
};
pub use money::{ParsePriceError, cents_string, parse_cents};
