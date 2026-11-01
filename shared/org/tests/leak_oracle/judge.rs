//! The verdict on one document: Emacs's export against every MAE channel.

use super::channels::{Channel, Kind, MaeView};
use super::emacs::EmacsOut;
use super::gen::{Allow, Site, Sites};
use super::view::{scan, strip_attr};
use std::collections::BTreeSet;

/// A marker MAE shows (in `channels`) that Emacs's HTML does not.
#[derive(Debug, Clone)]
pub struct Leak {
    pub marker: u32,
    pub leaf: &'static str,
    pub path: String,
    /// See [`category`].
    pub category: String,
    /// Every channel showing it where it is not allowed, and whether it
    /// shows there only inside an `id` attribute.
    pub channels: Vec<(String, bool)>,
}

#[derive(Debug, Clone)]
pub enum Verdict {
    Compared {
        leaks: Vec<Leak>,
        /// Markers Emacs shows and MAE's page does not (allowed).
        lost: Vec<(u32, &'static str)>,
    },
    /// MAE renders nothing (a macro failure); Emacs exports.
    FailClosed,
    /// Both refuse the document.
    BothFail,
    /// Emacs's export fails; MAE renders.
    EmacsErrorMaeRenders {
        class: String,
        registered: Option<&'static str>,
    },
    EmacsHang(&'static str),
    Missing,
}

/// Emacs errors where the register (`conformance/render-divergences.toml`)
/// lets MAE render with a warning: (error text, divergence id).
pub const REGISTERED_ERRORS: &[(&str, &str)] = &[
    ("Unknown Babel reference", "call-unknown-reference"),
    ("Wrong type argument: stringp", "exports-no-known-word"),
    ("input-file", "macros"),
    ("modification-time", "macros"),
];

/// An Emacs error with markers and numbers abstracted, for grouping.
pub fn error_class(e: &str) -> String {
    let mut out = String::new();
    let mut last_digit = false;
    for c in e.chars().take(120) {
        if c.is_ascii_digit() {
            if !last_digit {
                out.push('N');
            }
            last_digit = true;
        } else {
            out.push(c);
            last_digit = false;
        }
    }
    out.replace("qzNqz", "M")
}

/// The verdict on document `text`. A site whose headline marker
/// (`Allow::Meta(Some(h))`) is no longer in `text` (a minimised document),
/// or whose exposure marker Emacs does not show, is judged as
/// `Allow::Meta(None)`.
pub fn judge(emacs: &EmacsOut, mae: &MaeView, sites: &Sites, text: &str) -> Verdict {
    match emacs {
        EmacsOut::Hang(why) => Verdict::EmacsHang(why),
        EmacsOut::Missing => Verdict::Missing,
        EmacsOut::Error(_) if mae.empty => Verdict::BothFail,
        EmacsOut::Error(e) => Verdict::EmacsErrorMaeRenders {
            class: error_class(e),
            registered: REGISTERED_ERRORS
                .iter()
                .find(|(pat, _)| e.contains(pat))
                .map(|(_, id)| *id),
        },
        EmacsOut::Html(_) if mae.failed || mae.empty => Verdict::FailClosed,
        EmacsOut::Html(h) => compare(h, mae, sites, text),
    }
}

/// What a channel is judged against.
struct Cx<'a> {
    shown: &'a BTreeSet<u32>,
    present: &'a BTreeSet<u32>,
    sites: &'a Sites,
}

fn compare(html: &str, mae: &MaeView, sites: &Sites, text: &str) -> Verdict {
    let shown = scan(html);
    let present = scan(text);
    let cx = Cx {
        shown: &shown,
        present: &present,
        sites,
    };
    let mut leaks: Vec<Leak> = Vec::new();
    for ch in &mae.channels {
        for (m, id_only) in disallowed(ch, &cx) {
            match leaks.iter_mut().find(|l| l.marker == m) {
                Some(l) => l.channels.push((ch.name.clone(), id_only)),
                None => leaks.push(new_leak(m, sites.get(&m), (ch.name.clone(), id_only))),
            }
        }
    }
    leaks.sort_by_key(|l| l.marker);
    let page: BTreeSet<u32> = mae
        .channels
        .iter()
        .filter(|c| c.name == "text" || c.name == "pd=false,ids=explicit,res=false:body.html")
        .flat_map(|c| scan(&c.text))
        .collect();
    let lost = shown
        .difference(&page)
        .map(|m| (*m, sites.get(m).map_or("unplanted", |s| s.leaf)))
        .collect();
    Verdict::Compared { leaks, lost }
}

/// What a leak is grouped (and minimised) by: the leaf, prefixed by the
/// metadata or title container holding it and suffixed by the macro pair
/// or region around it. Inside a citation, `citation>LEAF` only: a
/// citation is printed as its source (one cause, shape (d)), which would
/// otherwise show under every leaf a citation can hold.
pub fn category(leaf: &str, path: &str) -> String {
    if leaf.ends_with("cite-key") || path.contains("cite-") {
        return format!("citation>{leaf}");
    }
    let containers = [
        "headline-title",
        "doc-title",
        "property-value",
        "alias-value",
    ];
    // Leaves that leak by themselves, wherever they stand.
    let alone = [
        "target-name",
        "link-rel",
        "link-w",
        "id-store-desc",
        "id-fragment",
        "id-path",
    ];
    let ctx = containers
        .iter()
        .find(|c| path.contains(*c))
        .filter(|_| !alone.contains(&leaf));
    let pair = path
        .split('>')
        .rev()
        .find(|p| p.starts_with("pair:") || p.starts_with("region:"));
    let ctx = ctx.map_or(String::new(), |c| format!("{c}>"));
    let pair = pair.map_or(String::new(), |p| format!("@{p}"));
    format!("{ctx}{leaf}{pair}")
}

fn new_leak(marker: u32, site: Option<&Site>, hit: (String, bool)) -> Leak {
    let leaf = site.map_or("unplanted", |s| s.leaf);
    let path = site.map_or(String::new(), |s| s.path.clone());
    let category = category(leaf, &path);
    Leak {
        marker,
        leaf,
        path,
        category,
        channels: vec![hit],
    }
}

/// The markers `ch` shows that Emacs does not and the site does not
/// allow there, each with whether it is only inside an `id` attribute.
fn disallowed(ch: &Channel, cx: &Cx) -> Vec<(u32, bool)> {
    let all = scan(&ch.text);
    if all.is_subset(cx.shown) {
        return Vec::new();
    }
    let outside_href = scan(&strip_attr(&ch.text, "href"));
    let outside_id = scan(&strip_attr(&ch.text, "id"));
    all.difference(cx.shown)
        .filter(|m| !allowed(cx.sites.get(m), ch, cx, outside_href.contains(m)))
        .map(|m| (*m, !outside_id.contains(m)))
        .collect()
}

fn allowed(site: Option<&Site>, ch: &Channel, cx: &Cx, outside_href: bool) -> bool {
    let Some(site) = site else {
        return false;
    };
    let unexposed = site.exposure.is_some_and(|x| !cx.shown.contains(&x));
    let meta = match site.allow {
        Allow::Meta(h) => h.is_none_or(|h| cx.shown.contains(&h) || !cx.present.contains(&h)),
        _ => unexposed,
    };
    let href = site.allow == Allow::Href && !outside_href;
    (meta && (ch.kind == Kind::Meta || ch.pd)) || href
}
