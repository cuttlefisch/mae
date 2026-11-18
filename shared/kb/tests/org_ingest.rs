//! `org_ingest`: MAE's KB derivation on `mae-org` (ADR-112 O4, #941 PR1).
//!
//! Adversarial (principle #14): the store form is checked as properties
//! over generated bodies (idempotent; raw and stored bodies make the same
//! edges), against the old parser where they must agree (TODO keywords,
//! the property adapter), and on hostile input. What the old parser and
//! `mae-org` disagree on is the differential's job
//! (`org_semantics_differential.rs`), not this file's.

use mae_kb::org::{parse_org_multi, ParsedLink};
use mae_kb::org_ingest::{
    edges_of_body, is_edge, org_options, parse_file, parse_nodes, store_body,
    transclusions_of_body, DONE_STATES, TODO_STATES,
};
use mae_org::{parse, semantics_with};
use proptest::prelude::*;
use std::time::Instant;

/// An edge's payload, without its display text (presentation, not graph):
/// target, fragment, relation, weight and confidence bits, attributes.
type Payload = (
    String,
    Option<String>,
    String,
    u64,
    u64,
    Vec<(String, String)>,
);

fn payload(l: &ParsedLink) -> Payload {
    (
        l.target.clone(),
        l.fragment.clone(),
        l.rel_type.clone(),
        l.weight.to_bits(),
        l.confidence.to_bits(),
        l.attrs.clone(),
    )
}

fn payloads(body: &str) -> Vec<Payload> {
    edges_of_body(body).iter().map(payload).collect()
}

// ------------------------------------------------------------ options (G1)

/// The six keywords the old parser knew are TODO keywords here too, with
/// the same title left over: dropping one from `org_options` changes what
/// every `* NEXT …` heading means.
#[test]
fn the_old_parsers_todo_keywords_are_todo_keywords() {
    for kw in TODO_STATES.iter().chain(DONE_STATES.iter()) {
        let text = format!("* {kw} [#B] Write it\n:PROPERTIES:\n:ID: n\n:END:\n");
        let old = &parse_org_multi(&text)[0];
        let new = &parse_nodes(&text)[0];
        assert_eq!(new.todo_state.as_deref(), Some(*kw), "{kw}");
        assert_eq!(new.todo_state, old.todo_state, "{kw}");
        assert_eq!(new.title, "Write it", "{kw}");
        assert_eq!(new.title, old.title, "{kw}");
        assert_eq!(new.priority, Some('B'), "{kw}");
    }
    // Done states are done: the agenda's split.
    let seqs = &org_options().parse.todo_sequences;
    assert_eq!(seqs.len(), 1);
    assert_eq!(seqs[0].todo, TODO_STATES);
    assert_eq!(seqs[0].done, DONE_STATES);
}

/// A file's own `#+TODO:` replaces MAE's keywords (ADR-112 D4: declared
/// in the file), where the old parser ignored it.
#[test]
fn an_in_file_todo_line_replaces_the_keywords() {
    let text = "#+TODO: OPEN | CLOSED\n* NEXT a\n:PROPERTIES:\n:ID: a\n:END:\n* OPEN b\n:PROPERTIES:\n:ID: b\n:END:\n";
    let nodes = parse_nodes(text);
    assert_eq!(nodes[0].todo_state, None);
    assert_eq!(nodes[0].title, "NEXT a");
    assert_eq!(nodes[1].todo_state.as_deref(), Some("OPEN"));
    assert_eq!(nodes[1].title, "b");
}

// --------------------------------------------------------- properties (G2)

