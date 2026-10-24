//! Asking for the passphrase that opens the key file. Opening it takes
//! about a second by design, so it runs off the window's thread.

use std::path::PathBuf;

use adw::prelude::*;
use assistant_keys::{ApiKeys, KeyFileError, Passphrase, load, open};
use gtk::{gio, glib};

use crate::i18n::gettext;

/// Opens the key file at `path` with `passphrase`, off the window's
/// thread, and hands the keys or the reason to `done` on it.
pub fn open_key_file(
    path: PathBuf,
    passphrase: Passphrase,
    done: impl FnOnce(Result<ApiKeys, KeyFileError>) + 'static,
) {
    let job = gio::spawn_blocking(move || {
        let bytes = load(&path)?.ok_or(KeyFileError::CannotOpen)?;
        open(&bytes, &passphrase)
    });
    glib::spawn_future_local(async move {
        done(job.await.unwrap_or(Err(KeyFileError::CannotOpen)));
    });
}

/// The unlock page's parts the app reacts to.
pub struct UnlockPage {
    pub root: adw::ToolbarView,
    pub entry: adw::PasswordEntryRow,
    pub unlock: gtk::Button,
    pub forget: gtk::Button,
    pub problem: gtk::Label,
}

impl UnlockPage {
    pub fn new() -> Self {
        let entry = adw::PasswordEntryRow::builder()
            .title(gettext("Passphrase"))
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        list.append(&entry);
        let problem = gtk::Label::builder()
            .wrap(true)
            .css_classes(["error"])
            .visible(false)
            .build();
        let unlock = gtk::Button::builder()
            .label(gettext("_Unlock"))
            .use_underline(true)
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let forget = gtk::Button::builder()
            .label(gettext("Forgot the passphrase?"))
            .halign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .build();
        column.append(&list);
        column.append(&problem);
        column.append(&unlock);
        column.append(&forget);
        let status = adw::StatusPage::builder()
            .icon_name("system-lock-screen-symbolic")
            .css_classes(["compact"])
            .title(gettext("Locked"))
            .description(gettext("Type your passphrase to use your saved API key."))
            .child(
                &adw::Clamp::builder()
                    .maximum_size(400)
                    .child(&column)
                    .build(),
            )
            .build();
        let root = adw::ToolbarView::new();
        root.add_top_bar(&adw::HeaderBar::new());
        root.set_content(Some(&status));
        {
            let unlock = unlock.clone();
            entry.connect_entry_activated(move |_| unlock.emit_clicked());
        }
        Self {
            root,
            entry,
            unlock,
            forget,
            problem,
        }
    }

    /// The typed passphrase, cleared from the entry.
    pub fn take(&self) -> Option<Passphrase> {
        let text = self.entry.text().to_string();
        self.entry.set_text("");
        Passphrase::new(text)
    }

    pub fn show_problem(&self, text: Option<&str>) {
        self.problem.set_label(text.unwrap_or(""));
        self.problem.set_visible(text.is_some());
    }

    pub fn busy(&self, busy: bool) {
        self.unlock.set_sensitive(!busy);
        self.entry.set_sensitive(!busy);
    }
}
