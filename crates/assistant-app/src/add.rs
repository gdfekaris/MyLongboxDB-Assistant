//! The page where the owner types the books to look up, one a line, and
//! sees what each line was read as before anything is sent.

use adw::prelude::*;
use assistant_core::lines::{BookQuery, LineProblem, parse_lines};
use assistant_core::queue::MAX_BATCH;

use crate::i18n::{gettext, gettext_f};

pub struct AddPage {
    pub root: adw::ToolbarView,
    text: gtk::TextView,
    list: gtk::ListBox,
    summary: gtk::Label,
    pub batch: adw::SpinRow,
    pub look_up: gtk::Button,
}

impl AddPage {
    pub fn new(header: &adw::HeaderBar, batch: usize, this_year: u16) -> std::rc::Rc<Self> {
        let intro = gtk::Label::builder()
            .label(gettext(
                "Type one book a line, such as Amazing Spider-Man (1963) 300, ASM #300, \
                 X-Men 1991 #1, or a range such as Thor (1966) 337-340.",
            ))
            .wrap(true)
            .xalign(0.0)
            .build();
        let text = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .accepts_tab(false)
            .build();
        let frame = gtk::Frame::builder()
            .child(
                &gtk::ScrolledWindow::builder()
                    .min_content_height(140)
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .child(&text)
                    .build(),
            )
            .build();
        let summary = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["heading"])
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .visible(false)
            .build();
        let batch_row = adw::SpinRow::with_range(1.0, MAX_BATCH as f64, 1.0);
        batch_row.set_title(&gettext("Books per run"));
        batch_row.set_subtitle(&gettext("The most books one run may hold"));
        batch_row.set_value(batch as f64);
        let settings = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        settings.append(&batch_row);
        let look_up = gtk::Button::builder()
            .label(gettext("_Look Up…"))
            .use_underline(true)
            .halign(gtk::Align::Center)
            .sensitive(false)
            .css_classes(["pill", "suggested-action"])
            .build();
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .build();
        for widget in [
            intro.upcast_ref::<gtk::Widget>(),
            frame.upcast_ref(),
            summary.upcast_ref(),
            list.upcast_ref(),
            settings.upcast_ref(),
            look_up.upcast_ref(),
        ] {
            column.append(widget);
        }
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
        let page = std::rc::Rc::new(Self {
            root,
            text,
            list,
            summary,
            batch: batch_row,
            look_up,
        });
        {
            let weak = std::rc::Rc::downgrade(&page);
            page.text.buffer().connect_changed(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.refresh(this_year);
                }
            });
        }
        {
            let weak = std::rc::Rc::downgrade(&page);
            page.batch.connect_value_notify(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.refresh(this_year);
                }
            });
        }
        page.refresh(this_year);
        page
    }

    fn typed(&self) -> String {
        let buffer = self.text.buffer();
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string()
    }

    /// The books typed, and the lines that could not be read.
    pub fn parsed(&self, this_year: u16) -> (Vec<BookQuery>, Vec<LineProblem>) {
        parse_lines(&self.typed(), this_year)
    }

    pub fn batch(&self) -> usize {
        usize::try_from(self.batch.value() as i64).unwrap_or(1)
    }

    /// Shows what the lines were read as, and whether they can be looked
    /// up as they are.
    fn refresh(&self, this_year: u16) {
        let (books, problems) = self.parsed(this_year);
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        for problem in &problems {
            let row = adw::ActionRow::builder()
                .title(gettext_f(
                    "Line {line}: {text}",
                    &[("line", &problem.line.to_string()), ("text", &problem.text)],
                ))
                .subtitle(&problem.problem)
                .title_lines(1)
                .css_classes(["error"])
                .build();
            row.set_use_markup(false);
            self.list.append(&row);
        }
        for book in &books {
            let row = adw::ActionRow::builder()
                .title(book.to_string())
                .title_lines(1)
                .build();
            row.set_use_markup(false);
            self.list.append(&row);
        }
        self.list
            .set_visible(!books.is_empty() || !problems.is_empty());
        let batch = self.batch();
        let (text, ready) = if !problems.is_empty() {
            (
                gettext("Fix or remove the lines that could not be read."),
                false,
            )
        } else if books.len() > batch {
            (
                gettext_f(
                    "{count} books is more than a run may hold ({batch}). Split them up, or \
                     raise Books per run.",
                    &[
                        ("count", &books.len().to_string()),
                        ("batch", &batch.to_string()),
                    ],
                ),
                false,
            )
        } else if books.is_empty() {
            (String::new(), false)
        } else if books.len() == 1 {
            (gettext("1 book to look up"), true)
        } else {
            (
                gettext_f(
                    "{count} books to look up",
                    &[("count", &books.len().to_string())],
                ),
                true,
            )
        };
        self.summary.set_label(&text);
        self.summary.set_visible(!text.is_empty());
        self.look_up.set_sensitive(ready);
    }

    pub fn clear(&self) {
        self.text.buffer().set_text("");
    }
}
