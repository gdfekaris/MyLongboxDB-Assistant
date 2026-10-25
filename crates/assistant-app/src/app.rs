//! The window: setting up, unlocking, adding books, and running their
//! lookups, and holding the unlocked keys under the session's rules.

use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use adw::prelude::*;
use assistant_core::cost::{Estimate, anthropic_price, dollars};
use assistant_core::lookup::{Draft, Settings as LookupSettings};
use assistant_core::queue::{self, Event, Item, RunOptions, Summary};
use assistant_core::results::{self, Photo, ReviewBook};
use assistant_core::write_export;
use assistant_keys::{ApiKey, ApiKeys, KdfParams, KeyFileError, Passphrase, save, seal};
use assistant_net::Client;
use assistant_net::anthropic::Anthropic;
use assistant_net::lookup::{AnthropicLookup, OpenRouterLookup};
use assistant_net::openrouter::{ModelInfo, OpenRouter};
use gtk::{gio, glib};
use relm4::{ComponentParts, ComponentSender, SimpleComponent};

use crate::add::AddPage;
use crate::config::{self, Config, ProviderChoice};
use crate::i18n::{gettext, gettext_f};
use crate::review::{List, ReviewMsg, ReviewPage};
use crate::run::{Book, Run, RunPage, Status};
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
    Batch(usize),
    LookUp,
    Estimated(Option<Estimate>),
    /// The owner confirmed a run, with the passphrase if the key is saved.
    Confirmed(Option<Passphrase>),
    RunKeys(Result<ApiKeys, KeyFileError>),
    RunEvent(Event),
    RunDone(Summary),
    CancelRun,
    LookUpMore,
    ShowReview,
    ShowAdd,
    Review(ReviewMsg),
    Edited(usize, &'static str, String),
    DiscardConfirmed(usize),
    /// Look up again the books of a run that failed or were not reached.
    TryAgain,
    Export,
    /// An export was written, or not: the books in it, and how many of
    /// their values were left out because they were not ticked.
    Exported(Result<(), String>, Vec<Draft>, usize),
}

/// Which work page is showing when the app is unlocked.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Add,
    Run,
    Review,
}

fn results_path() -> std::path::PathBuf {
    glib::user_data_dir().join("results.json")
}

