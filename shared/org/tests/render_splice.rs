//! `replace_body`, the splice the O4 write path lands on (ADR-092 D3): a
//! node's new body in the file's own line endings, always ending with one,
//! every other byte kept, and every other node (and this node's metadata)
//! unchanged when the result is parsed again; otherwise a refusal.

mod support;

use mae_org::{
    node_source, parse, replace_body, semantics, BodyEditError, LineEnding, ParseOptions,
    SemanticNode, Span,
};
use proptest::prelude::*;
use std::path::Path;

fn eol_of(e: LineEnding) -> &'static str {
    match e {
        LineEnding::Lf => "\n",
        LineEnding::CrLf => "\r\n",
        LineEnding::Cr => "\r",
    }
}

/// The test's own oracle for the bytes a successful edit must produce.
fn expected(text: &str, node: &SemanticNode, new_body: &str, eol: LineEnding) -> String {
    let end = eol_of(eol);
    let mut body = mae_org::with_line_ending(new_body, eol);
    if !body.is_empty() && !body.ends_with(end) {
        body.push_str(end);
    }
    let (s, e) = (node.body_span.start as usize, node.body_span.end as usize);
    let before = &text[..s];
    let prefix = if !body.is_empty() && !before.is_empty() && !before.ends_with(end) {
        end
    } else {
        ""
    };
    format!("{before}{prefix}{body}{}", &text[e..])
}

/// Checks one edit of `text`'s node `i`. Returns whether it was applied.
fn check_edit(text: &str, i: usize, new_body: &str) -> Result<bool, TestCaseError> {
    let doc = parse(text, &ParseOptions::default());
    let sem = semantics(&doc);
    let Some(node) = sem.nodes.get(i) else {
        return Ok(false);
    };
    match replace_body(text, &doc, node, new_body) {
        Ok(out) => {
            check_written(text, &sem.nodes, node, new_body, doc.line_ending(), &out)?;
            Ok(true)
        }
        Err(BodyEditError::HeadlineLine { .. }) => {
            let lines = new_body.replace('\r', "\n");
            prop_assert!(
                lines.lines().any(|l| l.starts_with('*')),
                "refused a body with no star line: {new_body:?}"
            );
            Ok(false)
        }
        Err(e) => {
            // Structure and line-ending refusals are allowed only for
            // bodies that could cause them.
            let plain = new_body
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '\n');
            // An item's body must stay indented under it: a plain line at
            // column 0 ends the item, and so its list (refused rightly).
            let item = node.origin == mae_org::NodeOrigin::Item;
            prop_assert!(!plain || item, "{e} for a plain body {new_body:?}");
            Ok(false)
        }
    }
}

/// A written edit: the exact bytes, the body read back, every other node.
fn check_written(
    text: &str,
    nodes: &[SemanticNode],
    node: &SemanticNode,
    new_body: &str,
    eol: LineEnding,
    out: &str,
) -> Result<(), TestCaseError> {
    prop_assert_eq!(out, &expected(text, node, new_body, eol));
    let again = parse(out, &ParseOptions::default());
    prop_assert_eq!(again.line_ending(), eol);
    let sem2 = semantics(&again);
    let edited = sem2.nodes.iter().find(|n| n.id == node.id);
    prop_assert!(edited.is_some(), "the node is gone:\n{out:?}");
    check_body_reads_back(&node_source(&again, edited.unwrap()), new_body, eol)?;
    // Every other node: the same, its spans moved by the edit.
    let (s, e) = (node.body_span.start, node.body_span.end);
    let delta = out.len() as i64 - text.len() as i64;
    let mv = |p: u32| if p >= e { (p as i64 + delta) as u32 } else { p };
    for n in nodes.iter().filter(|n| n.id != node.id) {
        prop_assert!(n.body_span.end <= s || n.body_span.start >= e);
        let m = sem2.nodes.iter().find(|m| m.id == n.id);
        prop_assert!(m.is_some(), "{} is gone", n.id);
        let m = m.unwrap();
        prop_assert_eq!(&m.title, &n.title);
        prop_assert_eq!(&m.properties, &n.properties);
        prop_assert_eq!(&m.own_tags, &n.own_tags);
        prop_assert_eq!(m.level, n.level);
        let span = |a: u32, b: u32| Span {
            start: mv(a),
            end: mv(b),
        };
        prop_assert_eq!(m.span, span(n.span.start, n.span.end));
        prop_assert_eq!(m.body_span, span(n.body_span.start, n.body_span.end));
    }
    Ok(())
}

