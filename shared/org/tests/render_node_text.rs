//! `node_text`: a KB node's search text is its title, its body and the
//! footnotes they reference, wherever each note is defined (ADR-112 D3).

use mae_org::{node_text, parse, semantics, ParseOptions};

fn text_of(doc: &str, id: &str) -> String {
    let d = parse(doc, &ParseOptions::default());
    node_text(&d, &semantics(&d), id).expect("the node")
}

const DOC: &str = "* A[fn:t]\n:PROPERTIES:\n:ID: a\n:END:\nA body cites[fn:2] and[fn:1].\n** B\n:PROPERTIES:\n:ID: b\n:END:\nB body[fn:1].\n* Notes\n[fn:1] NOTEONE.\n[fn:2] NOTETWO cites[fn:3].\n[fn:3] NOTETHREE.\n[fn:t] NOTETITLE.\n[fn:unused] NOTEUNUSED.\n";

#[test]
fn notes_defined_outside_the_body_are_in_its_text() {
    let a = text_of(DOC, "a");
    for want in [
        "A body cites and.",
        "NOTETITLE",
        "NOTETWO",
        "NOTEONE",
        "NOTETHREE",
    ] {
        assert!(a.contains(want), "{want} missing:\n{a}");
    }
    assert!(!a.contains("NOTEUNUSED"), "{a}");
    assert!(!a.contains("B body"), "{a}");
    // Each note once, in the order first referenced (title first).
    let at = |w: &str| a.find(w).unwrap();
    assert!(
        at("NOTETITLE") < at("NOTETWO") && at("NOTETWO") < at("NOTEONE"),
        "{a}"
    );
    assert!(at("NOTEONE") < at("NOTETHREE"), "{a}");
    assert_eq!(a.matches("NOTEONE").count(), 1, "{a}");
}

#[test]
fn a_node_has_only_the_notes_it_references() {
    let b = text_of(DOC, "b");
    assert!(b.contains("B body.") && b.contains("NOTEONE"), "{b}");
    for absent in ["NOTETWO", "NOTETHREE", "NOTETITLE", "A body"] {
        assert!(!b.contains(absent), "{absent} in:\n{b}");
    }
}

#[test]
fn a_note_in_an_unrendered_region_is_not_drawn() {
    let doc = "* A\n:PROPERTIES:\n:ID: a\n:END:\nRef[fn:1].\n#+begin_comment\n[fn:1] SECRET.\n#+end_comment\n";
    let a = text_of(doc, "a");
    assert!(!a.contains("SECRET"), "{a}");
}
