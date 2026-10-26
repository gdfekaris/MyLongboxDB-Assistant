//! The page where the owner gives the books to look up: typed one a line,
//! or as photos of their covers, one book a photo with its back if they
//! took one. It shows what each was read as before anything is sent.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use assistant_core::lines::{BookQuery, LineProblem, parse_lines};
use assistant_core::queue::MAX_BATCH;
use assistant_media::Edit;
use gtk::{gio, glib};

use crate::i18n::{gettext, gettext_f};
use crate::photo_editor;
use crate::photos::{self, Picked};

/// The size of a photo's thumbnail in the list.
const THUMBNAIL: i32 = 56;
/// The longest edge of the picture the editor shows.
const EDITOR_EDGE: u32 = 1200;

/// A book given as photos: its front cover, and its back if taken.
#[derive(Clone, Debug)]
pub struct PhotoBook {
    pub front: Picked,
    pub back: Option<Picked>,
}

impl PhotoBook {
    /// How the book is named until it is looked up.
    pub fn label(&self) -> String {
        match &self.back {
            Some(back) => gettext_f(
                "Photos {front} and {back}",
                &[("front", &self.front.name), ("back", &back.name)],
            ),
            None => gettext_f("Photo {front}", &[("front", &self.front.name)]),
        }
    }
}

/// Which side of a book a photo being read is for.
#[derive(Clone, Copy)]
enum Slot {
    /// A new book.
    New,
    /// The back of the book at this place.
    Back(usize),
}

pub struct AddPage {
    pub root: adw::ToolbarView,
    text: gtk::TextView,
    list: gtk::ListBox,
    photo_list: gtk::ListBox,
    problem_list: gtk::ListBox,
    summary: gtk::Label,
    pub batch: adw::SpinRow,
    pub look_up: gtk::Button,
    add_photos: gtk::Button,
    photos: RefCell<Vec<PhotoBook>>,
    /// Why photos were not added, until the next are picked.
    problems: RefCell<Vec<String>>,
    /// Photos still being read.
    reading: Cell<usize>,
    this_year: u16,
}

