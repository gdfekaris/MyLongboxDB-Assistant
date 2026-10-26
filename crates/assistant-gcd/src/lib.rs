//! The owner's own copy of the Grand Comics Database: its data dump,
//! which the owner downloads from comics.org and picks in the app, read
//! once into a compact index in the app's own folder, and that index
//! searched before a lookup goes to the web.
//!
//! The dump is untrusted input. It is opened read-only, with SQLite's
//! defensive settings and its views and triggers switched off, only the
//! tables and columns the index needs are read, each checked to be a
//! plain table, and every value is cleaned and capped as the model's
//! answers are before it is kept.

#![forbid(unsafe_code)]

mod catalog;
mod dump;
mod index;
mod keys;

pub use catalog::{About, Catalog, CatalogError, MAX_RECORD, Record};
pub use index::{INDEX_FILE, ImportError, Imported, Step, import};
pub use keys::{barcode_key, number_key, series_key};
