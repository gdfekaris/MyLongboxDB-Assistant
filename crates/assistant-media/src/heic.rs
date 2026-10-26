//! Reading HEIC photos, the format iPhones save in, through libheif.

use image::metadata::Orientation;
use image::{DynamicImage, ImageBuffer, Rgb, Rgba};
use libheif_rs::{
    ColorSpace, HeifContext, HeifError, HeifErrorSubCode, ImageHandle, LibHeif, RgbChroma,
    SecurityLimits,
};

use crate::decode::{
    Colour, DecodeError, Decoded, Format, MAX_PIXELS, check_shown_size, check_size, max_alloc,
};

/// Decodes the main picture of a HEIC file, turned the way the file
/// says to show it. libheif applies the file's own rotation and mirroring,
/// which the format says take the place of any EXIF orientation, so none
/// is left to apply.
pub(crate) fn decode(data: &[u8]) -> Result<Decoded, DecodeError> {
    let lib = LibHeif::new();
    let context = open(data)?;
    let handle = context.primary_image_handle().map_err(damaged)?;
    let (width, height) = (handle.width(), handle.height());
    check_shown_size(width, height)?;
    // The picture is decoded whole before any crop, so its full stored
    // size must fit too.
    let stored = |n: i32| u32::try_from(n).unwrap_or(0);
    check_size(stored(handle.ispe_width()), stored(handle.ispe_height()))?;
    let image = pixels(&lib, &handle)
        .map_err(|e| refusal(e, width, height))?
        .ok_or_else(|| {
            DecodeError::Damaged("the decoded picture does not match its stated size".into())
        })?;
    Ok(Decoded {
        image,
        format: Format::Heic,
        colour: colour(&handle),
        orientation: Orientation::NoTransforms,
    })
}

/// The colour space the file states, preferring an ICC profile, which
/// iPhones embed for Display P3.
fn colour(handle: &ImageHandle) -> Colour {
    if let Some(icc) = handle.color_profile_raw()
        && !icc.data.is_empty()
    {
        return Colour::Icc(icc.data);
    }
    match handle.color_profile_nclx() {
        Some(nclx) => Colour::Cicp {
            primaries: u8::try_from(nclx.color_primaries() as u32).unwrap_or(2),
            transfer: u8::try_from(nclx.transfer_characteristics() as u32).unwrap_or(2),
        },
        None => Colour::Unstated,
    }
}

/// Reads the file's structure, with libheif's own limits set first.
/// They are a second guard behind the size checks, and also cover what
/// those cannot see, such as a grid of many tiles.
fn open(data: &[u8]) -> Result<HeifContext<'_>, DecodeError> {
    let mut context = HeifContext::new().map_err(damaged)?;
    context.set_security_limits(&limits()).map_err(damaged)?;
    context.read_bytes(data).map_err(damaged)?;
    Ok(context)
}

fn limits() -> SecurityLimits {
    let mut limits = SecurityLimits::new();
    limits.set_max_image_size_pixels(MAX_PIXELS);
    limits.set_max_total_memory(max_alloc());
    limits
}

/// Why libheif would not decode a picture of the given size.
fn refusal(e: HeifError, width: u32, height: u32) -> DecodeError {
    match e.sub_code {
        HeifErrorSubCode::SecurityLimitExceeded => DecodeError::TooManyPixels { width, height },
        HeifErrorSubCode::UnsupportedCodec => DecodeError::Unsupported,
        _ => damaged(e),
    }
}

fn damaged(e: HeifError) -> DecodeError {
    DecodeError::Damaged(e.message)
}

/// Decodes to 8 bits a channel, or 16 when the photo has more, so the
/// colour conversion that follows keeps the extra precision.
fn pixels(lib: &LibHeif, handle: &ImageHandle) -> Result<Option<DynamicImage>, HeifError> {
    let alpha = handle.has_alpha_channel();
    let deep = handle.luma_bits_per_pixel() > 8;
    let chroma = match (deep, alpha) {
        (false, false) => RgbChroma::Rgb,
        (false, true) => RgbChroma::Rgba,
        (true, false) => RgbChroma::HdrRgbLe,
        (true, true) => RgbChroma::HdrRgbaLe,
    };
    let image = lib.decode(handle, ColorSpace::Rgb(chroma), None)?;
    let planes = image.planes();
    let Some(plane) = planes.interleaved else {
        return Ok(None);
    };
    let channels = if alpha { 4 } else { 3 };
    let (width, height) = (plane.width, plane.height);
    let Ok(rows) = usize::try_from(height) else {
        return Ok(None);
    };
    let Some(row_bytes) = usize::try_from(width)
        .ok()
        .and_then(|w| w.checked_mul(channels * if deep { 2 } else { 1 }))
    else {
        return Ok(None);
    };
    // Checked so the slicing below cannot panic should libheif hand back
    // less than it describes, or an empty plane, whose zero-byte rows
    // could not be stepped through.
    let needed = plane
        .stride
        .checked_mul(rows.saturating_sub(1))
        .and_then(|n| n.checked_add(row_bytes));
    if rows == 0
        || row_bytes == 0
        || plane.stride < row_bytes
        || needed.is_none_or(|n| plane.data.len() < n)
    {
        return Ok(None);
    }
    let rows = plane.data.chunks(plane.stride).take(rows);
    let image = if deep {
        // libheif gives the depth of one channel here, such as 10.
        let bits = plane.bits_per_pixel;
        let samples: Vec<u16> = rows
            .flat_map(|row| row[..row_bytes].as_chunks::<2>().0)
            .map(|b| widen(u16::from_le_bytes(*b), bits))
            .collect();
        if alpha {
            ImageBuffer::<Rgba<u16>, _>::from_raw(width, height, samples).map(DynamicImage::from)
        } else {
            ImageBuffer::<Rgb<u16>, _>::from_raw(width, height, samples).map(DynamicImage::from)
        }
    } else {
        let samples: Vec<u8> = rows.flat_map(|row| &row[..row_bytes]).copied().collect();
        if alpha {
            ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, samples).map(DynamicImage::from)
        } else {
            ImageBuffer::<Rgb<u8>, _>::from_raw(width, height, samples).map(DynamicImage::from)
        }
    };
    Ok(image)
}

