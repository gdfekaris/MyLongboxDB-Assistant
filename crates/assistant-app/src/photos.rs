//! The owner's photos: read away from the window, decoded on a thread
//! that can reach nothing, resized once to the size kept, and shown as
//! plain pixels, never through GTK's own image loaders.

use std::path::Path;

use assistant_core::Jpeg;
use assistant_media::{
    DEFAULT_LONG_EDGE, DEFAULT_QUALITY, Edit, MAX_SAVED_SIDE, Output, RgbImage, apply_edit,
    decode_bytes, decode_saved, encode, preview, read_file, upright,
};
use gtk::{gdk, glib};

use crate::i18n::{gettext, gettext_f};

/// The longest edge of a photo's thumbnail in the list.
const THUMBNAIL_EDGE: u32 = 160;

/// A photo read and ready to look up.
#[derive(Clone)]
pub struct Picked {
    /// The file's name, for the owner to tell photos apart.
    pub name: String,
    /// The file as picked, kept so an edit starts from the photo itself
    /// rather than from the smaller copy.
    pub original: std::rc::Rc<Vec<u8>>,
    /// Upright, resized, and re-encoded from pixels alone.
    pub jpeg: Jpeg,
    pub size: (u32, u32),
    pub thumbnail: RgbImage,
}

impl std::fmt::Debug for Picked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The bytes would flood any log.
        f.debug_struct("Picked")
            .field("name", &self.name)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// What reading a photo gives, before it is tied to the window.
pub struct Prepared {
    pub jpeg: Jpeg,
    pub size: (u32, u32),
    pub thumbnail: RgbImage,
}

/// Reads the photo at `path` within the photo limits. Slow for large
/// photos, so it runs away from the window.
pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    read_file(path).map_err(|e| e.to_string())
}

/// Decodes a photo's bytes, applies `edit`, and makes the JPEG kept and
/// sent. Slow for large photos, so it runs away from the window.
pub fn prepare(bytes: Vec<u8>, edit: Edit) -> Result<Prepared, String> {
    let full = confined(move || Ok(upright(decode_bytes(&bytes).map_err(|e| e.to_string())?)))?;
    let full = if edit.is_identity() {
        full
    } else {
        apply_edit(&full, &edit)
    };
    let encoded = encode(
        &full,
        Output {
            long_edge: DEFAULT_LONG_EDGE,
            quality: DEFAULT_QUALITY,
        },
    )
    .map_err(|e| e.to_string())?;
    let size = (encoded.width, encoded.height);
    Ok(Prepared {
        jpeg: Jpeg::new(encoded.full).ok_or_else(|| gettext("the photo could not be saved"))?,
        size,
        thumbnail: preview(&full, THUMBNAIL_EDGE),
    })
}

/// Decodes a photo for the editor: upright, at most `edge` pixels a
/// side. Slow, so it runs away from the window.
pub fn for_editing(bytes: Vec<u8>, edge: u32) -> Result<RgbImage, String> {
    let full = confined(move || Ok(upright(decode_bytes(&bytes).map_err(|e| e.to_string())?)))?;
    Ok(preview(&full, edge))
}

/// Decodes on a thread of its own that can reach no files, sockets, or
/// other descriptors, so a photo crafted to take over a decoder finds
/// nothing within reach of that thread. The decoders are the code most
/// exposed to files from elsewhere, and partly written in C.
fn confined<T: Send + 'static>(
    decode: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    // Looked up before the thread starts: a translation is read from a
    // file, which the confined thread may not be able to open.
    // Translators: {reason} says why the photo thread could not lock itself
    // down, in English.
    let unsafe_text = gettext("the photo could not be read safely: {reason}");
    let unreadable = gettext("the photo could not be read");
    let thread = std::thread::Builder::new()
        .name("photo".into())
        .spawn(move || {
            let _decoders = assistant_media::load_decoders();
            match assistant_sandbox::confine_thread() {
                Ok(assistant_sandbox::Enforced::Fully) => {}
                // The filter still holds where Landlock does not.
                Ok(level) => {
                    eprintln!("mylongboxdb-assistant: photo thread file limits: {level:?}")
                }
                Err(why) => return Err(unsafe_text.replace("{reason}", &why.to_string())),
            }
            decode()
        })
        .map_err(|e| e.to_string())?;
    thread.join().unwrap_or(Err(unreadable))
}

