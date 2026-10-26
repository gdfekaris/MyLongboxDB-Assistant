//! The book record, the checks MyLongboxDB's import applies, the lookup
//! steps, and the export writer. No interface and no network.

#![forbid(unsafe_code)]

pub mod codes;
pub mod collects;
pub mod cost;
pub mod date;
pub mod export;
pub mod issue_number;
pub mod lines;
pub mod lookup;
pub mod model;
pub mod money;
pub mod queue;
pub mod results;

pub use codes::{Barcode, Isbn};
pub use collects::{CollectsEntry, SeriesRef, format_collects, parse_collects};
pub use cost::{Estimate, Planned, Price, Usage, anthropic_price, dollars, image_tokens};
pub use date::{CoverDate, Date, DateError, PartialDate};
pub use export::{COLUMNS, ExportError, write_export};
pub use issue_number::{IssueNumber, IssueNumberError};
pub use lines::{BookQuery, LineProblem, MAX_BOOKS_PER_LINE, parse_lines};
pub use model::{
    Appearance, ArcPart, Binding, Book, Category, Confidence, CopyInfo, Credit, Distribution,
    EditionInfo, Grade, GradeType, IssueInfo, Jpeg, MagazineFormat, Role, SeriesInfo,
};
pub use money::{ParsePriceError, Picodollars, cents_string, parse_cents};
