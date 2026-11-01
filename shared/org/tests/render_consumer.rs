//! The consumer's fixtures (`tests/consumer/`, review round 5), each
//! measured against Emacs 30.2 by the consumer (the header comments say
//! what Emacs exports). 01, 02, 08, 10 and 11 were exposures, fixed here;
//! 03–07 and 09 match Emacs, pinned as they are.

use mae_org::{
    node_text, parse, render, semantics, text, ParseOptions, RenderDiagnostic, RenderOptions,
    Resolved, Resolver, TreeFormat,
};

fn fixture(name: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/consumer")
        .join(name);
    std::fs::read_to_string(p).unwrap()
}

/// Every output of `doc` under `r`, as one string, with the diagnostics.
fn everything(doc: &str, r: &Resolver) -> (String, Vec<RenderDiagnostic>) {
    let d = parse(doc, &ParseOptions::default());
    let sem = semantics(&d);
    let mut all = String::new();
    let mut diags = Vec::new();
    for opts in [
        RenderOptions::default(),
        RenderOptions::default()
            .with_property_drawers(true)
            .with_headline_ids(mae_org::HeadlineIds::Slug),
    ] {
        let out = render(&d, &sem, &opts, r);
        all.push_str(&out.to_json(TreeFormat::Html));
        all.push_str(&out.to_json(TreeFormat::Hast));
        diags = out.diagnostics;
    }
    all.push_str(&text(&d, &sem));
    for n in &sem.nodes {
        all.push_str(&node_text(&d, &sem, &n.id).unwrap_or_default());
    }
    (all, diags)
}

fn html(doc: &str, r: &Resolver) -> String {
    let d = parse(doc, &ParseOptions::default());
    mae_org::render_html(&d, &semantics(&d), &RenderOptions::default(), r)
}

/// A (01) and E (11): a dedicated target's name reaches no output, its
/// anchor is opaque (`t-N`), and a link without a description landing on
/// it shows the placeholder (Emacs: "No description for this link").
#[test]
fn a_target_name_reaches_no_output() {
    let r = Resolver::default();
    for (name, needle) in [
        ("01-target-name-in-id.org", "SECRETtarget"),
        ("11-target-link-no-description.org", "SECRETbare"),
    ] {
        let doc = fixture(name);
        let body = doc
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let (all, _) = everything(&body, &r);
        assert!(!all.contains(needle), "{name}: {all}");
    }
    let h = html(&fixture("01-target-name-in-id.org"), &r);
    assert!(
        h.contains("<span class=\"org-target\" id=\"t-1\"></span>") && h.contains("href=\"#t-1\""),
        "{h}"
    );
    let h = html(&fixture("11-target-link-no-description.org"), &r);
    assert!(
        h.contains(
            "href=\"#t-1\" data-org-rel=\"references\" data-org-w=\"1\" data-org-c=\"1\">[?]</a>"
        ),
        "{h}"
    );
    // Numbered by first appearance; a second target of the same name (any
    // case or spacing) is the first.
    let h = html("A <<one>> B <<Two  x>> C <<ONE>> [[two x]].\n", &r);
    assert!(
        h.contains("id=\"t-1\"") && h.contains("id=\"t-2\"") && h.contains("href=\"#t-2\""),
        "{h}"
    );
}

/// C (02): a reference whose definition sits only in a COMMENT subtree
/// renders nothing (Emacs's export fails), with a footnote_undefined
/// diagnostic at the reference.
#[test]
fn a_footnote_without_a_definition_renders_nothing_and_says_so() {
    let doc = fixture("02-footnote-def-in-comment-subtree.org");
    let d = parse(&doc, &ParseOptions::default());
    let out = render(
        &d,
        &semantics(&d),
        &RenderOptions::default(),
        &Resolver::default(),
    );
    let h = mae_org::to_html(&out.body);
    assert!(
        h.contains("<p>A reference here.</p>") && !h.contains("fn:c"),
        "{h}"
    );
    let at = doc.rfind("reference[fn:c]").unwrap() + "reference".len();
    let want = mae_org::Span {
        start: at as u32,
        end: (at + 6) as u32,
    };
    assert_eq!(out.diagnostics, [RenderDiagnostic::FootnoteUndefined(want)]);
    assert!(out
        .to_json(TreeFormat::Html)
        .contains("\"kind\":\"footnote_undefined\""));
}

