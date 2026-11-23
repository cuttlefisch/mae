//! Every string the engine itself writes, by configured locale (P-164).
//!
//! The chrome takes its words from here, never from a literal, so a second
//! locale changes every one of them. `en-US` is the fallback for a locale with
//! no catalogue, and [`resolve`] says when it was used, so the chrome can mark
//! what it wrote from it `lang="en-US"` (ADR-119 A4). A catalogue is complete
//! by construction: a missing field does not compile. Proper names the engine
//! never translates (palette and product names) are not here.
//!
//! Translations are written for this catalogue, never copied from another
//! generator's locale files. `eo` is proposed and awaits a speaker's review.

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

const ES_ES: Strings = Strings {
    locale: "es-ES",
    backlinks: "Enlaces a esta p\u{e1}gina",
    contents: "\u{cd}ndice",
    tags: "Etiquetas",
    tag_page_open: "P\u{e1}ginas con la etiqueta \u{ab}",
    tag_page_close: "\u{bb}",
    mode_toggle: "Modo oscuro",
    palette_menu: "Paleta de colores",
    accent_default: "Predeterminado",
    accents: &[
        ("red", "Rojo"),
        ("orange", "Naranja"),
        ("yellow", "Amarillo"),
        ("green", "Verde"),
        ("teal", "Verde azulado"),
        ("cyan", "Cian"),
        ("blue", "Azul"),
        ("violet", "Violeta"),
        ("magenta", "Magenta"),
    ],
};
const EO: Strings = Strings {
    locale: "eo",
    backlinks: "Ligiloj al \u{109}i tiu pa\u{11d}o",
    contents: "Enhavo",
    tags: "Etikedoj",
    tag_page_open: "Pa\u{11d}oj kun la etikedo \u{201c}",
    tag_page_close: "\u{201d}",
    mode_toggle: "Malhela re\u{11d}imo",
    palette_menu: "Kolorpaletro",
    accent_default: "Defa\u{16d}lta",
    accents: &[
        ("red", "Ru\u{11d}a"),
        ("orange", "Oran\u{11d}a"),
        ("yellow", "Flava"),
        ("green", "Verda"),
        ("teal", "Bluverda"),
        ("cyan", "Cejana"),
        ("blue", "Blua"),
        ("violet", "Viola"),
        ("magenta", "Magenta"),
    ],
};

/// Every catalogue, by its locale tag; `en-US` is also the fallback.
const CATALOGUES: [&Strings; 4] = [&EN_US, &FR_FR, &ES_ES, &EO];

/// The catalogue for `locale`, and whether it fell back: `(en-US's, true)`
/// for a locale with no catalogue. A caller writing chrome from a fallback
/// marks it `lang="en-US"` (the catalogue's own `locale`), while the page's
/// language stays the configured one.
pub fn resolve(locale: &str) -> (&'static Strings, bool) {
    match CATALOGUES.iter().find(|s| s.locale == locale) {
        Some(s) => (s, false),
        None => (&EN_US, true),
    }
}

/// The catalogue for `locale`, or `en-US`'s when there is none.
pub fn for_locale(locale: &str) -> &'static Strings {
    resolve(locale).0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every string a catalogue holds, by name. The destructuring is
    /// exhaustive, so a new field does not compile until it is listed here.
    fn fields(s: &Strings) -> Vec<(String, &'static str)> {
        let Strings {
            locale: _,
            backlinks,
            contents,
            tags,
            tag_page_open,
            tag_page_close,
            mode_toggle,
            palette_menu,
            accent_default,
            accents,
        } = *s;
        let mut out: Vec<(String, &'static str)> = [
            ("backlinks", backlinks),
            ("contents", contents),
            ("tags", tags),
            ("tag_page_open", tag_page_open),
            ("tag_page_close", tag_page_close),
            ("mode_toggle", mode_toggle),
            ("palette_menu", palette_menu),
            ("accent_default", accent_default),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        out.extend(accents.iter().map(|(id, n)| (format!("accent {id}"), *n)));
        out
    }

    /// Each other locale changes every string relative to en-US, except the
    /// ones declared here as the same in both languages: colour words the
    /// languages share, and eo's closing quotation mark, which is en-US's.
    /// A declared exception that differs fails too, so the list stays true.
    #[test]
    fn every_other_locale_changes_every_string_it_really_translates() {
        let same: [(&str, &[&str]); 3] = [
            (
                "fr-FR",
                &[
                    "accent orange",
                    "accent cyan",
                    "accent violet",
                    "accent magenta",
                ],
            ),
            ("es-ES", &["accent magenta"]),
            ("eo", &["tag_page_close", "accent magenta"]),
        ];
        let en = fields(for_locale("en-US"));
        for (locale, shared) in same {
            let (words, fell_back) = resolve(locale);
            assert!(!fell_back, "{locale}");
            let theirs = fields(words);
            assert_eq!(
                en.iter().map(|(k, _)| k).collect::<Vec<_>>(),
                theirs.iter().map(|(k, _)| k).collect::<Vec<_>>(),
                "{locale}: the same strings, in the same order"
            );
            for ((k, e), (_, t)) in en.iter().zip(&theirs) {
                assert!(!t.is_empty(), "{locale}: {k} is empty");
                assert_eq!(
                    e == t,
                    shared.contains(&k.as_str()),
                    "{locale}: {k} is {t:?}, en-US {e:?}"
                );
            }
        }
    }

    /// The catalogues are found, never the fallback: a configured es-ES or eo
    /// that rendered en-US's words would pass every other test.
    #[test]
    fn every_catalogued_locale_is_found_and_anything_else_falls_back() {
        for locale in ["en-US", "fr-FR", "es-ES", "eo"] {
            let (words, fell_back) = resolve(locale);
            assert!(!fell_back, "{locale} fell back");
            assert_eq!(words.locale, locale);
        }
        for other in ["xx-YY", "es", "es-MX", "eo-XX", ""] {
            assert_eq!(resolve(other), (&EN_US, true), "{other:?}");
            assert_eq!(for_locale(other), &EN_US, "{other:?}");
        }
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
        for words in CATALOGUES {
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