/// The store's property map: keys lower-cased, `ID` dropped, a file node's
/// `KIND`/`ALIASES` as fields only, a headline's kept as properties too:
/// the old parser's map, key for key.
#[test]
fn properties_are_stored_as_the_old_parser_stored_them() {
    let text = ":PROPERTIES:\n:ID: f\n:KIND: concept\n:ALIASES: a, b\n:Last-Accessed: 2026\n:END:\n#+title: F\n\n* H\n:PROPERTIES:\n:ID: h\n:KIND: task\n:ALIASES: x\n:Owner: me\n:END:\nbody\n";
    let old = parse_org_multi(text);
    let new = parse_nodes(text);
    assert_eq!(new.len(), 2);
    for (o, n) in old.iter().zip(&new) {
        assert_eq!(o.id, n.id);
        assert_eq!(o.properties, n.properties, "{}", n.id);
        assert_eq!(o.kind, n.kind, "{}", n.id);
        assert!(!n.properties.contains_key("id"), "{}", n.id);
    }
    assert_eq!(
        new[0].properties.get("last-accessed").map(String::as_str),
        Some("2026")
    );
    assert!(!new[0].properties.contains_key("kind"));
    assert_eq!(new[0].aliases, ["a", "b"]);
    assert_eq!(
        new[1].properties.get("kind").map(String::as_str),
        Some("task")
    );
    assert_eq!(
        new[1].properties.get("owner").map(String::as_str),
        Some("me")
    );
}

// ------------------------------------------------------------- bodies (G4)

/// A node's body is its `body_span`: no front matter, and it stops at the
/// first nested node, so a parent's links are not its child's.
#[test]
fn a_body_holds_its_own_text_only() {
    let text = "* P\n:PROPERTIES:\n:ID: p\n:END:\nsee [[id:a][A]]\n** C\n:PROPERTIES:\n:ID: c\n:END:\nsee [[id:b][B]]\n";
    let r = parse_file(text);
    assert_eq!(r.nodes[0].body, "see [[a|A]]\n");
    assert_eq!(r.nodes[1].body, "see [[b|B]]\n");
    let edges: Vec<(&str, &str)> = r
        .typed_links
        .iter()
        .map(|(s, l)| (s.as_str(), l.target.as_str()))
        .collect();
    assert_eq!(edges, [("p", "a"), ("c", "b")]);
}

/// #941 G5, pinned as it stands: a file's `#+TRANSCLUDE:` lines in its
/// header are file-node transclusions, but the header is front matter, so
/// the stored body does not carry them. Re-deriving transclusions from that
/// body would lose them; PR3's ingest must store `parse_file`'s.
#[test]
fn a_header_transclusion_is_the_files_but_not_in_its_body() {
    let text = ":PROPERTIES:\n:ID: m\n:END:\n#+title: M\n#+TRANSCLUDE: a content\n\nprose\n";
    let r = parse_file(text);
    assert_eq!(
        r.transclusions,
        [("m".into(), "a".into(), "content".into())]
    );
    assert!(transclusions_of_body(&r.nodes[0].body).is_empty());
    // A body that starts with the directive (kb_create's form) keeps it.
    assert_eq!(
        transclusions_of_body("#+TRANSCLUDE: a\n#+transclude: b reference\nx\n"),
        [
            ("a".into(), "content".into()),
            ("b".into(), "reference".into())
        ]
    );
}

// --------------------------------------------------------- the store form

