//! Addresses come from IDs only (ADR-119 D4): a retitle or a move never
//! changes a URL.
//!
//! An [`Addressing`] is built only through [`Addressing::live`] or
//! [`Addressing::export`], which validate the KB identifier and the base, so
//! no href can start with `//` (protocol-relative) or climb out of the base.

/// Where pages live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addressing(Kind);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// The live app: `/kb/<kb-id>/<id>`.
    Live { kb_id: String },
    /// The static export: `<base>/<id>/`.
    Export { base: String },
}

/// Why an [`Addressing`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressError {
    /// Not `^[a-z0-9][a-z0-9-]{0,62}$` (P-04).
    KbId(String),
    /// Not `""` or `/seg(/seg)*` with plain segments.
    Base(String),
}

impl Addressing {
    /// The live app's addressing for the KB `kb_id`.
    pub fn live(kb_id: &str) -> Result<Addressing, AddressError> {
        if !valid_kb_id(kb_id) {
            return Err(AddressError::KbId(kb_id.to_string()));
        }
        Ok(Addressing(Kind::Live {
            kb_id: kb_id.to_string(),
        }))
    }

    /// The static export's addressing under `base`: `""` (the site root) or
    /// `/a/b`, each segment of unreserved characters and never `.` or `..`.
    pub fn export(base: &str) -> Result<Addressing, AddressError> {
        let ok = base.is_empty()
            || base.strip_prefix('/').is_some_and(|rest| {
                rest.split('/').all(|s| {
                    !s.is_empty()
                        && s != "."
                        && s != ".."
                        && s.bytes().all(|b| {
                            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
                        })
                })
            });
        if !ok {
            return Err(AddressError::Base(base.to_string()));
        }
        Ok(Addressing(Kind::Export {
            base: base.to_string(),
        }))
    }

    pub fn is_export(&self) -> bool {
        matches!(self.0, Kind::Export { .. })
    }

    /// The tag index (P-71): `/kb/<kb-id>/tags` live, `<base>/tags/` in the
    /// export. [`RESERVED_IDS`] keeps a page from holding this address.
    pub fn tags_href(&self) -> String {
        match &self.0 {
            Kind::Live { kb_id } => format!("/kb/{kb_id}/tags"),
            Kind::Export { base } => format!("{base}/tags/"),
        }
    }

    /// The page of one tag (P-70), below the tag index.
    pub fn tag_href(&self, tag: &str) -> String {
        match &self.0 {
            Kind::Live { kb_id } => format!("/kb/{kb_id}/tags/{}", segment(tag)),
            Kind::Export { base } => format!("{base}/tags/{}/", segment(tag)),
        }
    }

    /// The URL of the page whose file-level ID is `id`, if `id` can be an
    /// address ([`valid_id`]).
    pub fn page_href(&self, id: &str) -> Option<String> {
        if !valid_id(id) {
            return None;
        }
        Some(match &self.0 {
            Kind::Live { kb_id } => format!("/kb/{kb_id}/{}", segment(id)),
            Kind::Export { base } => format!("{base}/{}/", segment(id)),
        })
    }
}

/// IDs a page may not hold, because the engine generates that address: the
/// tag index and the content index (P-166). Compared as the export compares directories, so `Tags`
/// is reserved too.
pub const RESERVED_IDS: &[&str] = &["tags", "index.json"];

/// Whether `id` names an address the engine generates (P-166).
pub fn is_reserved(id: &str) -> bool {
    RESERVED_IDS.iter().any(|r| export_collision_key(id) == *r)
}

/// Whether `t` is an Org tag: letters, digits, `_`, `@`, `#`, `%` (P-73). A
/// path or markup in a tag line is not a tag.
pub fn valid_tag(t: &str) -> bool {
    !t.is_empty()
        && t.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '@' | '#' | '%'))
}

/// Whether `id` can be an address. Not empty (that would give `//`), and not
/// `.` or `..`: a browser collapses those as dot segments, and percent-
/// encoding does not stop it (the URL standard reads `%2e` as `.`).
pub fn valid_id(id: &str) -> bool {
    // All dots: `.` and `..` are dot segments, and no longer run of dots can
    // be an export directory (Windows drops trailing dots, leaving nothing).
    !id.is_empty() && !id.bytes().all(|b| b == b'.') && !id.chars().any(refused_in_id)
}

