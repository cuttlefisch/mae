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
}

const EN_US: Strings = Strings {
    locale: "en-US",
    backlinks: "Links to this page",
};
const FR_FR: Strings = Strings {
    locale: "fr-FR",
    backlinks: "Liens vers cette page",
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
        assert_eq!(
            for_locale("xx-YY"),
            en,
            "an unknown locale falls back to en-US"
        );
    }
}
