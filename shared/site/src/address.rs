//! Addresses come from IDs only (ADR-119 D4): a retitle or a move never
//! changes a URL.

/// Where pages live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addressing {
    /// The live app: `/kb/<kb-id>/<id>`.
    Live { kb_id: String },
    /// The static export: `<base>/<id>/` (base ends without a slash; `""`
    /// is the site root).
    Export { base: String },
}

impl Addressing {
    /// The URL of the page whose file-level ID is `id`.
    pub fn page_href(&self, id: &str) -> String {
        match self {
            Addressing::Live { kb_id } => format!("/kb/{}/{}", segment(kb_id), segment(id)),
            Addressing::Export { base } => format!("{base}/{}/", segment(id)),
        }
    }
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
/// (so `/`, `?`, `#`, `%` and non-ASCII can never change the URL's shape).
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

/// The key two IDs collide under in the export, whose pages are directories:
/// on a case-insensitive filesystem `Abc/` and `abc/` are one (P-15).
pub fn export_collision_key(id: &str) -> String {
    id.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hrefs_are_built_from_the_id_alone_and_cannot_change_shape() {
        let live = Addressing::Live {
            kb_id: "kb-1".into(),
        };
        assert_eq!(
            live.page_href("concept:buffer"),
            "/kb/kb-1/concept%3Abuffer"
        );
        let export = Addressing::Export {
            base: String::new(),
        };
        assert_eq!(export.page_href("a/../b?c#d"), "/a%2F..%2Fb%3Fc%23d/");
        assert_eq!(export.page_href("é"), "/%C3%A9/");
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
