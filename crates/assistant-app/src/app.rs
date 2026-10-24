//! The window: setting up, unlocking, and the home page, and holding the
//! unlocked keys under the session's rules.

use std::time::Instant;

use adw::prelude::*;
use assistant_keys::{ApiKeys, KdfParams, KeyFileError, Passphrase, save, seal};
use assistant_net::Client;
use gtk::{gio, glib};
use relm4::{ComponentParts, ComponentSender, SimpleComponent};

use crate::config::{self, Config, ProviderChoice};
use crate::i18n::{gettext, gettext_f};
use crate::session::{Session, passphrase_problem};
use crate::setup::{self, SetupDone};
use crate::unlock::{UnlockPage, open_key_file};

/// The current year, which bounds the dates a lookup accepts.
fn this_year() -> u16 {
    glib::DateTime::now_local()
        .map(|d| u16::try_from(d.year()).unwrap_or(2026))
        .unwrap_or(2026)
}

pub enum AppMsg {
    SetupDone(Box<SetupDone>),
    Saved(Result<ApiKeys, KeyFileError>, Config),
    Unlock,
    Unlocked(Result<ApiKeys, KeyFileError>),
    Lock,
    /// The owner pressed a key or clicked.
    Touch,
    /// A periodic look at how long the app has been idle.
    Tick,
    SetUpAgain,
    ChangePassphrase,
    NewPassphrase(Passphrase),
    PassphraseSaved(Result<(), KeyFileError>),
    Forget,
    Forgotten,
}

impl std::fmt::Debug for AppMsg {
    /// Names the message only: some carry keys or passphrases.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SetupDone(_) => "SetupDone",
            Self::Saved(..) => "Saved",
            Self::Unlock => "Unlock",
            Self::Unlocked(_) => "Unlocked",
            Self::Lock => "Lock",
            Self::Touch => "Touch",
            Self::Tick => "Tick",
            Self::SetUpAgain => "SetUpAgain",
            Self::ChangePassphrase => "ChangePassphrase",
            Self::NewPassphrase(_) => "NewPassphrase",
            Self::PassphraseSaved(_) => "PassphraseSaved",
            Self::Forget => "Forget",
            Self::Forgotten => "Forgotten",
        })
    }
}

pub struct App {
    config: Config,
    session: Session,
    client: Client,
    window: adw::ApplicationWindow,
    stack: gtk::Stack,
    setup_holder: adw::Bin,
    unlock: UnlockPage,
    home: adw::StatusPage,
    toasts: adw::ToastOverlay,
    sender: ComponentSender<Self>,
}

impl App {
    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn show_setup(&self) {
        let sender = self.sender.clone();
        let nav = setup::build(self.client.clone(), this_year(), move |done| {
            sender.input(AppMsg::SetupDone(Box::new(done)));
        });
        self.setup_holder.set_child(Some(&nav));
        self.stack.set_visible_child_name("setup");
    }

    fn show_unlock(&self) {
        self.unlock.show_problem(None);
        self.unlock.busy(false);
        self.stack.set_visible_child_name("unlock");
        self.unlock.entry.grab_focus();
    }

    fn show_home(&self) {
        let provider = match self.config.provider {
            Some(ProviderChoice::OpenRouter) => gettext("OpenRouter"),
            _ => gettext("Anthropic"),
        };
        self.home.set_description(Some(&gettext_f(
            "Looking books up with {model} through {provider}. Adding books comes next.",
            &[("model", &self.config.model), ("provider", &provider)],
        )));
        self.stack.set_visible_child_name("home");
        self.setup_holder.set_child(None::<&gtk::Widget>);
    }

    /// The page the app starts on, or returns to after locking.
    fn show_start(&self) {
        if self.config.provider.is_none() {
            self.show_setup();
        } else if config::key_file().exists() {
            self.show_unlock();
        } else {
            self.show_setup();
        }
    }

    fn action(&self, name: &str, msg: fn() -> AppMsg) {
        let action = gio::SimpleAction::new(name, None);
        let sender = self.sender.clone();
        action.connect_activate(move |_, _| sender.input(msg()));
        self.window.add_action(&action);
    }

