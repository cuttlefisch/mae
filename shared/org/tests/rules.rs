//! Org's structural rules that decide KB node boundaries, each pinned on
//! its own (the conformance goldens also cover them, but a golden diff says
//! THAT something moved, these say WHICH rule broke). Cases marked
//! "oracle" were confirmed against Emacs org-element (Org 9.7.11).

mod support;

use mae_org::{
    parse, Block, DiagnosticKind, Headline, ParseOptions, SyntaxKind, SyntaxKind::*, TodoSequence,
};
use support::*;

/// (kind, begin, end) of the top-level children of the zeroth section, or
/// of the document when there is none.
fn kinds(text: &str) -> Vec<(SyntaxKind, usize, usize)> {
    let doc = parse_default(text);
    doc.syntax()
        .descendants()
        .filter(|n| {
            n.kind().org_element_name().is_some() && n.kind() != DOCUMENT && n.kind() != SECTION
        })
        .map(|n| {
            (
                n.kind(),
                n.text_range().start().into(),
                n.text_range().end().into(),
            )
        })
        .collect()
}

#[test]
fn a_tab_after_the_star_is_not_a_headline() {
    // oracle: `*\tB` is a paragraph.
    assert_eq!(kinds("*\tB\n"), vec![(PARAGRAPH, 0, 4)]);
    assert_eq!(parse_default("*\tB\n").headlines().count(), 0);
}

#[test]
fn an_indented_star_is_a_list_item_not_a_headline() {
    // oracle: ` *  C` is a plain list with one item, bullet `*`.
    let k = kinds(" *  C\n");
    assert_eq!(k[0], (PLAIN_LIST, 0, 6));
    assert_eq!(k[1], (ITEM, 0, 6));
    assert_eq!(parse_default(" *  C\n").headlines().count(), 0);
}

#[test]
fn a_star_glued_to_text_or_alone_is_a_paragraph() {
    // oracle: `*D` and a lone `*` are paragraphs.
    assert_eq!(kinds("*D\n"), vec![(PARAGRAPH, 0, 3)]);
    assert_eq!(kinds("*\n"), vec![(PARAGRAPH, 0, 2)]);
    assert_eq!(kinds("*"), vec![(PARAGRAPH, 0, 1)]);
}

#[test]
fn a_column_0_headline_ends_an_open_block_whose_begin_line_becomes_text() {
    // oracle: the block exists only if its end line comes before the next
    // headline; otherwise `#+begin_src` is paragraph text and `* B` is a
    // headline. Same for example and quote blocks and for drawers.
    for (open, close) in [
        ("#+begin_src sh", "#+end_src"),
        ("#+begin_example", "#+end_example"),
        ("#+begin_quote", "#+end_quote"),
        (":LOG:", ":END:"),
    ] {
        let text = format!("* A\n{open}\nx\n* B\n{close}\n");
        let doc = parse_default(&text);
        let titles: Vec<String> = doc.headlines().map(|h| h.title()).collect();
        assert_eq!(titles, vec!["A", "B"], "{open}");
        assert_eq!(doc.blocks().count(), 0, "{open}");
        let a = doc.headlines().next().expect("A");
        let para = a
            .section()
            .and_then(|s| s.children().next())
            .expect("A has a section");
        assert_eq!(para.kind(), PARAGRAPH, "{open}");
        assert!(para.to_string().starts_with(open), "{open}");
        let want = if open.starts_with(':') {
            DiagnosticKind::UnterminatedDrawer
        } else {
            DiagnosticKind::UnterminatedBlock
        };
        assert!(
            doc.diagnostics()
                .iter()
                .any(|d| d.kind == want && usize::from(d.span.start()) == 4),
            "{open}: {:?}",
            doc.diagnostics()
        );
    }
}

#[test]
fn an_indented_star_line_inside_a_block_does_not_end_it() {
    let doc = parse_default("#+begin_src sh\n * not a headline\n#+end_src\n");
    assert_eq!(doc.blocks().count(), 1);
    assert!(doc.diagnostics().is_empty());
}

