//! Tests of readying and saving photos.

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::metadata::Orientation;
use image::{
    DynamicImage, ExtendedColorType, ImageBuffer, ImageEncoder, Rgb, RgbImage, Rgba, RgbaImage,
};
use moxcms::ColorProfile;

use crate::fixtures::crc32;
use crate::pipeline::{MAX_PROFILE_BYTES, is_deep, source_profile};
use crate::{
    Colour, Decoded, Format, LONG_EDGES, Output, QUALITIES, THUMBNAIL_EDGE, decode_bytes, encode,
    upright,
};

const RED: [u8; 3] = [220, 30, 30];
const GREEN: [u8; 3] = [30, 200, 30];
const BLUE: [u8; 3] = [30, 30, 220];
const WHITE: [u8; 3] = [240, 240, 240];

/// A picture whose four quarters differ, so any turn or flip shows.
fn quarters(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        Rgb(match (x < width / 2, y < height / 2) {
            (true, true) => RED,
            (false, true) => GREEN,
            (true, false) => BLUE,
            (false, false) => WHITE,
        })
    })
}

/// The colours at the middles of the four quarters: top left, top right,
/// bottom left, bottom right.
fn corners(image: &RgbImage) -> [[u8; 3]; 4] {
    let (w, h) = image.dimensions();
    [
        image.get_pixel(w / 4, h / 4).0,
        image.get_pixel(3 * w / 4, h / 4).0,
        image.get_pixel(w / 4, 3 * h / 4).0,
        image.get_pixel(3 * w / 4, 3 * h / 4).0,
    ]
}

fn near(got: [u8; 3], want: [u8; 3], tolerance: i16) -> bool {
    got.iter()
        .zip(want)
        .all(|(g, w)| (i16::from(*g) - i16::from(w)).abs() <= tolerance)
}

fn assert_corners(image: &RgbImage, want: [[u8; 3]; 4], what: &str) {
    let got = corners(image);
    for (g, w) in got.iter().zip(want) {
        assert!(near(*g, w, 12), "{what}: {got:?} vs {want:?}");
    }
}

fn jpeg(image: &RgbImage) -> Vec<u8> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 95)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    out
}

fn png(
    data: &[u8],
    width: u32,
    height: u32,
    colour: ExtendedColorType,
    icc: Option<&[u8]>,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = PngEncoder::new(&mut out);
    if let Some(icc) = icc {
        encoder.set_icc_profile(icc.to_vec()).unwrap();
    }
    encoder.write_image(data, width, height, colour).unwrap();
    out
}

