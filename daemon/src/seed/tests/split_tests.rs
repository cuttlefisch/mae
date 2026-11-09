//! `split`/`regen` on hand-written files: the exact header form, the literal
//! body, and every per-file refusal.

use super::{page, problem_kinds};
use crate::seed::{normalise, regen, split, ProblemKind};

/// The publication KB's page shape (synthetic text): drawer with `ID` and a
/// long key, then `#+title:`, then `#+language:`, then the body; no
/// `#+filetags`. It must be canonical as written: F == N(F).
#[test]
fn a_publication_page_shape_is_canonical_as_written() {
    let f = ":PROPERTIES:\n\
             :ID: 0f8e2a8c-3b1d-4c5e-9a7f-2d6b8c1e4f30\n\
             :PROPUESTA-DE-CLASIFICACION-X: interno\n\
             :END:\n\
             #+title: Procedimiento de ejemplo\n\
             #+language: es\n\
             \n\
             Texto con [[id:5b7c9d1e-2f3a-4b5c-8d9e-0a1b2c3d4e5f][otra página]].\n";
    let s = split(f).expect("the shape splits");
    assert_eq!(s.id, "0f8e2a8c-3b1d-4c5e-9a7f-2d6b8c1e4f30");
    assert_eq!(s.title.as_deref(), Some("Procedimiento de ejemplo"));
    assert_eq!(s.filetags, None, "no #+filetags line means no tags field");
    assert!(
        s.body.starts_with("#+language: es\n"),
        "#+language stays in the body: {:?}",
        s.body
    );
    let n = regen(&s);
    assert!(
        !n.contains("#+filetags"),
        "no tags => no #+filetags line is emitted"
    );
    assert_eq!(n, f, "F == N(F) for this shape");
}

/// One space between key and value, and `:KEY:` alone for an empty value.
/// Kills: Emacs's padded `%-10s %s` form, or a trailing space after `:KEY:`.
#[test]
fn the_canonical_drawer_line_has_exactly_one_space() {
    let text = page(
        &[("ID", "a"), ("Mixed-Case", "v w"), ("EMPTY", "")],
        Some("T"),
        None,
        "x\n",
    );
    assert!(
        text.contains("\n:ID: a\n:Mixed-Case: v w\n:EMPTY:\n:END:\n"),
        "{text:?}"
    );
    let padded = text.replace(":ID: a", ":ID:       a");
    assert_ne!(padded, text);
    assert_eq!(
        normalise(&padded).unwrap(),
        text,
        "the padded form normalises to one space"
    );
}

/// Lifted lines are removed whole; nothing else moves. Kills: a body taken
/// from mae-org's `body_span` (which also drops leading comments and every
/// leading keyword) or a body run through link rewriting.
#[test]
fn the_body_is_the_file_minus_the_lifted_lines_byte_for_byte() {
    let body = "# un comentario\n#+author: alguien\n#+caption: c\n| a | b |\n\
                1. [[id:ens:paso-1][Paso]]\n2. [[id:ens:paso-1][Paso]]\n\
                [[id:ens:incluida]]\n- Valor :: [[id:iso:9001][iso:9001]]\n\
                Tipado [[id:ens:x?rel=answers&w=0.5&by=y][r]] y [[https://example.com][web]]\n\
                =[[id:no]]= ~[[id:no2]]~\n#+begin_src sh\necho \"[[id:tampoco]]\"\n#+end_src\n\
                * Sección\n:PROPERTIES:\n:ID: ancla-1\n:END:\n- ítem\n  :PROPERTIES:\n  :ID: ancla-2\n  :END:\n\
                fin sin salto   \n  [[id:ens:incluida]]";
    let f = page(
        &[("ID", "task:t1"), ("ROLE", "molecule")],
        Some("Título"),
        Some(&["a", "B"]),
        body,
    );
    let s = split(&f).unwrap();
    assert_eq!(s.body, body);
    let mut anchors = s.anchors.clone();
    anchors.sort();
    assert_eq!(
        anchors,
        vec!["ancla-1", "ancla-2"],
        "headline and item ids are anchors"
    );
    assert_eq!(regen(&s), f);
}

/// The id is what Org reads, never a uuid check and never the file name.
#[test]
fn non_uuid_ids_are_taken_as_written() {
    for id in ["ens:a.b-c", "decision:x", "ul:1", "plain"] {
        let s = split(&page(&[("ID", id)], None, None, "")).unwrap();
        assert_eq!(s.id, id);
    }
}

/// A file whose header is not canonical still splits; N rewrites only the
/// header. Kills: refusing (instead of reporting) a non-canonical header.
#[test]
fn a_blank_line_after_the_drawer_stays_in_the_body() {
    let f = ":PROPERTIES:\n:ID: x\n:END:\n\n#+TITLE: T\nbody\n";
    let s = split(f).unwrap();
    assert_eq!(s.body, "\nbody\n");
    assert_eq!(
        normalise(f).unwrap(),
        ":PROPERTIES:\n:ID: x\n:END:\n#+title: T\n\nbody\n"
    );
}