#[test]
fn lesser_block_contents_are_opaque_and_greater_block_contents_are_elements() {
    let doc = parse_default(
        "#+begin_src sh\n#+begin_quote\n- x\n#+end_src\n#+begin_quote\n- y\n#+end_quote\n",
    );
    let blocks: Vec<Block> = doc.blocks().collect();
    assert_eq!(blocks[0].kind(), SRC_BLOCK);
    assert_eq!(blocks[0].name(), "src");
    assert_eq!(blocks[0].parameters().as_deref(), Some("sh"));
    assert_eq!(
        blocks[0].raw_contents().as_deref(),
        Some("#+begin_quote\n- x\n")
    );
    assert_eq!(blocks[1].kind(), QUOTE_BLOCK);
    assert!(blocks[1]
        .syntax()
        .children()
        .any(|c| c.kind() == PLAIN_LIST));
    // Block names are case-insensitive; unknown names are special blocks.
    let doc = parse_default("#+BEGIN_Aside :x 1\nbody\n#+end_ASIDE\n");
    let b = doc.blocks().next().expect("special block");
    assert_eq!((b.kind(), b.name()), (SPECIAL_BLOCK, "Aside".to_string()));
}

#[test]
fn affiliated_keywords_live_inside_the_element_they_name() {
    let doc = parse_default("#+NAME: t\n#+ATTR_HTML: :x 1\n| a |\n\n#+NAME: orphan\n\ntext\n");
    let table = doc
        .syntax()
        .descendants()
        .find(|n| n.kind() == TABLE)
        .expect("table");
    assert_eq!(
        usize::from(table.text_range().start()),
        0,
        "the table starts at its first affiliated keyword"
    );
    assert_eq!(
        table
            .children()
            .filter(|c| c.kind() == AFFILIATED_KEYWORD)
            .count(),
        2
    );
    // Before a blank line an affiliated keyword is orphaned: a plain keyword.
    let kw: Vec<_> = doc.keywords().map(|k| (k.key(), k.value())).collect();
    assert_eq!(kw, vec![("NAME".to_string(), "orphan".to_string())]);
}

#[test]
fn todo_keywords_come_from_the_file_then_from_options() {
    let text = "#+TODO: A B | C\n* A x\n* C y\n* TODO z\n";
    let doc = parse_default(text);
    let todo: Vec<Option<String>> = doc.headlines().map(|h| h.todo()).collect();
    assert_eq!(todo, vec![Some("A".into()), Some("C".into()), None]);
    assert_eq!(
        doc.todo_sequences()[0],
        TodoSequence {
            todo: vec!["A".into(), "B".into()],
            done: vec!["C".into()]
        }
    );
    // The declaration may come AFTER the headlines (Emacs reads it buffer-wide)…
    let late = parse_default("* A x\n#+TODO: A | C\n");
    assert_eq!(
        late.headlines().next().and_then(|h| h.todo()),
        Some("A".into())
    );
    // …but not from inside an example block, where it is not a keyword.
    let hidden = parse_default("* A x\n#+begin_example\n#+TODO: A | C\n#+end_example\n");
    assert_eq!(hidden.headlines().next().and_then(|h| h.todo()), None);
    // Without in-file keywords the configured sequences apply.
    let opts = ParseOptions {
        todo_sequences: vec![TodoSequence {
            todo: vec!["NEXT".into()],
            done: vec!["DONE".into()],
        }],
        ..ParseOptions::default()
    };
    let doc = parse("* NEXT a\n* TODO b\n", &opts);
    let todo: Vec<Option<String>> = doc.headlines().map(|h| h.todo()).collect();
    assert_eq!(todo, vec![Some("NEXT".into()), None]);
}

#[test]
fn headline_views_read_every_part() {
    let doc =
        parse_default("*** DONE [#B] COMMENT A title  :t1:t2:\n:PROPERTIES:\n:ID: x\n:END:\n");
    let h: Headline = doc.headlines().next().expect("headline");
    assert_eq!(h.level(), 3);
    assert_eq!(h.todo().as_deref(), Some("DONE"));
    assert_eq!(h.priority(), Some('B'));
    assert!(h.is_commented());
    assert_eq!(h.title(), "A title");
    assert_eq!(h.tags(), vec!["t1".to_string(), "t2".to_string()]);
    assert_eq!(
        h.property_drawer().and_then(|p| p.get("id")).as_deref(),
        Some("x")
    );
}

