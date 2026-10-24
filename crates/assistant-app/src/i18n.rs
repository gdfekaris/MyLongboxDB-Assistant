//! Text shown to people, in their own language when a translation for
//! it is installed. The same functions as MyLongboxDB's.
//!
//! Every message goes through one of these functions, so the template
//! for translators can be made with `xgettext`. Messages with values in
//! them name each one in braces, such as `Looked up {count} books`, so a
//! translation can put them in whatever order its language needs.

use gtk::glib;

/// The app's translation domain, which names its catalogues.
pub const DOMAIN: &str = "mylongboxdb-assistant";

/// `msgid` in the reader's language.
pub fn gettext(msgid: &str) -> String {
    glib::dgettext(None, msgid).into()
}

/// `template` with each `{name}` replaced by its value. A name not
/// given is left as it is, so a slip in a translation shows rather than
/// losing text.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}').and_then(|close| {
            let name = &after[..close];
            values
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, value)| (close, *value))
        }) {
            Some((close, value)) => {
                out.push_str(value);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// [`gettext`], then [`fill`].
pub fn gettext_f(msgid: &str, values: &[(&str, &str)]) -> String {
    fill(&gettext(msgid), values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_fill_their_places_in_any_order() {
        assert_eq!(
            fill(
                "Looked up {count} books with {model}",
                &[("model", "Claude"), ("count", "3")]
            ),
            "Looked up 3 books with Claude"
        );
        assert_eq!(fill("{a}{a}", &[("a", "x")]), "xx");
        // Unknown names and stray braces are kept, values are not
        // filled a second time.
        assert_eq!(fill("{nope} {", &[("a", "x")]), "{nope} {");
        assert_eq!(fill("{a}", &[("a", "{a}")]), "{a}");
        assert_eq!(fill("no values", &[]), "no values");
    }

    #[test]
    fn untranslated_messages_come_back_unchanged() {
        assert_eq!(gettext("Unlock"), "Unlock");
        assert_eq!(gettext_f("{n} books", &[("n", "5")]), "5 books");
    }
}
