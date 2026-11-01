//! `replace_body`, the splice the O4 write path lands on (ADR-092 D3): a
//! node's new body in the file's own line endings, always ending with one,
//! every other byte kept, and every other node (and this node's metadata)
//! unchanged when the result is parsed again; otherwise a refusal.

mod support;

use mae_org::{
    node_source, parse, semantics, BodyEditError, LineEnding, ParseOptions, SemanticNode,
    SemanticsOptions, Span,
};

/// The splice under default options (those the documents here are read
/// with).
fn replace_body(
    text: &str,
    doc: &mae_org::Document,
    node: &SemanticNode,
    new_body: &str,
) -> Result<String, BodyEditError> {
    mae_org::replace_body(
        text,
        doc,
        node,
        new_body,
        &ParseOptions::default(),
        &SemanticsOptions::default(),
    )
}
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
        // Removing a name the rest of the file can resolve is refused
        // whatever the new body: the old body must have held it.
        Err(BodyEditError::ChangesName { name, .. }) => {
            let old = &text[node.body_span.start as usize..node.body_span.end as usize];
            prop_assert!(
                old.contains(name.as_str()) || new_body.contains(name.as_str()),
                "refused for a name {name:?} neither body holds"
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
        let span = |a: u32, b: u32| Span {
            start: mv(a),
            end: mv(b),
        };
        // The whole node (id, title, TODO keyword, priority, tags,
        // properties, aliases, kind, level), its spans moved.
        let mut want = n.clone();
        want.span = span(n.span.start, n.span.end);
        want.body_span = span(n.body_span.start, n.body_span.end);
        prop_assert_eq!(m, &want);
    }
    // Every link of another node, its span moved (review M1: links were not
    // compared, so a `#+LINK:` in one body retargeted another's).
    let before = semantics(&parse(text, &ParseOptions::default()));
    let other = |l: &&mae_org::SemanticLink| l.from.as_deref() != Some(node.id.as_str());
    let was: Vec<_> = before
        .links
        .iter()
        .filter(other)
        .map(|l| {
            let mut l = l.clone();
            l.span = Span {
                start: mv(l.span.start),
                end: mv(l.span.end),
            };
            l
        })
        .collect();
    let now: Vec<_> = sem2.links.iter().filter(other).cloned().collect();
    prop_assert_eq!(was, now);
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

/// Review M1: the check re-read the file with its OLD in-file TODO
/// keywords, compared nodes but not links, and found a repeated `:ID:`
/// only when it came first. Each of these edits changes the file outside
/// the body, and must be refused.
#[test]
fn an_edit_that_changes_what_the_file_declares_or_links_is_refused() {
    let edit = |text: &str, id: &str, body: &str| {
        let doc = parse(text, &ParseOptions::default());
        let sem = semantics(&doc);
        let n = sem.nodes.iter().find(|n| n.id == id).expect("node");
        replace_body(text, &doc, n, body)
    };
    // The reviewer's: removing a #+TODO: in one body un-keywords another
    // headline's WAIT.
    let todo = "* Setup\n:PROPERTIES:\n:ID: a\n:END:\n#+TODO: WAIT | GONE\n* WAIT Other task\n:PROPERTIES:\n:ID: b\n:END:\ntext\n";
    assert!(matches!(
        edit(todo, "a", "no keywords now\n"),
        Err(BodyEditError::ChangesDeclaration { .. })
    ));
    // Kept, it is fine.
    assert!(edit(todo, "a", "#+TODO: WAIT | GONE\nmore\n").is_ok());
    // Adding one is refused too.
    assert!(matches!(
        edit(todo, "b", "#+TODO: X | Y\n"),
        Err(BodyEditError::ChangesDeclaration { .. })
    ));
    // The reviewer's: a #+LINK abbreviation added in one body retargets
    // another node's link.
    let link = "* A\n:PROPERTIES:\n:ID: a\n:END:\nbody a\n* B\n:PROPERTIES:\n:ID: b\n:END:\nSee [[kb:target-node][x]].\n";
    assert!(matches!(
        edit(link, "a", "#+LINK: kb id:\n"),
        Err(BodyEditError::ChangesDeclaration { .. })
    ));
    // A radio target in one body makes a link in another.
    let radio = "* A\n:PROPERTIES:\n:ID: a\n:END:\nbody a\n* B\n:PROPERTIES:\n:ID: b\n:END:\nThe widget is here.\n";
    assert_eq!(
        edit(radio, "a", "<<<widget>>> defined\n"),
        Err(BodyEditError::ChangesName {
            kind: "radio target".into(),
            name: "widget".into()
        })
    );
    // A link of another node that the semantics read differently.
    let kb =
        "* A\n:PROPERTIES:\n:ID: a\n:END:\nbody a\n* B\n:PROPERTIES:\n:ID: b\n:END:\nSee [[x]].\n";
    // A target another node's `[[x]]` would land on (review round 2: it
    // used to be accepted, as the semantics' links did not change).
    assert_eq!(
        edit(kb, "a", "<<x>> here\n"),
        Err(BodyEditError::ChangesName {
            kind: "target".into(),
            name: "x".into()
        })
    );
    assert!(edit(radio, "a", "widget, not defined\n").is_ok());
    // An id another element holds, after it and before it.
    let ids = "* A\n:PROPERTIES:\n:ID: a\n:END:\nbody\n* B\n:PROPERTIES:\n:ID: b\n:END:\nbody\n";
    let item = |id: &str| format!("- x\n  :PROPERTIES:\n  :ID: {id}\n  :END:\n");
    for (node, id) in [("a", "b"), ("b", "a"), ("a", "a")] {
        assert_eq!(
            edit(ids, node, &item(id)),
            Err(BodyEditError::DuplicateId { id: id.into() }),
            "{node} gets {id}"
        );
    }
    // A new id nothing refers to is a new node nested in the body, which
    // would end the body at it: refused, though not as a duplicate.
    assert_eq!(
        edit(ids, "a", &item("c")),
        Err(BodyEditError::ChangesNode { id: "a".into() })
    );
}

/// The options the check reads with are the caller's: a body that keeps
/// another node's keyword under the caller's TODO keywords is fine, and
/// one that changes it under them is refused.
#[test]
fn the_check_reads_with_the_callers_options() {
    let text = "* A\n:PROPERTIES:\n:ID: a\n:END:\nbody\n* NEXT B\n:PROPERTIES:\n:ID: b\n:END:\n";
    let opts = ParseOptions::default()
        .with_todo_sequences(vec![mae_org::TodoSequence::new(["NEXT"], ["DONE"])]);
    let doc = parse(text, &opts);
    let sem = mae_org::semantics_with(&doc, &SemanticsOptions::default());
    let a = sem.nodes.iter().find(|n| n.id == "a").unwrap();
    let sopts = SemanticsOptions::default();
    assert!(mae_org::replace_body(text, &doc, a, "new\n", &opts, &sopts).is_ok());
    // An in-body declaration replaces the caller's keywords: B's NEXT
    // stops being one.
    assert!(matches!(
        mae_org::replace_body(text, &doc, a, "#+TODO: X | Y\n", &opts, &sopts),
        Err(BodyEditError::ChangesDeclaration { .. })
    ));
}

/// Review round 2 (M1): the body may not add, remove or change a name the
/// rest of the file can resolve: another node's rendering would change
/// (its footnote, its `[[target]]`, a `#+CALL:` of a block, results found by
/// name). The reviewer's case: one edit adds all three, and the other
/// node's hidden `PRIVATE-RESULT` would have been rendered.
#[test]
fn an_edit_that_adds_or_removes_a_name_is_refused() {
    let text = "* Editable\n:PROPERTIES:\n:ID: e1\n:END:\nold body\n\n* Other\n:PROPERTIES:\n:ID: o1\n:END:\nSee note[fn:x] and [[tgt]].\n\n#+CALL: fn()\n\n#+RESULTS:\n: PRIVATE-RESULT\n";
    let edit = |t: &str, body: &str| {
        let doc = parse(t, &ParseOptions::default());
        let sem = semantics(&doc);
        let n = sem.nodes.iter().find(|n| n.id == "e1").expect("e1");
        replace_body(t, &doc, n, body)
    };
    let s1 = "new body <<tgt>>\n\n#+NAME: fn\n#+begin_src elisp :exports none\n1\n#+end_src\n\n[fn:x] INJECTED footnote text\n";
    assert!(
        matches!(edit(text, s1), Err(BodyEditError::ChangesName { .. })),
        "{:?}",
        edit(text, s1)
    );
    // Each a name the rest of the file refers to (`[fn:x]`, `[[tgt]]`,
    // `#+CALL: fn()`), or one it names itself (`CUSTOM_ID` of Other).
    let text = text.replace(":ID: o1\n", ":ID: o1\n:CUSTOM_ID: c\n");
    let text = text.as_str();
    let cases = [
        ("footnote", "x", "[fn:x] a definition\n"),
        ("footnote", "x", "inline[fn:x:defined here]\n"),
        ("target", "tgt", "a <<tgt>> here\n"),
        ("name", "fn", "#+NAME: fn\n| a |\n"),
        ("name", "tgt", "#+TBLNAME: tgt\n| a |\n"),
        ("name", "tgt", "#+SRCNAME: tgt\n| a |\n"),
        (
            "CUSTOM_ID",
            "c",
            "- x\n  :PROPERTIES:\n  :CUSTOM_ID: c\n  :END:\n",
        ),
    ];
    for (kind, name, body) in cases {
        assert_eq!(
            edit(text, body),
            Err(BodyEditError::ChangesName {
                kind: kind.into(),
                name: name.into()
            }),
            "{body:?}"
        );
    }
    // Removing one is refused too; keeping it is fine.
    let with = text.replace("old body", "old <<tgt>> body");
    assert_eq!(
        edit(&with, "gone\n"),
        Err(BodyEditError::ChangesName {
            kind: "target".into(),
            name: "tgt".into()
        })
    );
    assert!(edit(&with, "kept <<tgt>> still\n").is_ok());
    assert!(edit(text, "plain new words\n").is_ok());
    // Round 3: a name the body defines and only the body refers to (or
    // nothing does) is the body's own; so are a new paragraph and a
    // footnote whose reference and definition are both in the body.
    for body in [
        "plain new words\n\nand a second paragraph\n",
        "a <<own>> target, see [[own]]\n",
        "a <<<nobody>>> radio target\n",
        "#+NAME: unused\n| a |\n",
        "#+RESULTS: lonely\n: r\n",
        "a note[fn:mine] here\n\n[fn:mine] the body's own note\n",
    ] {
        let r = edit(text, body);
        assert!(r.is_ok(), "{body:?}: {r:?}");
    }
}

/// The last line of defence, which no precise check covers: every other
/// node renders as before. Rewording a footnote definition the body holds
/// changes no name, but another node that references it shows the new
/// text.
#[test]
fn an_edit_that_changes_how_another_node_renders_is_refused() {
    let text = "* Editable\n:PROPERTIES:\n:ID: e1\n:END:\nold body\n\n[fn:x] the note as it was\n\n* Other\n:PROPERTIES:\n:ID: o1\n:END:\nSee note[fn:x].\n";
    let doc = parse(text, &ParseOptions::default());
    let sem = semantics(&doc);
    let e1 = sem.nodes.iter().find(|n| n.id == "e1").unwrap();
    assert_eq!(
        replace_body(text, &doc, e1, "new body\n\n[fn:x] the note reworded\n"),
        Err(BodyEditError::ChangesRendering {
            what: "the page".into()
        })
    );
    // The same definition, kept as it was: fine.
    assert!(replace_body(text, &doc, e1, "new body\n\n[fn:x] the note as it was\n").is_ok());
}

/// Round 3 (the second reviewer): removing the link comparison was wrong.
/// A footnote definition's links belong to the node that references it,
/// and a rendering does not show every field the semantics give a link:
/// retargeting `[[id:one]]` in A's definition of B's note changes B's
/// links. And an attribute no rendering shows (`?k=`) changes them too:
/// only the link comparison sees that.
#[test]
fn an_edit_that_changes_another_nodes_links_is_refused() {
    let text = "* A\n:PROPERTIES:\n:ID: a\n:END:\nBody A.\n\n[fn:x] See [[id:one?k=v1][it]].\n* B\n:PROPERTIES:\n:ID: b\n:END:\nBody B[fn:x].\n";
    let doc = parse(text, &ParseOptions::default());
    let sem = semantics(&doc);
    let a = sem.nodes.iter().find(|n| n.id == "a").unwrap();
    for body in [
        "Body A.\n\n[fn:x] See [[id:two?k=v1][it]].\n",
        "Body A.\n\n[fn:x] See [[id:one?k=v2][it]].\n",
    ] {
        assert_eq!(
            replace_body(text, &doc, a, body),
            Err(BodyEditError::ChangesLinks {
                id: Some("b".into())
            }),
            "{body:?}"
        );
    }
    assert!(replace_body(
        text,
        &doc,
        a,
        "Body A, reworded.\n\n[fn:x] See [[id:one?k=v1][it]].\n"
    )
    .is_ok());
}

/// Round 3 (the second reviewer): accepted edits that changed the output
/// OUTSIDE every KB node, or the edited node's own metadata, or published
/// a footnote definition elsewhere. Each document has a SECRET the edit
/// would publish; each edit is refused.
#[test]
fn an_edit_that_changes_anything_rendered_outside_its_body_is_refused() {
    let named = "#+NAME: fn\n#+begin_src sh\nprint(1)\n#+end_src\n";
    let cases: &[(&str, &str, &str)] = &[
        // A `#+CALL:` under a heading that is no node: the called block's
        // language decides `header-args:python :exports none`.
        (
            "#+PROPERTY: header-args:python :exports none\n* Not a node\n#+CALL: fn()\n\n#+RESULTS:\n: SECRETRESULT\n\n* A\n:PROPERTIES:\n:ID: a\n:END:\n#+NAME: fn\n#+begin_src python\nprint(1)\n#+end_src\n",
            "a",
            named,
        ),
        (
            "#+PROPERTY: header-args:python :exports none\n* Title call_fn() {{{results(=SECRETINL=)}}}\nText call_fn() {{{results(=SECRETINL2=)}}} end.\n* A\n:PROPERTIES:\n:ID: a\n:END:\n#+NAME: fn\n#+begin_src python\nprint(1)\n#+end_src\n",
            "a",
            named,
        ),
        (
            "#+PROPERTY: header-args:python :exports none\n#+CALL: fn()\n\n#+RESULTS:\n: SECRETZERO\n\n* A\n:PROPERTIES:\n:ID: a\n:END:\n#+NAME: fn\n#+begin_src python\nprint(1)\n#+end_src\n",
            "a",
            named,
        ),
        // The edited node's own title and alias.
        (
            "#+PROPERTY: header-args:python :exports none\n* A call_fn() {{{results(=SECRETOWN=)}}}\n:PROPERTIES:\n:ID: a\n:ALIASES: x call_fn() {{{results(=SECRETALIAS=)}}}\n:END:\n#+NAME: fn\n#+begin_src python\nprint(1)\n#+end_src\n",
            "a",
            named,
        ),
        // A block's :exports decides named results elsewhere.
        (
            "* Not a node\n#+RESULTS: fn\n: SECRETNAMED\n\n* A\n:PROPERTIES:\n:ID: a\n:END:\n#+NAME: fn\n#+begin_src sh :exports code\necho hi\n#+end_src\n",
            "a",
            "#+NAME: fn\n#+begin_src sh :exports both\necho hi\n#+end_src\n",
        ),
        // A reference added in the body publishes another node's, or the
        // zeroth section's, definition.
        (
            "* A\n:PROPERTIES:\n:ID: a\n:END:\nBody A.\n* B\n:PROPERTIES:\n:ID: b\n:END:\nBody B.\n\n[fn:x] Def SECRETDEF with [[id:other][a link]].\n",
            "a",
            "Body A[fn:x].\n",
        ),
        (
            "Zeroth[fn:x].\n* A\n:PROPERTIES:\n:ID: a\n:END:\nBody A.\n* B\n:PROPERTIES:\n:ID: b\n:END:\nBody B.\n\n[fn:y] Unreferenced SECRETY def.\n",
            "a",
            "Body A[fn:y].\n",
        ),
        // A definition in the body that the zeroth section references.
        (
            "Zeroth text[fn:z].\n* A\n:PROPERTIES:\n:ID: a\n:END:\nBody A.\n\n[fn:z] Old SECRETOLD def.\n",
            "a",
            "Body A.\n\n[fn:z] New SECRETNEW def.\n",
        ),
    ];
    for (text, id, body) in cases {
        let doc = parse(text, &ParseOptions::default());
        let sem = semantics(&doc);
        let n = sem.nodes.iter().find(|n| n.id == *id).unwrap();
        let r = replace_body(text, &doc, n, body);
        assert!(
            r.is_err(),
            "accepted:\n{text}\n--- body:\n{body}\n--- gives:\n{r:?}"
        );
    }
}

/// What only the probing resolver sees: a note in A's body that A cites
/// first (so its links are A's own, which the link comparison leaves to
/// A) and B cites too. Retargeting its link changes what B's page shows,
/// but under a resolver that resolves nothing both targets render alike.
#[test]
fn a_retargeted_link_in_a_shared_note_is_refused() {
    let text = "* A\n:PROPERTIES:\n:ID: a\n:END:\nSee[fn:x].\n\n[fn:x] See [[id:one][it]].\n* B\n:PROPERTIES:\n:ID: b\n:END:\nBody B[fn:x].\n";
    let doc = parse(text, &ParseOptions::default());
    let sem = semantics(&doc);
    let a = sem.nodes.iter().find(|n| n.id == "a").unwrap();
    assert_eq!(
        replace_body(text, &doc, a, "See[fn:x].\n\n[fn:x] See [[id:two][it]].\n"),
        Err(BodyEditError::ChangesRendering {
            what: "the page".into()
        })
    );
}

/// Review round 4: the third reviewer's documents (as written), whose
/// edits of A's body the checks accepted though they change what renders
/// outside it.
const NOTES: &str = "* A\n:PROPERTIES:\n:ID: a\n:END:\nold body\n\n* B\n:PROPERTIES:\n:ID: b\n:END:\nsee[fn:b]\n\n[fn:b] the b note\n";
const NAMES: &str = "* A\n:PROPERTIES:\n:ID: a\n:END:\nold body\n\n* B\n:PROPERTIES:\n:ID: b\n:END:\nsee [[foo bar]] and [[Intro]].\n\n* Intro\nintro text\n\n* foo bar\nx\n";
const COUNTERS: &str = "* A\n:PROPERTIES:\n:ID: a\n:END:\nold body\n\n* B\n:PROPERTIES:\n:ID: b\n:END:\nnumber {{{n}}}\n";
const KEYWORD: &str = "* A\n:PROPERTIES:\n:ID: a\n:END:\nold body\n\n* B\n:PROPERTIES:\n:ID: b\n:END:\nsays {{{keyword(K)}}}\n";

/// Each edit of node `a` is refused.
fn refused(cases: &[(&str, &str, &str)]) {
    for (name, text, body) in cases {
        let r = one(text, body);
        assert!(
            r.is_err(),
            "{name}: accepted:\n{text}\n--- body:\n{body}\n--- gives:\n{r:?}"
        );
    }
}

/// Each edit of node `a` is written.
fn written(cases: &[(&str, &str, &str)]) {
    for (name, text, body) in cases {
        let doc = parse(text, &ParseOptions::default());
        let sem = semantics(&doc);
        let a = sem.nodes.iter().find(|n| n.id == "a").unwrap();
        let r = replace_body(text, &doc, a, body);
        assert!(
            r.is_ok(),
            "{name}: refused: {r:?}\n{text}\n--- body:\n{body}"
        );
    }
}

/// M1: the comparison left the body out, so what it numbers did not
/// count: a new note, an anonymous one, or a reference to B's note in A
/// renumbers B's reference or moves its back-link; a counter or a keyword
/// the body adds changes what B's macros expand to. Compared with the
/// body present (its own output left out), each is refused; the body's
/// own text, and a note numbered after every other, are still written.
#[test]
fn an_edit_that_renumbers_or_recounts_outside_the_body_is_refused() {
    refused(&[
        ("own note", NOTES, "mine[fn:mine]\n\n[fn:mine] my note\n"),
        ("anonymous note", NOTES, "inline[fn:: anon note]\n"),
        ("reference to B's note", NOTES, "ref[fn:b]\n"),
        ("counter", COUNTERS, "a {{{n}}}\n"),
        ("keyword", KEYWORD, "new\n#+K: value\n"),
    ]);
    written(&[
        ("plain text", NOTES, "a new body\n"),
        ("own target", NAMES, "here <<mine>> it is\n"),
        ("own counter after none", KEYWORD, "a {{{n}}}\n"),
        (
            "own note last",
            "* B\n:PROPERTIES:\n:ID: b\n:END:\nsee[fn:b]\n\n[fn:b] the b note\n\n* A\n:PROPERTIES:\n:ID: a\n:END:\nold body\n",
            "mine[fn:mine]\n\n[fn:mine] my note\n",
        ),
    ]);
}
