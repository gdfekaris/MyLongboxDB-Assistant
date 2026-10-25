//! Setting the app up: the provider, its API key, the model, and whether
//! to save the key under a passphrase. Shown on first run and from
//! Settings. The key is checked by listing the provider's models with it,
//! which also offers the models the app can use.

use std::rc::Rc;
use std::sync::atomic::AtomicBool;

use adw::prelude::*;
use assistant_core::dollars;
use assistant_core::lookup::{Settings as LookupSettings, estimate_book};
use assistant_keys::{ApiKey, Passphrase};
use assistant_net::anthropic::{Anthropic, DEFAULT_MODEL};
use assistant_net::openrouter::OpenRouter;
use assistant_net::{Client, NetError};
use gtk::{gio, glib};

use crate::config::ProviderChoice;
use crate::i18n::{gettext, gettext_f};
use crate::session::passphrase_problem;

/// What setting up decided.
pub struct SetupDone {
    pub provider: ProviderChoice,
    pub model: String,
    pub key: ApiKey,
    /// `None` to keep the key only until the app closes.
    pub passphrase: Option<Passphrase>,
}

/// A model the app can use, as offered.
#[derive(Clone)]
struct Offered {
    id: String,
    name: String,
    /// What one book should cost, if the model's price is known.
    per_book: Option<String>,
}

/// The models a key can use that take photos, keep to a schema, and can
/// search the web: what a lookup needs.
fn usable_models(
    client: &Client,
    provider: ProviderChoice,
    key: &ApiKey,
    this_year: u16,
) -> Result<Vec<Offered>, NetError> {
    let cancel = AtomicBool::new(false);
    let settings = LookupSettings::balanced(this_year);
    let per_book = |price: Option<assistant_core::Price>| {
        price.map(|p| dollars(estimate_book(&p, &settings).likely))
    };
    Ok(match provider {
        ProviderChoice::Anthropic => Anthropic::new(client, key)
            .models(&cancel)?
            .into_iter()
            .filter(|m| m.image_input && m.structured_outputs && m.web_search)
            .map(|m| Offered {
                per_book: per_book(assistant_core::anthropic_price(&m.id)),
                id: m.id,
                name: m.display_name,
            })
            .collect(),
        ProviderChoice::OpenRouter => OpenRouter::new(client, key)
            .models(&cancel)?
            .into_iter()
            .filter(|m| m.image_input && m.structured_outputs && m.tools)
            .map(|m| Offered {
                per_book: per_book(m.price()),
                id: m.id,
                name: m.name,
            })
            .collect(),
    })
}

fn page(title: &str, content: &impl IsA<gtk::Widget>) -> adw::NavigationPage {
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    let clamp = adw::Clamp::builder()
        .maximum_size(560)
        .child(content)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(12)
        .margin_end(12)
        .build();
    toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build(),
    ));
    adw::NavigationPage::new(&toolbar, title)
}

fn column() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .build()
}

fn paragraph(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .xalign(0.0)
        .build()
}

fn next_button(label: &str) -> gtk::Button {
    gtk::Button::builder()
        .label(label)
        .halign(gtk::Align::Center)
        .css_classes(["pill", "suggested-action"])
        .build()
}

/// What the owner is told before anything is sent anywhere.
fn notice() -> String {
    gettext(
        "MyLongboxDB Assistant sends the books you add to the AI provider you choose: \
         each cover photo, re-encoded so it carries no location or camera details, and \
         the lines you type, with the app's own instructions. If you choose OpenRouter, \
         it passes them on to the company running the model. Your MyLongboxDB \
         collection, your file names, and anything about your computer are never sent.",
    )
}

fn advice(provider: ProviderChoice) -> String {
    match provider {
        ProviderChoice::Anthropic => gettext(
            "Use a key made just for this app, in a workspace with a monthly spend limit \
             set in the Claude Console, and revoke it at the first doubt. The key is \
             checked by listing the models it can use.",
        ),
        ProviderChoice::OpenRouter => gettext(
            "Use a key made just for this app, with a credit limit set on the key in \
             OpenRouter's settings, and revoke it at the first doubt. The key is checked \
             by listing the models it can use.",
        ),
    }
}

