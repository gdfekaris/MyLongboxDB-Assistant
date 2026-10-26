//! The owner's copy of the Grand Comics Database: the dialog that adds,
//! replaces, or removes it, and the import that reads the dump the owner
//! picks, on a thread that can reach nothing but that file and the
//! folder the index is kept in.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use adw::prelude::*;
use assistant_gcd::{About, Catalog, CatalogError, ImportError, Imported, Step};
use gtk::{gio, glib};

use crate::i18n::{gettext, gettext_f};

/// The folder the index is kept in.
pub fn folder() -> PathBuf {
    glib::user_data_dir().join("gcd")
}

/// The owner's copy, if they added one; the error says why one they
/// added cannot be used.
pub fn open() -> Result<Option<Catalog>, String> {
    match Catalog::open(&folder()) {
        Ok(catalog) => Ok(Some(catalog)),
        Err(CatalogError::Missing) => Ok(None),
        Err(CatalogError::Outdated) => Err(gettext(
            "Your copy of the Grand Comics Database must be added again for this version of the app.",
        )),
        Err(CatalogError::Unreadable(why)) => Err(gettext_f(
            "Your copy of the Grand Comics Database could not be read: {why}",
            &[("why", &why)],
        )),
    }
}

/// Deletes the index, and anything an import left beside it.
pub fn remove() -> std::io::Result<()> {
    match std::fs::remove_dir_all(folder()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// What the copy holds, in a sentence.
pub fn described(about: &About) -> String {
    let date = about
        .dated
        .map_or_else(|| gettext("undated"), |d| d.to_string());
    gettext_f(
        "Your copy is dated {date} and holds {series} series and {issues} issues.",
        &[
            ("date", &date),
            ("series", &grouped(about.series)),
            ("issues", &grouped(about.issues)),
        ],
    )
}

/// A count with its thousands grouped, as 2,622,121.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What an import is doing, for the progress dialog.
fn doing(step: Step, count: u64) -> String {
    let count = grouped(count);
    let count = count.as_str();
    match step {
        Step::Unpacking => gettext_f("Unpacking the zip: {count} MB", &[("count", count)]),
        Step::Series => gettext_f("Reading series: {count}", &[("count", count)]),
        Step::Issues => gettext_f("Reading issues: {count}", &[("count", count)]),
        Step::Stories => gettext_f("Reading stories: {count}", &[("count", count)]),
        Step::Credits => gettext_f("Reading credits: {count}", &[("count", count)]),
        Step::IssueCredits => gettext_f("Reading editors: {count}", &[("count", count)]),
        Step::Characters => gettext_f("Reading characters: {count}", &[("count", count)]),
        Step::Indexing => gettext("Making the index"),
    }
}

/// How far along an import is, from 0 to 1, by its steps.
fn fraction(step: Step) -> f64 {
    let steps = [
        Step::Unpacking,
        Step::Series,
        Step::Issues,
        Step::Stories,
        Step::Credits,
        Step::IssueCredits,
        Step::Characters,
        Step::Indexing,
    ];
    let at = steps.iter().position(|s| *s == step).unwrap_or(0);
    at as f64 / steps.len() as f64
}

/// Why an import failed, for the owner.
pub fn failed(error: &ImportError) -> String {
    match error {
        ImportError::Unreadable(why) => gettext_f(
            "The file could not be read as the Grand Comics Database's data dump: {why}",
            &[("why", why)],
        ),
        ImportError::TooLarge => gettext("The file is far larger than the GCD's data dump."),
        ImportError::NotGcd(why) => gettext_f(
            "The file is not the Grand Comics Database's SQLite data dump: {why}",
            &[("why", why)],
        ),
        ImportError::Cancelled => gettext("Cancelled. Your copy is as it was."),
        ImportError::CannotWrite(why) => gettext_f(
            "The index could not be saved: {why}. Check there is room for it.",
            &[("why", why)],
        ),
    }
}

/// The dialog shown while an import runs.
pub struct Progress {
    dialog: adw::AlertDialog,
    bar: gtk::ProgressBar,
    started: Instant,
    shown: Option<Instant>,
}

impl Progress {
    /// Shows the progress dialog; Cancel sets `cancel`.
    pub fn new(window: &impl IsA<gtk::Widget>, cancel: Arc<AtomicBool>) -> Self {
        let bar = gtk::ProgressBar::builder()
            .show_text(true)
            .text(gettext("Starting"))
            .build();
        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Adding the Grand Comics Database"))
            .body(gettext(
                "This takes a minute or two. Lookups can go on as usual once it is done.",
            ))
            .extra_child(&bar)
            .close_response("cancel")
            .build();
        dialog.add_response("cancel", &gettext("_Cancel"));
        dialog.set_response_appearance("cancel", adw::ResponseAppearance::Destructive);
        dialog.connect_response(None, move |dialog, _| {
            cancel.store(true, Ordering::Relaxed);
            dialog.set_response_enabled("cancel", false);
        });
        // Only Cancel or the end of the import closes it.
        dialog.set_can_close(false);
        dialog.present(Some(window));
        Self {
            dialog,
            bar,
            started: Instant::now(),
            shown: None,
        }
    }

    /// Shows a step's progress, a few times a second at most.
    pub fn show(&mut self, step: Step, count: u64) {
        let now = Instant::now();
        if self
            .shown
            .is_some_and(|at| now - at < Duration::from_millis(200))
        {
            return;
        }
        self.shown = Some(now);
        self.bar.set_fraction(fraction(step));
        self.bar.set_text(Some(&doing(step, count)));
    }

    pub fn close(self) -> Duration {
        self.dialog.set_can_close(true);
        self.dialog.close();
        self.started.elapsed()
    }
}

/// Reads the dump at `path` into the index, on a thread of its own that
/// can read only that file and write only in the index's folder, and
/// reports through `progress` and then `done`; or says why it could not
/// start.
pub fn import(
    path: PathBuf,
    cancel: Arc<AtomicBool>,
    progress: impl Fn(Step, u64) + Send + 'static,
    done: impl FnOnce(Result<Imported, ImportError>) + Send + 'static,
) -> std::io::Result<()> {
    let folder = folder();
    std::fs::create_dir_all(&folder)?;
    // Looked up before the thread starts: a translation is read from a
    // file, which the thread may not open once limited.
    let unsafe_text = gettext("the file could not be read safely: {reason}");
    std::thread::Builder::new()
        .name("gcd".into())
        .spawn(move || {
            let urandom = Path::new("/dev/urandom");
            let result = match assistant_sandbox::confine_to_files(&[&path, urandom], &[&folder]) {
                Ok(level) => {
                    if level != assistant_sandbox::Enforced::Fully {
                        eprintln!("mylongboxdb-assistant: GCD thread file limits: {level:?}");
                    }
                    assistant_gcd::import(&path, &folder, &mut |step, n| progress(step, n), &cancel)
                }
                Err(why) => Err(ImportError::Unreadable(
                    unsafe_text.replace("{reason}", &why.to_string()),
                )),
            };
            done(result);
        })
        .map(|_| ())
}

/// Asks for the dump to read.
pub fn pick(window: &impl IsA<gtk::Window>, then: impl FnOnce(gio::File) + 'static) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(&gettext("Grand Comics Database data dump")));
    for suffix in ["zip", "db", "sqlite", "sqlite3"] {
        filter.add_suffix(suffix);
    }
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title(gettext("Choose the GCD Data Dump"))
        .modal(true)
        .filters(&filters)
        .default_filter(&filter)
        .build();
    dialog.open(Some(window), None::<&gio::Cancellable>, move |picked| {
        if let Ok(file) = picked {
            then(file);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_grouped_by_thousands() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(2_622_121), "2,622,121");
    }

    #[test]
    fn progress_runs_through_the_steps() {
        assert_eq!(fraction(Step::Unpacking), 0.0);
        assert!(fraction(Step::Credits) > fraction(Step::Issues));
        assert!(fraction(Step::Indexing) < 1.0);
    }
}