impl AddPage {
    pub fn new(header: &adw::HeaderBar, batch: usize, this_year: u16) -> Rc<Self> {
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
        let list = boxed_list();
        let typed = column(&[intro.upcast_ref(), frame.upcast_ref(), list.upcast_ref()]);

        let photo_intro = gtk::Label::builder()
            .label(gettext(
                "Add photos of your books' front covers, one book a photo. If you took a \
                 book's back cover too, add it to that book.",
            ))
            .wrap(true)
            .xalign(0.0)
            .build();
        let add_photos = gtk::Button::builder()
            .label(gettext("_Add Photos…"))
            .use_underline(true)
            .halign(gtk::Align::Start)
            .build();
        let problem_list = boxed_list();
        let photo_list = boxed_list();
        let photo_page = column(&[
            photo_intro.upcast_ref(),
            add_photos.upcast_ref(),
            problem_list.upcast_ref(),
            photo_list.upcast_ref(),
        ]);

        let tabs = adw::ViewStack::new();
        tabs.add_titled_with_icon(
            &typed,
            Some("typed"),
            &gettext("Typed"),
            "document-edit-symbolic",
        );
        tabs.add_titled_with_icon(
            &photo_page,
            Some("photos"),
            &gettext("Photos"),
            "camera-photo-symbolic",
        );
        let switcher = adw::InlineViewSwitcher::builder()
            .stack(&tabs)
            .halign(gtk::Align::Center)
            .build();

        let summary = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["heading"])
            .build();
        let batch_row = adw::SpinRow::with_range(1.0, MAX_BATCH as f64, 1.0);
        batch_row.set_title(&gettext("Books per run"));
        batch_row.set_subtitle(&gettext("The most books one run may hold"));
        batch_row.set_value(batch as f64);
        let settings = boxed_list();
        settings.set_visible(true);
        settings.append(&batch_row);
        let look_up = gtk::Button::builder()
            .label(gettext("_Look Up…"))
            .use_underline(true)
            .halign(gtk::Align::Center)
            .sensitive(false)
            .css_classes(["pill", "suggested-action"])
            .build();
        let page = column(&[
            switcher.upcast_ref(),
            tabs.upcast_ref(),
            summary.upcast_ref(),
            settings.upcast_ref(),
            look_up.upcast_ref(),
        ]);
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
                        .child(&page)
                        .build(),
                )
                .build(),
        ));
        let page = Rc::new(Self {
            root,
            text,
            list,
            photo_list,
            problem_list,
            summary,
            batch: batch_row,
            look_up,
            add_photos,
            photos: RefCell::new(Vec::new()),
            problems: RefCell::new(Vec::new()),
            reading: Cell::new(0),
            this_year,
        });
        let weak = Rc::downgrade(&page);
        page.text.buffer().connect_changed(move |_| refresh(&weak));
        let weak = Rc::downgrade(&page);
        page.batch.connect_value_notify(move |_| refresh(&weak));
        let weak = Rc::downgrade(&page);
        page.add_photos.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.pick(Slot::New);
            }
        });
        page.refresh();
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

    /// The books given as photos.
    pub fn photo_books(&self) -> Vec<PhotoBook> {
        self.photos.borrow().clone()
    }

    pub fn batch(&self) -> usize {
        usize::try_from(self.batch.value() as i64).unwrap_or(1)
    }

    /// Whether every book given can be looked up as it is.
    pub fn ready(&self) -> bool {
        self.look_up.is_sensitive()
    }

    /// Asks for photos for a new book each, or for one book's back.
    fn pick(self: &Rc<Self>, slot: Slot) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&gettext("Photos")));
        for mime in [
            "image/jpeg",
            "image/png",
            "image/webp",
            "image/heic",
            "image/heif",
        ] {
            filter.add_mime_type(mime);
        }
        for suffix in ["jpg", "jpeg", "png", "webp", "heic", "heif"] {
            filter.add_suffix(suffix);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(match slot {
                Slot::New => gettext("Add Photos"),
                Slot::Back(_) => gettext("Add Back Cover"),
            })
            .modal(true)
            .filters(&filters)
            .default_filter(&filter)
            .build();
        let window = self.root.root().and_downcast::<gtk::Window>();
        let page = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let files: Vec<gio::File> = match slot {
                Slot::New => match dialog.open_multiple_future(window.as_ref()).await {
                    Ok(model) => (0..model.n_items())
                        .filter_map(|i| model.item(i).and_downcast::<gio::File>())
                        .collect(),
                    Err(_) => return,
                },
                Slot::Back(_) => match dialog.open_future(window.as_ref()).await {
                    Ok(file) => vec![file],
                    Err(_) => return,
                },
            };
            if let Some(page) = page.upgrade() {
                page.problems.borrow_mut().clear();
                page.read(files, slot).await;
            }
        });
    }

    /// Reads the picked photos one at a time, so only one is ever held at
    /// full size, and adds each as it is ready.
    async fn read(self: &Rc<Self>, files: Vec<gio::File>, slot: Slot) {
        self.reading.set(self.reading.get() + files.len());
        self.refresh();
        for file in files {
            let name = file
                .basename()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| gettext("photo"));
            let read = match file.path() {
                Some(path) => gio::spawn_blocking(move || {
                    let bytes = photos::read(&path)?;
                    let prepared = photos::prepare(bytes.clone(), Edit::default())?;
                    Ok((bytes, prepared))
                })
                .await
                .unwrap_or_else(|_| Err(gettext("the photo could not be read"))),
                None => Err(gettext("the photo could not be opened")),
            };
            self.reading.set(self.reading.get().saturating_sub(1));
            match read {
                Ok((bytes, prepared)) => {
                    let picked = Picked {
                        name,
                        original: Rc::new(bytes),
                        jpeg: prepared.jpeg,
                        size: prepared.size,
                        thumbnail: prepared.thumbnail,
                    };
                    let mut photos = self.photos.borrow_mut();
                    match slot {
                        Slot::New => photos.push(PhotoBook {
                            front: picked,
                            back: None,
                        }),
                        Slot::Back(i) => {
                            if let Some(book) = photos.get_mut(i) {
                                book.back = Some(picked);
                            }
                        }
                    }
                }
                Err(why) => self
                    .problems
                    .borrow_mut()
                    .push(photos::not_added(&name, &why)),
            }
            self.show_photos();
            self.refresh();
        }
    }

    /// Opens the editor on one of a book's photos, and remakes the photo
    /// with the owner's changes when they save.
    fn edit(self: &Rc<Self>, i: usize, back: bool) {
        let picked = {
            let photos = self.photos.borrow();
            let Some(book) = photos.get(i) else {
                return;
            };
            match (back, &book.back) {
                (false, _) => book.front.clone(),
                (true, Some(b)) => b.clone(),
                (true, None) => return,
            }
        };
        let title = if back {
            gettext("Edit Back Cover")
        } else {
            gettext("Edit Front Cover")
        };
        let Some(window) = self.root.root().and_downcast::<gtk::Window>() else {
            return;
        };
        let page = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let bytes = picked.original.as_ref().clone();
            let preview = gio::spawn_blocking(move || photos::for_editing(bytes, EDITOR_EDGE))
                .await
                .unwrap_or_else(|_| Err(gettext("the photo could not be read")));
            let Some(page) = page.upgrade() else {
                return;
            };
            let preview = match preview {
                Ok(preview) => preview,
                Err(why) => {
                    page.problems
                        .borrow_mut()
                        .push(photos::not_edited(&picked.name, &why));
                    page.show_photos();
                    return;
                }
            };
            let weak = Rc::downgrade(&page);
            photo_editor::open(&window, &title, preview, move |edit| {
                if let Some(page) = weak.upgrade() {
                    let picked = picked.clone();
                    glib::spawn_future_local(async move { page.remake(picked, edit).await });
                }
            });
        });
    }

    /// Makes the kept photo again from the original with `edit`, and puts
    /// it in place of the one it was made from, wherever that now is.
    async fn remake(self: &Rc<Self>, picked: Picked, edit: Edit) {
        self.reading.set(self.reading.get() + 1);
        self.refresh();
        let bytes = picked.original.as_ref().clone();
        let made = gio::spawn_blocking(move || photos::prepare(bytes, edit))
            .await
            .unwrap_or_else(|_| Err(gettext("the photo could not be read")));
        self.reading.set(self.reading.get().saturating_sub(1));
        match made {
            Ok(prepared) => {
                for book in self.photos.borrow_mut().iter_mut() {
                    for slot in std::iter::once(&mut book.front).chain(book.back.as_mut()) {
                        if Rc::ptr_eq(&slot.original, &picked.original) {
                            slot.jpeg = prepared.jpeg.clone();
                            slot.size = prepared.size;
                            slot.thumbnail = prepared.thumbnail.clone();
                        }
                    }
                }
            }
            Err(why) => self
                .problems
                .borrow_mut()
                .push(photos::not_edited(&picked.name, &why)),
        }
        self.show_photos();
        self.refresh();
    }

    /// Changes the photo books, then shows them as they now are.
    fn change(self: &Rc<Self>, change: impl FnOnce(&mut Vec<PhotoBook>)) {
        change(&mut self.photos.borrow_mut());
        self.show_photos();
        self.refresh();
    }

    fn show_photos(self: &Rc<Self>) {
        while let Some(row) = self.problem_list.first_child() {
            self.problem_list.remove(&row);
        }
        for problem in self.problems.borrow().iter() {
            let row = adw::ActionRow::builder()
                .title(problem)
                .title_lines(3)
                .css_classes(["error"])
                .build();
            row.set_use_markup(false);
            self.problem_list.append(&row);
        }
        self.problem_list
            .set_visible(!self.problems.borrow().is_empty());

        while let Some(row) = self.photo_list.first_child() {
            self.photo_list.remove(&row);
        }
        let books = self.photos.borrow().clone();
        for (i, book) in books.iter().enumerate() {
            self.photo_list.append(&self.photo_row(i, book, &books));
        }
        self.photo_list.set_visible(!books.is_empty());
    }

    fn photo_row(
        self: &Rc<Self>,
        i: usize,
        book: &PhotoBook,
        books: &[PhotoBook],
    ) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(&book.front.name)
            .subtitle(match &book.back {
                Some(back) => gettext_f("Back cover: {name}", &[("name", &back.name)]),
                None => gettext("Front cover only"),
            })
            .title_lines(1)
            .build();
        row.set_use_markup(false);
        // Front then back, each in the same square whatever its shape, so
        // the names line up.
        let thumbnails = gtk::Box::builder()
            .spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        for (picked, back) in
            std::iter::once((&book.front, false)).chain(book.back.as_ref().map(|b| (b, true)))
        {
            let image = gtk::Image::builder().pixel_size(THUMBNAIL).build();
            if let Some(texture) = photos::texture_of(&picked.thumbnail) {
                image.set_paintable(Some(&texture));
            }
            // A photo is edited by choosing it.
            let tip = if back {
                gettext("Edit Back Cover")
            } else {
                gettext("Edit Front Cover")
            };
            let edit = gtk::Button::builder()
                .child(&image)
                .tooltip_text(&tip)
                .css_classes(["flat"])
                .build();
            edit.update_property(&[gtk::accessible::Property::Label(&tip)]);
            let page = Rc::downgrade(self);
            edit.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    page.edit(i, back);
                }
            });
            thumbnails.append(&edit);
        }
        row.add_prefix(&thumbnails);
        let button = |icon: &str, tip: String| {
            let button = gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(&tip)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            button.update_property(&[gtk::accessible::Property::Label(&tip)]);
            row.add_suffix(&button);
            button
        };
        let page = Rc::downgrade(self);
        let act = move |f: fn(&Rc<Self>, usize)| {
            let page = page.clone();
            move |_: &gtk::Button| {
                if let Some(page) = page.upgrade() {
                    f(&page, i);
                }
            }
        };
        match &book.back {
            Some(_) => {
                button(
                    "object-flip-horizontal-symbolic",
                    gettext("Swap Front and Back"),
                )
                .connect_clicked(act(|p, i| {
                    p.change(|books| {
                        if let Some(book) = books.get_mut(i)
                            && let Some(back) = book.back.take()
                        {
                            book.back = Some(std::mem::replace(&mut book.front, back));
                        }
                    });
                }));
                button("edit-clear-symbolic", gettext("Remove Back Cover")).connect_clicked(act(
                    |p, i| {
                        p.change(|books| {
                            if let Some(book) = books.get_mut(i) {
                                book.back = None;
                            }
                        });
                    },
                ));
            }
            None => {
                // A photo taken just after a book's front is often its back.
                let above_has_room = i > 0 && books[i - 1].back.is_none();
                if above_has_room {
                    button("go-up-symbolic", gettext("Use as Back of the Book Above"))
                        .connect_clicked(act(|p, i| {
                            p.change(|books| {
                                if i > 0 && i < books.len() && books[i - 1].back.is_none() {
                                    let photo = books.remove(i).front;
                                    books[i - 1].back = Some(photo);
                                }
                            });
                        }));
                }
                button("list-add-symbolic", gettext("Add Back Cover…"))
                    .connect_clicked(act(|p, i| p.pick(Slot::Back(i))));
            }
        }
        button("user-trash-symbolic", gettext("Remove Book")).connect_clicked(act(|p, i| {
            p.change(|books| {
                if i < books.len() {
                    books.remove(i);
                }
            });
        }));
        row
    }

    /// Shows what the lines were read as, and whether the books given can
    /// be looked up as they are.
    fn refresh(&self) {
        let (books, problems) = self.parsed(self.this_year);
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
        let count = books.len() + self.photos.borrow().len();
        let batch = self.batch();
        let (text, ready) = if !problems.is_empty() {
            (
                gettext("Fix or remove the lines that could not be read."),
                false,
            )
        } else if self.reading.get() > 0 {
            (
                gettext_f(
                    "Reading photos: {count} to go…",
                    &[("count", &self.reading.get().to_string())],
                ),
                false,
            )
        } else if count > batch {
            (
                gettext_f(
                    "{count} books is more than a run may hold ({batch}). Split them up, or \
                     raise Books per run.",
                    &[("count", &count.to_string()), ("batch", &batch.to_string())],
                ),
                false,
            )
        } else if count == 0 {
            (String::new(), false)
        } else if count == 1 {
            (gettext("1 book to look up"), true)
        } else {
            (
                gettext_f("{count} books to look up", &[("count", &count.to_string())]),
                true,
            )
        };
        self.summary.set_label(&text);
        self.summary.set_visible(!text.is_empty());
        self.look_up.set_sensitive(ready);
        self.add_photos.set_sensitive(self.reading.get() == 0);
    }

    /// Empties both tabs, once their books are looked up.
    pub fn clear(self: &Rc<Self>) {
        self.photos.borrow_mut().clear();
        self.problems.borrow_mut().clear();
        self.show_photos();
        self.text.buffer().set_text("");
        // Emptying text that is already empty does not count as a change,
        // so after a run of photos alone the count would stay behind.
        self.refresh();
    }
}

fn refresh(page: &Weak<AddPage>) {
    if let Some(page) = page.upgrade() {
        page.refresh();
    }
}

fn boxed_list() -> gtk::ListBox {
    gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .visible(false)
        .build()
}

fn column(children: &[&gtk::Widget]) -> gtk::Box {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .build();
    for child in children {
        column.append(*child);
    }
    column
}