#[test]
fn property_drawers_only_where_org_reads_them() {
    // Right after the headline, or after its planning line.
    let doc =
        parse_default("* a\nSCHEDULED: <2026-11-01 Sun>\n:PROPERTIES:\n:Custom_Key: v\n:END:\n");
    let p = doc
        .headlines()
        .next()
        .and_then(|h| h.property_drawer())
        .expect("property drawer");
    let props = p.properties();
    assert_eq!(
        (
            props[0].key.as_str(),
            props[0].key_as_written(),
            props[0].value.as_str()
        ),
        ("CUSTOM_KEY", "Custom_Key".to_string(), "v")
    );
    // After a blank line it is an ordinary drawer named PROPERTIES.
    let doc = parse_default("* a\n\n:PROPERTIES:\n:ID: x\n:END:\n");
    assert!(doc
        .headlines()
        .next()
        .and_then(|h| h.property_drawer())
        .is_none());
    // After the planning line it is the headline's, unless a blank line
    // intervenes (oracle).
    let doc = parse_default("* a\nDEADLINE: <2026-11-02 Mon>\n\n:PROPERTIES:\n:ID: x\n:END:\n");
    assert!(doc
        .headlines()
        .next()
        .and_then(|h| h.property_drawer())
        .is_none());
    // After leading comments it is the file's, unless a blank line intervenes.
    assert!(parse_default("# c\n\n:PROPERTIES:\n:ID: f\n:END:\n")
        .property_drawer()
        .is_none());
    // At the top of the file, and after leading comments, it is the file's.
    assert!(parse_default("# c\n:PROPERTIES:\n:ID: f\n:END:\n")
        .property_drawer()
        .is_some());
    assert!(parse_default("#+title: t\n:PROPERTIES:\n:ID: f\n:END:\n")
        .property_drawer()
        .is_none());
}

#[test]
fn a_list_item_id_drawer_is_a_drawer_inside_the_item() {
    // MAE's list-item `:ID:` drawers are plain drawers to Org (oracle).
    let text = read(&repo_root().join("shared/org/tests/fixtures/list-item-drawer.org"));
    let doc = parse_default(&text);
    let item = doc
        .syntax()
        .descendants()
        .find(|n| n.kind() == ITEM)
        .expect("item");
    let drawer = item
        .descendants()
        .find(|n| n.kind() == DRAWER)
        .expect("drawer in item");
    assert!(drawer.to_string().contains(":ID: 7a1c9e0e"));
    assert_eq!(
        doc.syntax()
            .descendants()
            .filter(|n| n.kind() == PROPERTY_DRAWER)
            .count(),
        0
    );
}

#[test]
fn nesting_beyond_the_cap_is_opaque_with_a_diagnostic_not_recursion() {
    let opts = ParseOptions {
        max_depth: 10,
        ..ParseOptions::default()
    };
    let open: String = (0..50).map(|i| format!("#+begin_b{i}\n")).collect();
    let close: String = (0..50).rev().map(|i| format!("#+end_b{i}\n")).collect();
    let text = format!("{open}core\n{close}");
    let doc = parse(&text, &opts);
    assert_eq!(doc.syntax().to_string(), text);
    let d: Vec<_> = doc
        .diagnostics()
        .iter()
        .filter(|d| d.kind == DiagnosticKind::DepthLimit)
        .collect();
    assert_eq!(d.len(), 1);
    assert!(doc
        .syntax()
        .descendants_with_tokens()
        .any(|e| e.kind() == OPAQUE));
    check_tree(&doc, &text, 10).expect("invariants under the cap");
    // Under the default cap the same 50 levels are all parsed.
    let doc = parse_default(&text);
    assert!(doc.diagnostics().is_empty());
    assert_eq!(doc.blocks().count(), 50);
}

#[test]
fn diagnostics_are_data_with_spans() {
    let doc = parse_default("#+begin_src\nx\n\n:DRAWER:\ny\n");
    let kinds: Vec<_> = doc
        .diagnostics()
        .iter()
        .map(|d| (d.kind, u32::from(d.span.start()), u32::from(d.span.end())))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (DiagnosticKind::UnterminatedBlock, 0, 11),
            (DiagnosticKind::UnterminatedDrawer, 15, 23)
        ]
    );
}

#[test]
fn edge_inputs_round_trip() {
    for text in [
        "",
        "\n",
        "\r\n",
        "\r",
        "*",
        "* ",
        "\u{feff}",
        "\u{feff}\n",
        "a",
        " ",
        "\t\n\t",
        "#+",
        ":",
        "-",
        "|",
        "[fn:",
    ] {
        let doc = parse_default(text);
        assert_eq!(doc.syntax().to_string(), text, "{text:?}");
        check_tree(&doc, text, mae_org::DEFAULT_MAX_DEPTH)
            .unwrap_or_else(|e| panic!("{text:?}: {e}"));
    }
}
