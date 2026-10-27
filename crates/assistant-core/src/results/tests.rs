use serde_json::{Value as Json, json};

use super::*;
use crate::lines::parse_lines;

const YEAR: u16 = 2026;

fn draft() -> Draft {
    let query = parse_lines("Astonishing Robot Tales (1954) #1", YEAR)
        .0
        .remove(0);
    Draft {
        query,
        identity: Identity::Identified,
        sources: vec![Source {
            url: "https://example.org/robot-tales-1".into(),
            title: "Robot Tales #1".into(),
            cited_text: "Cover date: March 1954".into(),
        }],
        values: vec![
            Value {
                key: "series",
                text: "Astonishing Robot Tales".into(),
                source: Some(0),
                confidence: Confidence::High,
                note: String::new(),
                accepted: true,
            },
            Value {
                key: "key_issue",
                text: "yes".into(),
                source: Some(0),
                confidence: Confidence::High,
                note: String::new(),
                accepted: false,
            },
        ],
        credits: vec![Entry {
            name: "Ada Sprocket".into(),
            detail: Detail::Role(Role::Writer),
            source: Some(0),
            confidence: Confidence::Medium,
            note: String::new(),
            accepted: true,
        }],
        characters: vec![Entry {
            name: "Tinman".into(),
            detail: Detail::FirstAppearance(true),
            source: Some(0),
            confidence: Confidence::High,
            note: String::new(),
            accepted: true,
        }],
        teams: Vec::new(),
        story_arcs: vec![Entry {
            name: "Rise of the Robots".into(),
            detail: Detail::Part(Some(2)),
            source: Some(0),
            confidence: Confidence::Low,
            note: "only a forum says so".into(),
            accepted: false,
        }],
        research: "Notes.".into(),
    }
}

fn ambiguous() -> Draft {
    Draft {
        identity: Identity::Ambiguous(vec![Candidate {
            series: "X-Men".into(),
            start_year: Some(1991),
            volume: Some(2),
            publisher: "Marvel".into(),
            why: "the relaunch".into(),
        }]),
        values: Vec::new(),
        ..draft()
    }
}

fn books() -> Vec<ReviewBook> {
    vec![ReviewBook::new(draft()), ReviewBook::new(ambiguous())]
}

#[test]
fn identified_books_start_included_and_others_never_are() {
    let books = books();
    assert!(books[0].included);
    assert!(!books[1].included);
}

#[test]
fn results_read_back_as_written() {
    let text = to_json(&books());
    assert!(!text.contains("sk-"));
    assert_eq!(from_json(&text, YEAR).unwrap(), books());
}

fn altered(change: impl Fn(&mut Json)) -> Vec<ReviewBook> {
    let mut file: Json = serde_json::from_str(&to_json(&books())).unwrap();
    change(&mut file);
    from_json(&file.to_string(), YEAR).unwrap()
}

#[test]
fn an_altered_file_cannot_slip_in_what_a_lookup_could_not() {
    // A value that its field refuses is cleared and not accepted.
    let read = altered(|f| {
        f["books"][0]["values"][0]["key"] = json!("isbn");
        f["books"][0]["values"][0]["text"] = json!("9780000000000");
    });
    let isbn = read[0].draft.value("isbn").unwrap();
    assert_eq!(isbn.text, "");
    assert!(!isbn.accepted);
    // Unknown fields and repeated ones are dropped; a source that is not
    // there becomes none.
    let read = altered(|f| {
        f["books"][0]["values"][1]["key"] = json!("series");
        f["books"][0]["credits"][0]["source"] = json!(7);
        f["books"][0]["values"]
            .as_array_mut()
            .unwrap()
            .push(json!({"key": "url_to_open", "text": "https://evil.example", "source": null, "confidence": "high", "note": "", "accepted": true}));
    });
    assert_eq!(read[0].draft.values.len(), 1);
    assert_eq!(read[0].draft.credits[0].source, None);
    // An unsettled book cannot be made included.
    let read = altered(|f| f["books"][1]["included"] = json!(true));
    assert!(!read[1].included);
    // Hidden direction marks are taken out of names.
    let read = altered(|f| f["books"][0]["credits"][0]["name"] = json!("Ada\u{202e}Sprocket"));
    assert_eq!(read[0].draft.credits[0].name, "AdaSprocket");
}