fn menu_button(menu: &gio::Menu) -> gtk::MenuButton {
    gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(menu)
        .tooltip_text(gettext("Main Menu"))
        .build()
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
            Self::Batch(_) => "Batch",
            Self::LookUp => "LookUp",
            Self::Estimated(_) => "Estimated",
            Self::Confirmed(_) => "Confirmed",
            Self::RunKeys(_) => "RunKeys",
            Self::RunEvent(_) => "RunEvent",
            Self::RunDone(_) => "RunDone",
            Self::CancelRun => "CancelRun",
            Self::LookUpMore => "LookUpMore",
            Self::ShowReview => "ShowReview",
            Self::ShowAdd => "ShowAdd",
            Self::Review(_) => "Review",
            Self::Edited(..) => "Edited",
            Self::DiscardConfirmed(_) => "DiscardConfirmed",
            Self::TryAgain => "TryAgain",
            Self::Export => "Export",
            Self::Exported(..) => "Exported",
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
    add: Rc<AddPage>,
    run_page: RunPage,
    run: Option<Run>,
    review: ReviewPage,
    review_button: gtk::Button,
    /// Looked-up books kept until exported.
    results: Vec<ReviewBook>,
    view: View,
    /// The result a pending lookup of a picked candidate replaces.
    replacing: Option<usize>,
    /// The books waiting for the owner to confirm their run.
    pending: Vec<Book>,
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

    /// The work page the owner was on.
    fn show_work(&self) {
        match (self.view, &self.run) {
            (View::Run, Some(run)) => {
                self.run_page.show(run);
                self.stack.set_visible_child_name("run");
            }
            (View::Review, _) if !self.results.is_empty() => {
                self.review.show_list(&self.results);
                self.stack.set_visible_child_name("review");
            }
            _ => self.stack.set_visible_child_name("add"),
        }
        self.setup_holder.set_child(None::<&gtk::Widget>);
    }

    /// Works out the pending books' cost, then asks the owner to confirm.
    fn ask_to_run(&self) {
        let settings = self.lookup_settings();
        let books = self.pending.clone();
        let estimate = move |price: &assistant_core::cost::Price| {
            books.iter().fold(Estimate::default(), |sum, b| {
                sum.plus(b.estimate(price, &settings))
            })
        };
        match self.config.provider {
            Some(ProviderChoice::OpenRouter) => {
                let Some(key) = self.session.key(ProviderChoice::OpenRouter).cloned() else {
                    return;
                };
                let (client, model) = (self.client.clone(), self.config.model.clone());
                let job = gio::spawn_blocking(move || {
                    OpenRouter::new(&client, &key)
                        .models(&AtomicBool::new(false))
                        .ok()
                        .and_then(|models| models.into_iter().find(|m| m.id == model))
                        .and_then(|m| m.price())
                        .map(|price| estimate(&price))
                });
                let sender = self.sender.clone();
                glib::spawn_future_local(async move {
                    sender.input(AppMsg::Estimated(job.await.ok().flatten()));
                });
            }
            _ => {
                let estimate = anthropic_price(&self.config.model).map(|price| estimate(&price));
                self.confirm_run(estimate);
            }
        }
    }

    fn save_results(&self) {
        if let Err(e) = results::save(&results_path(), &self.results) {
            self.toast(&gettext_f(
                "The looked-up books could not be saved: {why}",
                &[("why", &e.to_string())],
            ));
        }
        self.review.show_list(&self.results);
        self.review_button.set_visible(!self.results.is_empty());
    }

    fn edit_value(&self, index: usize, key: &'static str, typed: Option<&str>) {
        let Some(book) = self.results.get(index) else {
            return;
        };
        let Some(field) = assistant_core::lookup::spec(key) else {
            return;
        };
        let entry = adw::EntryRow::builder()
            .title(gettext(field.label))
            .activates_default(true)
            .build();
        entry.set_text(
            typed.unwrap_or_else(|| book.draft.value(key).map_or("", |v| v.text.as_str())),
        );
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        list.append(&entry);
        let dialog = adw::AlertDialog::builder()
            .heading(gettext_f(
                "Edit {field}",
                &[("field", &gettext(field.label))],
            ))
            .body(gettext(field.hint))
            .extra_child(&list)
            .close_response("cancel")
            .default_response("save")
            .build();
        dialog.set_property("focus-widget", &entry);
        dialog.add_responses(&[("cancel", &gettext("_Cancel")), ("save", &gettext("_Save"))]);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        let sender = self.sender.clone();
        dialog.choose(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |response| {
                if response == "save" {
                    sender.input(AppMsg::Edited(index, key, entry.text().to_string()));
                }
            },
        );
    }

    fn confirm_discard(&self, index: usize) {
        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Discard This Book?"))
            .body(gettext(
                "Its looked-up details are deleted. Looking it up again costs as much again.",
            ))
            .close_response("cancel")
            .build();
        dialog.add_responses(&[
            ("cancel", &gettext("_Cancel")),
            ("discard", &gettext("_Discard")),
        ]);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        let sender = self.sender.clone();
        dialog.choose(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |response| {
                if response == "discard" {
                    sender.input(AppMsg::DiscardConfirmed(index));
                }
            },
        );
    }

    fn review_msg(&mut self, msg: ReviewMsg) {
        match msg {
            ReviewMsg::Open(i) => {
                if let Some(book) = self.results.get(i) {
                    self.review.show_book(i, book);
                }
            }
            ReviewMsg::Include(i, on) => {
                if let Some(book) = self.results.get_mut(i) {
                    book.included =
                        on && book.draft.identity == assistant_core::lookup::Identity::Identified;
                    self.save_results();
                }
            }
            ReviewMsg::AcceptValue(i, key, on) => {
                let value = self
                    .results
                    .get_mut(i)
                    .and_then(|b| b.draft.values.iter_mut().find(|v| v.key == key));
                if let Some(value) = value {
                    value.accepted = on && !value.text.is_empty();
                    self.save_results();
                }
            }
            ReviewMsg::AcceptEntry(i, list, n, on) => {
                let entry = self.results.get_mut(i).and_then(|b| {
                    let d = &mut b.draft;
                    match list {
                        List::Credits => d.credits.get_mut(n),
                        List::Characters => d.characters.get_mut(n),
                        List::Teams => d.teams.get_mut(n),
                        List::Arcs => d.story_arcs.get_mut(n),
                    }
                });
                if let Some(entry) = entry {
                    entry.accepted = on;
                    self.save_results();
                }
            }
            ReviewMsg::Edit(i, key) => self.edit_value(i, key, None),
            ReviewMsg::Discard(i) => self.confirm_discard(i),
            ReviewMsg::Choose(i, n) => {
                let chosen = self.results.get(i).and_then(|b| match &b.draft.identity {
                    assistant_core::lookup::Identity::Ambiguous(candidates) => {
                        candidates.get(n).map(|c| {
                            let query = assistant_core::lookup::choose(&b.draft.query, c);
                            // A book from photos is looked up from them
                            // again, so their reading and its edition come
                            // along.
                            match &b.front {
                                Some(front) => Book::photos(
                                    query.to_string(),
                                    front.clone(),
                                    b.back.clone(),
                                    Vec::new(),
                                    Some(query),
                                ),
                                None => Book::line(query),
                            }
                        })
                    }
                    _ => None,
                });
                if let Some(book) = chosen {
                    self.pending = vec![book];
                    self.replacing = Some(i);
                    self.ask_to_run();
                }
            }
        }
    }

    /// Tells the owner something in a dialog of its own, for what should
    /// not slip by as a toast.
    fn tell(&self, heading: &str, body: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body(body)
            .close_response("close")
            .build();
        dialog.add_response("close", &gettext("_Close"));
        dialog.present(Some(&self.window));
    }

    /// Asks where to save the included books as a MyLongboxDB export,
    /// once every one of them would import.
    fn export(&self) {
        let year = this_year();
        let included: Vec<&ReviewBook> = self.results.iter().filter(|b| b.included).collect();
        let mut refused = Vec::new();
        let mut books = Vec::new();
        for b in &included {
            let Some(book) = b.book(year) else {
                continue;
            };
            if let Some(problem) = book.problems().into_iter().next() {
                refused.push(format!("{}: {problem}", b.draft.query));
            }
            books.push(book);
        }
        if books.is_empty() {
            return;
        }
        if !refused.is_empty() {
            self.tell(
                &gettext("Some Books Would Not Import"),
                &gettext_f(
                    "Correct these books, or leave them out of the export:\n\n{books}",
                    &[("books", &refused.join("\n"))],
                ),
            );
            return;
        }
        let drafts: Vec<Draft> = included.iter().map(|b| b.draft.clone()).collect();
        let unticked = drafts.iter().map(Draft::unaccepted).sum();
        let name = glib::DateTime::now_local()
            .and_then(|d| d.format("%Y-%m-%d"))
            .map_or_else(
                |_| "mylongboxdb-assistant.zip".to_owned(),
                |date| format!("mylongboxdb-assistant-{date}.zip"),
            );
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Export"))
            .modal(true)
            .initial_name(name)
            .build();
        let sender = self.sender.clone();
        dialog.save(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |picked| {
                let file = match picked {
                    Ok(file) => file,
                    Err(e) if e.matches(gtk::DialogError::Dismissed) => return,
                    Err(e) => {
                        sender.input(AppMsg::Exported(Err(e.to_string()), Vec::new(), 0));
                        return;
                    }
                };
                glib::spawn_future_local(async move {
                    let written = gio::spawn_blocking(move || {
                        let mut out = std::io::Cursor::new(Vec::new());
                        write_export(&books, &mut out)
                            .map(|()| out.into_inner())
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .unwrap_or_else(|_| Err(gettext("the export could not be made")));
                    let result = match written {
                        Ok(bytes) => file
                            .replace_contents_future(
                                bytes,
                                None,
                                false,
                                gio::FileCreateFlags::REPLACE_DESTINATION,
                            )
                            .await
                            .map(|_| ())
                            .map_err(|(_, e)| e.to_string()),
                        Err(why) => Err(why),
                    };
                    sender.input(AppMsg::Exported(result, drafts, unticked));
                });
            },
        );
    }

    fn lookup_settings(&self) -> LookupSettings {
        LookupSettings::balanced(this_year())
    }

    /// Asks the owner to confirm a run at its estimated cost, and for the
    /// passphrase when the key is saved.
    fn confirm_run(&self, estimate: Option<Estimate>) {
        let count = self.pending.len();
        let cost = match estimate {
            Some(e) => gettext_f(
                "With {model}, this should cost about {likely}, and at most {most}, \
                 web searches included.",
                &[
                    ("model", &self.config.model),
                    ("likely", &dollars(e.likely)),
                    ("most", &dollars(e.most)),
                ],
            ),
            None => gettext_f(
                "The cost with {model} is not known in advance.",
                &[("model", &self.config.model)],
            ),
        };
        let saved = self.session.is_saved();
        let body = if saved {
            gettext_f("{cost} Type your passphrase to start.", &[("cost", &cost)])
        } else {
            cost
        };
        let dialog = adw::AlertDialog::builder()
            .heading(if count == 1 {
                gettext("Look Up 1 Book?")
            } else {
                gettext_f("Look Up {count} Books?", &[("count", &count.to_string())])
            })
            .body(body)
            .close_response("cancel")
            .default_response("go")
            .build();
        dialog.add_responses(&[
            ("cancel", &gettext("_Cancel")),
            ("go", &gettext("_Look Up")),
        ]);
        dialog.set_response_appearance("go", adw::ResponseAppearance::Suggested);
        let entry = adw::PasswordEntryRow::builder()
            .title(gettext("Passphrase"))
            .activates_default(true)
            .build();
        if saved {
            let list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .build();
            list.append(&entry);
            dialog.set_extra_child(Some(&list));
            dialog.set_property("focus-widget", &entry);
            dialog.set_response_enabled("go", false);
            let weak = dialog.downgrade();
            entry.connect_changed(move |e| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.set_response_enabled("go", !e.text().is_empty());
                }
            });
        }
        let sender = self.sender.clone();
        dialog.choose(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |response| {
                let text = entry.text().to_string();
                entry.set_text("");
                if response != "go" {
                    return;
                }
                sender.input(AppMsg::Confirmed(if saved {
                    Passphrase::new(text)
                } else {
                    None
                }));
            },
        );
    }

    /// Starts looking up the pending books with `keys`, on threads of
    /// their own; their progress comes back as messages.
    fn start_run(&mut self, keys: &ApiKeys) {
        let Some(provider) = self.config.provider else {
            return;
        };
        let key: Option<ApiKey> = match provider {
            ProviderChoice::Anthropic => keys.anthropic.clone(),
            ProviderChoice::OpenRouter => keys.openrouter.clone(),
        };
        let Some(key) = key else {
            self.toast(&gettext(
                "There is no key for this provider; set the app up again.",
            ));
            return;
        };
        let books = std::mem::take(&mut self.pending);
        let queries: Vec<Item> = books.iter().map(|b| b.item.clone()).collect();
        let mut run = Run::new(books);
        run.replaces = self.replacing.take();
        let cancel = run.cancel.clone();
        self.run = Some(run);
        self.view = View::Run;
        self.show_work();
        let options = RunOptions {
            batch: self.config.batch,
            ..RunOptions::new(self.lookup_settings())
        };
        let client = self.client.clone();
        let model = self.config.model.clone();
        let input = self.sender.input_sender().clone();
        gio::spawn_blocking(move || {
            let emit = |event: Event| input.emit(AppMsg::RunEvent(event));
            let summary = match provider {
                ProviderChoice::Anthropic => {
                    let provider = AnthropicLookup {
                        api: Anthropic::new(&client, &key),
                        model,
                    };
                    queue::run(&provider, &queries, &options, &cancel, &emit)
                }
                ProviderChoice::OpenRouter => {
                    // Prices are not needed: OpenRouter reports what each
                    // request cost.
                    let provider = OpenRouterLookup {
                        api: OpenRouter::new(&client, &key),
                        model: ModelInfo {
                            name: model.clone(),
                            id: model,
                            image_input: true,
                            structured_outputs: true,
                            tools: true,
                            prompt_price: None,
                            completion_price: None,
                            image_price: None,
                            web_search_price: None,
                        },
                    };
                    queue::run(&provider, &queries, &options, &cancel, &emit)
                }
            };
            if let Ok(summary) = summary {
                input.emit(AppMsg::RunDone(summary));
            }
        });
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
        let config = Config::load(&config::path());
        let add = AddPage::new(&header, config.batch, this_year());
        let run_header = adw::HeaderBar::new();
        run_header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(&menu)
                .tooltip_text(gettext("Main Menu"))
                .build(),
        );
        let run_page = RunPage::new(&run_header);
        let review = {
            let sender = sender.clone();
            ReviewPage::new(&menu_button(&menu), move |msg| {
                sender.input(AppMsg::Review(msg))
            })
        };
        let review_button = gtk::Button::builder()
            .label(gettext("_Review"))
            .use_underline(true)
            .build();
        header.pack_start(&review_button);

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&setup_holder, Some("setup"));
        stack.add_named(&unlock.root, Some("unlock"));
        stack.add_named(&add.root, Some("add"));
        stack.add_named(&run_page.root, Some("run"));
        stack.add_named(&review.root, Some("review"));
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

        {
            let sender = sender.clone();
            add.look_up
                .connect_clicked(move |_| sender.input(AppMsg::LookUp));
        }
        {
            let sender = sender.clone();
            add.batch.connect_value_notify(move |row| {
                sender.input(AppMsg::Batch(
                    usize::try_from(row.value() as i64).unwrap_or(1),
                ));
            });
        }
        {
            let sender = sender.clone();
            run_page
                .cancel
                .connect_clicked(move |_| sender.input(AppMsg::CancelRun));
        }
        {
            let sender = sender.clone();
            run_page
                .again
                .connect_clicked(move |_| sender.input(AppMsg::LookUpMore));
        }
        {
            let sender = sender.clone();
            run_page
                .retry
                .connect_clicked(move |_| sender.input(AppMsg::TryAgain));
        }
        {
            let sender = sender.clone();
            review
                .export
                .connect_clicked(move |_| sender.input(AppMsg::Export));
        }
        for button in [&run_page.review, &review_button] {
            let sender = sender.clone();
            button.connect_clicked(move |_| sender.input(AppMsg::ShowReview));
        }
        {
            let sender = sender.clone();
            review
                .add_books
                .connect_clicked(move |_| sender.input(AppMsg::ShowAdd));
        }
        // Results that cannot be read are set aside rather than lost, and
        // rather than overwritten by the next save.
        let (results, unreadable) = match results::load(&results_path(), this_year()) {
            Ok(results) => (results, None),
            Err(why) => {
                let _ = std::fs::rename(
                    results_path(),
                    results_path().with_extension("unreadable.json"),
                );
                (Vec::new(), Some(why))
            }
        };
        review_button.set_visible(!results.is_empty());
        let model = Self {
            session: Session::locked(config::key_file().exists(), Instant::now()),
            config,
            client: Client::new(),
            window,
            stack,
            setup_holder,
            unlock,
            add,
            run_page,
            run: None,
            review,
            review_button,
            results,
            view: View::Add,
            pending: Vec::new(),
            replacing: None,
            toasts,
            sender,
        };
        model.action("lock", || AppMsg::Lock);
        model.action("passphrase", || AppMsg::ChangePassphrase);
        model.action("setup", || AppMsg::SetUpAgain);
        model.action("forget", || AppMsg::Forget);
        model.show_start();
        model.review.show_list(&model.results);
        if let Some(why) = unreadable {
            model.toast(&gettext_f(
                "The saved looked-up books could not be read and were set aside: {why}",
                &[("why", &why)],
            ));
        }
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
                    batch: self.config.batch,
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
                self.show_work();
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
                    self.show_work();
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
            AppMsg::Batch(batch) => {
                self.config.batch = batch;
                let _ = self.config.save(&config::path());
            }
            AppMsg::LookUp => {
                if !self.add.ready() {
                    return;
                }
                let (lines, _) = self.add.parsed(this_year());
                let mut books: Vec<Book> = lines.into_iter().map(Book::line).collect();
                books.extend(self.add.photo_books().into_iter().map(|b| {
                    let mut sizes = vec![b.front.size];
                    sizes.extend(b.back.as_ref().map(|p| p.size));
                    Book::photos(
                        b.label(),
                        Photo::new(b.front.jpeg.clone()),
                        b.back.as_ref().map(|p| Photo::new(p.jpeg.clone())),
                        sizes,
                        None,
                    )
                }));
                if books.len() > self.config.batch {
                    return;
                }
                self.pending = books;
                self.replacing = None;
                self.ask_to_run();
            }
            AppMsg::Estimated(estimate) => self.confirm_run(estimate),
            AppMsg::Confirmed(Some(passphrase)) => {
                let sender = sender.clone();
                open_key_file(config::key_file(), passphrase, move |opened| {
                    sender.input(AppMsg::RunKeys(opened));
                });
            }
            AppMsg::Confirmed(None) => {
                if let Some(keys) = self.session.keys().cloned() {
                    self.start_run(&keys);
                }
            }
            AppMsg::RunKeys(Ok(keys)) => {
                self.session.unlock(keys.clone(), true, Instant::now());
                self.start_run(&keys);
            }
            AppMsg::RunKeys(Err(e)) => {
                self.toast(&error_text(&e));
            }
            AppMsg::RunEvent(event) => {
                if let Some(run) = &mut self.run {
                    run.apply(event);
                    self.run_page.show(run);
                }
                // Each looked-up book is kept as soon as it is done; a
                // picked candidate's takes its unsettled entry's place.
                let (found, replaces) = self.run.as_ref().map_or((Vec::new(), None), |run| {
                    let found = run
                        .status
                        .iter()
                        .zip(&run.books)
                        .filter_map(|(s, book)| match s {
                            Status::Done(draft) => {
                                let draft = draft.as_ref().clone();
                                Some(match &book.photos {
                                    Some((front, back)) => {
                                        ReviewBook::with_photos(draft, front.clone(), back.clone())
                                    }
                                    None => ReviewBook::new(draft),
                                })
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    (found, run.replaces)
                });
                for book in found {
                    if self.results.iter().any(|b| b.draft == book.draft) {
                        continue;
                    }
                    match replaces.filter(|i| *i < self.results.len()) {
                        Some(i) => self.results[i] = book,
                        None => self.results.push(book),
                    }
                    self.save_results();
                }
            }
            AppMsg::RunDone(summary) => {
                if let Some(run) = &mut self.run {
                    run.finish(&summary);
                    self.run_page.show(run);
                }
            }
            AppMsg::CancelRun => {
                if let Some(run) = &self.run {
                    run.cancel();
                    self.run_page.show(run);
                }
            }
            AppMsg::LookUpMore => {
                if self.run.as_ref().is_some_and(|r| r.finished) {
                    self.run = None;
                    self.add.clear();
                    self.view = View::Add;
                    self.show_work();
                }
            }
            AppMsg::ShowReview => {
                if self.run.as_ref().is_some_and(|r| r.finished) {
                    self.run = None;
                    self.add.clear();
                }
                self.view = View::Review;
                self.review.close_book();
                self.show_work();
            }
            AppMsg::ShowAdd => {
                self.view = if self.run.is_some() {
                    View::Run
                } else {
                    View::Add
                };
                self.show_work();
            }
            AppMsg::Review(msg) => self.review_msg(msg),
            AppMsg::Edited(i, key, text) => {
                let Some(book) = self.results.get_mut(i) else {
                    return;
                };
                match book.draft.set_value(key, &text, this_year()) {
                    Ok(()) => {
                        self.save_results();
                        if let Some(book) = self.results.get(i) {
                            self.review.refresh_book(i, book);
                        }
                    }
                    Err(why) => {
                        self.toast(&why);
                        // Asked again with what was typed, to correct it.
                        self.edit_value(i, key, Some(&text));
                    }
                }
            }
            AppMsg::TryAgain => {
                let others = self
                    .run
                    .as_ref()
                    .filter(|r| r.finished)
                    .map(Run::unfinished)
                    .unwrap_or_default();
                if !others.is_empty() {
                    self.pending = others;
                    self.replacing = None;
                    self.ask_to_run();
                }
            }
            AppMsg::Export => self.export(),
            AppMsg::Exported(Err(why), ..) => self.tell(
                &gettext("Could Not Export"),
                &gettext_f(
                    "Nothing was exported, and the books are still here: {why}",
                    &[("why", &why)],
                ),
            ),
            AppMsg::Exported(Ok(()), drafts, unticked) => {
                // Exported books are no longer kept; matched by content,
                // since the list may have changed while the file was saved.
                self.results
                    .retain(|b| !(b.included && drafts.contains(&b.draft)));
                self.save_results();
                self.review.close_book();
                if self.results.is_empty() {
                    self.view = View::Add;
                }
                self.show_work();
                let exported = if drafts.len() == 1 {
                    gettext("1 book was exported.")
                } else {
                    gettext_f(
                        "{count} books were exported.",
                        &[("count", &drafts.len().to_string())],
                    )
                };
                let mut body = gettext_f(
                    "{exported} In MyLongboxDB, choose Import Books and pick the file; \
                     it shows what will be added before anything is.",
                    &[("exported", &exported)],
                );
                if unticked > 0 {
                    body.push_str("\n\n");
                    body.push_str(&gettext_f(
                        "{unticked} values you did not tick were left out.",
                        &[("unticked", &unticked.to_string())],
                    ));
                }
                self.tell(&gettext("Export Saved"), &body);
            }
            AppMsg::DiscardConfirmed(i) => {
                if i < self.results.len() {
                    self.results.remove(i);
                    self.review.close_book();
                    self.save_results();
                    if self.results.is_empty() {
                        self.view = View::Add;
                        self.show_work();
                    }
                }
            }
            AppMsg::Forgotten => {
                let _ = std::fs::remove_file(config::key_file());
                self.session = Session::locked(false, Instant::now());
                self.show_setup();
            }
        }
    }
}