/// The body read back is what was written (one final ending added when
/// missing; a CR a line ending in a CR file). Leading blank lines are the
/// drawer's post-blank to Org, so they may be missing; nothing else.
fn check_body_reads_back(
    lf_body: &str,
    new_body: &str,
    eol: LineEnding,
) -> Result<(), TestCaseError> {
    let mut want = new_body.to_string();
    if eol == LineEnding::Cr {
        want = want.replace('\r', "\n");
    }
    if !want.is_empty() && !want.ends_with('\n') {
        want.push('\n');
    }
    let at = want.find(lf_body);
    prop_assert!(at.is_some(), "{lf_body:?} not in {want:?}");
    let at = at.unwrap();
    let (head, tail) = (&want[..at], &want[at + lf_body.len()..]);
    prop_assert!(
        head.trim().is_empty() && tail.trim().is_empty(),
        "{lf_body:?} vs {want:?}"
    );
    Ok(())
}

/// Every corpus file (and the render fixtures) that has a node, in its
/// LF, CRLF and CR spellings.
fn corpus() -> Vec<String> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = support::org_files(&here.join("render"));
    files.extend(support::org_files(&here.join("semantics")));
    files.extend(support::bundled_corpus());
    let mut out = Vec::new();
    for f in files {
        let Ok(t) = std::fs::read_to_string(&f) else {
            continue;
        };
        if t.contains('\r') || t.len() > 20_000 {
            continue;
        }
        let d = parse(&t, &ParseOptions::default());
        if semantics(&d).nodes.is_empty() {
            continue;
        }
        out.push(t.replace('\n', "\r\n"));
        out.push(t.replace('\n', "\r"));
        out.push(t);
    }
    out
}

fn body() -> impl Strategy<Value = String> {
    let line = prop_oneof![
        4 => "[a-z ]{0,12}",
        1 => "[a-z]{1,5}\r[a-z]{0,3}",
        1 => "\\*{1,3} [a-z]{1,6}",
        1 => "- [a-z]{1,6}",
        1 => "#\\+[a-z_]{1,8}:? ?[a-z]{0,5}",
        1 => ":[A-Z]{1,6}:",
        1 => ",\\* [a-z]{1,4}",
    ];
    (
        prop::collection::vec(line, 0..5),
        prop_oneof![Just(""), Just("\n"), Just("\r")],
    )
        .prop_map(|(ls, tail)| format!("{}{tail}", ls.join("\n")))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(768))]

    #[test]
    fn a_body_edit_keeps_every_other_byte_and_node(
        pick in any::<prop::sample::Index>(),
        node in 0usize..6,
        new_body in body(),
    ) {
        let corpus = corpus_cached();
        let text = pick.get(corpus);
        check_edit(text, node, &new_body)?;
    }

    #[test]
    fn a_plain_body_is_always_written(
        pick in any::<prop::sample::Index>(),
        node in 0usize..6,
        new_body in "([a-z ]{0,10}\n){0,4}[a-z ]{0,10}",
    ) {
        let corpus = corpus_cached();
        let text = pick.get(corpus);
        let d = parse(text, &ParseOptions::default());
        if node < semantics(&d).nodes.len() {
            // A plain body is refused only for a list item (see check_edit).
            check_edit(text, node, &new_body)?;
        }
    }
}

