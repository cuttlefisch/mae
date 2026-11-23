//! `RenderOptions::headline_levels`: Org's `#+OPTIONS: H:N` (issue #951).
//! A headline whose relative level is above N renders as ox-html's
//! `org-html-headline` renders a low-level one: a list item carrying the
//! headline's line and its contents.
//!
//! Conformance: each fixture in `tests/headline-levels/` was exported by
//! Emacs 30.2 (Org 9.7.11, `emacs -Q`) through
//! `conformance/macro-export.el` (ox-html, body only, `num:nil`, so Org's
//! low-level lists are `<ul>`; `org-html-toplevel-hlevel` its default 2).
//! [`ORG_PAGES`] records each page's skeleton ([`skeleton`]: headings,
//! lists, items and the words, in order); MAE's page of the same fixture,
//! at hlevel 2 with the fixture's `H:`, must have the same one. The ignored
//! test re-exports the fixtures and checks the record:
//!
//! ```sh
//! cargo test -p mae-org --test render_headline_levels -- --ignored --nocapture
//! ```

mod support;

use mae_org::{
    parse, render_html, semantics, HeadlineIds, HeadlineLevels, ParseOptions, RenderOptions,
    Resolver, ToplevelHlevel,
};
use std::path::{Path, PathBuf};
use support::*;

/// Each fixture, the `H:` its `#+OPTIONS:` line gives (3, Org's default,
/// when it has none), the level of its top-level headlines, and the
/// skeleton of Emacs's page.
const ORG_PAGES: &[(&str, usize, usize, &str)] = &[
    (
        "default",
        HeadlineLevels::ORG_DEFAULT,
        1,
        "h2 L1 h3 L2 h4 L3 [ * L4 four [ * L5 five ] * L4b ] h2 M1",
    ),
    (
        "excluded-sibling",
        1,
        1,
        "h2 A [ * Kept * Kept too [ * Under ] ]",
    ),
    // The excluded `*` does not count: `**` is the top level.
    ("excluded-top", 1, 2, "h2 First [ * Under ]"),
    ("h0", 0, 1, "[ * A x [ * B ] * C ]"),
    (
        "h2",
        2,
        1,
        "h2 One Body one. h3 Two Body two. [ * Three a Body three a. [ * Four Body four. ] \
         * TODO Three b tag [ * a plain list ] ] h3 Two b",
    ),
    (
        "offset",
        1,
        2,
        "Intro. h2 Top [ * Under u [ * Deep ] ] h2 Top2",
    ),
];

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/headline-levels")
}

fn page(doc: &str, opts: &RenderOptions) -> String {
    let d = parse(doc, &ParseOptions::default());
    render_html(&d, &semantics(&d), opts, &Resolver::default())
}

fn opts(levels: Option<usize>, hlevel: u8) -> RenderOptions {
    RenderOptions::default()
        .with_toplevel_hlevel(ToplevelHlevel::new(hlevel).unwrap())
        .with_headline_levels(levels.map(HeadlineLevels::new))
}

/// What a page shows and how it nests, read the same way from Org's HTML
/// and MAE's: `hN` for a heading, `[` and `]` for a `<ul>`, `*` for an
/// `<li>`, then the words of the text (a tag ends a word); every other
/// tag, attribute and whitespace difference dropped (Org's tag padding
/// `&#xa0;` is a space).
fn skeleton(html: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut text = String::new();
    let mut rest = html;
    let flush = |text: &mut String, out: &mut Vec<String>| {
        let t = text.replace("&#xa0;", " ");
        out.extend(t.split_whitespace().map(str::to_string));
        text.clear();
    };
    while let Some(i) = rest.find('<') {
        text.push_str(&rest[..i]);
        let end = rest[i..].find('>').map_or(rest.len(), |e| i + e + 1);
        let tag = &rest[i + 1..end - 1];
        let name: String = tag
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '/')
            .collect();
        let mark = match name.as_str() {
            "ul" => Some("[".to_string()),
            "/ul" => Some("]".to_string()),
            "li" => Some("*".to_string()),
            h if h.len() == 2 && h.starts_with('h') && h.as_bytes()[1].is_ascii_digit() => {
                Some(h.to_string())
            }
            _ => None,
        };
        // Every tag ends a word: MAE writes no whitespace between blocks.
        flush(&mut text, &mut out);
        if let Some(m) = mark {
            out.push(m);
        }
        rest = &rest[end..];
    }
    text.push_str(rest);
    flush(&mut text, &mut out);
    out.join(" ")
}

