//! The forms series names and issue numbers are matched by, so a title
//! typed or read from a cover finds the series the catalogue files it
//! under, whatever its case, punctuation, or leading The.

/// A series name reduced to what tells series apart: lower case, & read
/// as and, punctuation as spaces, and without a leading The.
pub fn series_key(name: &str) -> String {
    let lower = name.to_lowercase().replace('&', " and ");
    let spaced: String = lower
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let mut words: Vec<&str> = spaced.split_whitespace().collect();
    if words.len() > 1 && words[0] == "the" {
        words.remove(0);
    }
    words.join(" ")
}

/// An issue number as the catalogue and a cover may both write it: lower
/// case, without # or No., and without the brackets the catalogue puts
/// round a number the issue does not print, as in [nn] or [3].
pub fn number_key(number: &str) -> String {
    let mut text = number.trim().to_lowercase().replace('½', "1/2");
    if let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        text = inner.to_owned();
    }
    for mark in ["#", "no.", "no "] {
        if let Some(rest) = text.strip_prefix(mark) {
            text = rest.to_owned();
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_match_whatever_their_case_and_punctuation() {
        assert_eq!(
            series_key("The Amazing Spider-Man"),
            series_key("amazing spider man")
        );
        assert_eq!(series_key("Batman & Robin"), "batman and robin");
        assert_eq!(
            series_key("Batman/Spawn: War Devil"),
            "batman spawn war devil"
        );
        assert_eq!(series_key("Kraven's Last Hunt"), "kraven s last hunt");
        // A title that is only The keeps it.
        assert_eq!(series_key("The"), "the");
        assert_eq!(series_key("Les Robots Étonnants"), "les robots étonnants");
    }

    #[test]
    fn numbers_match_as_printed_or_bracketed() {
        assert_eq!(number_key("[nn]"), "nn");
        assert_eq!(number_key("[3]"), "3");
        assert_eq!(number_key("#300"), "300");
        assert_eq!(number_key("No. 7"), "7");
        assert_eq!(number_key("½"), "1/2");
        assert_eq!(number_key(" Annual  3 "), "annual 3");
    }
}
