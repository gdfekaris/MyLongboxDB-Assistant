//! Reading a photo file into pixels, within hard limits, so a huge or
//! hostile file is turned away before it can use much memory.

use std::fmt;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, RgbImage};

/// Largest file accepted.
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// Most pixels accepted: 100 megapixels, more than any phone camera.
pub const MAX_PIXELS: u64 = 100_000_000;

/// Longest side accepted, so a 1 by 100-million-pixel strip is refused
/// too.
pub const MAX_SIDE: u32 = 40_000;

/// The photo formats accepted, as found from the file's contents rather
/// than its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    WebP,
    Heic,
}

/// A decoded photo, with what is needed to show it the right way up and
/// in the right colours. Nothing else from the file is kept.
#[derive(Debug)]
pub struct Decoded {
    pub image: DynamicImage,
    pub format: Format,
    pub colour: Colour,
    /// How the pixels must be turned to stand upright.
    pub orientation: Orientation,
}

/// The colour space the photo's pixels are in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Colour {
    /// None stated, which by convention means sRGB.
    Unstated,
    /// An ICC profile, as cameras and phones embed.
    Icc(Vec<u8>),
    /// Code points from ITU-T H.273, as HEIC files may give instead.
    Cicp { primaries: u8, transfer: u8 },
}

