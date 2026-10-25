//! Reviewing looked-up books before export: a list of the books, and for
//! each, every value with where it came from and how sure the lookup was,
//! for the owner to accept, edit, or leave out. Pages and URLs are shown
//! as text; the app never opens them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use assistant_core::lookup::{COVER_PHOTO, Detail, Draft, Entry, FIELDS, Identity, Source, Value};
use assistant_core::model::{Confidence, Role};
use assistant_core::results::ReviewBook;

use crate::i18n::{gettext, gettext_f};
use crate::photos;

/// The size of a book's thumbnail in the list.
const THUMBNAIL: i32 = 48;
/// The longest edge of a photo on a book's page.
const PHOTO_EDGE: u32 = 480;

/// One of a book's lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum List {
    Credits,
    Characters,
    Teams,
    Arcs,
}

/// What the owner did on the review pages.
#[derive(Debug)]
pub enum ReviewMsg {
    Open(usize),
    Include(usize, bool),
    AcceptValue(usize, &'static str, bool),
    AcceptEntry(usize, List, usize, bool),
    Edit(usize, &'static str),
    Discard(usize),
    /// Look the book up again as the candidate the owner picked.
    Choose(usize, usize),
}

type Emit = Rc<dyn Fn(ReviewMsg)>;

fn role_label(role: Role) -> String {
    gettext(match role {
        Role::Writer => "Writer",
        Role::Artist => "Artist",
        Role::Penciller => "Penciller",
        Role::Inker => "Inker",
        Role::Colorist => "Colorist",
        Role::Letterer => "Letterer",
        Role::CoverArtist => "Cover artist",
        Role::Editor => "Editor",
        Role::Article => "Article",
        Role::Interview => "Interview",
    })
}

fn confidence_label(c: Confidence) -> String {
    gettext(match c {
        Confidence::High => "sure",
        Confidence::Medium => "fairly sure",
        Confidence::Low => "unsure",
    })
}

/// A page's site, from its address, for showing where a value came from.
fn site(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

/// Where a value came from, in a line or two.
fn provenance(
    source: Option<usize>,
    sources: &[Source],
    confidence: Confidence,
    note: &str,
) -> String {
    let mut lines = Vec::new();
    match source.and_then(|i| sources.get(i)) {
        Some(s) if s.url == COVER_PHOTO => {
            lines.push(gettext_f(
                "From your cover photo ({confidence})",
                &[("confidence", &confidence_label(confidence))],
            ));
            if !s.cited_text.is_empty() {
                lines.push(format!("\u{201c}{}\u{201d}", s.cited_text));
            }
        }
        Some(s) => {
            lines.push(gettext_f(
                "From {site} ({confidence})",
                &[
                    ("site", site(&s.url)),
                    ("confidence", &confidence_label(confidence)),
                ],
            ));
            if !s.cited_text.is_empty() {
                lines.push(format!("\u{201c}{}\u{201d}", s.cited_text));
            }
        }
        None => lines.push(gettext("Your own value")),
    }
    if !note.is_empty() {
        lines.push(note.to_owned());
    }
    lines.join("\n")
}

fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

fn plain_row(title: &str, subtitle: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .subtitle_lines(6)
        .build();
    row.set_use_markup(false);
    row
}

/// A row with a tick for accepting what it shows.
fn accept_row(
    title: &str,
    subtitle: &str,
    accepted: bool,
    on_toggle: impl Fn(bool) + 'static,
) -> adw::ActionRow {
    let row = plain_row(title, subtitle);
    let check = gtk::CheckButton::builder()
        .active(accepted)
        .valign(gtk::Align::Center)
        .tooltip_text(gettext("Include in the export"))
        .build();
    let flag = gtk::Label::builder()
        .label(gettext("To look at"))
        .valign(gtk::Align::Center)
        .css_classes(["caption", "warning"])
        .visible(!accepted)
        .build();
    {
        let flag = flag.clone();
        check.connect_toggled(move |c| {
            flag.set_visible(!c.is_active());
            on_toggle(c.is_active());
        });
    }
    row.add_prefix(&check);
    row.add_suffix(&flag);
    row
}

fn value_row(
    index: usize,
    value: &Value,
    label: &str,
    sources: &[Source],
    emit: &Emit,
) -> adw::ActionRow {
    let key = value.key;
    let row = {
        let emit = emit.clone();
        accept_row(
            label,
            &format!(
                "{}\n{}",
                value.text,
                provenance(value.source, sources, value.confidence, &value.note)
            ),
            value.accepted,
            move |on| emit(ReviewMsg::AcceptValue(index, key, on)),
        )
    };
    row.add_suffix(&edit_button(index, key, emit));
    row
}

fn edit_button(index: usize, key: &'static str, emit: &Emit) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name("document-edit-symbolic")
        .valign(gtk::Align::Center)
        .tooltip_text(gettext("Edit"))
        .css_classes(["flat"])
        .build();
    let emit = emit.clone();
    button.connect_clicked(move |_| emit(ReviewMsg::Edit(index, key)));
    button
}

fn entry_rows(
    index: usize,
    list: List,
    entries: &[Entry],
    sources: &[Source],
    emit: &Emit,
) -> Vec<adw::ActionRow> {
    entries
        .iter()
        .enumerate()
        .map(|(n, e)| {
            let detail = match e.detail {
                Detail::Role(role) => role_label(role),
                Detail::FirstAppearance(true) => gettext("First appearance"),
                Detail::FirstAppearance(false) => String::new(),
                Detail::Part(Some(part)) => {
                    gettext_f("Part {part}", &[("part", &part.to_string())])
                }
                Detail::Part(None) => String::new(),
            };
            let source = provenance(e.source, sources, e.confidence, &e.note);
            let subtitle = if detail.is_empty() {
                source
            } else {
                format!("{detail}\n{source}")
            };
            let emit = emit.clone();
            accept_row(&e.name, &subtitle, e.accepted, move |on| {
                emit(ReviewMsg::AcceptEntry(index, list, n, on));
            })
        })
        .collect()
}

/// A book's state, in a few words, for the list.
pub fn status(book: &ReviewBook) -> String {
    match &book.draft.identity {
        Identity::Identified => match book.draft.unaccepted() {
            0 => gettext("Ready"),
            n => gettext_f("{n} to look at", &[("n", &n.to_string())]),
        },
        Identity::Ambiguous(_) => gettext("Which series? Pick one"),
        Identity::NotFound => gettext("No matching issue found"),
        Identity::NoSources => gettext("No reliable sources found"),
    }
}

/// A book's review page content.
fn book_content(index: usize, book: &ReviewBook, emit: &Emit) -> gtk::Box {
    let draft: &Draft = &book.draft;
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .build();
    let top = group("");
    if draft.identity == Identity::Identified {
        let include = adw::SwitchRow::builder()
            .title(gettext("Include in the export"))
            .active(book.included)
            .build();
        let emit = emit.clone();
        include.connect_active_notify(move |r| emit(ReviewMsg::Include(index, r.is_active())));
        top.add(&include);
    }
    let discard = adw::ButtonRow::builder()
        .title(gettext("Discard This Book"))
        .css_classes(["destructive-action"])
        .build();
    {
        let emit = emit.clone();
        discard.connect_activated(move |_| emit(ReviewMsg::Discard(index)));
    }
    top.add(&discard);
    column.append(&top);

    // The owner's own photos, as they will go into the export.
    let shown: Vec<(String, gtk::Picture)> = [
        (gettext("Front cover"), &book.front),
        (gettext("Back cover"), &book.back),
    ]
    .into_iter()
    .filter_map(|(label, photo)| {
        let texture = photos::kept(&photo.as_ref()?.jpeg, PHOTO_EDGE)?;
        let picture = gtk::Picture::builder()
            .paintable(&texture)
            .content_fit(gtk::ContentFit::Contain)
            .height_request(240)
            .hexpand(true)
            .alternative_text(&label)
            .build();
        Some((label, picture))
    })
    .collect();
    if !shown.is_empty() {
        let group = group(&gettext("Your Photos"));
        let pictures = gtk::Box::builder().spacing(12).homogeneous(true).build();
        for (label, picture) in shown {
            let side = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .build();
            side.append(&picture);
            side.append(
                &gtk::Label::builder()
                    .label(&label)
                    .css_classes(["caption", "dim-label"])
                    .build(),
            );
            pictures.append(&side);
        }
        group.add(&pictures);
        column.append(&group);
    }

    match &draft.identity {
        Identity::Identified => {
            let details = group(&gettext("Details"));
            details.set_description(Some(&gettext(
                "Ticked values go into the export; unticked ones are left out.",
            )));
            for field in FIELDS {
                if let Some(value) = draft.value(field.key).filter(|v| !v.text.is_empty()) {
                    details.add(&value_row(
                        index,
                        value,
                        &gettext(field.label),
                        &draft.sources,
                        emit,
                    ));
                }
            }
            column.append(&details);
            for (title, list, entries) in [
                (gettext("Credits"), List::Credits, &draft.credits),
                (gettext("Characters"), List::Characters, &draft.characters),
                (gettext("Teams"), List::Teams, &draft.teams),
                (gettext("Story arcs"), List::Arcs, &draft.story_arcs),
            ] {
                if entries.is_empty() {
                    continue;
                }
                let g = group(&title);
                for row in entry_rows(index, list, entries, &draft.sources, emit) {
                    g.add(&row);
                }
                column.append(&g);
            }
            let blanks = group("");
            let expander = adw::ExpanderRow::builder()
                .title(gettext("Blank fields"))
                .subtitle(gettext("Left blank by the lookup; you can fill them in"))
                .build();
            for field in FIELDS.iter().filter(|f| f.from_text) {
                let value = draft.value(field.key);
                if value.is_some_and(|v| !v.text.is_empty()) {
                    continue;
                }
                let note = value.map_or("", |v| v.note.as_str());
                let row = plain_row(&gettext(field.label), note);
                row.add_suffix(&edit_button(index, field.key, emit));
                expander.add_row(&row);
            }
            blanks.add(&expander);
            column.append(&blanks);
        }
        Identity::Ambiguous(candidates) => {
            let g = group(&gettext("Which Series Did You Mean?"));
            g.set_description(Some(&gettext(
                "Picking one looks the book up again as that series, at the usual cost.",
            )));
            for (n, c) in candidates.iter().enumerate() {
                let mut title = c.series.clone();
                if let Some(year) = c.start_year {
                    title.push_str(&format!(" ({year})"));
                }
                if let Some(volume) = c.volume {
                    title.push_str(&format!(" Vol. {volume}"));
                }
                let subtitle = [c.publisher.as_str(), c.why.as_str()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" — ");
                let row = plain_row(&title, &subtitle);
                let pick = gtk::Button::builder()
                    .label(gettext("Look Up This One"))
                    .valign(gtk::Align::Center)
                    .build();
                let emit = emit.clone();
                pick.connect_clicked(move |_| emit(ReviewMsg::Choose(index, n)));
                row.add_suffix(&pick);
                g.add(&row);
            }
            column.append(&g);
        }
        Identity::NotFound | Identity::NoSources => {
            let g = group(&status(book));
            g.set_description(Some(&gettext(
                "Check the line's series and number, or look it up again later.",
            )));
            column.append(&g);
        }
    }

    if !draft.sources.is_empty() {
        let g = group("");
        let expander = adw::ExpanderRow::builder()
            .title(gettext("Sources"))
            .subtitle(gettext("The pages the lookup cited, shown as text"))
            .build();
        for s in &draft.sources {
            expander.add_row(&plain_row(
                &s.title,
                &format!("{}\n{}", s.url, s.cited_text),
            ));
        }
        g.add(&expander);
        column.append(&g);
    }
    if !draft.research.is_empty() {
        let g = group("");
        let expander = adw::ExpanderRow::builder()
            .title(gettext("Research notes"))
            .build();
        let label = gtk::Label::builder()
            .label(&draft.research)
            .wrap(true)
            .xalign(0.0)
            .selectable(true)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        expander.add_row(&label);
        g.add(&expander);
        column.append(&g);
    }
    column
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(
            &adw::Clamp::builder()
                .maximum_size(720)
                .margin_top(24)
                .margin_bottom(24)
                .margin_start(12)
                .margin_end(12)
                .child(child)
                .build(),
        )
        .build()
}

pub struct ReviewPage {
    pub root: adw::NavigationView,
    list: gtk::ListBox,
    summary: gtk::Label,
    pub export: gtk::Button,
    pub add_books: gtk::Button,
    emit: Emit,
    /// The book whose page is open, if one is.
    open: Rc<Cell<Option<usize>>>,
    /// Thumbnails already made, by the name their photo is kept by.
    thumbnails: RefCell<HashMap<String, Option<gtk::gdk::Texture>>>,
}

impl ReviewPage {
    pub fn new(menu: &gtk::MenuButton, emit: impl Fn(ReviewMsg) + 'static) -> Self {
        let summary = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .build();
        column.append(&summary);
        column.append(&list);
        let export = gtk::Button::builder()
            .label(gettext("_Export…"))
            .use_underline(true)
            .css_classes(["suggested-action"])
            .build();
        let add_books = gtk::Button::builder()
            .label(gettext("_Add Books"))
            .use_underline(true)
            .build();
        let header = adw::HeaderBar::new();
        header.pack_start(&add_books);
        header.pack_end(menu);
        header.pack_end(&export);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&scrolled(&column)));
        let root = adw::NavigationView::new();
        root.add(&adw::NavigationPage::with_tag(
            &toolbar,
            &gettext("Review"),
            "list",
        ));
        Self {
            root,
            list,
            summary,
            export,
            add_books,
            emit: Rc::new(emit),
            open: Rc::new(Cell::new(None)),
            thumbnails: RefCell::new(HashMap::new()),
        }
    }

    /// Shows the list of books as they now stand.
    pub fn show_list(&self, books: &[ReviewBook]) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        // Thumbnails of books no longer here are not kept.
        self.thumbnails.borrow_mut().retain(|name, _| {
            books
                .iter()
                .any(|b| b.front.as_ref().is_some_and(|p| p.name() == name))
        });
        for (index, book) in books.iter().enumerate() {
            let row = adw::ActionRow::builder()
                .title(book.draft.query.to_string())
                .subtitle(status(book))
                .activatable(true)
                .build();
            row.set_use_markup(false);
            // Prefixes go in front of those already added, so the thumbnail,
            // added first, sits after the check box.
            if let Some(front) = &book.front {
                let texture = self
                    .thumbnails
                    .borrow_mut()
                    .entry(front.name().to_owned())
                    .or_insert_with(|| photos::kept(&front.jpeg, THUMBNAIL as u32 * 2))
                    .clone();
                let image = gtk::Image::builder().pixel_size(THUMBNAIL).build();
                image.set_paintable(texture.as_ref());
                row.add_prefix(&image);
            }
            if book.draft.identity == Identity::Identified {
                let include = gtk::CheckButton::builder()
                    .active(book.included)
                    .valign(gtk::Align::Center)
                    .tooltip_text(gettext("Include in the export"))
                    .build();
                let emit = self.emit.clone();
                include.connect_toggled(move |c| emit(ReviewMsg::Include(index, c.is_active())));
                row.add_prefix(&include);
            }
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let emit = self.emit.clone();
            row.connect_activated(move |_| emit(ReviewMsg::Open(index)));
            self.list.append(&row);
        }
        let included = books.iter().filter(|b| b.included).count();
        let summary = if books.len() == 1 {
            gettext_f(
                "1 looked-up book, {included} included in the export.",
                &[("included", &included.to_string())],
            )
        } else {
            gettext_f(
                "{count} looked-up books, {included} included in the export.",
                &[
                    ("count", &books.len().to_string()),
                    ("included", &included.to_string()),
                ],
            )
        };
        self.summary.set_label(&summary);
        self.export.set_sensitive(included > 0);
    }

