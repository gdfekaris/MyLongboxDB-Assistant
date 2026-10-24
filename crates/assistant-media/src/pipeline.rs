//! Turning a decoded photo into what the app keeps: upright, in sRGB, no
//! larger than asked, and saved as a JPEG holding nothing but pixels.

use std::borrow::Cow;
use std::fmt;
use std::ops::RangeInclusive;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};
use image::{DynamicImage, ExtendedColorType, ImageBuffer, ImageEncoder, Rgb, RgbImage};
use moxcms::{
    CicpColorPrimaries, CicpProfile, ColorProfile, DataColorSpace, Layout, MatrixCoefficients,
    TransferCharacteristics, TransformOptions,
};

use crate::decode::{Colour, Decoded};

/// The longest edge a saved photo has unless chosen otherwise.
pub const DEFAULT_LONG_EDGE: u32 = 1600;
/// The longest edges that may be chosen.
pub const LONG_EDGES: RangeInclusive<u32> = 1200..=2400;
/// The JPEG quality used unless chosen otherwise.
pub const DEFAULT_QUALITY: u8 = 85;
/// The JPEG qualities that may be chosen: below 60 covers show blocks,
/// and above 95 the files grow with no visible gain.
pub const QUALITIES: RangeInclusive<u8> = 60..=95;
/// The longest edge of a thumbnail, as shown in the cover grid.
pub const THUMBNAIL_EDGE: u32 = 320;

/// Rows converted to sRGB at a time, so the conversion needs little
/// memory beyond the picture itself.
const BAND_ROWS: usize = 64;

/// Readies a decoded photo for editing and saving: anything transparent
/// is laid over white, the colours are converted to sRGB, and the picture
/// is turned upright.
pub fn upright(decoded: Decoded) -> RgbImage {
    let Decoded {
        image,
        colour,
        orientation,
        ..
    } = decoded;
    let profile = source_profile(&colour);
    let rgb = if is_deep(&image) {
        srgb_from_16(flatten_16(image), profile.as_ref())
    } else {
        srgb_from_8(flatten_8(image), profile.as_ref())
    };
    let mut image = DynamicImage::ImageRgb8(rgb);
    image.apply_orientation(orientation);
    image.into_rgb8()
}

/// Whether the photo has more than 8 bits a channel.
pub(crate) fn is_deep(image: &DynamicImage) -> bool {
    let colour = image.color();
    colour.bytes_per_pixel() > colour.channel_count()
}

/// The photo with any transparency laid over white, as a cover would sit
/// on a white page. An RGBA picture is narrowed to RGB in its own buffer,
/// so a large photo is never held twice.
fn flatten_8(image: DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.into_rgb8();
    }
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    let over_white = |c: u8, a: u8| {
        let (c, a) = (u32::from(c), u32::from(a));
        u8::try_from((c * a + 255 * (255 - a) + 127) / 255).unwrap_or(u8::MAX)
    };
    let rgb = drop_alpha(rgba.into_raw(), over_white);
    RgbImage::from_raw(width, height, rgb).unwrap_or_else(|| RgbImage::new(width, height))
}

fn flatten_16(image: DynamicImage) -> ImageBuffer<Rgb<u16>, Vec<u16>> {
    if !image.color().has_alpha() {
        return image.into_rgb16();
    }
    let rgba = image.into_rgba16();
    let (width, height) = rgba.dimensions();
    let over_white = |c: u16, a: u16| {
        let (c, a) = (u64::from(c), u64::from(a));
        u16::try_from((c * a + 65535 * (65535 - a) + 32767) / 65535).unwrap_or(u16::MAX)
    };
    let rgb = drop_alpha(rgba.into_raw(), over_white);
    ImageBuffer::from_raw(width, height, rgb).unwrap_or_else(|| ImageBuffer::new(width, height))
}

