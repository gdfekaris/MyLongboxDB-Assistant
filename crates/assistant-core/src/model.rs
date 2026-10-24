//! A book as it goes into the export: every field MyLongboxDB keeps for a
//! series, an issue, and an edition, each already checked by the rules
//! MyLongboxDB's import applies. Copy-level fields, such as grade, price
//! paid, and storage box, are the owner's to add in MyLongboxDB and are
//! not here.

use std::fmt;

use crate::codes::{Barcode, Isbn};
use crate::collects::CollectsEntry;
use crate::date::{CoverDate, Date, PartialDate};
use crate::issue_number::IssueNumber;

macro_rules! code_enum {
    ($(#[$meta:meta])* pub enum $name:ident {
        $($(#[$vmeta:meta])* $variant:ident = $code:literal,)+
    }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// The code MyLongboxDB writes and reads in its CSV.
            pub const fn code(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)+
                }
            }

            pub fn from_code(code: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|v| v.code() == code)
            }
        }
    };
}

code_enum! {
    /// What kind of publication a series is.
    pub enum Category {
        Comic = "comic",
        Magazine = "magazine",
        CollectedEdition = "collected",
    }
}

code_enum! {
    pub enum MagazineFormat {
        Standard = "standard",
        Treasury = "treasury",
        Digest = "digest",
    }
}

code_enum! {
    pub enum Distribution {
        Direct = "direct",
        Newsstand = "newsstand",
    }
}

code_enum! {
    pub enum Binding {
        Hardcover = "hardcover",
        Softcover = "softcover",
        OversizedHardcover = "oversized_hardcover",
    }
}

code_enum! {
    /// What a creator did on an issue, in the order MyLongboxDB lists
    /// credits.
    pub enum Role {
        Writer = "writer",
        /// Drew the story, when the credit does not say who pencilled
        /// and who inked.
        Artist = "artist",
        Penciller = "penciller",
        Inker = "inker",
        Colorist = "colorist",
        Letterer = "letterer",
        CoverArtist = "cover_artist",
        Editor = "editor",
        /// Magazines: wrote an article.
        Article = "article",
        /// Magazines: gave or conducted an interview.
        Interview = "interview",
    }
}

/// The series a book belongs to. MyLongboxDB adds the book to the series
/// of the same name, start year, and volume if there is one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeriesInfo {
    pub name: String,
    pub start_year: Option<u16>,
    pub volume: Option<u32>,
    /// Empty when not known.
    pub publisher: String,
    /// Such as Vertigo; empty when none.
    pub imprint: String,
    /// How many issues the series ran to, for a finished one.
    pub known_issue_count: Option<u32>,
    /// Magazines only.
    pub magazine_format: Option<MagazineFormat>,
    /// Magazines only.
    pub black_and_white: Option<bool>,
}

/// A creator's credit on an issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credit {
    pub name: String,
    pub role: Role,
}

/// A character or team in an issue, and whether it is their first
/// appearance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub name: String,
    pub first: bool,
}

/// A story arc an issue is part of, and which part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArcPart {
    pub name: String,
    pub part: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssueInfo {
    /// `None` only for a collected edition that stands alone.
    pub number: Option<IssueNumber>,
    /// The number across a title's relaunches.
    pub legacy_number: Option<u32>,
    pub title: String,
    pub cover_date: Option<CoverDate>,
    pub release_date: Option<Date>,
    pub page_count: Option<u32>,
    pub is_key: bool,
    pub key_reason: String,
    pub synopsis: String,
    pub credits: Vec<Credit>,
    pub characters: Vec<Appearance>,
    pub teams: Vec<Appearance>,
    pub story_arcs: Vec<ArcPart>,
    /// Collected editions only: the issues the book reprints.
    pub collects: Vec<CollectsEntry>,
}

/// The printed version of an issue the owner has: its cover variant,
/// printing, and distribution, and what is printed on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditionInfo {
    /// Such as `Cover B`; empty for the standard cover.
    pub variant: String,
    pub printing: Option<u32>,
    pub distribution: Option<Distribution>,
    /// When this edition came out, for one published after the issue
    /// first was.
    pub published: Option<PartialDate>,
    /// The US price printed on the cover, in cents. MyLongboxDB takes
    /// cover prices in US dollars only.
    pub cover_price_cents: Option<i64>,
    /// The Canadian price printed beside it, in Canadian cents.
    pub canadian_cover_price_cents: Option<i64>,
    pub barcode: Option<Barcode>,
    pub isbn: Option<Isbn>,
    pub binding: Option<Binding>,
    /// Who drew this edition's cover, when it differs from the issue's.
    pub cover_artists: Vec<String>,
}

/// A JPEG made from a photo's pixels alone, ready to go into the export.
#[derive(Clone, PartialEq, Eq)]
pub struct Jpeg(Vec<u8>);

impl Jpeg {
    /// The bytes, if they start as a JPEG does; MyLongboxDB checks the
    /// photos it imports the same way before decoding them.
    pub fn new(bytes: Vec<u8>) -> Option<Self> {
        bytes
            .starts_with(&[0xFF, 0xD8, 0xFF])
            .then_some(Self(bytes))
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Jpeg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Jpeg({} bytes)", self.0.len())
    }
}

/// One book the owner has, as it goes into the export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Book {
    pub category: Category,
    pub series: SeriesInfo,
    pub issue: IssueInfo,
    pub edition: EditionInfo,
    pub front: Option<Jpeg>,
    pub back: Option<Jpeg>,
}