/// `skeleton` with each heading `levels` deeper (at most h6): Org's page
/// as MAE tags its headings, from the stars rather than from the relative
/// level (registered: `heading-level-stars`).
fn deeper_headings(skeleton: &str, levels: usize) -> String {
    let deeper = |w: &str| match w.strip_prefix('h').and_then(|n| n.parse::<usize>().ok()) {
        Some(n) if w.len() == 2 => format!("h{}", (n + levels).min(6)),
        _ => w.to_string(),
    };
    skeleton
        .split(' ')
        .map(deeper)
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn low_level_headlines_render_as_ox_html_renders_them() {
    for (name, h, top, org) in ORG_PAGES {
        let doc = read(&dir().join(format!("{name}.org")));
        let mae = skeleton(&page(&doc, &opts(Some(*h), 2)));
        assert_eq!(mae, deeper_headings(org, top - 1), "{name}.org (H:{h})");
    }
}

#[test]
fn heading_tags_follow_the_stars_not_the_relative_level() {
    // Org tags `**` <h2> when it is the top level (hlevel 2); MAE, <h3>.
    // Which headlines are items is Org's either way.
    let doc = read(&dir().join("offset.org"));
    assert_eq!(
        skeleton(&page(&doc, &opts(Some(1), 2))),
        "Intro. h3 Top [ * Under u [ * Deep ] ] h3 Top2"
    );
}

#[test]
fn the_cap_is_counted_in_headline_levels_not_html_levels() {
    // `toplevel_hlevel` moves every heading tag; which headlines are list
    // items stays put. Org's own example of the pair: hlevel 2 with H:2
    // makes `**` an <h3> and `***` a list item.
    let doc = read(&dir().join("h2.org"));
    let at = |hlevel| skeleton(&page(&doc, &opts(Some(2), hlevel)));
    assert_eq!(
        at(1),
        "h1 One Body one. h2 Two Body two. [ * Three a Body three a. [ * Four Body four. ] \
         * TODO Three b tag [ * a plain list ] ] h2 Two b"
    );
    // Clamped at h6, the cap still at the third level.
    assert_eq!(
        at(6),
        "h6 One Body one. h6 Two Body two. [ * Three a Body three a. [ * Four Body four. ] \
         * TODO Three b tag [ * a plain list ] ] h6 Two b"
    );
}

#[test]
fn the_cap_is_off_by_default_and_large_caps_change_nothing() {
    let doc = read(&dir().join("default.org"));
    let default = page(&doc, &RenderOptions::default());
    assert!(!default.contains("org-headline"), "{default}");
    assert_eq!(
        skeleton(&default),
        "h1 L1 h2 L2 h3 L3 h4 L4 four h5 L5 five h4 L4b h1 M1"
    );
    // Five levels deep: a cap of 5 or more is the same page as none.
    for n in [5, 6, usize::MAX] {
        assert_eq!(page(&doc, &opts(Some(n), 1)), default, "H:{n}");
    }
    // One less, and the deepest is an item.
    assert_eq!(
        skeleton(&page(&doc, &opts(Some(4), 1))),
        "h1 L1 h2 L2 h3 L3 h4 L4 four [ * L5 five ] h4 L4b h1 M1"
    );
}

#[test]
fn from_level_counts_from_the_given_level() {
    // As Org exports a subtree: its root is relative level 1.
    let doc = "* A\n** B\n*** C\n**** D\n";
    let from = |level| {
        let o = RenderOptions::default()
            .with_headline_levels(Some(HeadlineLevels::new(1).from_level(level)));
        skeleton(&page(doc, &o))
    };
    assert_eq!(from(1), "h1 A [ * B [ * C [ * D ] ] ]");
    assert_eq!(from(2), "h1 A h2 B [ * C [ * D ] ]");
    assert_eq!(from(3), "h1 A h2 B h3 C [ * D ]");
    // A level deeper than any headline: every one shallower, a heading.
    assert_eq!(from(9), "h1 A h2 B h3 C h4 D");
    assert_eq!(HeadlineLevels::new(4).from_level(2).levels(), 4);
}

#[test]
fn a_low_level_headline_keeps_its_anchors_and_links_land_on_it() {
    let doc = "* A\n** B\n:PROPERTIES:\n:ID: node-b\n:END:\nbee\n\
               ** C\n:PROPERTIES:\n:CUSTOM_ID: cee\n:END:\n\
               * Links\n[[*B]] [[#cee]] [[id:node-b]]\n";
    let capped = |ids| {
        let o = RenderOptions::default()
            .with_headline_ids(ids)
            .with_headline_levels(Some(HeadlineLevels::new(1)));
        page(doc, &o)
    };
    let explicit = capped(HeadlineIds::Explicit);
    for want in [
        "<ul class=\"org-headlines\"><li class=\"org-headline org-level-2\" id=\"id-node-b\">\
         <div class=\"org-headline-title\" id=\"h-b\">B</div><p>bee</p></li>",
        "<li class=\"org-headline org-level-2\" id=\"cid-cee\">\
         <div class=\"org-headline-title\" id=\"h-c\">C</div></li></ul>",
    ] {
        assert!(explicit.contains(want), "{want} missing: {explicit}");
    }
    // Every same-page link lands on an id the page carries.
    let lands = |html: &str| {
        let mut n = 0;
        for part in html.split("href=\"#").skip(1) {
            let id = &part[..part.find('"').unwrap()];
            assert!(
                html.contains(&format!("id=\"{id}\"")),
                "#{id} lands nowhere: {html}"
            );
            n += 1;
        }
        n
    };
    assert_eq!(lands(&explicit), 3, "{explicit}");
    let slug = capped(HeadlineIds::Slug);
    assert!(
        !slug.contains("id=\"id-") && !slug.contains("cid-"),
        "{slug}"
    );
    assert_eq!(lands(&slug), 3, "{slug}");
}

#[test]
fn an_unbalanced_run_of_siblings_is_one_well_formed_list() {
    // `B` and `C` are both children of `A`, `C` shallower: Org closes its
    // list after `B` and opens none for `C` (unbalanced HTML). Here both
    // are items of one list, and the page balances.
    let html = page("* A\n***** B\n**** C\n", &opts(Some(1), 2));
    assert_eq!(skeleton(&html), "h2 A [ * B * C ]");
    assert_eq!(html.matches("<ul").count(), html.matches("</ul>").count());
    assert_eq!(html.matches("<li").count(), html.matches("</li>").count());
}

#[test]
fn a_plain_list_before_a_low_level_headline_is_not_joined() {
    // The headline list opens after a plain list ending the section: they
    // are two lists, the plain one inside the section.
    let html = page("* A\n- item\n** B\n", &opts(Some(1), 1));
    assert_eq!(skeleton(&html), "h1 A [ * item ] [ * B ]");
    assert!(html.contains("<ul><li>"), "{html}");
}

#[test]
#[ignore = "needs emacs; run locally"]
fn emacs_pages_match_the_recorded_skeletons() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/mae-org-headline-levels")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&out).unwrap();
    let el = repo_root().join("shared/org/conformance/macro-export.el");
    let args: Vec<std::ffi::OsString> = vec![
        "-Q".into(),
        "--batch".into(),
        "-l".into(),
        el.into(),
        dir().into(),
        out.clone().into(),
    ];
    let exit = run_emacs_bounded(
        &args,
        2 * 1024 * 1024,
        std::time::Duration::from_secs(120),
        std::process::Stdio::null(),
    );
    eprintln!("emacs: {exit:?}");
    let mut fixtures: Vec<String> = org_files(&dir())
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    fixtures.sort();
    let recorded: Vec<&str> = ORG_PAGES.iter().map(|(n, ..)| *n).collect();
    assert_eq!(
        fixtures, recorded,
        "every fixture is recorded, and only those"
    );
    for (name, .., org) in ORG_PAGES {
        let html = std::fs::read_to_string(out.join(format!("{name}.html")))
            .unwrap_or_else(|e| panic!("{name}: no Emacs page ({e})"));
        let got = skeleton(&html);
        eprintln!("{name}: {got}");
        assert_eq!(&got, org, "{name}.org: Emacs's page");
    }
    let _ = std::fs::remove_dir_all(&out);
}