/// Characters no address may hold: controls (Cc), Unicode White_Space, the
/// bidirectional controls (an ID that reorders how it displays can pass for
/// another), and U+FEFF. The joiners (U+200C, U+200D) are allowed: real
/// scripts need them. The browser client's backend refuses exactly this set,
/// so every page the daemon publishes is reachable through it.
pub fn refused_in_id(c: char) -> bool {
    c.is_control()
        || c.is_whitespace()
        || matches!(
            c,
            '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
        )
}

/// A KB identifier: `^[a-z0-9][a-z0-9-]{0,62}$` (P-04).
pub fn valid_kb_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// One path segment: unreserved bytes kept, every other byte percent-encoded
/// (so `/`, `\`, `?`, `#`, `%` and non-ASCII can never change the URL's
/// shape).
pub fn segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The key two IDs collide under in the export, whose pages are directories
/// named by [`segment`]: case-folded (macOS, Windows) and without trailing
/// dots, which Windows drops from a directory name (P-15).
pub fn export_collision_key(id: &str) -> String {
    segment(id)
        .to_ascii_lowercase()
        .trim_end_matches('.')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hrefs_are_built_from_the_id_alone_and_cannot_change_shape() {
        let live = Addressing::live("kb-1").unwrap();
        assert_eq!(
            live.page_href("concept:buffer").unwrap(),
            "/kb/kb-1/concept%3Abuffer"
        );
        let export = Addressing::export("").unwrap();
        assert_eq!(
            export.page_href("a/../b?c#d").unwrap(),
            "/a%2F..%2Fb%3Fc%23d/"
        );
        assert_eq!(export.page_href("\\\\evil").unwrap(), "/%5C%5Cevil/");
        assert_eq!(export.page_href("é").unwrap(), "/%C3%A9/");
        assert_eq!(
            Addressing::export("/docs/v1")
                .unwrap()
                .page_href("x")
                .unwrap(),
            "/docs/v1/x/"
        );
    }

    #[test]
    fn an_id_that_is_empty_or_a_dot_segment_has_no_address() {
        let export = Addressing::export("").unwrap();
        let live = Addressing::live("k").unwrap();
        for bad in ["", ".", ".."] {
            assert_eq!(export.page_href(bad), None, "{bad:?}");
            assert_eq!(live.page_href(bad), None, "{bad:?}");
        }
        assert_eq!(export.page_href("..."), None);
        for bad in [
            "a\u{0}b",
            "a b",
            "a\u{85}b",
            "a\u{a0}b",
            "a\u{2028}b",
            "a\u{3000}b",
            "a\u{61c}b",
            "a\u{200e}b",
            "a\u{202e}b",
            "a\u{2066}b",
            "a\u{feff}b",
            "a\tb",
        ] {
            assert_eq!(export.page_href(bad), None, "{bad:?}");
        }
        // The joiners stay: real scripts need them.
        assert!(
            export.page_href("a\u{200c}b").is_some() && export.page_href("a\u{200d}b").is_some()
        );
        // Dots inside an ID are ordinary characters.
        assert_eq!(export.page_href("a..b").unwrap(), "/a..b/");
    }

    #[test]
    fn a_base_or_kb_id_that_could_redirect_an_href_is_refused() {
        for bad in [
            "//evil", "/", "x", "/a/", "/a//b", "/a/../b", "/./a", "/a b", "/a?b", "\\\\evil",
        ] {
            assert_eq!(
                Addressing::export(bad),
                Err(AddressError::Base(bad.into())),
                "{bad:?}"
            );
        }
        assert_eq!(Addressing::live("Kb"), Err(AddressError::KbId("Kb".into())));
        assert!(Addressing::live("../x").is_err());
    }

    #[test]
    fn export_collisions_fold_case_and_trailing_dots() {
        assert_eq!(export_collision_key("Abc"), export_collision_key("abc"));
        assert_eq!(export_collision_key("abc."), export_collision_key("abc"));
        assert_ne!(export_collision_key("a b"), export_collision_key("a_b"));
    }

    #[test]
    fn a_kb_id_is_lowercase_alphanumerics_and_hyphens_up_to_63() {
        for ok in ["a", "0", "kb-1", &"a".repeat(63)] {
            assert!(valid_kb_id(ok), "{ok}");
        }
        for bad in ["", "-a", "A", "a_b", "a.b", "a/b", &"a".repeat(64), "é"] {
            assert!(!valid_kb_id(bad), "{bad}");
        }
    }
}