    /// Opens a book's page, in place of any book page already open.
    pub fn show_book(&self, index: usize, book: &ReviewBook) {
        self.root.pop_to_tag("list");
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&scrolled(&book_content(index, book, &self.emit))));
        let page = adw::NavigationPage::with_tag(&toolbar, &book.draft.query.to_string(), "book");
        let open = self.open.clone();
        page.connect_hidden(move |_| open.set(None));
        self.root.push(&page);
        self.open.set(Some(index));
    }

    /// Shows a book's page again after a change, where the owner had
    /// scrolled to, so editing a value does not lose their place.
    pub fn refresh_book(&self, index: usize, book: &ReviewBook) {
        let toolbar = self
            .root
            .find_page("book")
            .and_then(|p| p.child())
            .and_then(|c| c.downcast::<adw::ToolbarView>().ok());
        let (Some(toolbar), Some(open)) = (toolbar, self.open.get()) else {
            return self.show_book(index, book);
        };
        if open != index {
            return self.show_book(index, book);
        }
        let at = toolbar
            .content()
            .and_then(|c| c.downcast::<gtk::ScrolledWindow>().ok())
            .map_or(0.0, |s| s.vadjustment().value());
        let content = scrolled(&book_content(index, book, &self.emit));
        // The new page grows as it is laid out, so its place is set once
        // its scroll range is large enough to hold it.
        let handler = Rc::new(RefCell::new(None));
        let id = content.vadjustment().connect_changed({
            let handler = handler.clone();
            move |adj| {
                if adj.upper() - adj.page_size() >= at {
                    if let Some(id) = handler.borrow_mut().take() {
                        adj.disconnect(id);
                    }
                    // Set during layout, the place would not be drawn.
                    let adj = adj.clone();
                    gtk::glib::idle_add_local_once(move || adj.set_value(at));
                }
            }
        });
        handler.replace(Some(id));
        toolbar.set_content(Some(&content));
    }

    /// Back to the list, closing any book page.
    pub fn close_book(&self) {
        self.root.pop_to_tag("list");
        self.open.set(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_s_site_is_its_host() {
        assert_eq!(site("https://www.comics.org/issue/123/"), "www.comics.org");
        assert_eq!(site("https://example.org?q=1"), "example.org");
        assert_eq!(site("example.org/x"), "example.org");
    }
}
