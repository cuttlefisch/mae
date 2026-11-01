//! A node's body never holds another node (ADR-092's single write path:
//! writing one node's body must never rewrite another). Pinned by example
//! for each element kind, then as a property over generated documents and
//! the bundled corpus: the bodies of two nodes never overlap, and no body
//! intersects a node nested in its element.

mod support;

use mae_org::{parse, semantics, ParseOptions, Semantics};
use proptest::prelude::*;
use std::path::Path;

fn sem(text: &str) -> Semantics {
    semantics(&parse(text, &ParseOptions::default()))
}

fn body<'a>(text: &'a str, s: &Semantics, id: &str) -> &'a str {
    let n = s.nodes.iter().find(|n| n.id == id).unwrap();
    &text[n.body_span.range()]
}

#[test]
fn an_items_body_ends_at_the_first_nested_node() {
    let text = "- outer\n  :PROPERTIES:\n  :ID: o\n  :END:\n  Outer text.\n  - plain sub-item [[id:x]]\n  - inner\n    :PROPERTIES:\n    :ID: i\n    :END:\n    Inner text.\n  After inner [[id:y]].\n";
    let s = sem(text);
    // A sub-item without an id is part of the body around it.
    assert_eq!(
        body(text, &s, "o"),
        "  Outer text.\n  - plain sub-item [[id:x]]\n"
    );
    assert_eq!(body(text, &s, "i"), "    Inner text.\n");
    // Text after the nested node is in no body; its link is still the
    // innermost node's.
    let after = text.find("  After inner").unwrap();
    assert!(s
        .nodes
        .iter()
        .all(|n| !n.body_span.range().contains(&after)));
    let y = s.links.iter().find(|l| l.target == "y").unwrap();
    assert_eq!(y.from.as_deref(), Some("o"));
}

#[test]
fn a_headlines_body_runs_through_children_without_ids() {
    let text = "* P\n:PROPERTIES:\n:ID: p\n:END:\nP text.\n** No id\nchild text [[id:x]]\n** Q\n:PROPERTIES:\n:ID: q\n:END:\nQ text.\n** After\nafter text [[id:y]]\n* H\n:PROPERTIES:\n:ID: h\n:END:\nBefore.\n- it\n  :PROPERTIES:\n  :ID: it\n  :END:\n  x\nAfter.\n";
    let s = sem(text);
    // The id-less child is the parent's, for its text as for its links.
    assert_eq!(
        body(text, &s, "p"),
        "P text.\n** No id\nchild text [[id:x]]\n"
    );
    assert_eq!(body(text, &s, "q"), "Q text.\n");
    let from = |t: &str| {
        let l = s.links.iter().find(|l| l.target == t).unwrap();
        l.from.clone()
    };
    assert_eq!(from("x").as_deref(), Some("p"));
    assert_eq!(from("y").as_deref(), Some("p"));
    // An item node in a headline's section ends the headline's body.
    assert_eq!(body(text, &s, "h"), "Before.\n");
    assert_eq!(body(text, &s, "it"), "  x\n");
}

#[test]
fn the_files_body_runs_to_the_first_node() {
    let text = ":PROPERTIES:\n:ID: f\n:END:\n#+title: t\nFile.\n* No id\nText.\n* N\n:PROPERTIES:\n:ID: n\n:END:\n";
    let s = sem(text);
    assert_eq!(body(text, &s, "f"), "File.\n* No id\nText.\n");
    // A duplicate id is not a node, so it does not end a body.
    let dup = ":PROPERTIES:\n:ID: f\n:END:\nA.\n* D\n:PROPERTIES:\n:ID: f\n:END:\nB.\n";
    let s = sem(dup);
    assert_eq!(body(dup, &s, "f"), &dup[dup.find("A.").unwrap()..]);
}

/// The invariant, on one document.
fn assert_bodies_disjoint(text: &str, s: &Semantics, what: &str) {
    let overlap = |a: std::ops::Range<usize>, b: std::ops::Range<usize>| {
        a.start.max(b.start) < a.end.min(b.end)
    };
    for (i, a) in s.nodes.iter().enumerate() {
        for (j, b) in s.nodes.iter().enumerate() {
            if i == j {
                continue;
            }
            assert!(
                !overlap(a.body_span.range(), b.body_span.range()),
                "{what}: bodies of {} and {} overlap in {text:?}",
                a.id,
                b.id
            );
            let nested = a.span.start <= b.span.start && b.span.end <= a.span.end;
            assert!(
                !nested || !overlap(a.body_span.range(), b.span.range()),
                "{what}: the body of {} holds {} in {text:?}",
                a.id,
                b.id
            );
        }
    }
}

#[test]
fn no_two_bodies_overlap_over_the_corpus() {
    let mut files = support::bundled_corpus();
    files.extend(support::org_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/semantics"),
    ));
    files.extend(support::fixtures());
    let mut nodes = 0;
    for path in &files {
        let text = support::read(path);
        let s = sem(&text);
        nodes += s.nodes.len();
        assert_bodies_disjoint(&text, &s, &path.display().to_string());
    }
    assert!(nodes > 100, "the corpus walk found only {nodes} nodes");
}

/// One generated line group: a headline, an item, text, a blank line, a
/// footnote definition or reference, each possibly carrying an id (ids
/// repeat sometimes, so duplicates are exercised too).
fn piece() -> impl Strategy<Value = (u8, u8, bool, bool, u8)> {
    (0u8..6, 0u8..3, any::<bool>(), any::<bool>(), 0u8..12)
}

fn render(pieces: &[(u8, u8, bool, bool, u8)], file_id: bool) -> String {
    let mut out = String::new();
    if file_id {
        out.push_str(":PROPERTIES:\n:ID: file\n:END:\n");
    }
    for (i, &(kind, depth, has_id, flag, id)) in pieces.iter().enumerate() {
        let d = depth as usize;
        let drawer =
            |indent: &str| format!("{indent}:PROPERTIES:\n{indent}:ID: n{id}\n{indent}:END:\n");
        match kind {
            0 => {
                let tag = if flag { " :noexport:" } else { "" };
                out.push_str(&format!("{} H{i}{tag}\n", "*".repeat(d + 1)));
                if has_id {
                    out.push_str(&drawer(""));
                }
            }
            1 => {
                let indent = "  ".repeat(d);
                out.push_str(&format!("{indent}- item {i}\n"));
                if has_id {
                    out.push_str(&drawer(&format!("{indent}  ")));
                }
            }
            2 => out.push_str(&format!("{}text [[id:t{i}]]\n", "  ".repeat(d))),
            3 => out.push('\n'),
            4 if flag => out.push_str(&format!("[fn:{}] note [[id:f{i}]]\n", id % 3)),
            4 => out.push_str(&format!("ref[fn:{}]\n", id % 3)),
            _ => out.push_str(&format!("{}:LOGBOOK:\n:ID: l{i}\n:END:\n", "  ".repeat(d))),
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    #[test]
    fn no_two_bodies_overlap_over_generated_documents(
        pieces in proptest::collection::vec(piece(), 0..40),
        file_id in any::<bool>(),
    ) {
        let text = render(&pieces, file_id);
        let s = sem(&text);
        assert_bodies_disjoint(&text, &s, "generated");
        for n in &s.nodes {
            prop_assert!(n.span.start <= n.body_span.start && n.body_span.end <= n.span.end);
        }
    }
}