/// RGBA samples made RGB in place, each colour laid over white by its
/// alpha. Pixel `i` is written at `3i`, never past where pixel `i + 1`
/// is still to be read from, at `4i + 4`.
fn drop_alpha<T: Copy>(mut samples: Vec<T>, over_white: impl Fn(T, T) -> T) -> Vec<T> {
    let pixels = samples.len() / 4;
    for i in 0..pixels {
        let [r, g, b, a] = [0, 1, 2, 3].map(|c| samples[4 * i + c]);
        samples[3 * i] = over_white(r, a);
        samples[3 * i + 1] = over_white(g, a);
        samples[3 * i + 2] = over_white(b, a);
    }
    samples.truncate(pixels * 3);
    samples.shrink_to_fit();
    samples
}

/// The largest colour profile read. Phones and cameras embed profiles of
/// well under 100 KB.
pub(crate) const MAX_PROFILE_BYTES: usize = 1024 * 1024;

/// The profile to convert from, or `None` when the pixels are already
/// sRGB or their colour space cannot be used, in which case they are
/// kept as they are rather than refusing the photo.
pub(crate) fn source_profile(colour: &Colour) -> Option<ColorProfile> {
    match colour {
        Colour::Unstated => None,
        // A photo's profile is a few kilobytes; one far larger is not
        // worth handing to the profile reader, which a hostile one could
        // keep busy.
        Colour::Icc(icc) if icc.len() > MAX_PROFILE_BYTES => None,
        Colour::Icc(icc) => ColorProfile::new_from_slice(icc)
            .ok()
            .filter(|p| p.color_space == DataColorSpace::Rgb),
        Colour::Cicp {
            primaries,
            transfer,
        } => {
            // Only codes naming a real space with a usable curve are
            // taken; reserved and unspecified ones count as unstated, and
            // sRGB itself needs no conversion.
            let known_primaries = matches!(primaries, 1 | 4..=12 | 22);
            let known_transfer = matches!(transfer, 1 | 4..=6 | 8..=10 | 13..=16 | 18);
            if !known_primaries || !known_transfer || (*primaries, *transfer) == (1, 13) {
                return None;
            }
            Some(ColorProfile::new_from_cicp(CicpProfile {
                color_primaries: CicpColorPrimaries::try_from(*primaries).ok()?,
                transfer_characteristics: TransferCharacteristics::try_from(*transfer).ok()?,
                matrix_coefficients: MatrixCoefficients::Identity,
                full_range: true,
            }))
        }
    }
}

fn srgb_from_8(rgb: RgbImage, profile: Option<&ColorProfile>) -> RgbImage {
    let Some(transform) = profile.and_then(|p| {
        p.create_transform_8bit(
            Layout::Rgb,
            &ColorProfile::new_srgb(),
            Layout::Rgb,
            TransformOptions::default(),
        )
        .ok()
    }) else {
        return rgb;
    };
    // Converted in place, a band at a time, so the photo is held once.
    let mut rgb = rgb;
    let band = band_len(rgb.width());
    let mut source = vec![0; band.min(rgb.as_raw().len())];
    let samples: &mut [u8] = &mut rgb;
    for (n, dst) in samples.chunks_mut(band).enumerate() {
        let src = &mut source[..dst.len()];
        src.copy_from_slice(dst);
        if transform.transform(src, dst).is_err() {
            // A band that cannot be converted is put back as it was. If
            // the first cannot, none can, and the photo is left as it is.
            dst.copy_from_slice(src);
            if n == 0 {
                break;
            }
        }
    }
    rgb
}