fn corpus_cached() -> &'static Vec<String> {
    static C: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        let c = corpus();
        assert!(c.len() >= 30, "corpus too small: {}", c.len());
        c
    })
}

fn one(text: &str, new_body: &str) -> Result<String, BodyEditError> {
    let doc = parse(text, &ParseOptions::default());
    let sem = semantics(&doc);
    replace_body(text, &doc, &sem.nodes[0], new_body)
}

#[test]
fn the_cases_the_contract_names() {
    let lf = "* A\n:PROPERTIES:\n:ID: a\n:END:\nold\n* B\n";
    // No final newline: one is added, so B stays a headline.
    assert_eq!(
        one(lf, "new").unwrap(),
        "* A\n:PROPERTIES:\n:ID: a\n:END:\nnew\n* B\n"
    );
    // Empty: the body goes, B follows the drawer.
    assert_eq!(
        one(lf, "").unwrap(),
        "* A\n:PROPERTIES:\n:ID: a\n:END:\n* B\n"
    );
    // In the file's convention.
    let crlf = lf.replace('\n', "\r\n");
    assert_eq!(one(&crlf, "x\ny").unwrap(), crlf.replace("old", "x\r\ny"));
    let cr = lf.replace('\n', "\r");
    assert_eq!(one(&cr, "x\ny\n").unwrap(), cr.replace("old", "x\ry"));
    // A body ending in a CR: content in an LF file, then the line ending.
    assert_eq!(one(lf, "x\r").unwrap(), lf.replace("old\n", "x\r\n"));
    // A headline line is refused, at any depth, in any convention.
    for t in [lf.to_string(), crlf.clone(), cr.clone()] {
        assert_eq!(
            one(&t, "ok\n* B2\n"),
            Err(BodyEditError::HeadlineLine { line: 1 })
        );
        assert_eq!(
            one(&t, "*** deep"),
            Err(BodyEditError::HeadlineLine { line: 0 })
        );
    }
    // In a CR file a CR is a line ending: still a headline line.
    assert_eq!(
        one(&cr, "ok\r* B2"),
        Err(BodyEditError::HeadlineLine { line: 1 })
    );
    // Not a headline: no blank after the stars, or not at column 0.
    assert!(one(lf, "*bold* text\n ** indented\n").is_ok());
    // The text must be the document's.
    let doc = parse(lf, &ParseOptions::default());
    let sem = semantics(&doc);
    assert_eq!(
        replace_body("* other\n", &doc, &sem.nodes[0], "x"),
        Err(BodyEditError::StaleDocument)
    );
}

#[test]
fn a_headline_at_end_of_file_gets_its_line_ending_first() {
    let t = "* A\n:PROPERTIES:\n:ID: a\n:END:";
    let out = one(t, "body").unwrap();
    assert_eq!(out, format!("{t}\nbody\n"));
}

#[test]
fn an_edit_that_would_change_another_node_is_refused() {
    // An item's body that dedents ends the item and its list: the next
    // item node would change its place.
    let t =
        "- a\n  :PROPERTIES:\n  :ID: i\n  :END:\n  body\n- b\n  :PROPERTIES:\n  :ID: j\n  :END:\n";
    let doc = parse(t, &ParseOptions::default());
    let sem = semantics(&doc);
    let i = sem.nodes.iter().find(|n| n.id == "i").unwrap();
    let r = replace_body(t, &doc, i, "  fine\nnot indented\n");
    assert!(matches!(r, Err(BodyEditError::ChangesNode { .. })), "{r:?}");
    assert!(replace_body(t, &doc, i, "  fine\n").is_ok());
    // (`BodyEditError::LineEnding` guards a file whose only bare LF is in
    // the body; no node can be in such a file, since in an LF file every
    // drawer line that makes a node ends with a bare LF.)
}