    /// Asks for a new passphrase twice.
    fn ask_new_passphrase(&self) {
        let first = adw::PasswordEntryRow::builder()
            .title(gettext("New passphrase"))
            .activates_default(true)
            .build();
        let second = adw::PasswordEntryRow::builder()
            .title(gettext("New passphrase again"))
            .activates_default(true)
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        list.append(&first);
        list.append(&second);
        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Change Passphrase"))
            .body(gettext("At least 12 characters, typed twice."))
            .extra_child(&list)
            .focus_widget(&first)
            .close_response("cancel")
            .default_response("change")
            .build();
        dialog.add_responses(&[
            ("cancel", &gettext("_Cancel")),
            ("change", &gettext("C_hange")),
        ]);
        dialog.set_response_appearance("change", adw::ResponseAppearance::Suggested);
        dialog.set_response_enabled("change", false);
        // Change is offered only for a passphrase long enough and typed
        // the same twice, so nothing else can be submitted.
        let ready = {
            let (first, second, dialog) = (first.clone(), second.clone(), dialog.downgrade());
            move || {
                let (a, b) = (first.text(), second.text());
                if let Some(dialog) = dialog.upgrade() {
                    dialog
                        .set_response_enabled("change", passphrase_problem(&a).is_none() && a == b);
                }
            }
        };
        {
            let ready = ready.clone();
            first.connect_changed(move |_| ready());
        }
        second.connect_changed(move |_| ready());
        let sender = self.sender.clone();
        dialog.choose(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |response| {
                let text = first.text().to_string();
                first.set_text("");
                second.set_text("");
                if response == "change"
                    && let Some(passphrase) = Passphrase::new(text)
                {
                    sender.input(AppMsg::NewPassphrase(passphrase));
                }
            },
        );
    }

    fn confirm_forget(&self) {
        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Remove the Saved Key?"))
            .body(gettext(
                "The encrypted key file is deleted, and you set the app up again with a key. \
                 The key itself stays valid at your provider until you revoke it there.",
            ))
            .close_response("cancel")
            .build();
        dialog.add_responses(&[
            ("cancel", &gettext("_Cancel")),
            ("remove", &gettext("_Remove")),
        ]);
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        let sender = self.sender.clone();
        dialog.choose(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |response| {
                if response == "remove" {
                    sender.input(AppMsg::Forgotten);
                }
            },
        );
    }

    /// Seals `keys` under `passphrase` and saves them, off the window's
    /// thread; `then` gets the outcome on it.
    fn seal_and_save(
        keys: ApiKeys,
        passphrase: Passphrase,
        then: impl FnOnce(Result<ApiKeys, KeyFileError>) + 'static,
    ) {
        let job = gio::spawn_blocking(move || {
            let bytes = seal(&keys, &passphrase, KdfParams::DEFAULT)?;
            save(&config::key_file(), &bytes)?;
            Ok(keys)
        });
        glib::spawn_future_local(async move {
            then(job.await.unwrap_or(Err(KeyFileError::CannotOpen)));
        });
    }
}

/// A key file error as a sentence for the owner.
fn error_text(error: &KeyFileError) -> String {
    match error {
        KeyFileError::CannotOpen => gettext("The passphrase is wrong, or the key file is damaged."),
        KeyFileError::NewerVersion => {
            gettext("The key file was saved by a newer version of the app.")
        }
        KeyFileError::NoRandomness => gettext("The system could not supply random numbers."),
        // Messages the app made itself, such as a passphrase too short.
        KeyFileError::Io(e) if e.kind() == std::io::ErrorKind::Other => e.to_string(),
        KeyFileError::Io(e) => gettext_f(
            "The key file could not be read or saved: {why}",
            &[("why", &e.to_string())],
        ),
    }
}

impl SimpleComponent for App {
    type Init = ();
    type Input = AppMsg;
    type Output = ();
    type Root = adw::ApplicationWindow;
    type Widgets = ();

    fn init_root() -> Self::Root {
        adw::ApplicationWindow::builder()
            .title(gettext("MyLongboxDB Assistant"))
            .default_width(760)
            .default_height(620)
            .build()
    }