/// Each rewrite, exactly: the query and fragment kept, `id:` and an id
/// search option dropped, every non-KB bracket link flattened.
#[test]
fn store_form_cases() {
    let cases = [
        ("[[id:x?rel=teaches&w=0.5][D]]", "[[x?rel=teaches&w=0.5|D]]"),
        ("[[ID:x]]", "[[x]]"),
        ("[[concept:a#frag][D]]", "[[concept:a#frag|D]]"),
        ("[[id:x::*h][D]]", "[[x|D]]"),
        ("[[id:x::*h?rel=r]]", "[[x?rel=r]]"),
        ("[[bare-uuid-1][D]]", "[[bare-uuid-1|D]]"),
        ("[[x|D]]", "[[x|D]]"),
        ("[[x?rel=r|D]]", "[[x?rel=r|D]]"),
        (
            "[[https://e.org/a?b#c][site]]",
            "site (https://e.org/a?b#c)",
        ),
        ("[[https://e.org]]", "(https://e.org)"),
        ("[[file:a.org::*h][F]]", "F (file:a.org::*h)"),
        ("[[*Heading]]", "(*Heading)"),
        ("[[#custom][C]]", "C (#custom)"),
        // A store form that would read as something else stays as written.
        ("[[id:/abs]]", "[[id:/abs]]"),
        ("[[id:help:x]]", "[[id:help:x]]"),
        ("[[id:#c]]", "[[id:#c]]"),
        ("[[id:x][a]b]]", "[[id:x][a]b]]"),
        // Flattening that would make a plain link an edge: kept.
        ("[[file:a.org][id:yy]]", "[[file:a.org][id:yy]]"),
        // Not links the semantics read: kept.
        ("# [[id:x][c]]", "# [[id:x][c]]"),
        ("=[[id:x]]=", "=[[id:x]]="),
        ("id:x", "id:x"),
        ("<https://e.org>", "<https://e.org>"),
        // A table cell: the store form's `|` would split it.
        ("| [[id:x][D]] | b |", "| [[id:x][D]] | b |"),
    ];
    for (raw, want) in cases {
        assert_eq!(store_body(raw), want, "{raw:?}");
        assert_eq!(payloads(raw), payloads(want), "{raw:?}");
        // Each link is judged on its own: a link kept as written does not
        // keep a good neighbour from its store form.
        let (raw2, want2) = (
            format!("{raw}\n\n[[id:ok][K]]"),
            format!("{want}\n\n[[ok|K]]"),
        );
        assert_eq!(store_body(&raw2), want2, "{raw2:?}");
    }
    let src = "#+begin_src org\n[[id:x][D]]\n#+end_src\n";
    assert_eq!(store_body(src), src);
}

/// A replacement that reads right alone can read otherwise in its place:
/// then nothing in the text is rewritten (found by the properties below).
#[test]
fn a_rewrite_that_changes_its_context_is_refused() {
    for raw in [
        // `[[(https://e.org)]]`: the flattened text makes a coderef link.
        "[[[[https://e.org]]]]",
        // `id:a|b (https://e.org)`: a plain id link to `a`, a new edge.
        "(r)id:[[https://e.org][a|b]][[",
    ] {
        assert_eq!(store_body(raw), raw, "{raw:?}");
        let with_ok = format!("{raw} [[id:ok][K]]");
        assert_eq!(store_body(&with_ok), with_ok, "{raw:?}");
    }
}

/// Edges are `semantics()`'s KB links, every one of them, whatever node
/// of the body's own text they sit under.
#[test]
fn edges_are_the_semantics_kb_links() {
    let body = "intro [[id:a?rel=cites&c=0.2&by=ai][A]] [[https://e.org]]\n* sub\n:PROPERTIES:\n:ID: s\n:END:\nid:bb and [[*h]] [[concept:c#f]]\n# [[id:hidden]]\n";
    let o = org_options();
    let sem = semantics_with(&parse(body, &o.parse), &o.semantics);
    let want: Vec<_> = sem
        .links
        .iter()
        .filter(|l| is_edge(l))
        .map(|l| l.target.clone())
        .collect();
    assert_eq!(want, ["a", "bb", "concept:c"]);
    let got = edges_of_body(body);
    assert_eq!(
        got.iter().map(|l| l.target.clone()).collect::<Vec<_>>(),
        want
    );
    assert_eq!(got[0].rel_type, "cites");
    assert_eq!(got[1].rel_type, "references");
    assert_eq!((got[1].weight, got[1].confidence), (1.0, 1.0));
    assert_eq!(got[0].confidence, 0.2);
    assert_eq!(got[0].attrs, [("by".to_string(), "ai".to_string())]);
    assert_eq!(got[2].fragment.as_deref(), Some("f"));
}

