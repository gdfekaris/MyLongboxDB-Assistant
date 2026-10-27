//! MyLongboxDB Assistant desktop application.

#![forbid(unsafe_code)]

mod about;
mod add;
mod app;
mod catalogue;
mod config;
mod format;
mod i18n;
mod photo_editor;
mod photos;
mod review;
mod run;
mod session;
mod setup;
mod unlock;

const APP_ID: &str = "io.github.gdfekaris.MyLongboxDB.Assistant";

fn main() {
    use_translations();
    let protections = restrict_self();
    relm4::RelmApp::new(APP_ID).run::<app::App>(protections);
}

/// Points gettext at the translations installed beside the app, in
/// `share/locale` next to the `bin` folder that holds it: `/app` in the
/// Flatpak. Without them the app is in English.
fn use_translations() {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.join("share/locale")));
    if let Some(dir) = dir
        && let Err(why) = assistant_sandbox::locale::use_translations(i18n::DOMAIN, &dir)
    {
        eprintln!("mylongboxdb-assistant: {why}");
    }
}

/// Locks the app down before GTK or anything else starts a thread or
/// opens a file or socket, since threads already running would escape
/// the file limits. From then on it changes files only in its own
/// folders and connects over TCP only to port 443. The Flatpak sandbox
/// already keeps it from the owner's files, so if an unusual kernel
/// refuses a restriction, the app says so and carries on.
fn restrict_self() -> app::Protections {
    use gtk::glib;
    let (data, cache, config) = (
        glib::user_data_dir(),
        glib::user_cache_dir(),
        glib::user_config_dir(),
    );
    for dir in [&data, &cache, &config] {
        let _ = std::fs::create_dir_all(dir);
    }
    // Files saved through the file chooser are reached through the
    // document portal's folder; the graphics driver's devices live under
    // /dev.
    let portal = glib::user_runtime_dir().join("doc");
    let devices = std::path::Path::new("/dev");
    let files = match assistant_sandbox::limit_access(&[&data, &cache, &config, &portal, devices]) {
        Ok(level) => {
            if level != assistant_sandbox::Enforced::Fully {
                eprintln!("mylongboxdb-assistant: access limits enforced: {level:?}");
            }
            level
        }
        Err(why) => {
            eprintln!("mylongboxdb-assistant: could not limit access: {why}");
            assistant_sandbox::Enforced::Not
        }
    };
    let sockets = match assistant_sandbox::limit_sockets() {
        Ok(()) => true,
        Err(why) => {
            eprintln!("mylongboxdb-assistant: could not limit sockets: {why}");
            false
        }
    };
    app::Protections { files, sockets }
}