/// Converts a deep photo while it still has its extra bits, then narrows
/// it to 8, so the conversion does not round twice.
fn srgb_from_16(rgb: ImageBuffer<Rgb<u16>, Vec<u16>>, profile: Option<&ColorProfile>) -> RgbImage {
    let transform = profile.and_then(|p| {
        p.create_transform_16bit(
            Layout::Rgb,
            &ColorProfile::new_srgb(),
            Layout::Rgb,
            TransformOptions::default(),
        )
        .ok()
    });
    let (width, height) = rgb.dimensions();
    let band = band_len(width);
    let mut out = vec![0; rgb.as_raw().len()];
    let mut converted = vec![0; band.min(rgb.as_raw().len())];
    for (src, dst) in rgb.as_raw().chunks(band).zip(out.chunks_mut(band)) {
        let samples = match &transform {
            Some(t) => {
                let converted = &mut converted[..src.len()];
                if t.transform(src, converted).is_err() {
                    converted.copy_from_slice(src);
                }
                &*converted
            }
            None => src,
        };
        for (o, s) in dst.iter_mut().zip(samples) {
            *o = narrow(*s);
        }
    }
    RgbImage::from_raw(width, height, out).unwrap_or_else(|| RgbImage::new(width, height))
}

fn band_len(width: u32) -> usize {
    usize::try_from(width)
        .unwrap_or(usize::MAX)
        .saturating_mul(3 * BAND_ROWS)
}

/// A 16-bit sample rounded to the nearest 8-bit one.
fn narrow(sample: u16) -> u8 {
    u8::try_from((u32::from(sample) * 255 + 32767) / 65535).unwrap_or(u8::MAX)
}

/// How a photo is saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    /// The longest edge in pixels, kept within [`LONG_EDGES`].
    pub long_edge: u32,
    /// The JPEG quality, kept within [`QUALITIES`].
    pub quality: u8,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            long_edge: DEFAULT_LONG_EDGE,
            quality: DEFAULT_QUALITY,
        }
    }
}

/// A photo ready to keep.
#[derive(Clone, Debug)]
pub struct Encoded {
    /// The JPEG shown on the book's page.
    pub full: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// A small JPEG for the cover grid.
    pub thumbnail: Vec<u8>,
}

/// Why a photo could not be saved.
#[derive(Debug)]
pub struct EncodeError(String);

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the photo could not be saved ({})", self.0)
    }
}

impl std::error::Error for EncodeError {}

/// Shrinks the photo to the chosen size, never enlarging it, and saves it
/// and its thumbnail as JPEGs. Only pixels are written, so nothing from
/// the original file can come along.
pub fn encode(image: &RgbImage, output: Output) -> Result<Encoded, EncodeError> {
    let edge = output
        .long_edge
        .clamp(*LONG_EDGES.start(), *LONG_EDGES.end());
    let quality = output.quality.clamp(*QUALITIES.start(), *QUALITIES.end());
    let full = fit(image, edge);
    let thumbnail = fit(&full, THUMBNAIL_EDGE);
    let full_jpeg = jpeg(&full, quality)?;
    Ok(Encoded {
        width: full.width(),
        height: full.height(),
        thumbnail: jpeg(&thumbnail, quality)?,
        full: full_jpeg,
    })
}

/// The photo scaled so its longest edge is at most `edge`.
fn fit(image: &RgbImage, edge: u32) -> Cow<'_, RgbImage> {
    let (width, height) = image.dimensions();
    if width.max(height) <= edge {
        return Cow::Borrowed(image);
    }
    let (width, height) = scaled(width, height, edge);
    Cow::Owned(imageops::resize(image, width, height, FilterType::Lanczos3))
}

/// The size with the longest side at `edge` and the other in proportion,
/// rounded and at least one pixel.
fn scaled(width: u32, height: u32, edge: u32) -> (u32, u32) {
    let shrink = |short: u32, long: u32| {
        let n = (u64::from(short) * u64::from(edge) + u64::from(long) / 2) / u64::from(long);
        u32::try_from(n).unwrap_or(edge).max(1)
    };
    if width >= height {
        (edge, shrink(height, width))
    } else {
        (shrink(width, height), edge)
    }
}

fn jpeg(image: &RgbImage, quality: u8) -> Result<Vec<u8>, EncodeError> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
        .map_err(|e| EncodeError(e.to_string()))?;
    Ok(out)
}
