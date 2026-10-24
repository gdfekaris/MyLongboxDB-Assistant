//! Image import pipeline: decode within limits, orient, convert color,
//! resize, and re-encode so no source metadata survives.

#![forbid(unsafe_code)]

mod decode;
mod edit;
#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod fixtures;
#[cfg(feature = "heic")]
mod heic;
mod pipeline;
#[cfg(test)]
mod pipeline_tests;

pub use decode::{
    Colour, DecodeError, Decoded, Decoders, Format, MAX_FILE_BYTES, MAX_PIXELS, MAX_SAVED_SIDE,
    MAX_SIDE, decode_bytes, decode_file, decode_saved, load_decoders, read_file,
};
pub use edit::{Crop, Edit, MAX_STRAIGHTEN, MIN_CROP, apply as apply_edit, preview, turned_size};
/// The picture types the pipeline works in.
pub use image::{Rgb, RgbImage};
pub use pipeline::{
    DEFAULT_LONG_EDGE, DEFAULT_QUALITY, EncodeError, Encoded, LONG_EDGES, Output, QUALITIES,
    THUMBNAIL_EDGE, encode, upright,
};
