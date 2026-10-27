//! The About window: which version of the app is running, and what copy
//! of the Grand Comics Database lookups check, if any.

use adw::prelude::*;
use assistant_gcd::About as Copy;
use gtk::glib;

use crate::i18n::{gettext, gettext_f};

/// Where the source code is kept. Shown as text rather than a link, since
/// the app hands nothing to other programs, a web browser included.
const REPOSITORY: &str = "https://github.com/gdfekaris/MyLongboxDB-Assistant";

/// The paragraphs below the version: what the app does, the owner's GCD
/// copy and the data's license, and where the source is.
fn comments(copy: Option<&Copy>) -> Vec<String> {
    let mut comments = vec![gettext(
        "Looks up your comic books from cover photos or typed lines with a Claude model, \
         using your own API key, and writes them as an export for MyLongboxDB to import.",
    )];
    if let Some(copy) = copy {
        comments.push(gettext_f(
            "Lookups check your copy of the Grand Comics Database. {copy} Its data is the \
             Grand Comics Database's, shared under the CC BY-SA 4.0 license.",
            &[("copy", &crate::catalogue::described(copy))],
        ));
    }
    comments.push(gettext_f(
        "Source code: {address}",
        &[("address", REPOSITORY)],
    ));
    comments
}

pub fn show(parent: &impl IsA<gtk::Widget>, copy: Option<Copy>) {
    // The comments are read as markup; none of this text is meant as any.
    let comments: Vec<String> = comments(copy.as_ref())
        .iter()
        .map(|c| glib::markup_escape_text(c).to_string())
        .collect();
    let dialog = adw::AboutDialog::builder()
        .application_name("MyLongboxDB Assistant")
        .application_icon(crate::APP_ID)
        .developer_name("gdfekaris")
        .version(env!("CARGO_PKG_VERSION"))
        .copyright("© 2026 gdfekaris")
        .license_type(gtk::License::MitX11)
        .comments(comments.join("\n\n"))
        .build();
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gcd_copy_is_named_with_its_license_only_when_added() {
        let without = comments(None);
        assert_eq!(without.len(), 2);
        assert!(without[1].ends_with(REPOSITORY));
        let copy = Copy {
            dated: "2026-10-16".parse().ok(),
            series: 233_635,
            issues: 2_622_121,
        };
        let with = comments(Some(&copy));
        assert_eq!(with.len(), 3);
        assert!(with[1].contains("dated 2026-10-16"), "{}", with[1]);
        assert!(with[1].contains("CC BY-SA 4.0"));
    }

    #[test]
    fn the_version_shown_is_the_metainfo_s_latest_release() {
        let metainfo =
            include_str!("../../../data/io.github.gdfekaris.MyLongboxDB.Assistant.metainfo.xml");
        let latest = metainfo
            .split("<release version=\"")
            .nth(1)
            .and_then(|r| r.split('"').next());
        assert_eq!(latest, Some(env!("CARGO_PKG_VERSION")));
    }
}
