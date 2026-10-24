//! Points gettext at the app's own translations. GLib looks up each
//! message, but the domain and the folder holding its catalogues are
//! set through the C library, which no other crate here exposes.

use std::ffi::{CStr, CString, c_char};
use std::path::Path;

use crate::Error;

// All three are glibc's: they copy the strings passed and return either
// a pointer the C library owns or NULL on failure.
unsafe extern "C" {
    fn bindtextdomain(domainname: *const c_char, dirname: *const c_char) -> *mut c_char;
    fn bind_textdomain_codeset(domainname: *const c_char, codeset: *const c_char) -> *mut c_char;
    fn textdomain(domainname: *const c_char) -> *mut c_char;
}

fn c_string(what: &str, text: &[u8]) -> Result<CString, Error> {
    CString::new(text).map_err(|_| Error(format!("the {what} has a NUL byte")))
}

/// Makes `domain` the app's translation domain, found under `dir` as
/// `dir/<language>/LC_MESSAGES/<domain>.mo`, with messages always
/// returned as UTF-8. Interface files with no domain of their own use it
/// too.
pub fn use_translations(domain: &str, dir: &Path) -> Result<(), Error> {
    let domain = c_string("translation domain", domain.as_bytes())?;
    let dir = c_string("translations folder", dir.as_os_str().as_encoded_bytes())?;
    let utf8 = c"UTF-8";
    // SAFETY: each argument is a valid NUL-terminated string that lives
    // until the call returns, and the C library copies what it keeps.
    // The returned pointers are only compared with NULL.
    let failed = unsafe {
        bindtextdomain(domain.as_ptr(), dir.as_ptr()).is_null()
            || bind_textdomain_codeset(domain.as_ptr(), utf8.as_ptr()).is_null()
            || textdomain(domain.as_ptr()).is_null()
    };
    if failed {
        return Err(Error(format!(
            "the translations could not be set up: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

/// The folder translations for `domain` are looked up in, and the
/// domain used when none is named, as the C library has them now.
pub fn current(domain: &str) -> Option<(String, String)> {
    let domain = CString::new(domain).ok()?;
    // SAFETY: passing NULL asks for the current values without changing
    // them; the returned strings are owned by the C library and are
    // copied before anything else can change them on this thread.
    unsafe {
        let dir = bindtextdomain(domain.as_ptr(), std::ptr::null());
        let default = textdomain(std::ptr::null());
        if dir.is_null() || default.is_null() {
            return None;
        }
        Some((
            CStr::from_ptr(dir).to_string_lossy().into_owned(),
            CStr::from_ptr(default).to_string_lossy().into_owned(),
        ))
    }
}