#[test]
fn each_malformed_header_is_a_named_refusal() {
    use ProblemKind::*;
    let cases: &[(&str, ProblemKind)] = &[
        ("\u{feff}:PROPERTIES:\n:ID: x\n:END:\n", ByteOrderMark),
        (":PROPERTIES:\r\n:ID: x\r\n:END:\r\n", CarriageReturn),
        ("#+title: T\n", NoFileDrawer),
        ("\n:PROPERTIES:\n:ID: x\n:END:\n", NoFileDrawer),
        ("# c\n:PROPERTIES:\n:ID: x\n:END:\n", NoFileDrawer),
        (":PROPERTIES:\n:ROLE: atom\n:END:\nbody\n", MissingId),
        (":PROPERTIES:\n:ID: x\n:id: y\n:END:\n", MultipleIds),
        (":PROPERTIES:\n:ID:\n:END:\n", EmptyId),
        (":PROPERTIES:\n:ID: a b\n:END:\n", InvalidId),
        (":PROPERTIES:\n:ID: x\n:Role: a\n:ROLE: b\n:END:\n", DuplicateProperty),
        (":PROPERTIES:\n:ID: x\n:TAGS+: a\n:END:\n", AppendProperty),
        (":PROPERTIES:\n:ID: x\n:END:\n#+title: A\n#+title: B\n", MultipleTitle),
        (":PROPERTIES:\n:ID: x\n:END:\n#+filetags: :a:\n#+filetags: :b:\n", MultipleFiletags),
        (":PROPERTIES:\n:ID: x\n:END:\ntext\n#+title: late\n", MisplacedHeaderKeyword),
        (":PROPERTIES:\n:ID: x\n:END:\n* H\n#+filetags: :a:\n", MisplacedHeaderKeyword),
        (":PROPERTIES:\n:ID: x\n:END:\n#+filetags: a b\n", BadFiletags),
        (":PROPERTIES:\n:ID: x\n:END:\n#+filetags: :a::b:\n", BadFiletags),
        (":PROPERTIES:\n:ID: x\n:END:\ntext\n#+TODO: A | B\n", Declaration),
        (":PROPERTIES:\n:ID: x\n:END:\n#+MACRO: m x\n", Declaration),
        (":PROPERTIES:\n:ID: x\n:END:\n#+link: l https://e/%s\n", Declaration),
        (":PROPERTIES:\n:ID: x\n:END:\n#+kind: concept\n", ExtensionKeyword),
        (":PROPERTIES:\n:ID: x\n:END:\n* A\n:PROPERTIES:\n:ID: d\n:END:\n* B\n:PROPERTIES:\n:ID: d\n:END:\n", DuplicateId),
        (":PROPERTIES:\n:ID: x\n:END:\n* A\n:PROPERTIES:\n:ID: x\n:END:\n", DuplicateId),
    ];
    for (text, kind) in cases {
        assert!(
            problem_kinds(text).contains(kind),
            "{text:?} must be refused as {kind:?}, got {:?}",
            problem_kinds(text)
        );
    }
}

/// Every problem in one file is listed, not just the first.
#[test]
fn a_file_with_several_problems_lists_all_of_them() {
    let f = ":PROPERTIES:\n:ID: x\n:ROLE: a\n:role: b\n:END:\n#+kind: c\n#+TODO: A\n";
    let kinds = problem_kinds(f);
    for k in [
        ProblemKind::DuplicateProperty,
        ProblemKind::ExtensionKeyword,
        ProblemKind::Declaration,
    ] {
        assert!(kinds.contains(&k), "{k:?} missing from {kinds:?}");
    }
}

/// Why the planner does not reuse org ingest: its body for this file is not
/// the literal text (links rewritten, header kept), while `split`'s is.
#[test]
fn org_ingest_would_not_meet_this_contract() {
    let body = "Ver [[id:ens:x][x]].\n";
    let f = page(&[("ID", "ens:y")], Some("T"), None, body);
    let ingest = mae_kb::org::parse_org_multi_result(&f);
    assert_ne!(ingest.nodes[0].body, body);
    assert_eq!(split(&f).unwrap().body, body);
}

/// Q5: the kind is carried exactly as written, case included, with no
/// mapping. Kills: `NodeKind::from_str_lossy`, which turns both into `note`.
#[test]
fn the_kind_is_carried_raw() {
    for kind in ["Procedimiento", "Hub", "registro de calidad"] {
        let s = split(&page(&[("ID", "k"), ("KIND", kind)], None, None, "")).unwrap();
        assert_eq!(s.kind(), Some(kind));
    }
    assert_eq!(
        split(&page(&[("ID", "k")], None, None, "")).unwrap().kind(),
        None
    );
}

