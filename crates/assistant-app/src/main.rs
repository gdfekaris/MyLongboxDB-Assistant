//! MyLongboxDB Assistant desktop application.

#![forbid(unsafe_code)]

use adw::prelude::*;
use relm4::prelude::*;

const APP_ID: &str = "io.github.gdfekaris.MyLongboxDB.Assistant";

struct App;

#[relm4::component]
impl SimpleComponent for App {
    type Init = ();
    type Input = ();
    type Output = ();

    view! {
        adw::ApplicationWindow {
            set_title: Some("MyLongboxDB Assistant"),
            set_default_size: (720, 560),

            adw::ToolbarView {
                add_top_bar = &adw::HeaderBar {},

                #[wrap(Some)]
                set_content = &adw::StatusPage {
                    set_title: "MyLongboxDB Assistant",
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = App;
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }
}

fn main() {
    relm4::RelmApp::new(APP_ID).run::<App>(());
}
