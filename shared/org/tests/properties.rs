//! Gate 2 (ADR-112 D5): totality and tree invariants on random input.
//!
//! Two generators: arbitrary Unicode strings, and "org-shaped" text built from
//! the fragments that decide structure (stars, block and drawer delimiters,
//! bullets, keywords, CR/CRLF/BOM, tabs). Random bytes almost never form a
//! block, so the second generator is what actually exercises the recognisers.
//! Every case checks: no panic, a lossless print, the invariants of
//! `support::check_tree`, determinism, and a small `max_depth` respected.

mod support;

use mae_org::{parse, ParseOptions, TodoSequence};
use proptest::prelude::*;
use support::*;

const FRAGMENTS: &[&str] = &[
    "* ",
    "** ",
    "*** TODO ",
    "*\t",
    "*",
    " * ",
    "#+begin_src sh",
    "#+end_src",
    "#+BEGIN_QUOTE",
    "#+END_QUOTE",
    "#+begin_x",
    "#+end_x",
    "#+BEGIN: dyn",
    "#+END:",
    ":PROPERTIES:",
    ":END:",
    ":ID: v",
    ":LOG:",
    "- ",
    "  - ",
    "1. ",
    "+ ",
    "[ ] ",
    "[X] ",
    " :: ",
    "| a |",
    "|---|",
    "+--+--+",
    "#+NAME: n",
    "#+CAPTION[s]: c",
    "#+TITLE: t",
    "#+TODO: A | B",
    "#+TBLFM: x",
    "[fn:1] ",
    "CLOCK: [2026-01-01 Thu 10:00]",
    "SCHEDULED: <2026-01-01 Thu>",
    "# ",
    "#",
    ": ",
    ":",
    "-----",
    "%%(d)",
    "\\begin{e}",
    "\\end{e}",
    "[#A] ",
    "COMMENT ",
    ":tag:",
    "text",
    "é",
    "日本",
    "  ",
    "\t",
    "\n",
    "\n",
    "\n",
    "\r\n",
    "\r",
    "\u{feff}",
];

fn org_shaped() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(FRAGMENTS), 0..120).prop_map(|v| v.concat())
}

fn check_all(text: &str) -> Result<(), TestCaseError> {
    let opts = ParseOptions::default();
    let doc = parse(text, &opts);
    check_tree(&doc, text, opts.max_depth).map_err(TestCaseError::fail)?;
    prop_assert_eq!(&doc, &parse(text, &opts), "parse is not deterministic");
    // A tiny nesting cap must still give a lossless tree within the cap.
    let shallow = ParseOptions {
        max_depth: 3,
        ..ParseOptions::default()
    };
    let doc = parse(text, &shallow);
    check_tree(&doc, text, 3).map_err(TestCaseError::fail)?;
    // Configured TODO keywords change tokens, never the text.
    let todo = ParseOptions {
        todo_sequences: vec![TodoSequence {
            todo: vec!["A".into()],
            done: vec!["B".into()],
        }],
        ..ParseOptions::default()
    };
    prop_assert_eq!(parse(text, &todo).syntax().to_string(), text);
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4000))]

    #[test]
    fn org_shaped_input_never_panics_and_keeps_every_invariant(text in org_shaped()) {
        check_all(&text)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn arbitrary_unicode_never_panics_and_keeps_every_invariant(text in any::<String>()) {
        check_all(&text)?;
    }

    /// Splicing org fragments into real corpus text: mutations of documents
    /// that have structure to break.
    #[test]
    fn corpus_text_with_spliced_fragments_keeps_every_invariant(
        file in 0usize..1000,
        at in 0usize..100_000,
        frag in org_shaped(),
    ) {
        let files = bundled_corpus();
        let text = read(&files[file % files.len()]);
        let mut cut = at % (text.len() + 1);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let spliced = format!("{}{}{}", &text[..cut], frag, &text[cut..]);
        check_all(&spliced)?;
    }
}
