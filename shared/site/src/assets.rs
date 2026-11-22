//! The site's static assets, embedded at compile time from the browser
//! runtime's committed build output (ADR-119 D2, D17; owner, 2026-11-22: the
//! palette's `dist/` is committed and CI checks it against a fresh build).
//!
//! Each asset is served under a content-addressed name, so a browser may cache
//! it forever (`immutable`) and a change always reaches it under a new name.
//! Assets are public: they hold no KB content, and a `<link>` cannot carry a
//! bearer token.

/// The palettes' CSS variables (`shared/site/web/palette/dist/palette.css`).
pub const PALETTE_CSS: &str = include_str!("../web/palette/dist/palette.css");

/// The URL prefix every asset is served under.
pub const PREFIX: &str = "/site-assets/";

/// One embedded asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// Its content-addressed file name (`palette-<hash>.css`).
    pub name: &'static str,
    pub content_type: &'static str,
    pub body: &'static str,
}

/// 64-bit FNV-1a: a stable content address for cache busting (not a security
/// property; the bytes are public and come from this binary).
const fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

const fn hex16(mut v: u64) -> [u8; 16] {
    let digits = b"0123456789abcdef";
    let mut out = [0u8; 16];
    let mut i = 16;
    while i > 0 {
        i -= 1;
        out[i] = digits[(v & 0xf) as usize];
        v >>= 4;
    }
    out
}

const PALETTE_HASH: [u8; 16] = hex16(fnv1a(PALETTE_CSS.as_bytes()));
const PALETTE_NAME_BYTES: [u8; 28] = {
    let mut n = *b"palette-0000000000000000.css";
    let mut i = 0;
    while i < 16 {
        n[8 + i] = PALETTE_HASH[i];
        i += 1;
    }
    n
};

/// The palette stylesheet.
pub const PALETTE: Asset = Asset {
    // ASCII by construction: a fixed ASCII template and lowercase hex digits.
    name: match std::str::from_utf8(&PALETTE_NAME_BYTES) {
        Ok(s) => s,
        Err(_) => panic!("ASCII by construction"),
    },
    content_type: "text/css; charset=utf-8",
    body: PALETTE_CSS,
};

/// Every embedded asset.
pub const ALL: &[Asset] = &[PALETTE];

/// The asset served at `path`, if any (`/site-assets/<name>`, exactly).
pub fn at(path: &str) -> Option<Asset> {
    let name = path.strip_prefix(PREFIX)?;
    ALL.iter().copied().find(|a| a.name == name)
}

/// The URL a page links an asset at.
pub fn href(asset: Asset) -> String {
    format!("{PREFIX}{}", asset.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_is_the_committed_build_and_named_by_its_content() {
        assert!(PALETTE.body.starts_with("/* Generated from palettes/"));
        assert!(PALETTE.body.contains("--ui-background:"));
        assert!(PALETTE.name.starts_with("palette-") && PALETTE.name.ends_with(".css"));
        assert_eq!(PALETTE.name.len(), 28);
        // A one-byte change moves the name.
        let mut other = PALETTE_CSS.as_bytes().to_vec();
        other[0] ^= 1;
        assert_ne!(fnv1a(&other), fnv1a(PALETTE_CSS.as_bytes()));
    }

    #[test]
    fn only_an_exact_asset_path_is_served() {
        assert_eq!(at(&href(PALETTE)), Some(PALETTE));
        for bad in [
            "/site-assets/",
            "/site-assets/palette.css",
            "/site-assets/../x",
            "/x/palette-0.css",
        ] {
            assert_eq!(at(bad), None, "{bad}");
        }
        assert_eq!(at(&format!("{}/", href(PALETTE))), None);
    }
}
