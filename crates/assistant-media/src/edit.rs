//! The owner's edits to a photo before it is saved: quarter turns, a
//! small straightening turn, and a crop.

use image::imageops::{self, FilterType};
use image::{Rgb, RgbImage};

/// The furthest a photo can be straightened either way, in degrees.
pub const MAX_STRAIGHTEN: f32 = 15.0;

/// The smallest crop, as a share of each side, so a slip of the mouse
/// cannot leave a sliver.
pub const MIN_CROP: f32 = 0.05;

/// A crop, as shares of the turned and straightened picture's width and
/// height, so it survives the picture being shown at any size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Crop {
    /// The whole picture.
    pub const FULL: Self = Self {
        left: 0.0,
        top: 0.0,
        right: 1.0,
        bottom: 1.0,
    };

    /// The crop kept within the picture, in order, and no smaller than
    /// [`MIN_CROP`].
    #[must_use]
    pub fn clamped(self) -> Self {
        let span = |a: f32, b: f32| {
            let (a, b) = (a.min(b).clamp(0.0, 1.0), a.max(b).clamp(0.0, 1.0));
            if b - a >= MIN_CROP {
                (a, b)
            } else if a + MIN_CROP <= 1.0 {
                (a, a + MIN_CROP)
            } else {
                (1.0 - MIN_CROP, 1.0)
            }
        };
        let finite = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
        let (left, right) = span(finite(self.left, 0.0), finite(self.right, 1.0));
        let (top, bottom) = span(finite(self.top, 0.0), finite(self.bottom, 1.0));
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    /// The same part of the picture once it is turned a quarter
    /// clockwise.
    #[must_use]
    pub fn turned_right(self) -> Self {
        Self {
            left: 1.0 - self.bottom,
            top: self.left,
            right: 1.0 - self.top,
            bottom: self.right,
        }
    }

    /// The same part of the picture once it is turned a quarter
    /// anticlockwise.
    #[must_use]
    pub fn turned_left(self) -> Self {
        Self {
            left: self.top,
            top: 1.0 - self.right,
            right: self.bottom,
            bottom: 1.0 - self.left,
        }
    }

    /// The crop in pixels of a `width` × `height` picture: left, top,
    /// width, height, at least one pixel each way.
    pub fn pixels(self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let c = self.clamped();
        let at = |share: f32, size: u32| {
            // Shares are within 0..=1, so this stays within the size.
            ((share * size as f32).round() as u32).min(size)
        };
        let (x0, x1) = (at(c.left, width), at(c.right, width));
        let (y0, y1) = (at(c.top, height), at(c.bottom, height));
        let x0 = x0.min(width.saturating_sub(1));
        let y0 = y0.min(height.saturating_sub(1));
        (x0, y0, (x1 - x0).max(1), (y1 - y0).max(1))
    }
}

/// Everything the editor can change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edit {
    /// Quarter turns clockwise, 0 to 3.
    pub quarter_turns: u8,
    /// Degrees clockwise, within [`MAX_STRAIGHTEN`] either way.
    pub straighten: f32,
    pub crop: Crop,
}

impl Default for Edit {
    fn default() -> Self {
        Self {
            quarter_turns: 0,
            straighten: 0.0,
            crop: Crop::FULL,
        }
    }
}

impl Edit {
    /// Turns the picture a quarter clockwise, keeping the crop on the
    /// same part of it.
    pub fn turn_right(&mut self) {
        self.quarter_turns = (self.quarter_turns + 1) % 4;
        self.crop = self.crop.turned_right();
    }

    pub fn turn_left(&mut self) {
        self.quarter_turns = (self.quarter_turns + 3) % 4;
        self.crop = self.crop.turned_left();
    }

    /// Sets the straightening, kept within its limits.
    pub fn set_straighten(&mut self, degrees: f32) {
        self.straighten = if degrees.is_finite() {
            degrees.clamp(-MAX_STRAIGHTEN, MAX_STRAIGHTEN)
        } else {
            0.0
        };
    }

    /// Whether the edit changes nothing.
    pub fn is_identity(&self) -> bool {
        self.quarter_turns.is_multiple_of(4)
            && self.straighten == 0.0
            && self.crop.clamped() == Crop::FULL
    }
}

/// The size of a `width` × `height` picture after `quarter_turns`.
/// Straightening keeps the size, enlarging the picture just enough that
/// no empty corners show.
pub fn turned_size(width: u32, height: u32, quarter_turns: u8) -> (u32, u32) {
    if quarter_turns % 2 == 1 {
        (height, width)
    } else {
        (width, height)
    }
}

