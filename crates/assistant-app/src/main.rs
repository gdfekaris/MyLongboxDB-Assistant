//! MyLongboxDB Assistant desktop application.

#![forbid(unsafe_code)]

mod app;
mod config;
mod i18n;
mod session;
mod setup;
mod unlock;

const APP_ID: &str = "io.github.gdfekaris.MyLongboxDB.Assistant";

fn main() {
    relm4::RelmApp::new(APP_ID).run::<app::App>(());
}