impl Book {
    pub fn new(category: Category, series_name: impl Into<String>) -> Self {
        Self {
            category,
            series: SeriesInfo {
                name: series_name.into(),
                ..SeriesInfo::default()
            },
            issue: IssueInfo::default(),
            edition: EditionInfo::default(),
            front: None,
            back: None,
        }
    }

    /// What MyLongboxDB's import would refuse in this book, each as a
    /// sentence for the owner; empty when it would import.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut problem = |text: &str| problems.push(text.to_owned());
        if self.series.name.split_whitespace().next().is_none() {
            problem("every book needs a series");
        }
        if self
            .series
            .start_year
            .is_some_and(|y| !(1..=9999).contains(&y))
        {
            problem("a series' start year must be between 1 and 9999");
        }
        if self.series.volume == Some(0) {
            problem("volumes are numbered from 1");
        }
        if self.category != Category::Magazine
            && (self.series.magazine_format.is_some() || self.series.black_and_white.is_some())
        {
            problem("only magazines have a magazine format or black-and-white pages");
        }
        // MyLongboxDB's import takes a leading `#` off, so `#` alone is no
        // number at all.
        let numbered = self
            .issue
            .number
            .as_ref()
            .is_some_and(|n| !n.as_str().trim_start_matches('#').trim().is_empty());
        if self.issue.number.is_some() && !numbered {
            problem("an issue number needs more than `#`");
        } else if !numbered && self.category != Category::CollectedEdition {
            problem("comics and magazines need an issue number");
        }
        if !self.issue.collects.is_empty() && self.category != Category::CollectedEdition {
            problem("only collected editions list the issues they reprint");
        }
        if self.issue.story_arcs.iter().any(|a| a.part == Some(0)) {
            problem("story arc parts are numbered from 1");
        }
        if self.edition.printing == Some(0) {
            problem("printings are numbered from 1");
        }
        if self.edition.cover_price_cents.is_some_and(|c| c < 0)
            || self
                .edition
                .canadian_cover_price_cents
                .is_some_and(|c| c < 0)
        {
            problem("prices cannot be negative");
        }
        let names = self.issue.credits.iter().map(|c| c.name.as_str()).chain(
            self.issue
                .characters
                .iter()
                .chain(&self.issue.teams)
                .map(|a| a.name.as_str()),
        );
        let names = names
            .chain(self.issue.story_arcs.iter().map(|a| a.name.as_str()))
            .chain(self.edition.cover_artists.iter().map(String::as_str));
        if names.into_iter().any(|n| n.trim().is_empty()) {
            problem("a name in a list is blank");
        }
        problems
    }
}

/// How sure the model was of a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comic() -> Book {
        let mut book = Book::new(Category::Comic, "Amazing Spider-Man");
        book.issue.number = Some(IssueNumber::new("300").unwrap());
        book
    }

    #[test]
    fn codes_match_mylongboxdb() {
        assert_eq!(Category::CollectedEdition.code(), "collected");
        assert_eq!(Role::CoverArtist.code(), "cover_artist");
        assert_eq!(
            Binding::from_code("oversized_hardcover"),
            Some(Binding::OversizedHardcover)
        );
        assert_eq!(Distribution::from_code("Direct"), None);
    }

    #[test]
    fn a_plain_comic_has_no_problems() {
        assert!(comic().problems().is_empty());
    }

    #[test]
    fn what_mylongboxdb_refuses_is_reported() {
        type Change = fn(&mut Book);
        let cases: [(&str, Change); 10] = [
            ("series", |b| b.series.name = "  ".into()),
            ("start year", |b| b.series.start_year = Some(0)),
            ("volume", |b| b.series.volume = Some(0)),
            ("magazine", |b| b.series.black_and_white = Some(true)),
            ("issue number", |b| b.issue.number = None),
            ("more than", |b| {
                b.issue.number = Some(IssueNumber::new(" # ").unwrap())
            }),
            ("reprint", |b| {
                b.issue.collects = crate::collects::parse_collects("Thor (1966) 337").unwrap();
            }),
            ("part", |b| {
                b.issue.story_arcs.push(ArcPart {
                    name: "Kraven's Last Hunt".into(),
                    part: Some(0),
                });
            }),
            ("printing", |b| b.edition.printing = Some(0)),
            ("blank", |b| b.edition.cover_artists.push(" ".into())),
        ];
        for (word, change) in cases {
            let mut book = comic();
            change(&mut book);
            let problems = book.problems();
            assert_eq!(problems.len(), 1, "{word}: {problems:?}");
            assert!(problems[0].contains(word), "{word}: {problems:?}");
        }
    }

    #[test]
    fn a_collected_edition_may_stand_alone() {
        let mut book = Book::new(Category::CollectedEdition, "Watchmen");
        book.issue.collects = crate::collects::parse_collects("Watchmen (1986) 1-12").unwrap();
        assert!(book.problems().is_empty());
    }

    #[test]
    fn jpegs_are_recognised_by_their_start() {
        assert!(Jpeg::new(vec![0xFF, 0xD8, 0xFF, 0xE0]).is_some());
        assert!(Jpeg::new(b"\x89PNG".to_vec()).is_none());
        assert_eq!(
            format!("{:?}", Jpeg::new(vec![0xFF, 0xD8, 0xFF]).unwrap()),
            "Jpeg(3 bytes)"
        );
    }
}