// ------------------------------------------------------------- properties

const PIECES: &[&str] = &[
    "[[",
    "]]",
    "][",
    "|",
    "?",
    "&",
    "#",
    "::",
    "id:",
    "ID:",
    "concept:x",
    "x",
    "y",
    "rel=r",
    "w=0.5",
    "c=2",
    "k=v",
    "https://e.org",
    "file:a.org",
    "*h",
    "(r)",
    " ",
    "\n",
    "\r\n",
    "\t",
    "* ",
    "** ",
    "- ",
    "| ",
    "# ",
    "=",
    "~",
    "*",
    "/",
    "\\",
    "[",
    "]",
    "(",
    ")",
    "é",
    "\u{feff}",
    ":PROPERTIES:\n:ID: n1\n:END:\n",
    ":PROPERTIES:\n:ID: n2\n:END:\n",
    "#+begin_src\n",
    "#+end_src\n",
    "#+TRANSCLUDE: ",
    "#+title: ",
    "#+LINK: ab id:%s\n",
    "ab:z",
    "<<<x>>>",
    "[fn:1]",
    "\n[fn:1] ",
    "COMMENT ",
    ":noexport:",
];

fn link() -> impl Strategy<Value = String> {
    let target = prop::sample::select(vec!["x", "concept:a", "bare-1", "https://e.org", "*h", ""]);
    let id = prop::bool::ANY;
    let frag = prop::option::of(prop::sample::select(vec!["f", " f ", ""]));
    let query = prop::option::of(prop::sample::select(vec![
        "rel=r",
        "rel=r&w=0.3",
        "c=x&k=v",
        "",
    ]));
    let search = prop::option::of(Just("::*s"));
    let desc = prop::option::of(prop::sample::select(vec![
        "D", " D ", "a|b", "*b* c", "id:qq",
    ]));
    let bar = prop::option::of(Just("|E"));
    (target, id, frag, query, search, desc, bar).prop_map(|(t, id, f, q, s, d, b)| {
        let mut p = String::from(if id { "id:" } else { "" });
        p.push_str(t);
        if let Some(s) = s {
            p.push_str(s);
        }
        if let Some(f) = f {
            p.push('#');
            p.push_str(f);
        }
        if let Some(q) = q {
            p.push('?');
            p.push_str(q);
        }
        if let Some(b) = b {
            p.push_str(b);
        }
        match d {
            Some(d) => format!("[[{p}][{d}]]"),
            None => format!("[[{p}]]"),
        }
    })
}

fn body() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        3 => prop::sample::select(PIECES).prop_map(str::to_string),
        2 => link(),
    ];
    prop::collection::vec(piece, 0..40).prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// The store form of the store form is itself.
    #[test]
    fn store_body_is_idempotent(b in body()) {
        let once = store_body(&b);
        prop_assert_eq!(store_body(&once), once);
    }

    /// A stored body declares exactly the edges of its source.
    #[test]
    fn raw_and_stored_bodies_make_the_same_edges(b in body()) {
        prop_assert_eq!(payloads(&store_body(&b)), payloads(&b));
    }

    /// Every node's stored body is in store form already, and read alone
    /// (as PR2 will re-derive it, without its file) it invents no edge its
    /// file does not make.
    #[test]
    fn stored_bodies_invent_no_edges(b in body()) {
        let file = payloads(&b);
        for n in parse_file(&b).nodes {
            prop_assert_eq!(&store_body(&n.body), &n.body);
            let mut rest = file.clone();
            for e in payloads(&n.body) {
                let i = rest.iter().position(|f| *f == e);
                prop_assert!(i.is_some(), "{}: {:?} not in the file's edges", n.id, e);
                rest.remove(i.unwrap_or_default());
            }
        }
    }
}

// ---------------------------------------------------------------- hostile

