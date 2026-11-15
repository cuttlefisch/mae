//! `RenderOptions::toplevel_hlevel`: Org's `org-html-toplevel-hlevel`. Only the
//! heading tag moves; everything else a page links or styles by stays put.

use mae_org::{
    parse, render_html, semantics, HeadlineIds, ParseOptions, RenderOptions, Resolver,
    ToplevelHlevel,
};

const DOC: &str = "* One\n** Two\n*** Three\n**** Four\n***** Five\n****** Six\n******* Seven\n";

fn html(doc: &str, level: u8) -> String {
    let d = parse(doc, &ParseOptions::default());
    let opts = RenderOptions::default()
        .with_headline_ids(HeadlineIds::Slug)
        .with_toplevel_hlevel(ToplevelHlevel::new(level).unwrap());
    render_html(&d, &semantics(&d), &opts, &Resolver::default())
}

/// The heading tags in document order.
fn heading_tags(html: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("<h") {
        let t = &rest[i + 1..i + 3];
        if t.as_bytes()[1].is_ascii_digit() {
            tags.push(t.to_string());
        }
        rest = &rest[i + 3..];
    }
    tags
}

#[test]
fn the_level_map_is_exact_and_clamps_at_h6() {
    assert_eq!(
        heading_tags(&html(DOC, 1)),
        ["h1", "h2", "h3", "h4", "h5", "h6", "h6"]
    );
    assert_eq!(
        heading_tags(&html(DOC, 2)),
        ["h2", "h3", "h4", "h5", "h6", "h6", "h6"]
    );
    assert_eq!(
        heading_tags(&html(DOC, 6)),
        ["h6", "h6", "h6", "h6", "h6", "h6", "h6"]
    );
}

#[test]
fn the_default_is_one_so_existing_output_is_unchanged() {
    let d = parse(DOC, &ParseOptions::default());
    let opts = RenderOptions::default().with_headline_ids(HeadlineIds::Slug);
    let default = render_html(&d, &semantics(&d), &opts, &Resolver::default());
    assert_eq!(default, html(DOC, 1));
    assert_eq!(ToplevelHlevel::default().get(), 1);
}

#[test]
fn classes_sections_and_anchors_do_not_move() {
    // Everything but the digit of each heading tag is identical at every level.
    let erase = |s: String| {
        let mut out = s;
        for n in 1..=6 {
            out = out
                .replace(&format!("<h{n}"), "<hX")
                .replace(&format!("</h{n}>"), "</hX>");
        }
        out
    };
    let base = erase(html(DOC, 1));
    assert!(base.contains("org-level-7"), "{base}");
    for level in 2..=6 {
        assert_eq!(erase(html(DOC, level)), base, "level {level}");
    }
}

#[test]
fn only_one_to_six_exist() {
    assert!(ToplevelHlevel::new(0).is_none());
    assert!(ToplevelHlevel::new(7).is_none());
    assert!(ToplevelHlevel::new(255).is_none());
    for n in 1..=6 {
        assert_eq!(ToplevelHlevel::new(n).unwrap().get(), n);
    }
}