/// An id with a character that renders as nothing or as a plain space looks
/// identical to a different id. Kills: checking only ASCII whitespace and Cc.
#[test]
fn ids_with_invisible_or_blank_characters_are_refused() {
    for id in [
        "a\u{200B}b",
        "a\u{00A0}b",
        "a\u{FEFF}b",
        "a\u{202E}b",
        "a\u{0085}b",
        "a\u{2060}b",
        "a\u{E0041}b",
    ] {
        let kinds = problem_kinds(&page(&[("ID", id)], None, None, ""));
        assert_eq!(kinds, vec![ProblemKind::InvalidId], "{id:?}");
    }
    assert!(problem_kinds(&page(&[("ID", "ñandú-ü:1")], None, None, "")).is_empty());
}

/// Kills: no NUL check (mae-org accepts the byte, and it would be stored).
#[test]
fn a_nul_byte_anywhere_is_refused() {
    let f = page(&[("ID", "n")], None, None, "texto\0oculto\n");
    assert_eq!(problem_kinds(&f), vec![ProblemKind::NulByte]);
}

/// Every keyword Org reads as a buffer setting is refused wherever it is.
/// Kills: a list that stops at the TODO/MACRO/LINK/PROPERTY/SETUPFILE subset.
#[test]
fn every_buffer_setting_keyword_is_refused() {
    for key in [
        "ARCHIVE",
        "BIND",
        "CATEGORY",
        "COLUMNS",
        "CONSTANTS",
        "EXCLUDE_TAGS",
        "INCLUDE",
        "LINK",
        "MACRO",
        "OPTIONS",
        "PRIORITIES",
        "PROPERTY",
        "SELECT_TAGS",
        "SEQ_TODO",
        "SETUPFILE",
        "STARTUP",
        "TAGS",
        "TODO",
        "TYP_TODO",
    ] {
        for case in [key.to_string(), key.to_ascii_lowercase()] {
            let f = page(
                &[("ID", "d")],
                Some("T"),
                None,
                &format!("texto\n#+{case}: x\n"),
            );
            assert!(
                problem_kinds(&f).contains(&ProblemKind::Declaration),
                "#+{case} must be refused"
            );
        }
    }
    let ok = page(
        &[("ID", "d")],
        Some("T"),
        None,
        "#+language: es\n#+author: a\n#+date: hoy\n",
    );
    assert!(
        problem_kinds(&ok).is_empty(),
        "export metadata keywords are body text"
    );
}

/// `drawer_id` (history) and `split` read a file's identity through the same
/// functions, so no drawer can make them disagree. Kills: a second, line-based
/// drawer reader (the reviewer's `:ID:X: v` case read "X: v" as the id).
#[test]
fn the_history_id_reader_agrees_with_split_on_odd_drawers() {
    for text in [
        ":PROPERTIES:\n:ID:X: v\n:ID: real\n:END:\n#+title: t\n",
        ":PROPERTIES:\n:ID: a\n:id: b\n:END:\n",
        ":PROPERTIES:\n:ID+: a\n:END:\n",
        ":properties:\n:id:  y  \n:end:\nx\n",
        ":PROPERTIES:\n:ROLE: x\n:END:\n",
        ":PROPERTIES:\n:ID: a\u{200B}\n:END:\n",
        ":PROPERTIES:\n\n:ID: a\n:END:\n",
        "  :PROPERTIES:\n  :ID: a\n  :END:\n",
    ] {
        let by_split = split(text).ok().map(|s| s.id);
        assert_eq!(crate::seed::drawer_id(text), by_split, "{text:?}");
    }
    let first = split(":PROPERTIES:\n:ID:X: v\n:ID: real\n:END:\n").unwrap();
    assert_eq!(first.id, "real", "Org reads `:ID:X:` as the key `ID:X`");
}

/// The drawer line-count guard in `split` targets a drawer holding a line that
/// is not a property. mae-org (like Org) does not read such a drawer as a
/// property drawer at all, so today these are refused before the guard runs;
/// the guard stays for a mae-org change that would otherwise drop the line.
#[test]
fn drawer_lines_that_are_not_properties_never_reach_the_line_count_guard() {
    for text in [
        ":PROPERTIES:\n:ID: x\n\n:ROLE: a\n:END:\n",
        ":PROPERTIES:\n:ID: x\n# c\n:END:\n",
        ":PROPERTIES:\n:ID: x\nplain text\n:END:\n",
    ] {
        assert_eq!(
            problem_kinds(text),
            vec![ProblemKind::NoFileDrawer],
            "{text:?}"
        );
    }
}