/// Every entry point on `text`; the stored body makes the text's edges.
fn run_all(name: &str, text: &str) {
    let _ = parse_file(text);
    let stored = store_body(text);
    let _ = transclusions_of_body(text);
    assert_eq!(payloads(&stored), payloads(text), "{name}");
}

/// Families that double in size, built to break a hand-rolled bracket
/// scanner or to make the per-link check of `store_body` expensive.
fn family(name: &str, n: usize) -> String {
    match name {
        "[[" => "[[".repeat(n),
        "[[x" => "[[x".repeat(n),
        "]]" => "]]".repeat(n),
        "same link" => "[[id:x?rel=r][d]] ".repeat(n),
        "distinct links" => (0..n)
            .map(|i| format!("[[id:n{i}?w=0.5][d{i}]] "))
            .collect(),
        "distinct web links" => (0..n)
            .map(|i| format!("[[https://e.org/{i}][d]] "))
            .collect(),
        "nested" => format!("{}{}", "[[a".repeat(n), "]]".repeat(n)),
        "a node per link" => (0..n)
            .map(|i| format!("* h\n:PROPERTIES:\n:ID: n{i}\n:END:\n[[id:x{i}][d]]\n"))
            .collect(),
        _ => unreachable!("{name}"),
    }
}

const FAMILIES: [&str; 8] = [
    "[[",
    "[[x",
    "]]",
    "same link",
    "distinct links",
    "distinct web links",
    "nested",
    "a node per link",
];

/// Total on hostile text: no panic, and (nextest's timeout) no hang. About
/// 1 MB per family, plus the small cases a bracket scanner gets wrong.
#[test]
fn hostile_inputs_neither_panic_nor_hang() {
    for (name, text) in [
        ("unclosed", "a [[id:x][never closed\nmore [["),
        ("nested", "[[a[[b]]]] [[id:a[[id:b]]]]"),
        ("bar and bracket", "[[a|b][c]] [[|]] [[||]] [[]] [[][]]"),
        (
            "crlf",
            "* H\r\n:PROPERTIES:\r\n:ID: h\r\n:END:\r\n[[id:x][D]]\r\n",
        ),
        ("bom", "\u{feff}:PROPERTIES:\n:ID: f\n:END:\n[[id:x]]\n"),
        ("lone cr", "a\r[[id:x]\r]\r"),
    ] {
        run_all(name, text);
    }
    for name in FAMILIES {
        let n = (1 << 20) / family(name, 1).len().max(1);
        let text = family(name, n);
        let t = Instant::now();
        run_all(name, &text);
        eprintln!("{name}: {} bytes in {:?}", text.len(), t.elapsed());
    }
}

/// Release builds only (debug times are not the product's): the time per
/// input byte grows roughly linearly, as mae-org's own scaling gate
/// measures it (`shared/org/tests/scaling.rs`): t(8N)/t(N) · 8/B < 24 with
/// B the byte ratio, where linear is ~8 and quadratic 64.
#[cfg(not(debug_assertions))]
#[test]
fn hostile_inputs_scale_linearly() {
    let time = |text: &str| {
        (0..3)
            .map(|_| {
                let t = Instant::now();
                run_all("timed", text);
                t.elapsed()
            })
            .min()
            .unwrap_or_default()
    };
    let mut slow = Vec::new();
    for name in FAMILIES {
        let n = 16 * 1024 / family(name, 1).len().max(1);
        let (small, big) = (family(name, n), family(name, 8 * n));
        let (ts, tb) = (time(&small), time(&big));
        let bytes = big.len() as f64 / small.len() as f64;
        let ratio = tb.as_secs_f64() / ts.as_secs_f64().max(1e-9) * 8.0 / bytes;
        eprintln!("{name}: {ts:?} -> {tb:?}, normalised x{ratio:.1}");
        if ratio >= 24.0 {
            slow.push(format!("{name}: x{ratio:.1}"));
        }
    }
    assert!(slow.is_empty(), "{slow:?}");
}
