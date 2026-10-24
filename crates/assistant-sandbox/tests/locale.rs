//! Pointing gettext at a folder of translations, in a test process of
//! its own, since the setting is process-wide.

use std::ffi::{CStr, CString, c_char};
use std::fs;
use std::path::PathBuf;

use assistant_sandbox::locale::{current, use_translations};

unsafe extern "C" {
    fn gettext(msgid: *const c_char) -> *mut c_char;
}

/// A compiled message catalogue (.mo) holding `pairs`, which must be
/// sorted by their first string, as GNU gettext's format requires.
fn catalogue(pairs: &[(&str, &str)]) -> Vec<u8> {
    let n = pairs.len() as u32;
    let header = 28;
    let originals_at = header;
    let translations_at = originals_at + 8 * n;
    let mut strings_at = translations_at + 8 * n;
    let mut tables = Vec::new();
    let mut strings = Vec::new();
    for column in 0..2 {
        for pair in pairs {
            let text = if column == 0 { pair.0 } else { pair.1 };
            tables.extend((text.len() as u32).to_le_bytes());
            tables.extend(strings_at.to_le_bytes());
            strings.extend(text.as_bytes());
            strings.push(0);
            strings_at += text.len() as u32 + 1;
        }
    }
    let mut mo = Vec::new();
    for word in [
        0x9504_12de_u32,
        0,
        n,
        originals_at,
        translations_at,
        0,
        strings_at,
    ] {
        mo.extend(word.to_le_bytes());
    }
    mo.extend(tables);
    mo.extend(strings);
    mo
}

#[test]
fn messages_come_from_the_app_folder() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("locale");
    let _ = fs::remove_dir_all(&dir);
    let messages = dir.join("en_US/LC_MESSAGES");
    fs::create_dir_all(&messages).unwrap();
    fs::write(
        messages.join("assistant-test.mo"),
        catalogue(&[
            ("", "Content-Type: text/plain; charset=UTF-8\n"),
            ("Export a Copy", "Export a Copy Now \u{2713}"),
        ]),
    )
    .unwrap();

    use_translations("assistant-test", &dir).unwrap();
    let (folder, default) = current("assistant-test").unwrap();
    assert_eq!(PathBuf::from(folder), dir);
    assert_eq!(default, "assistant-test");

    // SAFETY: the locale name is a valid C string; setlocale copies it.
    let set = unsafe { libc::setlocale(libc::LC_ALL, c"en_US.UTF-8".as_ptr()) };
    if set.is_null() {
        eprintln!("skipped: the en_US.UTF-8 locale is not installed");
        return;
    }
    // SAFETY: tests in this file run one at a time on a process of
    // their own, and nothing else reads the environment meanwhile.
    unsafe { std::env::remove_var("LANGUAGE") };
    let lookup = |text: &str| {
        let text = CString::new(text).unwrap();
        // SAFETY: gettext reads the valid C string and returns either it
        // or a string in the loaded catalogue, which stays loaded.
        unsafe { CStr::from_ptr(gettext(text.as_ptr())) }
            .to_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(lookup("Export a Copy"), "Export a Copy Now \u{2713}");
    assert_eq!(lookup("Not translated"), "Not translated");
}