/// A kept photo to show, at most `edge` pixels a side. Kept photos are
/// read back from a file that could have been changed, so only a JPEG
/// the app could have made is decoded, by the size-limited decoder.
pub fn kept(jpeg: &Jpeg, edge: u32) -> Option<gdk::Texture> {
    let image = decode_saved(jpeg.bytes(), MAX_SAVED_SIDE).ok()?;
    texture_of(&preview(&image, edge))
}

/// Hands pixels to GTK to show.
pub fn texture_of(image: &RgbImage) -> Option<gdk::Texture> {
    let (width, height) = image.dimensions();
    let stride = usize::try_from(width).ok()?.checked_mul(3)?;
    let texture = gdk::MemoryTexture::new(
        i32::try_from(width).ok()?,
        i32::try_from(height).ok()?,
        gdk::MemoryFormat::R8g8b8,
        &glib::Bytes::from(image.as_raw().as_slice()),
        stride,
    );
    Some(gdk::Texture::from(texture))
}

/// Why a photo could not be edited, as a line for the owner.
pub fn not_edited(name: &str, why: &str) -> String {
    gettext_f(
        "{name} was not changed: {why}",
        &[("name", name), ("why", why)],
    )
}

/// Why a photo was not added, as a line for the owner.
pub fn not_added(name: &str, why: &str) -> String {
    gettext_f(
        "{name} was not added: {why}",
        &[("name", name), ("why", why)],
    )
}

/// The endings of the photo files the app takes, as the file chooser
/// offers them.
pub const SUFFIXES: [&str; 6] = ["jpg", "jpeg", "png", "webp", "heic", "heif"];

/// Of the files dropped on the page, by name and whether each is a file
/// on this computer: the places of those to read, and a line for each
/// refused. A photo dragged from a web page can arrive as its address
/// alone, and the app never downloads anything, so it must be saved
/// first. `one_only` is for a book's back, which takes a single photo.
pub fn dropped(files: &[(String, bool)], one_only: bool) -> (Vec<usize>, Vec<String>) {
    let mut taken = Vec::new();
    let mut refused = Vec::new();
    for (n, (name, local)) in files.iter().enumerate() {
        let suffix = name
            .rsplit_once('.')
            .map(|(_, s)| s.to_ascii_lowercase())
            .unwrap_or_default();
        if !local {
            refused.push(not_added(
                name,
                &gettext("only files on this computer can be added; save the photo first"),
            ));
        } else if !SUFFIXES.contains(&suffix.as_str()) {
            refused.push(not_added(
                name,
                &gettext("only JPEG, PNG, WebP, and HEIC photos can be added"),
            ));
        } else {
            taken.push(n);
        }
    }
    if one_only && taken.len() > 1 {
        refused.push(gettext(
            "A book's back cover is one photo; drop the photos one at a time.",
        ));
        taken.clear();
    }
    (taken, refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, bool)]) -> Vec<(String, bool)> {
        list.iter().map(|(n, l)| ((*n).to_owned(), *l)).collect()
    }

    #[test]
    fn only_photo_files_on_this_computer_are_taken_from_a_drop() {
        let (taken, refused) = dropped(
            &files(&[
                ("IMG_4890.HEIC", true),
                ("notes.txt", true),
                ("cover.jpg", false),
                ("back.Jpeg", true),
                ("README", true),
            ]),
            false,
        );
        assert_eq!(taken, [0, 3]);
        assert_eq!(refused.len(), 3);
        assert!(refused[0].starts_with("notes.txt "));
        assert!(refused[1].contains("save the photo first"));
    }

    #[test]
    fn a_book_s_back_takes_one_dropped_photo() {
        let (taken, refused) = dropped(&files(&[("back.jpg", true)]), true);
        assert_eq!(taken, [0]);
        assert!(refused.is_empty());
        let (taken, refused) = dropped(&files(&[("a.jpg", true), ("b.png", true)]), true);
        assert!(taken.is_empty());
        assert_eq!(refused.len(), 1);
        // Several for new books are each a book.
        let (taken, _) = dropped(&files(&[("a.jpg", true), ("b.png", true)]), false);
        assert_eq!(taken, [0, 1]);
    }
}
