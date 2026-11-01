//! Where anchors are, and where a link lands (ADR-112 D3, ADR-030).
//!
//! Every rendered headline's heading carries `h-` + the slug of its title,
//! whatever [`HeadlineIds`] says; under `Explicit` the section also carries
//! the node's `id-` or the custom `cid-` anchor. A KB link's `#fragment`
//! (`id:NODE#FRAGMENT`, ADR-030) names a heading by its title: it maps to
//! `#h-<slug(fragment)>` on the target's page, and on this page to that
//! heading when the page has it, else to the node.

use mae_org::{
    heading_anchor, parse, render, render_html, semantics, slug, HeadlineIds, ParseOptions,
    RenderOptions, Resolved, Resolver, SLUG_MAX,
};
use proptest::prelude::*;

fn html_with(doc: &str, ids: HeadlineIds, resolver: &Resolver) -> String {
    let d = parse(doc, &ParseOptions::default());
    let opts = RenderOptions::default().with_headline_ids(ids);
    render_html(&d, &semantics(&d), &opts, resolver)
}

fn html(doc: &str) -> String {
    html_with(doc, HeadlineIds::Explicit, &Resolver::default())
}

fn resolver(pairs: &[(&str, &str)]) -> Resolver {
    let mut r = Resolver::default();
    for (id, href) in pairs {
        r.targets.insert(id.to_string(), Resolved::new(*href));
    }
    r
}

/// The href of the `<a>` whose text is `label`.
fn href_of<'a>(html: &'a str, label: &str) -> &'a str {
    let end = html
        .find(&format!(">{label}</a>"))
        .unwrap_or_else(|| panic!("no link {label}:\n{html}"));
    let start = html[..end].rfind("<a ").expect("an <a>");
    let a = &html[start..end];
    let h = a.find("href=\"").expect("an href") + 6;
    &a[h..h + a[h..].find('"').expect("closed")]
}