/// Why a photo was turned away. Each reads as a sentence for the user.
#[derive(Debug)]
pub enum DecodeError {
    /// The file could not be read.
    Io(std::io::Error),
    TooLarge {
        bytes: u64,
    },
    TooManyPixels {
        width: u32,
        height: u32,
    },
    /// Not a photo in a format the app reads.
    Unsupported,
    /// The file claims to be a photo but its data is broken.
    Damaged(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "the file could not be read: {e}"),
            Self::TooLarge { bytes } => write!(
                f,
                "the file is {} MB; photos may be at most {} MB",
                bytes.div_ceil(1024 * 1024),
                MAX_FILE_BYTES / (1024 * 1024)
            ),
            Self::TooManyPixels { width, height } => write!(
                f,
                "the photo is {width} \u{d7} {height} pixels; photos may have at most {} \
                 megapixels and {MAX_SIDE} pixels on a side",
                MAX_PIXELS / 1_000_000
            ),
            Self::Unsupported => f.write_str("this is not a JPEG, PNG, WebP, or HEIC photo"),
            Self::Damaged(why) => write!(f, "the photo is damaged and cannot be read ({why})"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<std::io::Error> for DecodeError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Reads the photo at `path`. The file is only read, never changed.
pub fn decode_file(path: &Path) -> Result<Decoded, DecodeError> {
    decode_bytes(&read_file(path)?)
}

/// The bytes of the photo at `path`, refused without reading it all if
/// it is too large to decode, for decoding apart from reading.
pub fn read_file(path: &Path) -> Result<Vec<u8>, DecodeError> {
    let file = File::open(path)?;
    let bytes = file.metadata()?.len();
    if bytes > MAX_FILE_BYTES {
        return Err(DecodeError::TooLarge { bytes });
    }
    // The size is checked again while reading, in case the file grows.
    let mut data = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut data)?;
    let bytes = u64::try_from(data.len()).unwrap_or(u64::MAX);
    if bytes > MAX_FILE_BYTES {
        return Err(DecodeError::TooLarge { bytes });
    }
    Ok(data)
}

/// Reads a photo held in memory.
pub fn decode_bytes(data: &[u8]) -> Result<Decoded, DecodeError> {
    let bytes = u64::try_from(data.len()).unwrap_or(u64::MAX);
    if bytes > MAX_FILE_BYTES {
        return Err(DecodeError::TooLarge { bytes });
    }
    if is_heic(data) {
        return decode_heic(data);
    }
    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(DecodeError::Io)?;
    let format = match reader.format() {
        Some(ImageFormat::Jpeg) => Format::Jpeg,
        Some(ImageFormat::Png) => Format::Png,
        Some(ImageFormat::WebP) => Format::WebP,
        _ => return Err(DecodeError::Unsupported),
    };

    // The header gives the size without decoding, so an oversized photo
    // is refused before any pixels are allocated.
    let (width, height) = reader
        .into_dimensions()
        .map_err(|e| DecodeError::Damaged(e.to_string()))?;
    check_shown_size(width, height)?;
    let (image, colour, orientation) = decode_limited(data, width, height)?;
    Ok(Decoded {
        image,
        format,
        colour,
        orientation,
    })
}

/// Reads a photo kept in a collection, which the pipeline saved as a
/// JPEG no larger than `max_side` either way, such as
/// [`MAX_SAVED_SIDE`] for a full photo or
/// [`THUMBNAIL_EDGE`](crate::THUMBNAIL_EDGE) for a thumbnail.
///
/// A collection may come from someone else, so anything the pipeline
/// would not have written is refused before decoding: another format,
/// HEIC included, would reach a decoder the app's own photos never
/// need, and an oversized picture would take memory on every screen
/// that shows it.
pub fn decode_saved(data: &[u8], max_side: u32) -> Result<RgbImage, DecodeError> {
    let bytes = u64::try_from(data.len()).unwrap_or(u64::MAX);
    if bytes > MAX_FILE_BYTES {
        return Err(DecodeError::TooLarge { bytes });
    }
    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(DecodeError::Io)?;
    if reader.format() != Some(ImageFormat::Jpeg) {
        return Err(DecodeError::Unsupported);
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|e| DecodeError::Damaged(e.to_string()))?;
    check_shown_size(width, height)?;
    if width > max_side || height > max_side {
        return Err(DecodeError::TooManyPixels { width, height });
    }
    let (image, _, _) = decode_limited(data, width, height)?;
    Ok(image.into_rgb8())
}

/// Keeps the photo decoders loaded for as long as it lives.
///
/// libheif may load its decoders as plugins when it starts, which reads
/// files, so a thread that will lose file access starts it first and
/// keeps this until its decoding is done. Where plugin loading is off,
/// as in the Flatpak, it costs nothing.
pub struct Decoders {
    #[cfg(feature = "heic")]
    _heif: libheif_rs::LibHeif,
}

pub fn load_decoders() -> Decoders {
    Decoders {
        #[cfg(feature = "heic")]
        _heif: libheif_rs::LibHeif::new(),
    }
}

/// The largest side of a full photo the pipeline saves.
pub const MAX_SAVED_SIDE: u32 = *crate::LONG_EDGES.end();

#[cfg(feature = "heic")]
fn decode_heic(data: &[u8]) -> Result<Decoded, DecodeError> {
    crate::heic::decode(data)
}

#[cfg(not(feature = "heic"))]
fn decode_heic(_: &[u8]) -> Result<Decoded, DecodeError> {
    Err(DecodeError::Unsupported)
}

/// Whether the file is a HEIF container holding HEVC pictures, judged by
/// the brands in its opening `ftyp` box. Other HEIF kinds, such as AVIF,
/// are left for the other readers to turn away.
fn is_heic(data: &[u8]) -> bool {
    const BRANDS: [&[u8; 4]; 8] = [
        b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"hevm", b"hevs",
    ];
    let Some(size) = data
        .get(..4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    else {
        return false;
    };
    let Ok(size) = usize::try_from(size) else {
        return false;
    };
    if data.get(4..8) != Some(b"ftyp") || size < 16 || size > data.len() {
        return false;
    }
    // The major brand, then after the minor version the compatible ones.
    let (compatible, _) = data[16..size].as_chunks::<4>();
    std::iter::once(&[data[8], data[9], data[10], data[11]])
        .chain(compatible)
        .any(|brand| BRANDS.contains(&brand))
}

/// Like [`check_size`], for the size a photo is shown at, which must
/// also have at least one pixel each way: code further on divides by it
/// and steps through rows of it.
pub(crate) fn check_shown_size(width: u32, height: u32) -> Result<(), DecodeError> {
    if width == 0 || height == 0 {
        return Err(DecodeError::Damaged("the photo has no pixels".into()));
    }
    check_size(width, height)
}

pub(crate) fn check_size(width: u32, height: u32) -> Result<(), DecodeError> {
    if u64::from(width) * u64::from(height) > MAX_PIXELS || width > MAX_SIDE || height > MAX_SIDE {
        return Err(DecodeError::TooManyPixels { width, height });
    }
    Ok(())
}

/// Decodes with the allocation caps on, a second guard should a header
/// understate what decoding needs. The colour profile and orientation are
/// read on the way; a damaged one is ignored rather than refusing the
/// photo, since the pixels are what matter.
fn decode_limited(
    data: &[u8],
    width: u32,
    height: u32,
) -> Result<(DynamicImage, Colour, Orientation), DecodeError> {
    let refused = |e| match e {
        image::ImageError::Limits(_) => DecodeError::TooManyPixels { width, height },
        other => DecodeError::Damaged(other.to_string()),
    };
    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(DecodeError::Io)?;
    reader.limits(limits());
    let mut decoder = reader.into_decoder().map_err(refused)?;
    // The picture itself counts against the allocation cap too, which
    // decoding through a decoder does not do by itself.
    let mut caps = limits();
    caps.reserve(decoder.total_bytes()).map_err(refused)?;
    decoder.set_limits(caps).map_err(refused)?;
    let colour = match decoder.icc_profile() {
        Ok(Some(icc)) if !icc.is_empty() => Colour::Icc(icc),
        _ => Colour::Unstated,
    };
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let image = DynamicImage::from_decoder(decoder).map_err(refused)?;
    Ok((image, colour, orientation))
}

/// Caps on what decoding may allocate, matching the pixel limit.
fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(max_alloc());
    limits
}

/// The most decoding may allocate: 100 megapixels of 16-bit RGBA, plus
/// room for the decoder's own buffers.
pub(crate) fn max_alloc() -> u64 {
    MAX_PIXELS * 8 + 64 * 1024 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::codecs::webp::WebPEncoder;
    use image::{ExtendedColorType, ImageEncoder};

    /// A small generated picture, so tests never need real photos.
    fn picture() -> RgbImage {
        RgbImage::from_fn(30, 20, |x, y| {
            image::Rgb([(x * 8) as u8, (y * 12) as u8, 128])
        })
    }

    fn encoded(format: ImageFormat) -> Vec<u8> {
        let pic = picture();
        let mut out = Vec::new();
        match format {
            ImageFormat::Jpeg => JpegEncoder::new_with_quality(&mut out, 90)
                .write_image(&pic, 30, 20, ExtendedColorType::Rgb8)
                .unwrap(),
            ImageFormat::WebP => WebPEncoder::new_lossless(&mut out)
                .write_image(&pic, 30, 20, ExtendedColorType::Rgb8)
                .unwrap(),
            _ => pic
                .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
                .unwrap(),
        }
        out
    }

    /// A PNG that claims `width` × `height` pixels, with correct
    /// checksums but no picture data.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        use crate::fixtures::crc32;
        let mut chunk = b"IHDR".to_vec();
        chunk.extend(width.to_be_bytes());
        chunk.extend(height.to_be_bytes());
        chunk.extend([8, 2, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(13u32.to_be_bytes());
        png.extend(&chunk);
        png.extend(crc32(&chunk).to_be_bytes());
        // An empty data chunk: readers look for one before giving the size.
        png.extend(0u32.to_be_bytes());
        png.extend(b"IDAT");
        png.extend(crc32(b"IDAT").to_be_bytes());
        png
    }

    #[test]
    fn jpeg_png_and_webp_decode() {
        for (format, expected) in [
            (ImageFormat::Jpeg, Format::Jpeg),
            (ImageFormat::Png, Format::Png),
            (ImageFormat::WebP, Format::WebP),
        ] {
            let decoded = decode_bytes(&encoded(format)).unwrap();
            assert_eq!(decoded.format, expected);
            assert_eq!((decoded.image.width(), decoded.image.height()), (30, 20));
        }
    }

    #[test]
    fn other_files_are_not_photos() {
        for data in [
            b"GIF89a\x01\x00\x01\x00\x00\x00\x00;".as_slice(),
            b"just some text",
            b"",
        ] {
            assert!(matches!(decode_bytes(data), Err(DecodeError::Unsupported)));
        }
    }

    #[test]
    fn huge_pictures_are_refused_before_decoding() {
        for (width, height) in [(20_000, 20_000), (40_001, 1), (1, 40_001)] {
            match decode_bytes(&png_header(width, height)) {
                Err(DecodeError::TooManyPixels {
                    width: w,
                    height: h,
                }) => {
                    assert_eq!((w, h), (width, height));
                }
                other => panic!("{width}x{height}: {other:?}"),
            }
        }
        // Exactly at the limit is let through to decoding, which then
        // finds no picture data.
        assert!(matches!(
            decode_bytes(&png_header(10_000, 10_000)),
            Err(DecodeError::Damaged(_))
        ));
    }

    #[test]
    fn saved_photos_must_be_jpegs_the_pipeline_could_have_written() {
        let jpeg = |width, height| {
            let mut out = Vec::new();
            JpegEncoder::new(&mut out)
                .write_image(
                    RgbImage::new(width, height).as_raw(),
                    width,
                    height,
                    ExtendedColorType::Rgb8,
                )
                .unwrap();
            out
        };
        let read = decode_saved(&jpeg(30, 20), 30).unwrap();
        assert_eq!(read.dimensions(), (30, 20));
        assert!(matches!(
            decode_saved(&jpeg(31, 20), 30),
            Err(DecodeError::TooManyPixels { .. })
        ));
        assert!(matches!(
            decode_saved(&jpeg(20, MAX_SAVED_SIDE + 1), MAX_SAVED_SIDE),
            Err(DecodeError::TooManyPixels { .. })
        ));
        for other in [ImageFormat::Png, ImageFormat::WebP] {
            assert!(matches!(
                decode_saved(&encoded(other), 100),
                Err(DecodeError::Unsupported)
            ));
        }
        assert!(matches!(
            decode_saved(HEIC, 100),
            Err(DecodeError::Unsupported)
        ));
        assert!(decode_saved(b"\xFF\xD8\xFF not really a JPEG", 100).is_err());
        assert!(decode_saved(b"", 100).is_err());
    }

    #[test]
    fn a_photo_without_pixels_is_damaged() {
        for (width, height) in [(0, 0), (0, 10), (10, 0)] {
            assert!(matches!(
                check_shown_size(width, height),
                Err(DecodeError::Damaged(_))
            ));
            assert!(matches!(
                decode_bytes(&png_header(width, height)),
                Err(DecodeError::Damaged(_))
            ));
        }
        assert!(check_shown_size(1, 1).is_ok());
    }

    #[test]
    fn size_limits_hold_at_their_edges() {
        assert!(check_size(10_000, 10_000).is_ok());
        assert!(check_size(10_000, 10_001).is_err());
        assert!(check_size(40_000, 2_500).is_ok());
        assert!(check_size(40_001, 1).is_err());
        assert!(check_size(1, 40_001).is_err());
    }

    #[test]
    fn decoding_caps_allow_the_largest_accepted_photo() {
        // 100 megapixels of 16-bit RGBA must fit, or a legitimate photo
        // at the limit would be refused; the library's own default cap
        // is lower than that.
        let caps = limits();
        assert!(caps.max_alloc.unwrap() >= MAX_PIXELS * 8);
        assert_eq!(caps.max_image_width, Some(MAX_SIDE));
        assert_eq!(caps.max_image_height, Some(MAX_SIDE));
    }

    #[test]
    fn decoding_has_its_own_allocation_cap() {
        // Even past the header check, a picture too big to hold is
        // refused rather than allocated.
        assert!(matches!(
            decode_limited(&png_header(30_000, 30_000), 30_000, 30_000),
            Err(DecodeError::TooManyPixels { .. })
        ));
    }

    #[test]
    fn broken_photos_are_damaged() {
        let mut jpeg = encoded(ImageFormat::Jpeg);
        jpeg.truncate(jpeg.len() / 3);
        assert!(matches!(decode_bytes(&jpeg), Err(DecodeError::Damaged(_))));
        let mut png = encoded(ImageFormat::Png);
        png.truncate(60);
        assert!(matches!(decode_bytes(&png), Err(DecodeError::Damaged(_))));
    }

    use crate::fixtures::GRADIENT_HEIC as HEIC;

    /// The HEIC fixture with its stated size changed, as a hostile file
    /// might.
    #[cfg(feature = "heic")]
    fn heic_claiming(width: u32, height: u32) -> Vec<u8> {
        let mut data = HEIC.to_vec();
        let at = data.windows(4).position(|w| w == b"ispe").unwrap() + 8;
        data[at..at + 4].copy_from_slice(&width.to_be_bytes());
        data[at + 4..at + 8].copy_from_slice(&height.to_be_bytes());
        data
    }

    #[test]
    fn heic_is_known_by_its_brands() {
        assert!(is_heic(HEIC));
        let ftyp = |brands: &[&[u8; 4]]| {
            let mut data = (16 + 4 * u32::try_from(brands.len() - 1).unwrap())
                .to_be_bytes()
                .to_vec();
            data.extend(b"ftyp");
            data.extend(brands[0]);
            data.extend([0; 4]);
            for b in &brands[1..] {
                data.extend(*b);
            }
            data
        };
        assert!(is_heic(&ftyp(&[b"mif1", b"miaf", b"heic"])));
        assert!(is_heic(&ftyp(&[b"heix"])));
        // AVIF shares the container but not the codec.
        assert!(!is_heic(&ftyp(&[b"avif", b"mif1", b"miaf"])));
        assert!(!is_heic(&encoded(ImageFormat::Png)));
        // A box claiming more than the file holds, or too little.
        let mut long = ftyp(&[b"heic"]);
        long[3] = 200;
        assert!(!is_heic(&long));
        let mut short = ftyp(&[b"heic"]);
        short[3] = 8;
        assert!(!is_heic(&short));
        assert!(!is_heic(&HEIC[..10]));
    }

    #[cfg(not(feature = "heic"))]
    #[test]
    fn heic_needs_the_heic_feature() {
        assert!(matches!(decode_bytes(HEIC), Err(DecodeError::Unsupported)));
    }

    #[cfg(feature = "heic")]
    #[test]
    #[ignore = "needs libheif with an HEVC decoder, as built into the Flatpak"]
    fn heic_decodes() {
        let decoded = decode_bytes(HEIC).unwrap();
        assert_eq!(decoded.format, Format::Heic);
        let DynamicImage::ImageRgb8(image) = decoded.image else {
            panic!(
                "an 8-bit photo should stay 8-bit: {:?}",
                decoded.image.color()
            );
        };
        assert_eq!(image.dimensions(), (48, 32));
        // Lossy, so each sample need only be close to the original.
        for (x, y, pixel) in image.enumerate_pixels() {
            let expected = [(x * 5) % 256, (y * 8) % 256, 128];
            for (got, want) in pixel.0.iter().zip(expected) {
                let diff = i64::from(*got) - i64::from(want);
                assert!(diff.abs() <= 12, "({x}, {y}): {pixel:?} vs {expected:?}");
            }
        }
    }

    #[cfg(feature = "heic")]
    #[test]
    #[ignore = "needs libheif with an HEVC decoder, as built into the Flatpak"]
    fn deep_heic_keeps_its_precision() {
        let decoded = decode_bytes(crate::fixtures::DEEP_HEIC).unwrap();
        let DynamicImage::ImageRgb16(image) = decoded.image else {
            panic!(
                "a 10-bit photo should be 16-bit: {:?}",
                decoded.image.color()
            );
        };
        assert_eq!(image.dimensions(), (16, 16));
        // Ten-bit values stretched to sixteen: 1023 would be 65535.
        let stretch = |v: u32| (v * 65535 + 511) / 1023;
        for (x, y, pixel) in image.enumerate_pixels() {
            let expected = [stretch(x * 64), stretch(y * 64), stretch(512)];
            for (got, want) in pixel.0.iter().zip(expected) {
                let diff = i64::from(*got) - i64::from(want);
                assert!(diff.abs() <= 2000, "({x}, {y}): {pixel:?} vs {expected:?}");
            }
        }
    }

    #[cfg(feature = "heic")]
    #[test]
    fn huge_heic_is_refused_before_decoding() {
        for (width, height) in [(20_000, 20_000), (40_001, 1)] {
            match decode_bytes(&heic_claiming(width, height)) {
                Err(DecodeError::TooManyPixels {
                    width: w,
                    height: h,
                }) => assert_eq!((w, h), (width, height)),
                other => panic!("{width}x{height}: {other:?}"),
            }
        }
    }

    #[cfg(feature = "heic")]
    #[test]
    fn broken_heic_is_damaged() {
        for len in [40, 200, HEIC.len() / 2] {
            match decode_bytes(&HEIC[..len]) {
                Err(DecodeError::Damaged(_)) => {}
                other => panic!("{len} bytes: {other:?}"),
            }
        }
    }

    #[test]
    fn big_files_are_refused_unread() {
        let too_big = usize::try_from(MAX_FILE_BYTES + 1).unwrap();
        assert!(matches!(
            decode_bytes(&vec![0; too_big]),
            Err(DecodeError::TooLarge { .. })
        ));
        let dir = std::env::temp_dir().join("assistant-media-test-big");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.jpg");
        // A sparse file: large on paper, no disk used.
        File::create(&path)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        let err = decode_file(&path).unwrap_err();
        assert!(matches!(err, DecodeError::TooLarge { .. }));
        assert_eq!(
            err.to_string(),
            "the file is 51 MB; photos may be at most 50 MB"
        );
        let good = dir.join("good.png");
        std::fs::write(&good, encoded(ImageFormat::Png)).unwrap();
        assert_eq!(decode_file(&good).unwrap().format, Format::Png);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
