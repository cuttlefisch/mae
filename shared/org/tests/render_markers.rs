//! Unrendered regions contribute nothing (ADR-112 D2/D3): every unrendered
//! construct in a generated document carries a unique marker as text, as an
//! id (`:ID:`, `CUSTOM_ID`, a target), as a title and as a link target that
//! the resolver DOES resolve (to an href holding the marker). No marker may
//! appear anywhere in the hast, the HTML, the node metadata or `text()`.

mod render_check;

use mae_org::{
    parse, render, semantics, text, HeadlineIds, ParseOptions, RenderOptions, Resolved, Resolver,
    TreeFormat,
};
use proptest::prelude::*;

/// Rendered content: never holds a marker (`QZX`).
const RENDERED: &[&str] = &[
    "Plain *text* with [[id:shown][a link]] and a note[fn:ok].\n",
    "- item\n- [X] item\n",
    "| a | b |\n",
    "<<<radio>>> and radio.\n",
    "#+begin_quote\nquoted\n#+end_quote\n",
    "[fn:ok] A referenced note.\n\n\n",
];

/// Unrendered constructs; `{M}` becomes a fresh marker. A subtree construct
/// is followed by a headline, so what comes after it is outside it.
const UNRENDERED: &[&str] = &[
    "# {M} [[id:{M}][{M}]] <<{M}>> <<<{M}>>> [fn:{M}]\n",
    "#+begin_comment\n{M} [[id:{M}][{M}]]\n:ID: {M}\n<<{M}>>\n#+end_comment\n",
    "#+begin_export html\n<a href=\"{M}\">{M}</a>\n#+end_export\n",
    ":LOGBOOK:\n{M} [[id:{M}][{M}]] <<{M}>> [[{M}]]\n:END:\n",
    "* COMMENT {M}\n:PROPERTIES:\n:ID: {M}\n:CUSTOM_ID: {M}\n:END:\n{M} [[id:{M}][{M}]] <<{M}>>\n#+TRANSCLUDE: {M}\n* Next\n",
    "* {M} :noexport:\n:PROPERTIES:\n:ID: {M}\n:END:\n{M} [[id:{M}]] <<{M}>>\n** {M}\n* Next\n",
    "* Kept :ARCHIVE:\n:PROPERTIES:\n:ID: {M}\n:CUSTOM_ID: {M}\n:END:\n{M} [[id:{M}][{M}]]\n** {M}\n* Next\n",
    "[fn:{M}] {M} [[id:{M}][{M}]] <<{M}>>\n\n\n",
    "#+HTML: <b id=\"{M}\">{M}</b>\n",
    "Snippet @@html:<a href=\"{M}\">{M}</a>@@ here.\n",
    "#+ATTR_HTML: :id {M} :onclick {M} :class {M}\nAttributed paragraph.\n",
    "Inline call_{M}({M}) here.\n",
];

/// A document: pieces in order, each unrendered one with its own marker.
fn marked_doc() -> impl Strategy<Value = (String, Vec<String>)> {
    let piece = prop_oneof![
        prop::sample::select(RENDERED).prop_map(|s| (s, false)),
        prop::sample::select(UNRENDERED).prop_map(|s| (s, true)),
    ];
    prop::collection::vec(piece, 1..20).prop_map(|pieces| {
        let mut doc = String::new();
        let mut markers = Vec::new();
        for (i, (p, marked)) in pieces.into_iter().enumerate() {
            if marked {
                let m = format!("QZX{i:03}QZX");
                doc.push_str(&p.replace("{M}", &m));
                markers.push(m);
            } else {
                doc.push_str(p);
            }
        }
        (doc, markers)
    })
}

/// The same, with export selection: only the `:pick:` tree is rendered, so
/// the zeroth section and every other tree are unselected.
fn selected_doc() -> impl Strategy<Value = (String, Vec<String>)> {
    marked_doc().prop_map(|(body, mut markers)| {
        let m = "QZXSELQZX".to_string();
        let doc = format!(
            "#+SELECT_TAGS: pick\n{m} zeroth [[id:{m}]] <<{m}>>\n* Other {m}\n:PROPERTIES:\n:ID: {m}\n:END:\n{m}\n* Keep :pick:\n{body}"
        );
        markers.push(m);
        (doc, markers)
    })
}

/// A resolver that resolves every marker (and `shown`) to an href holding
/// it: a leaked link would carry its marker into the output.
fn resolver(markers: &[String]) -> Resolver {
    let mut r = Resolver::default();
    for m in markers.iter().chain(std::iter::once(&"shown".to_string())) {
        let href = format!("/{m}.html");
        r.targets.insert(
            m.clone(),
            Resolved {
                href,
                title: Some(m.clone()),
            },
        );
    }
    r
}

/// Every output of `doc`, as one string per surface.
fn outputs(doc: &str, markers: &[String]) -> Vec<(String, String)> {
    let d = parse(doc, &ParseOptions::default());
    let sem = semantics(&d);
    let r = resolver(markers);
    let mut out = Vec::new();
    for opts in [
        RenderOptions::new(HeadlineIds::Explicit, true, true),
        RenderOptions::new(HeadlineIds::Slug, false, false),
    ] {
        let rendered = render(&d, &sem, &opts, &r);
        out.push(("hast".into(), rendered.to_json(TreeFormat::Hast)));
        out.push(("html".into(), rendered.to_json(TreeFormat::Html)));
        out.push(("body html".into(), mae_org::to_html(&rendered.body)));
    }
    out.push(("text".into(), text(&d, &sem)));
    out
}

fn assert_no_marker(doc: &str, markers: &[String]) -> Result<(), TestCaseError> {
    for (surface, s) in outputs(doc, markers) {
        for m in markers {
            prop_assert!(
                !s.contains(m.as_str()),
                "{m} leaked into {surface}:\n{doc}\n---\n{s}"
            );
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn no_marker_from_an_unrendered_region_reaches_any_output((doc, markers) in marked_doc()) {
        assert_no_marker(&doc, &markers)?;
    }

    #[test]
    fn no_marker_from_an_unselected_tree_reaches_any_output((doc, markers) in selected_doc()) {
        assert_no_marker(&doc, &markers)?;
    }
}

/// The generator is not vacuous: rendered content does reach the output
/// (a renderer that drew nothing would pass the property above).
#[test]
fn rendered_content_does_reach_the_output() {
    let doc = format!(
        "{}{}",
        RENDERED.concat(),
        UNRENDERED[0].replace("{M}", "QZX1QZX")
    );
    let outs = outputs(&doc, &["QZX1QZX".to_string()]);
    let html = &outs[2].1;
    for want in [
        "<strong>text</strong>",
        "href=\"/shown.html\"",
        "quoted",
        "A referenced note",
    ] {
        assert!(html.contains(want), "{want} missing: {html}");
    }
}