/// Every `href="#X"` names an `id="X"` of the same output.
fn same_page_links_land(html: &str) {
    for part in html.split("href=\"#").skip(1) {
        let id = &part[..part.find('"').expect("closed")];
        assert!(
            html.contains(&format!("id=\"{id}\"")),
            "#{id} lands nowhere:\n{html}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn slug_is_idempotent_and_well_formed(title in "\\PC{0,120}") {
        let s = slug(&title);
        prop_assert_eq!(slug(&s), s.clone());
        prop_assert!(s.len() <= SLUG_MAX, "{} > {}", s.len(), SLUG_MAX);
        prop_assert!(!s.starts_with('-') && !s.ends_with('-'), "{s:?}");
        prop_assert!(s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
        prop_assert!(!s.contains("--"));
    }

    #[test]
    fn slug_is_idempotent_on_long_wordy_titles(words in prop::collection::vec("[A-Za-z0-9]{1,20}", 1..12), sep in "[ ,.:;!?-]{1,3}") {
        let title = words.join(&sep);
        let s = slug(&title);
        prop_assert_eq!(slug(&s), s.clone());
        prop_assert!(s.len() <= SLUG_MAX && !s.ends_with('-'));
    }
}

#[test]
fn a_slug_at_the_limit_does_not_end_in_a_dash() {
    // 64 letters, a blank, then more: the separator must not be kept
    // without the character it separates.
    let title = format!("{} more", "a".repeat(SLUG_MAX));
    assert_eq!(slug(&title), "a".repeat(SLUG_MAX));
    let title = format!("{} b", "a".repeat(SLUG_MAX - 1));
    assert_eq!(slug(&title), "a".repeat(SLUG_MAX - 1));
}

#[test]
fn every_heading_carries_its_slug_anchor_whatever_the_mode() {
    let doc = "* Node one\n:PROPERTIES:\n:ID: n1\n:CUSTOM_ID: c1\n:END:\n* Custom\n:PROPERTIES:\n:CUSTOM_ID: c2\n:END:\n* Plain\n* Plain\n";
    let explicit = html(doc);
    for want in [
        "<section class=\"org-section org-level-1\" id=\"id-n1\"><h1 id=\"h-node-one\">",
        "<section class=\"org-section org-level-1\" id=\"cid-c2\"><h1 id=\"h-custom\">",
        "<section class=\"org-section org-level-1\"><h1 id=\"h-plain\">",
        "<h1 id=\"h-plain-2\">",
    ] {
        assert!(explicit.contains(want), "{want} missing:\n{explicit}");
    }
    let slugs = html_with(doc, HeadlineIds::Slug, &Resolver::default());
    assert!(
        !slugs.contains("id=\"id-") && !slugs.contains("id=\"cid-"),
        "{slugs}"
    );
    for want in [
        "<h1 id=\"h-node-one\">",
        "<h1 id=\"h-custom\">",
        "<h1 id=\"h-plain-2\">",
    ] {
        assert!(slugs.contains(want), "{want} missing:\n{slugs}");
    }
}

#[test]
fn a_cross_page_fragment_lands_on_the_target_heading() {
    let r = resolver(&[("x", "/notes/x.html"), ("y", "/notes/y.html#own")]);
    let doc = "[[id:x#Rope Internals][a]] [[id:x#rope-internals][b]] [[x#Rope   internals!][c]] [[id:y#rope][d]] [[id:x#¿?][e]]\n";
    let out = html_with(doc, HeadlineIds::Explicit, &r);
    // Written as the title or as its slug: the same anchor.
    for label in ["a", "b", "c"] {
        assert_eq!(href_of(&out, label), "/notes/x.html#h-rope-internals");
    }
    // The resolver's own fragment wins.
    assert_eq!(href_of(&out, "d"), "/notes/y.html#own");
    // A fragment with no letter or digit names no heading.
    assert_eq!(href_of(&out, "e"), "/notes/x.html");
    // On the target page, that anchor exists.
    let target = "* Rope internals\n";
    assert!(html(target).contains(&format!("id=\"{}\"", heading_anchor("Rope Internals"))));
}

#[test]
fn a_same_page_fragment_lands_on_the_heading_or_else_the_node() {
    let doc = "* Buffer\n:PROPERTIES:\n:ID: buf\n:END:\n** Rope internals\nText.\n* Links\n[[id:buf#rope-internals][there]] [[id:buf#missing][fallback]] [[id:buf][node]]\n";
    let explicit = html(doc);
    assert_eq!(href_of(&explicit, "there"), "#h-rope-internals");
    assert_eq!(href_of(&explicit, "fallback"), "#id-buf");
    assert_eq!(href_of(&explicit, "node"), "#id-buf");
    same_page_links_land(&explicit);
    let slugs = html_with(doc, HeadlineIds::Slug, &Resolver::default());
    assert_eq!(href_of(&slugs, "there"), "#h-rope-internals");
    assert_eq!(href_of(&slugs, "fallback"), "#h-buffer");
    same_page_links_land(&slugs);
}

#[test]
fn an_archived_headlines_custom_id_resolves_as_in_org() {
    // Org exports the archived headline (not its contents) with its
    // CUSTOM_ID as the anchor, and `[[#carch]]` lands there.
    let doc = "* C :ARCHIVE:\n:PROPERTIES:\n:CUSTOM_ID: carch\n:END:\nc body\n* D\n[[#carch][to archived]]\n";
    let out = html(doc);
    assert!(out.contains("id=\"cid-carch\""), "{out}");
    assert_eq!(href_of(&out, "to archived"), "#cid-carch");
    assert!(!out.contains("c body"), "{out}");
    same_page_links_land(&out);
}

#[test]
fn node_metadata_names_the_anchor_links_land_on() {
    let doc = "* Node\n:PROPERTIES:\n:ID: n1\n:END:\n";
    let d = parse(doc, &ParseOptions::default());
    let sem = semantics(&d);
    for (ids, want) in [
        (HeadlineIds::Explicit, "id-n1"),
        (HeadlineIds::Slug, "h-node"),
    ] {
        let opts = RenderOptions::default().with_headline_ids(ids);
        let r = render(&d, &sem, &opts, &Resolver::default());
        assert_eq!(r.nodes[0].anchor.as_deref(), Some(want));
    }
}
