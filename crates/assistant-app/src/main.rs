//! MyLongboxDB Assistant desktop application.

#![forbid(unsafe_code)]

mod add;
mod app;
mod config;
mod i18n;
mod run;
mod session;
mod setup;
mod unlock;

const APP_ID: &str = "io.github.gdfekaris.MyLongboxDB.Assistant";

fn main() {
    relm4::RelmApp::new(APP_ID).run::<app::App>(());
}
