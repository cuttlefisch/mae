//! Every string the engine itself writes, by configured locale (P-164).
//!
//! The chrome takes its words from here, never from a literal, so a second
//! locale changes every one of them. `en-US` is the fallback for a locale with
//! no catalogue. Proper names the engine never translates (palette and product
//! names) are not here.

/// One locale's engine strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Strings {
    /// The locale this catalogue is for (BCP 47).
    pub locale: &'static str,
    /// The heading and accessible name of a page's backlinks.
    pub backlinks: &'static str,
    /// The heading and accessible name of a page's table of contents.
    pub contents: &'static str,
    /// The tag index's heading, and the accessible name of a page's tags.
    pub tags: &'static str,
    /// One tag's page heading, around the tag: `open` + tag + `close`.
    pub tag_page_open: &'static str,
    pub tag_page_close: &'static str,
}

const EN_US: Strings = Strings {
    locale: "en-US",
    backlinks: "Links to this page",
    contents: "Contents",
    tags: "Tags",
    tag_page_open: "Pages tagged \u{201c}",
    tag_page_close: "\u{201d}",
};
const FR_FR: Strings = Strings {
    locale: "fr-FR",
    backlinks: "Liens vers cette page",
    contents: "Sommaire",
    tags: "\u{c9}tiquettes",
    tag_page_open: "Pages \u{e9}tiquet\u{e9}es \u{ab}\u{a0}",
    tag_page_close: "\u{a0}\u{bb}",
};

/// The catalogue for `locale`, or `en-US`'s when there is none.
pub fn for_locale(locale: &str) -> &'static Strings {
    match locale {
        "fr-FR" => &FR_FR,
        _ => &EN_US,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_locale_changes_every_string() {
        let (en, fr) = (for_locale("en-US"), for_locale("fr-FR"));
        assert_ne!(en.backlinks, fr.backlinks);
        assert_ne!(en.tags, fr.tags);
        assert_ne!(en.contents, fr.contents);
        assert_ne!(en.tag_page_open, fr.tag_page_open);
        assert_ne!(en.tag_page_close, fr.tag_page_close);
        assert_eq!(
            for_locale("xx-YY"),
            en,
            "an unknown locale falls back to en-US"
        );
    }
}