/// B (10): a relation that is no snake_case word and a weight that is no
/// finite number are left out, with a link_attribute diagnostic; valid
/// ones (and the defaults of absent ones) are kept.
#[test]
fn a_kb_links_refused_values_are_left_out_and_reported() {
    let id = "aaaaaaaa-0000-0000-0000-000000000002";
    let r = Resolver::default().with_target(id, Resolved::new("/r/a2"));
    let doc = fixture("10-rel-value-not-a-word.org");
    let d = parse(&doc, &ParseOptions::default());
    let out = render(&d, &semantics(&d), &RenderOptions::default(), &r);
    let h = mae_org::to_html(&out.body);
    assert!(
        h.contains("<a class=\"org-link\" href=\"/r/a2\" data-org-c=\"1\">typed</a>"),
        "{h}"
    );
    assert!(
        matches!(out.diagnostics[..], [RenderDiagnostic::LinkAttribute(_)]),
        "{:?}",
        out.diagnostics
    );
    let h = html(
        &format!(
            "[[id:{id}?rel=part_of&w=0.25&c=x][a]] [[id:{id}?rel=Teaches][b]] [[id:{id}][c]]\n"
        ),
        &r,
    );
    assert!(
        h.contains("data-org-rel=\"part_of\" data-org-w=\"0.25\">a</a>"),
        "{h}"
    );
    assert!(
        h.contains("href=\"/r/a2\" data-org-w=\"1\" data-org-c=\"1\">b</a>"),
        "{h}"
    );
    assert!(
        h.contains("data-org-rel=\"references\" data-org-w=\"1\" data-org-c=\"1\">c</a>"),
        "{h}"
    );
}

/// D (08): a citation's key is shown nowhere (Emacs: `(??, ????)`).
#[test]
fn a_citation_key_reaches_no_output() {
    let (all, _) = everything(&fixture("08-citation-key.org"), &Resolver::default());
    assert!(!all.contains("SECcitekey"), "{all}");
    let h = html(&fixture("08-citation-key.org"), &Resolver::default());
    assert!(
        h.contains("A citation <span class=\"org-citation\">(??, ????)</span> in the text."),
        "{h}"
    );
}

/// 03–07 and 09 match Emacs (each fixture's header): pinned as they are.
#[test]
fn the_fixtures_that_match_emacs_render_as_they_did() {
    let r = Resolver::default().with_target(
        "aaaaaaaa-0000-0000-0000-000000000001",
        Resolved::new("/r/a1"),
    );
    let cases = [
        ("03-file-link-in-description.org", "<a class=\"org-link\" href=\"https://example.org\">desc &lt;file:/home/usr/SECangle&gt; x</a>"),
        ("04-nested-link-in-description.org", "<a class=\"org-link\" href=\"https://example.org\">see [[file:/home/usr/SECnestedfile][x</a> y]] after."),
        ("05-property-macro-exposes-drawer.org", "<p>A /home/usr/SECprop C</p>"),
        ("06-keyword-macro-from-noexport.org", "<p>Value: SECkeywordvalue</p>"),
        ("07-https-without-slashes.org", "<a class=\"org-link\" href=\"https:///SECthree\">three slashes</a>"),
        ("09-fragment-on-id-link.org", "href=\"/r/a1#h-home-usr-secfrag\""),
    ];
    for (name, want) in cases {
        let h = html(&fixture(name), &r);
        assert!(h.contains(want), "{name}: {h}");
    }
}
