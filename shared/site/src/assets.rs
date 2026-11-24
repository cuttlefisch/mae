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

/// The base rules: the page's colours over the palette's tokens
/// (`shared/site/web/base/base.css`, hand-written).
pub const BASE_CSS: &str = include_str!("../web/base/base.css");

/// The in-page router (`shared/site/web/router/dist/router.js`, built from
/// TypeScript by `tsc` and committed like the palette's output; ADR-119 D16).
pub const ROUTER_JS: &str = include_str!("../web/router/dist/router.js");

/// The pre-paint script (`shared/site/web/router/dist/prepaint.js`, generated
/// by the router's build from its choice functions and the palette's offer;
/// ADR-119 D17): a classic script in `<head>` that applies the reader's mode
/// and palette before first paint.
pub const PREPAINT_JS: &str = include_str!("../web/router/dist/prepaint.js");

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

/// `template` with the content hash of `body` written over its 16 zeros at
/// `at`: `<stem>-<fnv64 hex>.<ext>`.
const fn named<const N: usize>(mut template: [u8; N], at: usize, body: &str) -> [u8; N] {
    let hash = hex16(fnv1a(body.as_bytes()));
    let mut i = 0;
    while i < 16 {
        template[at + i] = hash[i];
        i += 1;
    }
    template
}

const fn ascii(bytes: &'static [u8]) -> &'static str {
    // ASCII by construction: a fixed ASCII template and lowercase hex digits.
    match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => panic!("ASCII by construction"),
    }
}

const PALETTE_NAME_BYTES: [u8; 28] = named(*b"palette-0000000000000000.css", 8, PALETTE_CSS);
const BASE_NAME_BYTES: [u8; 25] = named(*b"base-0000000000000000.css", 5, BASE_CSS);
const ROUTER_NAME_BYTES: [u8; 26] = named(*b"router-0000000000000000.js", 7, ROUTER_JS);
const PREPAINT_NAME_BYTES: [u8; 28] = named(*b"prepaint-0000000000000000.js", 9, PREPAINT_JS);

/// The palette stylesheet.
pub const PALETTE: Asset = Asset {
    name: ascii(&PALETTE_NAME_BYTES),
    content_type: "text/css; charset=utf-8",
    body: PALETTE_CSS,
};

/// The base stylesheet, linked after the palette so its tokens are defined.
pub const BASE: Asset = Asset {
    name: ascii(&BASE_NAME_BYTES),
    content_type: "text/css; charset=utf-8",
    body: BASE_CSS,
};

/// The router, loaded as a module script (deferred by default).
pub const ROUTER: Asset = Asset {
    name: ascii(&ROUTER_NAME_BYTES),
    content_type: "text/javascript; charset=utf-8",
    body: ROUTER_JS,
};

/// The pre-paint script, loaded as a classic, render-blocking script before
/// the stylesheets (no `defer`, no `module`), so its attributes are on
/// `<html>` when they first apply.
pub const PREPAINT: Asset = Asset {
    name: ascii(&PREPAINT_NAME_BYTES),
    content_type: "text/javascript; charset=utf-8",
    body: PREPAINT_JS,
};

