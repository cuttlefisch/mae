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
    /// The mode toggle's accessible name (P-91): the same whatever the state,
    /// which `aria-pressed` carries ("true" while dark is in effect).
    pub mode_toggle: &'static str,
    /// The palette button's accessible name, and its menu's (P-93).
    pub palette_menu: &'static str,
    /// The first item of a palette's accent submenu: its default (P-94).
    pub accent_default: &'static str,
    /// The accents' names, by the palette package's swatch id (P-94). Colour
    /// words, so translated; the palettes' own names are proper names (A4).
    pub accents: &'static [(&'static str, &'static str)],
}

const EN_US: Strings = Strings {
    locale: "en-US",
    backlinks: "Links to this page",
    contents: "Contents",
    tags: "Tags",
    tag_page_open: "Pages tagged \u{201c}",
    tag_page_close: "\u{201d}",
    mode_toggle: "Dark mode",
    palette_menu: "Color palette",
    accent_default: "Default",
    accents: &[
        ("red", "Red"),
        ("orange", "Orange"),
        ("yellow", "Yellow"),
        ("green", "Green"),
        ("teal", "Teal"),
        ("cyan", "Cyan"),
        ("blue", "Blue"),
        ("violet", "Violet"),
        ("magenta", "Magenta"),
    ],
};
const FR_FR: Strings = Strings {
    locale: "fr-FR",
    backlinks: "Liens vers cette page",
    contents: "Sommaire",
    tags: "\u{c9}tiquettes",
    tag_page_open: "Pages \u{e9}tiquet\u{e9}es \u{ab}\u{a0}",
    tag_page_close: "\u{a0}\u{bb}",
    mode_toggle: "Mode sombre",
    palette_menu: "Palette de couleurs",
    accent_default: "Par d\u{e9}faut",
    accents: &[
        ("red", "Rouge"),
        ("orange", "Orange"),
        ("yellow", "Jaune"),
        ("green", "Vert"),
        ("teal", "Bleu sarcelle"),
        ("cyan", "Cyan"),
        ("blue", "Bleu"),
        ("violet", "Violet"),
        ("magenta", "Magenta"),
    ],
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
        assert_ne!(en.mode_toggle, fr.mode_toggle);
        assert_ne!(en.palette_menu, fr.palette_menu);
        assert_ne!(en.accent_default, fr.accent_default);
        assert_ne!(en.accents, fr.accents);
        assert_eq!(
            for_locale("xx-YY"),
            en,
            "an unknown locale falls back to en-US"
        );
    }

    /// P-94: every accent the palettes offer has a name in every catalogue,
    /// so no submenu item falls back to a bare id. The accent ids are the
    /// keys of each palette's `accents.items` in the palette build's
    /// themes.json (pretty-printed: those keys alone sit at ten spaces and
    /// open an object).
    #[test]
    fn every_offered_accent_has_a_name_in_every_locale() {
        let themes = include_str!("../web/palette/dist/themes.json");
        let mut ids: Vec<&str> = themes
            .lines()
            .filter_map(|l| l.strip_prefix("          \""))
            .filter_map(|l| l.strip_suffix("\": {"))
            .filter(|l| !l.starts_with(' '))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        assert!(ids.contains(&"orange") && ids.len() >= 7, "{ids:?}");
        for words in [for_locale("en-US"), for_locale("fr-FR")] {
            for id in &ids {
                let named = words.accents.iter().find(|(k, _)| k == id);
                assert!(
                    named.is_some_and(|(_, n)| !n.is_empty()),
                    "{}: {id}",
                    words.locale
                );
            }
            // Ids are what the markup's attribute names carry: [a-z-] only.
            for (k, _) in words.accents {
                assert!(
                    k.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                    "{k}"
                );
            }
        }
    }
}