/// Stretches a sample of `bits` bits to the full 16-bit range, so a
/// 10-bit white of 1023 becomes 65535 rather than a dim grey.
fn widen(sample: u16, bits: u8) -> u16 {
    if !(1..16).contains(&bits) {
        return sample;
    }
    let sample = sample.min((1 << bits) - 1);
    let shift = 16 - u32::from(bits);
    // Repeating the top bits in the gap fills it evenly to the maximum.
    let mut out = u32::from(sample) << shift;
    let mut fill = shift;
    while fill > 0 {
        let take = fill.min(u32::from(bits));
        out |= u32::from(sample) >> (u32::from(bits) - take) << (fill - take);
        fill -= take;
    }
    u16::try_from(out).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libheif_refuses_oversized_pictures_on_its_own() {
        // The fixture with its stored size raised past the pixel limit;
        // its crop still shows 48 × 32, so only libheif's limit is left
        // to stop it.
        let mut data = crate::fixtures::GRADIENT_HEIC.to_vec();
        let at = data.windows(4).position(|w| w == b"ispe").unwrap() + 8;
        data[at..at + 8].copy_from_slice(&[0, 0, 0x4e, 0x20, 0, 0, 0x4e, 0x20]);
        let lib = LibHeif::new();
        let context = open(&data).unwrap();
        let handle = context.primary_image_handle().unwrap();
        assert_eq!((handle.width(), handle.height()), (48, 32));
        let err = pixels(&lib, &handle).err().unwrap();
        assert!(matches!(
            err.sub_code,
            HeifErrorSubCode::SecurityLimitExceeded
        ));
    }

    /// The bindings are built for libheif 1.23; a different libheif, such
    /// as the Flatpak manifest updating it, must come with new bindings.
    #[test]
    fn the_bindings_are_those_of_the_libheif_linked() {
        let version = LibHeif::new().version();
        assert_eq!(version[..2], [1, 23], "libheif {version:?}");
    }

    /// Limits made while other threads start and stop libheif are the
    /// same each time, with the newest limits libheif has.
    #[test]
    fn the_limits_are_whole_and_steady() {
        let first = limits();
        assert_eq!(first.max_number_of_file_brands(), 1000);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    (0..200)
                        .map(|_| {
                            let _lib = LibHeif::new();
                            limits().max_number_of_file_brands()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for thread in threads {
            assert!(thread.join().unwrap().iter().all(|n| *n == 1000));
        }
    }

    #[test]
    fn libheif_limits_allow_the_largest_accepted_photo() {
        let caps = limits();
        assert_eq!(caps.max_image_size_pixels(), MAX_PIXELS);
        assert_eq!(caps.max_total_memory(), max_alloc());
        assert!(max_alloc() >= MAX_PIXELS * 8);
    }

    #[test]
    fn libheif_errors_read_as_refusals() {
        let error = |sub_code| HeifError {
            code: libheif_rs::HeifErrorCode::MemoryAllocationError,
            sub_code,
            message: "the reason".into(),
        };
        assert!(matches!(
            refusal(error(HeifErrorSubCode::SecurityLimitExceeded), 7, 9),
            DecodeError::TooManyPixels {
                width: 7,
                height: 9
            }
        ));
        assert!(matches!(
            refusal(error(HeifErrorSubCode::UnsupportedCodec), 7, 9),
            DecodeError::Unsupported
        ));
        match refusal(error(HeifErrorSubCode::EndOfData), 7, 9) {
            DecodeError::Damaged(why) => assert_eq!(why, "the reason"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn deep_samples_fill_the_whole_range() {
        assert_eq!(widen(0, 10), 0);
        assert_eq!(widen(1023, 10), 65535);
        assert_eq!(widen(4095, 12), 65535);
        assert_eq!(widen(512, 10), 32800);
        assert_eq!(widen(255, 8), 65535);
        assert_eq!(widen(1, 1), 65535);
        // Out-of-range input is clamped rather than wrapped.
        assert_eq!(widen(5000, 10), 65535);
        assert_eq!(widen(40000, 16), 40000);
    }
}
