//! The app's own files, written under the limits the app applies to
//! itself, so nothing it saves needs to write outside its own folders,
//! such as a temporary file in /tmp. A test process of its own, since
//! the limits cannot be lifted.

use std::fs;
use std::path::PathBuf;

use assistant_sandbox::{Enforced, limit_access};

/// Run as a program of its own, without the test runner's threads, since
/// the limits refuse to apply while other threads are running.
fn main() {
    the_key_file_and_results_save_within_the_app_folders();
    println!("store under limits: ok");
}

fn the_key_file_and_results_save_within_the_app_folders() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("store-under-limits");
    let _ = fs::remove_dir_all(&base);
    let data = base.join("data");
    fs::create_dir_all(&data).unwrap();
    if limit_access(&[&data]).unwrap() == Enforced::Not {
        eprintln!("skipped: Landlock is not available on this kernel");
        return;
    }

    // Made-up bytes: saving does not look inside the key file.
    let keys = data.join("keys");
    assistant_keys::save(&keys, b"not a real key file").unwrap();
    assistant_keys::save(&keys, b"replaced").unwrap();
    assert_eq!(assistant_keys::load(&keys).unwrap().unwrap(), b"replaced");

    // A made-up book, written through a temporary file and renamed.
    let results = data.join("results.json");
    let book = r#"{"version":1,"books":[{"included":false,"line":1,
        "series":"Astonishing Robot Tales","start_year":1954,"volume":null,
        "number":"1","identity":"not_found","candidates":[],"sources":[],
        "values":[],"credits":[],"characters":[],"teams":[],"story_arcs":[],
        "research":""}]}"#;
    let books = assistant_core::results::from_json(book, 2026).unwrap();
    assert_eq!(books.len(), 1);
    assistant_core::results::save(&results, &books).unwrap();
    assert_eq!(
        assistant_core::results::load(&results, 2026).unwrap(),
        books
    );
    // With none left, the file is removed, which is a change too.
    assistant_core::results::save(&results, &[]).unwrap();
    assert!(!results.exists());

    // Nothing was left behind in the folder.
    let left: Vec<_> = fs::read_dir(&data)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["keys"]);
}