/// The setup steps, in a navigation view; `done` gets their result.
pub fn build(
    client: Client,
    this_year: u16,
    done: impl Fn(SetupDone) + 'static,
) -> adw::NavigationView {
    let done = Rc::new(done);
    let nav = adw::NavigationView::new();

    // 1. Provider, after what is sent where.
    let first = column();
    first.append(&paragraph(&notice()));
    let group = adw::PreferencesGroup::builder()
        .title(gettext("Provider"))
        .build();
    let anthropic = gtk::CheckButton::new();
    anthropic.set_active(true);
    let openrouter = gtk::CheckButton::new();
    openrouter.set_group(Some(&anthropic));
    for (check, title, subtitle) in [
        (
            &anthropic,
            gettext("Anthropic"),
            gettext("Claude models, through Anthropic's own API"),
        ),
        (
            &openrouter,
            gettext("OpenRouter"),
            gettext("Many companies' models, through one API"),
        ),
    ] {
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(subtitle)
            .activatable_widget(check)
            .build();
        row.add_prefix(check);
        group.add(&row);
    }
    first.append(&group);
    let to_key = next_button(&gettext("_Next"));
    to_key.set_use_underline(true);
    first.append(&to_key);
    nav.add(&page(&gettext("Set Up"), &first));

    let chosen = {
        let openrouter = openrouter.clone();
        move || {
            if openrouter.is_active() {
                ProviderChoice::OpenRouter
            } else {
                ProviderChoice::Anthropic
            }
        }
    };

    {
        let nav = nav.clone();
        let done = done.clone();
        let client = client.clone();
        to_key.connect_clicked(move |_| {
            let provider = chosen();
            nav.push(&key_page(&nav, &client, provider, this_year, done.clone()));
        });
    }
    nav
}

/// 2. The key, checked by listing models with it.
fn key_page(
    nav: &adw::NavigationView,
    client: &Client,
    provider: ProviderChoice,
    this_year: u16,
    done: Rc<dyn Fn(SetupDone)>,
) -> adw::NavigationPage {
    let content = column();
    content.append(&paragraph(&advice(provider)));
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let entry = adw::PasswordEntryRow::builder()
        .title(gettext("API key"))
        .build();
    list.append(&entry);
    content.append(&list);
    let problem = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .css_classes(["error"])
        .visible(false)
        .build();
    content.append(&problem);
    let spinner = adw::Spinner::builder().visible(false).build();
    content.append(&spinner);
    let check = next_button(&gettext("_Check Key"));
    check.set_use_underline(true);
    content.append(&check);

    let nav = nav.clone();
    let client = client.clone();
    check.connect_clicked(move |button| {
        let Some(key) = ApiKey::new(entry.text().to_string()) else {
            problem.set_label(&gettext("That does not look like an API key."));
            problem.set_visible(true);
            return;
        };
        problem.set_visible(false);
        spinner.set_visible(true);
        button.set_sensitive(false);
        let job = {
            let client = client.clone();
            let key = key.clone();
            gio::spawn_blocking(move || usable_models(&client, provider, &key, this_year))
        };
        let (nav, button, spinner, problem, done, entry) = (
            nav.clone(),
            button.clone(),
            spinner.clone(),
            problem.clone(),
            done.clone(),
            entry.clone(),
        );
        glib::spawn_future_local(async move {
            let models = job.await;
            spinner.set_visible(false);
            button.set_sensitive(true);
            match models {
                Ok(Ok(models)) if !models.is_empty() => {
                    // Kept in the entry only until it is known to work,
                    // so a mistyped key can be corrected.
                    entry.set_text("");
                    nav.push(&model_page(&nav, provider, key, models, done));
                }
                Ok(Ok(_)) => {
                    problem.set_label(&gettext(
                        "The key works, but none of its models can read photos, keep to a \
                         schema, and search the web, which the app needs.",
                    ));
                    problem.set_visible(true);
                }
                Ok(Err(NetError::Status {
                    status: 401 | 403, ..
                })) => {
                    problem.set_label(&gettext(
                        "The provider did not accept this key. Check that it was copied \
                         whole and has not been revoked.",
                    ));
                    problem.set_visible(true);
                }
                Ok(Err(e)) => {
                    problem.set_label(&gettext_f(
                        "The key could not be checked: {why}",
                        &[("why", &e.to_string())],
                    ));
                    problem.set_visible(true);
                }
                Err(_) => {
                    problem.set_label(&gettext("The key could not be checked."));
                    problem.set_visible(true);
                }
            }
        });
    });
    page(&gettext("API Key"), &content)
}