    fn init(
        _: Self::Init,
        window: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let unlock = UnlockPage::new();
        let setup_holder = adw::Bin::new();

        let menu = gio::Menu::new();
        menu.append(Some(&gettext("_Lock")), Some("win.lock"));
        menu.append(
            Some(&gettext("Change _Passphrase…")),
            Some("win.passphrase"),
        );
        menu.append(Some(&gettext("_Set Up Again…")), Some("win.setup"));
        menu.append(Some(&gettext("_Remove Saved Key…")), Some("win.forget"));
        let header = adw::HeaderBar::new();
        header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(&menu)
                .tooltip_text(gettext("Main Menu"))
                .build(),
        );
        let home = adw::StatusPage::builder()
            .icon_name("system-search-symbolic")
            .title(gettext("Ready"))
            .build();
        let home_page = adw::ToolbarView::new();
        home_page.add_top_bar(&header);
        home_page.set_content(Some(&home));

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&setup_holder, Some("setup"));
        stack.add_named(&unlock.root, Some("unlock"));
        stack.add_named(&home_page, Some("home"));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        window.set_content(Some(&toasts));

        // Keys and clicks anywhere in the window count as use.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let sender = sender.clone();
            keys.connect_key_pressed(move |_, _, _, _| {
                sender.input(AppMsg::Touch);
                glib::Propagation::Proceed
            });
        }
        window.add_controller(keys);
        let clicks = gtk::GestureClick::new();
        clicks.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let sender = sender.clone();
            clicks.connect_pressed(move |_, _, _, _| sender.input(AppMsg::Touch));
        }
        window.add_controller(clicks);
        {
            let sender = sender.clone();
            glib::timeout_add_seconds_local(30, move || {
                sender.input(AppMsg::Tick);
                glib::ControlFlow::Continue
            });
        }
        {
            let sender = sender.clone();
            unlock
                .unlock
                .connect_clicked(move |_| sender.input(AppMsg::Unlock));
        }
        {
            let sender = sender.clone();
            unlock
                .forget
                .connect_clicked(move |_| sender.input(AppMsg::Forget));
        }

        let config = Config::load(&config::path());
        let model = Self {
            session: Session::locked(config::key_file().exists(), Instant::now()),
            config,
            client: Client::new(),
            window,
            stack,
            setup_holder,
            unlock,
            home,
            toasts,
            sender,
        };
        model.action("lock", || AppMsg::Lock);
        model.action("passphrase", || AppMsg::ChangePassphrase);
        model.action("setup", || AppMsg::SetUpAgain);
        model.action("forget", || AppMsg::Forget);
        model.show_start();
        ComponentParts { model, widgets: () }
    }

    fn update(&mut self, msg: AppMsg, sender: ComponentSender<Self>) {
        match msg {
            AppMsg::SetupDone(done) => {
                let SetupDone {
                    provider,
                    model,
                    key,
                    passphrase,
                } = *done;
                // A key for the other provider, already unlocked, is kept.
                let mut keys = self.session.keys().cloned().unwrap_or_default();
                match provider {
                    ProviderChoice::Anthropic => keys.anthropic = Some(key),
                    ProviderChoice::OpenRouter => keys.openrouter = Some(key),
                }
                let config = Config {
                    provider: Some(provider),
                    model,
                };
                match passphrase {
                    Some(passphrase) => {
                        let sender = sender.clone();
                        Self::seal_and_save(keys, passphrase, move |saved| {
                            sender.input(AppMsg::Saved(saved, config));
                        });
                    }
                    None => {
                        // Not saving means no earlier key file stays
                        // behind either.
                        let _ = std::fs::remove_file(config::key_file());
                        sender.input(AppMsg::Saved(Ok(keys), config));
                    }
                }
            }
            AppMsg::Saved(Ok(keys), config) => {
                if let Err(e) = config.save(&config::path()) {
                    self.toast(&gettext_f(
                        "Your choices could not be saved: {why}",
                        &[("why", e.message())],
                    ));
                }
                self.config = config;
                let saved = config::key_file().exists();
                self.session.unlock(keys, saved, Instant::now());
                self.show_home();
            }
            AppMsg::Saved(Err(e), _) => {
                self.toast(&gettext_f(
                    "The key could not be saved: {why}",
                    &[("why", &error_text(&e))],
                ));
            }
            AppMsg::Unlock => {
                let Some(passphrase) = self.unlock.take() else {
                    return;
                };
                self.unlock.show_problem(None);
                self.unlock.busy(true);
                let sender = sender.clone();
                open_key_file(config::key_file(), passphrase, move |opened| {
                    sender.input(AppMsg::Unlocked(opened));
                });
            }
            AppMsg::Unlocked(Ok(keys)) => {
                self.session.unlock(keys, true, Instant::now());
                if self
                    .config
                    .provider
                    .is_some_and(|p| self.session.key(p).is_some())
                {
                    self.show_home();
                } else {
                    self.show_setup();
                }
            }
            AppMsg::Unlocked(Err(e)) => {
                self.unlock.busy(false);
                self.unlock.show_problem(Some(&error_text(&e)));
                self.unlock.entry.grab_focus();
            }
            AppMsg::Lock => {
                self.session.lock();
                self.show_start();
            }
            AppMsg::Touch => self.session.touch(Instant::now()),
            AppMsg::Tick => {
                if self.session.lock_if_idle(Instant::now()) {
                    self.show_start();
                    self.toast(&gettext("Locked after a quarter of an hour unused."));
                }
            }
            AppMsg::SetUpAgain => self.show_setup(),
            AppMsg::ChangePassphrase => {
                if self.session.is_unlocked() {
                    self.ask_new_passphrase();
                } else {
                    self.toast(&gettext("Unlock first, then change the passphrase."));
                }
            }
            AppMsg::NewPassphrase(passphrase) => {
                let Some(keys) = self.session.keys().cloned() else {
                    return;
                };
                let sender = sender.clone();
                Self::seal_and_save(keys, passphrase, move |saved| {
                    sender.input(AppMsg::PassphraseSaved(saved.map(|_| ())));
                });
            }
            AppMsg::PassphraseSaved(Ok(())) => {
                if let Some(keys) = self.session.keys().cloned() {
                    self.session.unlock(keys, true, Instant::now());
                }
                self.toast(&gettext("The passphrase is changed."));
            }
            AppMsg::PassphraseSaved(Err(e)) => self.toast(&error_text(&e)),
            AppMsg::Forget => self.confirm_forget(),
            AppMsg::Forgotten => {
                let _ = std::fs::remove_file(config::key_file());
                self.session = Session::locked(false, Instant::now());
                self.show_setup();
            }
        }
    }
}