/// Every embedded asset.
pub const ALL: &[Asset] = &[PALETTE, BASE, ROUTER, PREPAINT];

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
        assert_eq!(at(&href(BASE)), Some(BASE));
        assert!(BASE.name.starts_with("base-") && BASE.name.len() == 25);
        assert_eq!(at(&href(ROUTER)), Some(ROUTER));
        assert!(ROUTER.name.starts_with("router-") && ROUTER.name.ends_with(".js"));
        // The committed build of the router, not a placeholder.
        assert!(ROUTER.body.contains("mae-router") && ROUTER.body.contains("redirect: \"manual\""));
        assert_eq!(at(&href(PREPAINT)), Some(PREPAINT));
        assert!(PREPAINT.name.starts_with("prepaint-") && PREPAINT.name.ends_with(".js"));
        assert_eq!(PREPAINT.name.len(), 28);
        assert!(PREPAINT.name[9..25].bytes().all(|b| b.is_ascii_hexdigit()));
    }

    /// The pre-paint script is the generated classic script: it carries the
    /// palette's offer and the choice functions, runs itself, and imports
    /// nothing (a classic script cannot), so it applies before first paint.
    #[test]
    fn the_prepaint_script_is_a_self_contained_classic_script() {
        let body = PREPAINT.body;
        assert!(body.contains("Generated by scripts/prepaint.mjs"), "{body}");
        for f in [
            "function storageKey(",
            "function readStored(",
            "function validAccent(",
            "function resolveChoice(",
            "function systemDark(",
            "function applyChoice(",
            "function prepaint(",
            "prepaint(window, data);",
        ] {
            assert!(body.contains(f), "{f}");
        }
        assert!(
            body.contains("var data = {\"default\":"),
            "the offer, inline"
        );
        for bad in ["import ", "export ", "import(", "innerHTML", "eval("] {
            assert!(!body.contains(bad), "{bad}");
        }
        // Every palette the stylesheet defines is one the script accepts.
        for theme in PALETTE_CSS.split("[data-theme=\"").skip(1) {
            let name = theme.split('"').next().unwrap_or_default();
            assert!(body.contains(&format!("\"{name}\":{{\"name\"")), "{name}");
        }
    }

    /// P-102 rests on the palette's contrast tests, so the base rules may
    /// colour the page with the palette's tokens only: no literal colour, and
    /// no token the palette does not define (a typo would fall back to the
    /// browser's default, the gap this stylesheet exists to close).
    #[test]
    fn the_base_rules_colour_the_page_with_palette_tokens_only() {
        let mut code = String::new();
        let mut rest = BASE_CSS;
        while let Some((before, after)) = rest.split_once("/*") {
            code.push_str(before);
            rest = after.split_once("*/").map_or("", |(_, a)| a);
        }
        code.push_str(rest);
        // Every declaration of a colour-bearing property, in every rule body.
        let colour_props = |p: &str| {
            p == "color"
                || p == "background"
                || p == "fill"
                || p == "stroke"
                || p.ends_with("-color")
        };
        let mut coloured = 0;
        for body in code.split('{').skip(1) {
            let body = body.split('}').next().unwrap_or("");
            for decl in body.split(';') {
                let Some((prop, value)) = decl.split_once(':') else {
                    continue;
                };
                let (prop, value) = (prop.trim(), value.trim());
                if !colour_props(prop) {
                    continue;
                }
                coloured += 1;
                let token = value
                    .strip_prefix("var(")
                    .and_then(|v| v.strip_suffix(')'))
                    .unwrap_or_else(|| panic!("{prop}: {value}: not a palette token"));
                assert!(
                    token.starts_with("--ui-") || token.starts_with("--code-"),
                    "{prop}: {token}: not a page or code token"
                );
                assert!(
                    PALETTE_CSS.contains(&format!("{token}:")),
                    "{prop}: {token}: not in the palette"
                );
            }
        }
        assert!(coloured >= 6, "the page, its text, links and panels");
        // P-106: each token role is coloured by its own palette token, and
        // the block by the code background and text.
        for role in crate::CodeRole::ALL {
            let class = role.class();
            let token = class.trim_start_matches("org-");
            assert!(
                code.contains(&format!(".{class} {{\n  color: var(--{token});\n}}")),
                "{class} is not coloured by --{token}"
            );
        }
        for decl in [
            "background: var(--code-background);",
            "color: var(--code-text);",
            "overflow-x: auto;",
            "white-space: pre;",
        ] {
            let rule = code.split("pre.org-src {").nth(1).unwrap_or_default();
            let rule = rule.split('}').next().unwrap_or_default();
            assert!(rule.contains(decl), "pre.org-src lacks {decl}");
        }
        for rule in ["a:link", "a:visited", "body", ":root[data-mode=\"dark\"]"] {
            assert!(code.contains(rule), "{rule} has no rule");
        }
    }
}