#[test]
fn a_saved_entry_holding_several_names_reads_back_as_one_each() {
    let read = altered(|f| {
        f["books"][0]["characters"][0]["name"] = json!("Tinman; Captain Cog; Tinman");
    });
    let characters = &read[0].draft.characters;
    let names: Vec<&str> = characters.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Tinman", "Captain Cog"]);
    assert!(characters.iter().all(|e| e.accepted && e.source == Some(0)));
    assert_eq!(characters[1].detail, Detail::FirstAppearance(true));
}

#[test]
fn unreadable_books_are_left_out_and_unreadable_files_refused() {
    let read = altered(|f| f["books"][0]["number"] = json!("   "));
    assert_eq!(read.len(), 1);
    let read = altered(|f| f["books"][0]["identity"] = json!("certain"));
    assert_eq!(read.len(), 1);
    let mut file: Json = serde_json::from_str(&to_json(&books())).unwrap();
    file["version"] = json!(2);
    assert!(from_json(&file.to_string(), YEAR).is_err());
    file["version"] = json!(1);
    file["extra"] = json!("x");
    assert!(from_json(&file.to_string(), YEAR).is_err());
    assert!(from_json("not json", YEAR).is_err());
}

fn folder(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("assistant-results-test-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_file_is_private_and_goes_once_nothing_is_left() {
    use std::os::unix::fs::PermissionsExt;

    let dir = folder("save");
    let path = dir.join("results.json");
    assert_eq!(load(&path, YEAR).unwrap(), Vec::new());
    save(&path, &books()).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(load(&path, YEAR).unwrap(), books());
    save(&path, &[]).unwrap();
    assert!(!path.exists());
    save(&path, &[]).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

fn jpeg(b: u8) -> Jpeg {
    Jpeg::new(vec![0xFF, 0xD8, 0xFF, b]).unwrap()
}

#[test]
fn photos_are_kept_beside_the_results_and_go_with_their_book() {
    use std::os::unix::fs::PermissionsExt;

    let dir = folder("photos");
    let path = dir.join("results.json");
    let photos = dir.join("results.photos");
    let mut kept = books();
    kept.push(ReviewBook::with_photos(
        draft(),
        Photo::new(jpeg(1)),
        Some(Photo::new(jpeg(2))),
    ));
    save(&path, &kept).unwrap();
    let files = || {
        let mut names: Vec<_> = std::fs::read_dir(&photos)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        names.sort();
        names
    };
    assert_eq!(files().len(), 2);
    for file in files() {
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let read = load(&path, YEAR).unwrap();
    assert_eq!(read, kept);
    assert_eq!(read.last().unwrap().back.as_ref().unwrap().jpeg, jpeg(2));

    // A book that goes takes its photos with it.
    kept.pop();
    save(&path, &kept).unwrap();
    assert!(!photos.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_owner_s_photos_are_the_exported_book_s_covers() {
    let typed = ReviewBook::new(draft());
    assert_eq!(typed.book(YEAR).unwrap().front, None);
    let photographed =
        ReviewBook::with_photos(draft(), Photo::new(jpeg(1)), Some(Photo::new(jpeg(2))));
    let book = photographed.book(YEAR).unwrap();
    assert_eq!(book.front, Some(jpeg(1)));
    assert_eq!(book.back, Some(jpeg(2)));
    let mut unsettled = photographed.clone();
    unsettled.draft.identity = Identity::NotFound;
    assert_eq!(unsettled.book(YEAR), None);
}

#[test]
fn a_photo_name_cannot_reach_outside_its_folder() {
    let dir = folder("photo-names");
    let path = dir.join("results.json");
    let book = ReviewBook::with_photos(draft(), Photo::new(jpeg(1)), None);
    save(&path, &[book]).unwrap();
    std::fs::write(dir.join("secret.jpg"), [0xFF, 0xD8, 0xFF, 9]).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let mut file: Json = serde_json::from_str(&text).unwrap();
    for name in ["../secret", "SECRET", "0123"] {
        file["books"][0]["front"] = json!(name);
        std::fs::write(&path, file.to_string()).unwrap();
        let read = load(&path, YEAR).unwrap();
        // The book is kept; the photo it names is not read.
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].front, None, "{name}");
    }
    // A missing photo, or one that is not a JPEG, is left out too.
    let book = ReviewBook::with_photos(draft(), Photo::new(jpeg(1)), None);
    save(&path, std::slice::from_ref(&book)).unwrap();
    let photos = dir.join("results.photos");
    let only = std::fs::read_dir(&photos)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&only, b"GIF89a").unwrap();
    assert_eq!(load(&path, YEAR).unwrap()[0].front, None);
    std::fs::remove_file(&only).unwrap();
    assert_eq!(load(&path, YEAR).unwrap()[0].front, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn results_from_before_photos_were_kept_still_read() {
    let mut file: Json = serde_json::from_str(&to_json(&books())).unwrap();
    for b in file["books"].as_array_mut().unwrap() {
        let b = b.as_object_mut().unwrap();
        b.remove("front");
        b.remove("back");
    }
    assert_eq!(from_json(&file.to_string(), YEAR).unwrap(), books());
}

#[test]
fn a_source_s_lines_are_kept_and_each_cleaned() {
    let mut books = books();
    books[0].draft.sources[0].cited_text = "Series: Robot Tales\nIssue: #1".into();
    let mut file: Json = serde_json::from_str(&to_json(&books)).unwrap();
    file["books"][0]["sources"][0]["cited_text"] =
        json!("Series: Robot\u{202e} Tales\n\n  Issue:   #1  ");
    let read = from_json(&file.to_string(), YEAR).unwrap();
    assert_eq!(
        read[0].draft.sources[0].cited_text,
        "Series: Robot Tales\nIssue: #1"
    );
    assert_eq!(from_json(&to_json(&books), YEAR).unwrap(), books);
}

#[test]
fn a_book_marked_reviewed_stays_marked() {
    let mut books = books();
    assert!(!books[0].reviewed);
    books[0].reviewed = true;
    let read = from_json(&to_json(&books), YEAR).unwrap();
    assert!(read[0].reviewed);
    assert!(!read[1].reviewed);
}

#[test]
fn results_from_before_books_were_marked_reviewed_still_read() {
    let mut file: Json = serde_json::from_str(&to_json(&books())).unwrap();
    for b in file["books"].as_array_mut().unwrap() {
        b.as_object_mut().unwrap().remove("reviewed");
    }
    assert_eq!(from_json(&file.to_string(), YEAR).unwrap(), books());
}

#[test]
fn a_copy_s_grade_is_kept_and_read_back_strictly() {
    let mut books = books();
    books[0].copy.grade = crate::Grade::parse("9.8");
    books[0].copy.grade_type = Some(crate::GradeType::Cgc);
    books[0].copy.cert_number = "4012345001".into();
    assert_eq!(from_json(&to_json(&books), YEAR).unwrap(), books);
    let mut file: Json = serde_json::from_str(&to_json(&books)).unwrap();
    file["books"][0]["grade"] = json!("9.85");
    file["books"][0]["grade_type"] = json!("ebay");
    file["books"][0]["cert_number"] = json!(format!("40\u{202E}12{}", "9".repeat(100)));
    let read = from_json(&file.to_string(), YEAR).unwrap();
    assert_eq!(read[0].copy.grade, None);
    assert_eq!(read[0].copy.grade_type, None);
    assert!(!read[0].copy.cert_number.contains('\u{202E}'));
    assert_eq!(read[0].copy.cert_number.chars().count(), MAX_CERT);
}

#[test]
fn only_a_grading_company_s_certificate_is_exported() {
    let mut book = books().remove(0);
    book.copy.grade = crate::Grade::parse("9");
    book.copy.cert_number = "4012345001".into();
    book.copy.grade_type = Some(crate::GradeType::Raw);
    let exported = book.book(YEAR).unwrap().copy;
    assert_eq!(exported.grade, crate::Grade::parse("9.0"));
    assert_eq!(exported.cert_number, "");
    book.copy.grade_type = Some(crate::GradeType::Pgx);
    assert_eq!(book.book(YEAR).unwrap().copy.cert_number, "4012345001");
}

#[test]
fn the_owner_s_own_values_are_checked_and_accepted() {
    let mut d = draft();
    d.set_value("cover_price", "$0.10", YEAR).unwrap();
    let price = d.value("cover_price").unwrap();
    assert_eq!(price.text, "0.10");
    assert!(price.accepted);
    assert_eq!(price.source, None);
    assert!(d.set_value("cover_price", "ten cents", YEAR).is_err());
    assert!(d.set_value("no_such_field", "x", YEAR).is_err());
    d.set_value("series", "", YEAR).unwrap();
    assert!(!d.value("series").unwrap().accepted);
    // The key issue and the doubtful arc are still to look at.
    assert_eq!(d.unaccepted(), 2);
}