/// A JPEG with extra segments placed straight after its start marker.
fn with_segments(jpeg: &[u8], segments: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut out = jpeg[..2].to_vec();
    for (marker, payload) in segments {
        out.extend([0xFF, *marker]);
        out.extend(u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        out.extend(payload);
    }
    out.extend(&jpeg[2..]);
    out
}

/// A PNG with an extra chunk placed before its picture data.
fn with_chunk(png: &[u8], kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let at = png.windows(4).position(|w| w == b"IDAT").unwrap() - 4;
    let mut chunk = kind.to_vec();
    chunk.extend(data);
    let mut out = png[..at].to_vec();
    out.extend(u32::try_from(data.len()).unwrap().to_be_bytes());
    out.extend(&chunk);
    out.extend(crc32(&chunk).to_be_bytes());
    out.extend(&png[at..]);
    out
}

/// EXIF data in TIFF form giving an orientation and a camera name.
fn tiff(orientation: u16, make: &[u8]) -> Vec<u8> {
    let mut t = b"II*\0".to_vec();
    t.extend(8u32.to_le_bytes());
    t.extend(2u16.to_le_bytes());
    // The camera name, stored after the directory: 8 + 2 + 2 × 12 + 4.
    t.extend(0x010Fu16.to_le_bytes());
    t.extend(2u16.to_le_bytes());
    t.extend(u32::try_from(make.len()).unwrap().to_le_bytes());
    t.extend(38u32.to_le_bytes());
    t.extend(0x0112u16.to_le_bytes());
    t.extend(3u16.to_le_bytes());
    t.extend(1u32.to_le_bytes());
    t.extend(orientation.to_le_bytes());
    t.extend([0, 0]);
    t.extend(0u32.to_le_bytes());
    t.extend(make);
    t
}

fn exif_segment(orientation: u16) -> (u8, Vec<u8>) {
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend(tiff(orientation, b"SecretCam\0"));
    (0xE1, payload)
}

/// Each EXIF orientation, from 1 to 8, with the quarters it should show
/// once upright, read from the stored quarters red, green, blue, white.
const ORIENTATIONS: [(u16, [[u8; 3]; 4], bool); 8] = [
    (1, [RED, GREEN, BLUE, WHITE], false),
    (2, [GREEN, RED, WHITE, BLUE], false),
    (3, [WHITE, BLUE, GREEN, RED], false),
    (4, [BLUE, WHITE, RED, GREEN], false),
    (5, [RED, BLUE, GREEN, WHITE], true),
    (6, [BLUE, RED, WHITE, GREEN], true),
    (7, [WHITE, GREEN, BLUE, RED], true),
    (8, [GREEN, WHITE, RED, BLUE], true),
];

#[test]
fn every_exif_orientation_turns_upright() {
    let stored = quarters(64, 32);
    for (n, want, turned) in ORIENTATIONS {
        let file = with_segments(&jpeg(&stored), &[exif_segment(n)]);
        let image = upright(decode_bytes(&file).unwrap());
        let size = if turned { (32, 64) } else { (64, 32) };
        assert_eq!(image.dimensions(), size, "orientation {n}");
        assert_corners(&image, want, &format!("orientation {n}"));
    }
}

#[test]
fn png_orientation_is_read_too() {
    let stored = quarters(64, 32);
    let file = png(stored.as_raw(), 64, 32, ExtendedColorType::Rgb8, None);
    let file = with_chunk(&file, b"eXIf", &tiff(6, b"SecretCam\0"));
    let image = upright(decode_bytes(&file).unwrap());
    assert_eq!(image.dimensions(), (32, 64));
    assert_corners(&image, ORIENTATIONS[5].1, "PNG orientation 6");
}

/// The marker of every segment before the picture data, and the data of
/// any APP0 segment.
fn segments(jpeg: &[u8]) -> Vec<(u8, Vec<u8>)> {
    assert_eq!(&jpeg[..2], [0xFF, 0xD8], "starts as a JPEG");
    assert_eq!(&jpeg[jpeg.len() - 2..], [0xFF, 0xD9], "ends as a JPEG");
    let mut found = Vec::new();
    let mut at = 2;
    loop {
        assert_eq!(jpeg[at], 0xFF);
        let marker = jpeg[at + 1];
        let len = usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
        found.push((marker, jpeg[at + 4..at + 2 + len].to_vec()));
        if marker == 0xDA {
            return found;
        }
        at += 2 + len;
    }
}

/// Checks a saved JPEG holds only what a picture needs: tables, the frame,
/// and the JFIF header, with no EXIF, XMP, ICC, IPTC, or comments.
fn assert_only_pixels(jpeg: &[u8]) {
    for (marker, data) in segments(jpeg) {
        match marker {
            // Quantization and Huffman tables, frame, restart interval,
            // and the picture data.
            0xDB | 0xC4 | 0xC0 | 0xC2 | 0xDD | 0xDA => {}
            0xE0 => assert!(data.starts_with(b"JFIF\0"), "APP0 that is not JFIF"),
            other => panic!("segment {other:#04x} in the saved photo"),
        }
    }
    assert!(
        !jpeg.windows(6).any(|w| w == b"Secret"),
        "text from the original survived"
    );
}

#[test]
fn saved_photos_hold_no_metadata() {
    let icc = ColorProfile::new_srgb().encode().unwrap();
    let mut icc_segment = b"ICC_PROFILE\0\x01\x01".to_vec();
    icc_segment.extend(&icc);
    let mut xmp = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    xmp.extend(b"<x:xmpmeta><SecretXmp/></x:xmpmeta>");
    let mut iptc = b"Photoshop 3.0\0".to_vec();
    iptc.extend(b"8BIM SecretIptc");
    let file = with_segments(
        &jpeg(&quarters(64, 32)),
        &[
            exif_segment(1),
            (0xE1, xmp),
            (0xE2, icc_segment),
            (0xED, iptc),
            (0xFE, b"SecretComment".to_vec()),
        ],
    );
    assert!(file.windows(6).any(|w| w == b"Secret"));
    let decoded = decode_bytes(&file).unwrap();
    assert_eq!(decoded.format, Format::Jpeg);
    let saved = encode(&upright(decoded), Output::default()).unwrap();
    assert_only_pixels(&saved.full);
    assert_only_pixels(&saved.thumbnail);

    let text = png(
        quarters(64, 32).as_raw(),
        64,
        32,
        ExtendedColorType::Rgb8,
        Some(&icc),
    );
    let text = with_chunk(&text, b"tEXt", b"Comment\0SecretText");
    let text = with_chunk(&text, b"eXIf", &tiff(1, b"SecretCam\0"));
    let saved = encode(&upright(decode_bytes(&text).unwrap()), Output::default()).unwrap();
    assert_only_pixels(&saved.full);
    assert_only_pixels(&saved.thumbnail);
}

/// What a Display P3 colour is in sRGB, worked out here from the published
/// conversion matrix rather than through the code under test.
fn p3_to_srgb(p3: [u8; 3]) -> [u8; 3] {
    let linear = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let encode = |c: f64| {
        let c = c.clamp(0.0, 1.0);
        let c = if c <= 0.003_130_8 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (c * 255.0).round() as u8
    };
    let [r, g, b] = p3.map(linear);
    [
        encode(1.224_940_2 * r - 0.224_940_2 * g),
        encode(-0.042_056_9 * r + 1.042_056_9 * g),
        encode(-0.019_637_6 * r - 0.078_636_1 * g + 1.098_273_7 * b),
    ]
}

const SAMPLES: [[u8; 3]; 5] = [
    [200, 100, 100],
    [90, 160, 70],
    [60, 90, 200],
    [128, 128, 128],
    [250, 220, 180],
];

#[test]
fn display_p3_photos_convert_to_srgb() {
    let p3 = ColorProfile::new_display_p3().encode().unwrap();
    let pixels: Vec<u8> = SAMPLES.iter().flatten().copied().collect();
    let file = png(&pixels, 5, 1, ExtendedColorType::Rgb8, Some(&p3));
    let image = upright(decode_bytes(&file).unwrap());
    for (x, sample) in (0..).zip(SAMPLES) {
        let got = image.get_pixel(x, 0).0;
        assert!(
            near(got, p3_to_srgb(sample), 2),
            "{sample:?} gave {got:?}, not {:?}",
            p3_to_srgb(sample)
        );
    }
    // The conversion changes colours, not greys.
    assert!(near(image.get_pixel(3, 0).0, [128, 128, 128], 1));
    assert!(!near(image.get_pixel(0, 0).0, SAMPLES[0], 4));

    // Code points for P3 give the same result as its ICC profile.
    let coded = upright(Decoded {
        image: DynamicImage::ImageRgb8(RgbImage::from_raw(5, 1, pixels).unwrap()),
        format: Format::Heic,
        colour: Colour::Cicp {
            primaries: 12,
            transfer: 13,
        },
        orientation: Orientation::NoTransforms,
    });
    for x in 0..5 {
        assert!(near(coded.get_pixel(x, 0).0, image.get_pixel(x, 0).0, 1));
    }
}

#[test]
fn deep_photos_convert_before_narrowing() {
    let p3 = ColorProfile::new_display_p3().encode().unwrap();
    let pixels: Vec<u8> = SAMPLES
        .iter()
        .flatten()
        .flat_map(|c| (u16::from(*c) * 257).to_ne_bytes())
        .collect();
    let file = png(&pixels, 5, 1, ExtendedColorType::Rgb16, Some(&p3));
    let decoded = decode_bytes(&file).unwrap();
    assert!(matches!(decoded.image, DynamicImage::ImageRgb16(_)));
    let image = upright(decoded);
    for (x, sample) in (0..).zip(SAMPLES) {
        assert!(near(image.get_pixel(x, 0).0, p3_to_srgb(sample), 2));
    }

    // Without a profile the samples are only narrowed, rounding to the
    // nearest 8-bit value.
    let plain: Vec<u8> = [0u16, 128, 129, 32896, 65535, 65200]
        .iter()
        .flat_map(|v| v.to_ne_bytes())
        .collect();
    let file = png(&plain, 2, 1, ExtendedColorType::Rgb16, None);
    let image = upright(decode_bytes(&file).unwrap());
    assert_eq!(image.as_raw(), &[0, 0, 1, 128, 255, 254]);
}

#[test]
fn srgb_and_unusable_profiles_leave_colours_alone() {
    let pixels: Vec<u8> = SAMPLES.iter().flatten().copied().collect();
    let gray = ColorProfile::new_gray_with_gamma(2.2).encode().unwrap();
    for colour in [
        Colour::Unstated,
        Colour::Icc(b"not a colour profile".to_vec()),
        Colour::Icc(gray),
        Colour::Cicp {
            primaries: 1,
            transfer: 13,
        },
        Colour::Cicp {
            primaries: 2,
            transfer: 2,
        },
        Colour::Cicp {
            primaries: 200,
            transfer: 13,
        },
    ] {
        let image = upright(Decoded {
            image: DynamicImage::ImageRgb8(RgbImage::from_raw(5, 1, pixels.clone()).unwrap()),
            format: Format::Png,
            colour: colour.clone(),
            orientation: Orientation::NoTransforms,
        });
        assert_eq!(image.as_raw(), &pixels, "{colour:?}");
    }
    let srgb = ColorProfile::new_srgb().encode().unwrap();
    let file = png(&pixels, 5, 1, ExtendedColorType::Rgb8, Some(&srgb));
    let image = upright(decode_bytes(&file).unwrap());
    for (got, want) in image.pixels().zip(SAMPLES) {
        assert!(near(got.0, want, 1));
    }
}

#[test]
fn an_oversized_profile_is_not_read() {
    let mut p3 = ColorProfile::new_display_p3().encode().unwrap();
    p3.resize(MAX_PROFILE_BYTES + 1, 0);
    assert!(source_profile(&Colour::Icc(p3)).is_none());
}

#[test]
fn damaged_profiles_leave_the_photo_whole() {
    // Every truncation of a real profile, and every byte of it set to
    // each extreme, as a hostile or damaged file might carry.
    let p3 = ColorProfile::new_display_p3().encode().unwrap();
    let mut damaged: Vec<Vec<u8>> = (0..p3.len()).map(|n| p3[..n].to_vec()).collect();
    for at in 0..p3.len() {
        for value in [0x00, 0xFF] {
            let mut profile = p3.clone();
            profile[at] = value;
            damaged.push(profile);
        }
    }
    let pixels = RgbImage::from_fn(4, 4, |x, y| Rgb([(x * 60) as u8, (y * 60) as u8, 128]));
    let deep = DynamicImage::ImageRgb8(pixels.clone()).into_rgb16();
    for (n, icc) in damaged.into_iter().enumerate() {
        let mut images = vec![DynamicImage::ImageRgb8(pixels.clone())];
        // A deep transform takes far longer to set up, so a sample of
        // the damaged profiles is tried at 16 bits too.
        if n % 16 == 0 {
            images.push(DynamicImage::ImageRgb16(deep.clone()));
        }
        for image in images {
            let decoded = Decoded {
                image,
                format: Format::Png,
                colour: Colour::Icc(icc.clone()),
                orientation: Orientation::NoTransforms,
            };
            assert_eq!(upright(decoded).dimensions(), (4, 4));
        }
    }
}

#[test]
fn only_photos_needing_conversion_get_a_profile() {
    let p3 = ColorProfile::new_display_p3().encode().unwrap();
    let gray = ColorProfile::new_gray_with_gamma(2.2).encode().unwrap();
    assert!(source_profile(&Colour::Icc(p3)).is_some());
    assert!(
        source_profile(&Colour::Cicp {
            primaries: 12,
            transfer: 13
        })
        .is_some()
    );
    for none in [
        Colour::Unstated,
        Colour::Icc(gray),
        Colour::Icc(b"junk".to_vec()),
        Colour::Cicp {
            primaries: 1,
            transfer: 13,
        },
        Colour::Cicp {
            primaries: 2,
            transfer: 2,
        },
        Colour::Cicp {
            primaries: 1,
            transfer: 2,
        },
        Colour::Cicp {
            primaries: 200,
            transfer: 13,
        },
        Colour::Cicp {
            primaries: 12,
            transfer: 200,
        },
    ] {
        assert!(source_profile(&none).is_none(), "{none:?}");
    }
}

#[test]
fn deep_means_more_than_8_bits_a_channel() {
    for (image, deep) in [
        (DynamicImage::new_rgb8(1, 1), false),
        (DynamicImage::new_rgba8(1, 1), false),
        (DynamicImage::new_luma8(1, 1), false),
        (DynamicImage::new_luma_a8(1, 1), false),
        (DynamicImage::new_rgb16(1, 1), true),
        (DynamicImage::new_rgba16(1, 1), true),
        (DynamicImage::new_luma16(1, 1), true),
    ] {
        assert_eq!(is_deep(&image), deep, "{:?}", image.color());
    }
}

#[test]
fn transparency_is_laid_over_white() {
    let pixels = RgbaImage::from_fn(4, 1, |x, _| {
        Rgba(match x {
            0 => [0, 0, 0, 0],
            1 => [200, 30, 30, 255],
            2 => [0, 0, 0, 128],
            _ => [20, 40, 60, 255],
        })
    });
    let want = [
        [255, 255, 255],
        [200, 30, 30],
        [127, 127, 127],
        [20, 40, 60],
    ];
    let file = png(pixels.as_raw(), 4, 1, ExtendedColorType::Rgba8, None);
    let image = upright(decode_bytes(&file).unwrap());
    let got: Vec<[u8; 3]> = image.pixels().map(|p| p.0).collect();
    assert_eq!(got, want);

    let deep: ImageBuffer<Rgba<u16>, Vec<u16>> = ImageBuffer::from_fn(4, 1, |x, y| {
        Rgba(pixels.get_pixel(x, y).0.map(|c| u16::from(c) * 257))
    });
    let bytes: Vec<u8> = deep.as_raw().iter().flat_map(|v| v.to_ne_bytes()).collect();
    let file = png(&bytes, 4, 1, ExtendedColorType::Rgba16, None);
    let image = upright(decode_bytes(&file).unwrap());
    let got: Vec<[u8; 3]> = image.pixels().map(|p| p.0).collect();
    assert_eq!(got, want);
}

#[test]
fn grey_photos_become_colour() {
    let file = png(&[0, 100, 255], 3, 1, ExtendedColorType::L8, None);
    let image = upright(decode_bytes(&file).unwrap());
    assert_eq!(image.as_raw(), &[0, 0, 0, 100, 100, 100, 255, 255, 255]);
}

fn size_of(jpeg: &[u8]) -> (u32, u32) {
    let image = image::load_from_memory(jpeg).unwrap();
    (image.width(), image.height())
}

#[test]
fn photos_shrink_to_the_chosen_edge_but_never_grow() {
    let cases = [
        // (picture, chosen edge, saved size, thumbnail size)
        ((3000, 2000), 1600, (1600, 1067), (320, 213)),
        ((2000, 3000), 1600, (1067, 1600), (213, 320)),
        ((800, 600), 1600, (800, 600), (320, 240)),
        ((300, 200), 1600, (300, 200), (300, 200)),
        ((5000, 10), 2400, (2400, 5), (320, 1)),
        ((5000, 1), 1600, (1600, 1), (320, 1)),
        ((3000, 2000), 9999, (2400, 1600), (320, 213)),
        ((3000, 2000), 10, (1200, 800), (320, 213)),
    ];
    for ((w, h), edge, full, thumb) in cases {
        let output = Output {
            long_edge: edge,
            ..Output::default()
        };
        let saved = encode(&quarters(w, h), output).unwrap();
        assert_eq!((saved.width, saved.height), full, "{w}x{h} at {edge}");
        assert_eq!(size_of(&saved.full), full, "{w}x{h} at {edge}");
        assert_eq!(size_of(&saved.thumbnail), thumb, "{w}x{h} at {edge}");
    }
    assert_eq!(*LONG_EDGES.start(), 1200);
    assert_eq!(*LONG_EDGES.end(), 2400);
    assert_eq!(THUMBNAIL_EDGE, 320);
}

#[test]
fn shrinking_keeps_the_picture() {
    let saved = encode(&quarters(3000, 2000), Output::default()).unwrap();
    let full = image::load_from_memory(&saved.full).unwrap().into_rgb8();
    assert_corners(&full, [RED, GREEN, BLUE, WHITE], "full size");
    let thumb = image::load_from_memory(&saved.thumbnail)
        .unwrap()
        .into_rgb8();
    assert_corners(&thumb, [RED, GREEN, BLUE, WHITE], "thumbnail");
}

#[test]
fn quality_is_kept_within_its_range() {
    // A busy picture, so quality shows in the file size.
    let busy = RgbImage::from_fn(400, 300, |x, y| {
        Rgb([(x * 7 + y * 3) as u8, (x * y) as u8, (y * 11) as u8])
    });
    let at = |quality| {
        let output = Output {
            quality,
            ..Output::default()
        };
        encode(&busy, output).unwrap().full
    };
    assert!(at(95).len() > at(85).len());
    assert!(at(85).len() > at(60).len());
    assert_eq!(at(10), at(*QUALITIES.start()));
    assert_eq!(at(100), at(*QUALITIES.end()));
    assert_eq!(at(0), at(60));
    assert_eq!(Output::default().quality, 85);
    assert_eq!(Output::default().long_edge, 1600);
}

#[test]
fn saved_photos_read_back() {
    let saved = encode(&quarters(64, 32), Output::default()).unwrap();
    let decoded = decode_bytes(&saved.full).unwrap();
    assert_eq!(decoded.format, Format::Jpeg);
    assert_eq!(decoded.colour, Colour::Unstated);
    assert_eq!(decoded.orientation, Orientation::NoTransforms);
    assert_eq!(upright(decoded).dimensions(), (64, 32));
}

#[cfg(feature = "heic")]
#[test]
#[ignore = "needs libheif with an HEVC decoder, as built into the Flatpak"]
fn heic_photos_go_all_the_way_through() {
    let decoded = decode_bytes(crate::fixtures::GRADIENT_HEIC).unwrap();
    assert_eq!(decoded.orientation, Orientation::NoTransforms);
    let saved = encode(&upright(decoded), Output::default()).unwrap();
    assert_eq!((saved.width, saved.height), (48, 32));
    assert_only_pixels(&saved.full);
    let deep = decode_bytes(crate::fixtures::DEEP_HEIC).unwrap();
    let saved = encode(&upright(deep), Output::default()).unwrap();
    assert_eq!((saved.width, saved.height), (16, 16));
}