/// 3. The model, with what a book should cost on each.
fn model_page(
    nav: &adw::NavigationView,
    provider: ProviderChoice,
    key: ApiKey,
    models: Vec<Offered>,
    done: Rc<dyn Fn(SetupDone)>,
) -> adw::NavigationPage {
    let content = column();
    content.append(&paragraph(&gettext(
        "Each book takes three requests: one that searches the web, one that turns what \
         it found into fields, and one that checks them. The estimates are first guesses.",
    )));
    let names: Vec<String> = models
        .iter()
        .map(|m| match &m.per_book {
            Some(cost) => gettext_f(
                "{model} — about {cost} a book",
                &[("model", &m.name), ("cost", cost)],
            ),
            None => gettext_f("{model} — cost unknown", &[("model", &m.name)]),
        })
        .collect();
    let strings = gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let combo = adw::ComboRow::builder()
        .title(gettext("Model"))
        .model(&strings)
        .build();
    // Model names come from the provider.
    combo.set_use_markup(false);
    let preferred = models
        .iter()
        .position(|m| m.id == DEFAULT_MODEL || m.id.ends_with("claude-opus-5.5"))
        .unwrap_or(0);
    combo.set_selected(u32::try_from(preferred).unwrap_or(0));
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    list.append(&combo);
    content.append(&list);
    let next = next_button(&gettext("_Next"));
    next.set_use_underline(true);
    content.append(&next);
    let nav2 = nav.clone();
    next.connect_clicked(move |_| {
        let index = usize::try_from(combo.selected()).unwrap_or(0);
        let Some(model) = models.get(index) else {
            return;
        };
        nav2.push(&save_page(
            provider,
            model.id.clone(),
            key.clone(),
            done.clone(),
        ));
    });
    page(&gettext("Model"), &content)
}

/// 4. Saving the key under a passphrase, or not at all.
fn save_page(
    provider: ProviderChoice,
    model: String,
    key: ApiKey,
    done: Rc<dyn Fn(SetupDone)>,
) -> adw::NavigationPage {
    let content = column();
    content.append(&paragraph(&gettext(
        "Saved keys are encrypted with a passphrase, which you type when the app starts, \
         again before each run, and after a quarter of an hour unused. Without saving, \
         the key is kept only until the app closes or locks.",
    )));
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let save = adw::SwitchRow::builder()
        .title(gettext("Save the key"))
        .active(true)
        .build();
    let first = adw::PasswordEntryRow::builder()
        .title(gettext("Passphrase"))
        .build();
    let second = adw::PasswordEntryRow::builder()
        .title(gettext("Passphrase again"))
        .build();
    list.append(&save);
    list.append(&first);
    list.append(&second);
    content.append(&list);
    save.bind_property("active", &first, "visible")
        .sync_create()
        .build();
    save.bind_property("active", &second, "visible")
        .sync_create()
        .build();
    let problem = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .css_classes(["error"])
        .visible(false)
        .build();
    content.append(&problem);
    let finish = next_button(&gettext("_Finish"));
    finish.set_use_underline(true);
    content.append(&finish);
    finish.connect_clicked(move |_| {
        let passphrase = if save.is_active() {
            let (a, b) = (first.text().to_string(), second.text().to_string());
            if let Some(why) = passphrase_problem(&a) {
                problem.set_label(&why);
                problem.set_visible(true);
                return;
            }
            if a != b {
                problem.set_label(&gettext("The two passphrases differ."));
                problem.set_visible(true);
                return;
            }
            first.set_text("");
            second.set_text("");
            Passphrase::new(a)
        } else {
            None
        };
        done(SetupDone {
            provider,
            model: model.clone(),
            key: key.clone(),
            passphrase,
        });
    });
    page(&gettext("Save"), &content)
}
