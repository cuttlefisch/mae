//! The HTML serialiser's output parses back, with an HTML5 parser, to the
//! element tree it came from: the tree is what a browser builds, so the
//! hast a consumer embeds and the HTML it publishes cannot disagree.
//!
//! html5ever (Servo's WHATWG tree builder) rather than a hand-written
//! re-parser: the failure modes are the parsing algorithm's (a `<p>` closed
//! by a block element inside it, text foster-parented out of a table, the
//! newline a `<pre>` swallows, CR, NUL), which a parser written alongside
//! the serialiser would share its blind spots on.

mod render_check;
mod support;

use html5ever::tendril::TendrilSink;
use html5ever::{local_name, ns, parse_fragment, ParseOpts, QualName};
use mae_org::{parse, render, semantics, HeadlineIds, ParseOptions, RenderOptions};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use proptest::prelude::*;
use render_check::{adversarial_resolver, from_tree, kb_targets, Dom};
use std::path::Path;

/// `html` parsed as the contents of a `<body>`.
fn parse_html(html: &str) -> Vec<Dom> {
    let ctx = QualName::new(None, ns!(html), local_name!("body"));
    let dom = parse_fragment(RcDom::default(), ParseOpts::default(), ctx, vec![], false).one(html);
    // The fragment parser wraps the result in an <html> element.
    let root = dom.document.children.borrow()[0].clone();
    let out = convert(&root.children.borrow());
    out
}

fn convert(nodes: &[Handle]) -> Vec<Dom> {
    let mut out: Vec<Dom> = Vec::new();
    for n in nodes {
        match &n.data {
            NodeData::Text { contents } => {
                let t = contents.borrow().to_string();
                match out.last_mut() {
                    Some(Dom::Text(prev)) => prev.push_str(&t),
                    _ => out.push(Dom::Text(t)),
                }
            }
            NodeData::Element { name, attrs, .. } => {
                let attrs = attrs
                    .borrow()
                    .iter()
                    .map(|a| (a.name.local.to_string(), a.value.to_string()))
                    .collect();
                let children = convert(&n.children.borrow());
                out.push(Dom::El(name.local.to_string(), attrs, children));
            }
            _ => {}
        }
    }
    out
}

fn round_trips(text_in: &str) -> Result<(), String> {
    let doc = parse(text_in, &ParseOptions::default());
    let sem = semantics(&doc);
    let resolver = adversarial_resolver(&kb_targets(&sem));
    for opts in [
        RenderOptions::default(),
        RenderOptions::new(HeadlineIds::Slug, true, true),
    ] {
        let out = render(&doc, &sem, &opts, &resolver);
        let mut trees = vec![out.body.clone()];
        for n in &out.nodes {
            trees.push(n.title.clone());
            trees.extend(n.aliases.iter().cloned());
            trees.extend(n.properties.iter().map(|(_, v)| v.clone()));
        }
        for t in trees {
            let html = mae_org::to_html(&t);
            let back = parse_html(&html);
            if back != from_tree(&t) {
                return Err(format!("HTML does not parse back to its tree:\n{html}"));
            }
        }
    }
    Ok(())
}

#[test]
fn corpus_html_parses_back_to_its_tree() {
    let mut files = support::bundled_corpus();
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    for d in ["fixtures", "semantics", "render"] {
        files.extend(support::org_files(&here.join(d)));
    }
    let mut failures = Vec::new();
    for f in &files {
        let Ok(t) = std::fs::read_to_string(f) else {
            continue;
        };
        if let Err(e) = round_trips(&t) {
            failures.push(format!("{}: {}", support::rel(f), &e[..e.len().min(2000)]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The parser is not lenient here: a tree a browser would restructure is
/// caught (a block element inside a paragraph is closed out of it).
#[test]
fn the_parse_back_check_catches_a_restructured_tree() {
    let html = "<p>a<div>b</div></p>";
    let back = parse_html(html);
    let want = vec![Dom::El(
        "p".into(),
        vec![],
        vec![
            Dom::Text("a".into()),
            Dom::El("div".into(), vec![], vec![Dom::Text("b".into())]),
        ],
    )];
    assert_ne!(back, want);
    // And CR is normalised, which is why the tree holds none.
    assert_eq!(parse_html("<p>a\r\nb\rc</p>"), parse_html("<p>a\nb\nc</p>"));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_text_parses_back_to_its_tree(doc in any::<String>()) {
        prop_assert!(round_trips(&doc).is_ok(), "{:?}", round_trips(&doc));
    }

    #[test]
    fn org_shaped_text_parses_back_to_its_tree(
        doc in prop::collection::vec(prop::sample::select(PIECES), 0..20).prop_map(|v| v.concat())
    ) {
        prop_assert!(round_trips(&doc).is_ok(), "{:?}", round_trips(&doc));
    }
}

const PIECES: &[&str] = &[
    "* H :t:\n",
    "text <b>&amp; \"q\" 'a'\n",
    "\u{0}nul\u{1}\r\n",
    "\u{feff}bom\n",
    "#+begin_src x\n\nleading blank line\n#+end_src\n",
    ": \n",
    "| a | <b> |\n|-\n| c |\n",
    "- a\n  - b\n- c :: d\n",
    "#+begin_verse\n  x\n\n y\n#+end_verse\n",
    "[[https://x.org][x]]\n",
    "[fn:1] note\n\n",
    "ref[fn:1]\n",
    "\\alpha \\nbsp \\\\\n",
    "<<<r>>> r\n",
    "#+CAPTION: c\nx\n",
];
