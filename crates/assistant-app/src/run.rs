//! The page that shows a run as it goes: each book's state, the cost so
//! far, and a way to cancel.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adw::prelude::*;
use assistant_core::dollars;
use assistant_core::lines::BookQuery;
use assistant_core::lookup::{Draft, Identity, LookupError, Spent};
use assistant_core::queue::{Event, Summary};

use crate::i18n::{gettext, gettext_f};

/// Where one book stands.
pub enum Status {
    Waiting,
    LookingUp,
    Done(Box<Draft>),
    Failed(LookupError),
    Skipped,
}

/// A run under way or finished, and what it found.
pub struct Run {
    pub queries: Vec<BookQuery>,
    pub status: Vec<Status>,
    pub spent: Spent,
    pub cancel: Arc<AtomicBool>,
    pub finished: bool,
    /// The kept result this run's one book replaces, when it looks up a
    /// candidate the owner picked.
    pub replaces: Option<usize>,
}

impl Run {
    pub fn new(queries: Vec<BookQuery>) -> Self {
        let status = queries.iter().map(|_| Status::Waiting).collect();
        Self {
            queries,
            status,
            spent: Spent::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            finished: false,
            replaces: None,
        }
    }

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Started(i) => self.set(i, Status::LookingUp),
            Event::Skipped(i) => self.set(i, Status::Skipped),
            Event::Finished {
                index,
                result,
                spent,
            } => {
                self.spent.add(spent);
                self.set(
                    index,
                    match result {
                        Ok(draft) => Status::Done(draft),
                        Err(e) => Status::Failed(e),
                    },
                );
            }
        }
    }

    fn set(&mut self, index: usize, status: Status) {
        if let Some(slot) = self.status.get_mut(index) {
            *slot = status;
        }
    }

    pub fn finish(&mut self, summary: &Summary) {
        self.finished = true;
        // The summary counts every request, including any whose book's
        // event was lost; it is the total.
        self.spent = summary.spent;
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The books that failed or were not looked up, to try again.
    pub fn unfinished(&self) -> Vec<BookQuery> {
        self.queries
            .iter()
            .zip(&self.status)
            .filter(|(_, s)| matches!(s, Status::Failed(_) | Status::Skipped))
            .map(|(q, _)| q.clone())
            .collect()
    }
}

/// A book's state in a few words.
fn describe(status: &Status) -> String {
    match status {
        Status::Waiting => gettext("Waiting"),
        Status::LookingUp => gettext("Looking up…"),
        Status::Skipped => gettext("Not looked up"),
        Status::Failed(e) => {
            // Messages from the lookup start in lowercase, to read inside
            // a sentence; here they stand alone.
            let text = e.to_string();
            let mut chars = text.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
        Status::Done(draft) => match &draft.identity {
            Identity::Identified => {
                let filled = draft.values.iter().filter(|v| !v.text.is_empty()).count();
                let review = draft
                    .values
                    .iter()
                    .filter(|v| !v.text.is_empty() && !v.accepted)
                    .count();
                gettext_f(
                    "Found: {filled} details, {review} to look at",
                    &[
                        ("filled", &filled.to_string()),
                        ("review", &review.to_string()),
                    ],
                )
            }
            Identity::Ambiguous(candidates) => gettext_f(
                "Which series? {count} could be meant",
                &[("count", &candidates.len().to_string())],
            ),
            Identity::NotFound => gettext("No matching issue found"),
            Identity::NoSources => gettext("No reliable sources found"),
        },
    }
}

pub struct RunPage {
    pub root: adw::ToolbarView,
    list: gtk::ListBox,
    progress: gtk::Label,
    pub cancel: gtk::Button,
    pub again: gtk::Button,
    pub retry: gtk::Button,
    pub review: gtk::Button,
}

impl RunPage {
    pub fn new(header: &adw::HeaderBar) -> Self {
        let progress = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["heading"])
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let cancel = gtk::Button::builder()
            .label(gettext("_Cancel"))
            .use_underline(true)
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .build();
        let again = gtk::Button::builder()
            .label(gettext("_Look Up More Books"))
            .use_underline(true)
            .halign(gtk::Align::Center)
            .visible(false)
            .css_classes(["pill"])
            .build();
        let retry = gtk::Button::builder()
            .label(gettext("_Try Unfinished Books Again"))
            .use_underline(true)
            .halign(gtk::Align::Center)
            .visible(false)
            .css_classes(["pill"])
            .build();
        let review = gtk::Button::builder()
            .label(gettext("_Review Results"))
            .use_underline(true)
            .halign(gtk::Align::Center)
            .visible(false)
            .css_classes(["pill", "suggested-action"])
            .build();
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .build();
        column.append(&progress);
        column.append(&list);
        column.append(&cancel);
        column.append(&review);
        column.append(&retry);
        column.append(&again);
        let root = adw::ToolbarView::new();
        root.add_top_bar(header);
        root.set_content(Some(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(
                    &adw::Clamp::builder()
                        .maximum_size(640)
                        .margin_top(24)
                        .margin_bottom(24)
                        .margin_start(12)
                        .margin_end(12)
                        .child(&column)
                        .build(),
                )
                .build(),
        ));
        Self {
            root,
            list,
            progress,
            cancel,
            again,
            retry,
            review,
        }
    }

    /// Shows the run as it now stands.
    pub fn show(&self, run: &Run) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        for (query, status) in run.queries.iter().zip(&run.status) {
            let row = adw::ActionRow::builder()
                .title(query.to_string())
                .subtitle(describe(status))
                .title_lines(1)
                .build();
            row.set_use_markup(false);
            if matches!(status, Status::LookingUp) {
                row.add_suffix(&adw::Spinner::new());
            }
            self.list.append(&row);
        }
        let done = run
            .status
            .iter()
            .filter(|s| !matches!(s, Status::Waiting | Status::LookingUp))
            .count();
        let mut cost = dollars(run.spent.cost);
        if !run.spent.cost_known {
            cost = gettext_f("{cost}, and some of it unknown", &[("cost", &cost)]);
        }
        let count = |f: fn(&Status) -> bool| run.status.iter().filter(|s| f(s)).count().to_string();
        self.progress.set_label(&if run.finished {
            gettext_f(
                "Done: {found} looked up, {failed} failed, {skipped} not looked up. Spent {cost}.",
                &[
                    ("found", &count(|s| matches!(s, Status::Done(_)))),
                    ("failed", &count(|s| matches!(s, Status::Failed(_)))),
                    ("skipped", &count(|s| matches!(s, Status::Skipped))),
                    ("cost", &cost),
                ],
            )
        } else {
            gettext_f(
                "{done} of {total} books done. Spent {cost} so far.",
                &[
                    ("done", &done.to_string()),
                    ("total", &run.queries.len().to_string()),
                    ("cost", &cost),
                ],
            )
        });
        self.cancel.set_visible(!run.finished);
        self.cancel
            .set_sensitive(!run.cancel.load(Ordering::Relaxed));
        self.again.set_visible(run.finished);
        let found = run.status.iter().any(|s| matches!(s, Status::Done(_)));
        self.review.set_visible(run.finished && found);
        let others = run
            .status
            .iter()
            .any(|s| matches!(s, Status::Failed(_) | Status::Skipped));
        self.retry.set_visible(run.finished && others);
    }
}