/// How much a `width` × `height` frame must enlarge a picture turned by
/// `radians` so the turned picture still covers the whole frame.
fn cover_scale(width: f32, height: f32, radians: f32) -> f32 {
    let (sin, cos) = radians.abs().sin_cos();
    (cos + height / width * sin).max(cos + width / height * sin)
}

/// Where each point of the turned, straightened frame comes from in the
/// original picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mapping {
    turns: u8,
    /// The straightening actually used, kept within its limits.
    pub(crate) radians: f32,
    scale: f32,
    frame: (f32, f32),
    original: (f32, f32),
}

impl Mapping {
    pub(crate) fn new(width: u32, height: u32, edit: &Edit) -> Self {
        let turns = edit.quarter_turns % 4;
        let (fw, fh) = turned_size(width, height, turns);
        let mut limited = Edit::default();
        limited.set_straighten(edit.straighten);
        let radians = limited.straighten.to_radians();
        let frame = (fw as f32, fh as f32);
        Self {
            turns,
            radians,
            scale: cover_scale(frame.0, frame.1, radians),
            frame,
            original: (width as f32, height as f32),
        }
    }

    /// The point of the original picture shown at `(fx, fy)` of the
    /// frame.
    pub(crate) fn source(&self, fx: f32, fy: f32) -> (f32, f32) {
        let (cx, cy) = (self.frame.0 / 2.0, self.frame.1 / 2.0);
        let (sin, cos) = self.radians.sin_cos();
        // Undo the straightening: shrink back, then turn back.
        let (dx, dy) = ((fx - cx) / self.scale, (fy - cy) / self.scale);
        let rx = dx * cos + dy * sin + cx;
        let ry = -dx * sin + dy * cos + cy;
        // Undo the quarter turns.
        let (ow, oh) = self.original;
        match self.turns {
            1 => (ry, oh - rx),
            2 => (ow - rx, oh - ry),
            3 => (ow - ry, rx),
            _ => (rx, ry),
        }
    }
}

/// The picture with the edit applied, made in one pass so only the
/// result is allocated.
pub fn apply(image: &RgbImage, edit: &Edit) -> RgbImage {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return image.clone();
    }
    let mapping = Mapping::new(w, h, edit);
    let (frame_w, frame_h) = turned_size(w, h, edit.quarter_turns % 4);
    let (x0, y0, out_w, out_h) = edit.crop.pixels(frame_w, frame_h);
    RgbImage::from_fn(out_w, out_h, |u, v| {
        // The centre of this pixel in the frame.
        let (ox, oy) = mapping.source((x0 + u) as f32 + 0.5, (y0 + v) as f32 + 0.5);
        sample(image, ox - 0.5, oy - 0.5)
    })
}

/// The colour at a point between pixel centres, blended from the four
/// nearest and held at the edges.
pub(crate) fn sample(image: &RgbImage, x: f32, y: f32) -> Rgb<u8> {
    let (w, h) = image.dimensions();
    let max_x = (w - 1) as f32;
    let max_y = (h - 1) as f32;
    let x = x.clamp(0.0, max_x);
    let y = y.clamp(0.0, max_y);
    let (x0, y0) = (x.floor(), y.floor());
    let (tx, ty) = (x - x0, y - y0);
    let (x0, y0) = (x0 as u32, y0 as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let [a, b, c, d] = [
        image.get_pixel(x0, y0).0,
        image.get_pixel(x1, y0).0,
        image.get_pixel(x0, y1).0,
        image.get_pixel(x1, y1).0,
    ];
    let mix = |i: usize| {
        let top = f32::from(a[i]) * (1.0 - tx) + f32::from(b[i]) * tx;
        let bottom = f32::from(c[i]) * (1.0 - tx) + f32::from(d[i]) * tx;
        (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8
    };
    Rgb([mix(0), mix(1), mix(2)])
}

/// A smaller copy for the editor to show and redraw quickly, with its
/// longest edge at most `edge`.
pub fn preview(image: &RgbImage, edge: u32) -> RgbImage {
    let (w, h) = image.dimensions();
    if w.max(h) <= edge || w == 0 || h == 0 {
        return image.clone();
    }
    let (pw, ph) = if w >= h {
        (
            edge,
            ((h as f32 * edge as f32 / w as f32).round() as u32).max(1),
        )
    } else {
        (
            ((w as f32 * edge as f32 / h as f32).round() as u32).max(1),
            edge,
        )
    };
    imageops::resize(image, pw, ph, FilterType::Triangle)
}
