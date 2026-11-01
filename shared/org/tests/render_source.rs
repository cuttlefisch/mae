//! The org renderer: the normal form of a document and the form a KB node
//! is edited in (ADR-092 D3; decisions in `src/render/source.rs`).
//! - normalising never changes what the text means: the semantics of
//!   `normalise(x)` are those of `x` (spans aside, which move);
//! - `normalise` is idempotent;
//! - a node's edit form survives normalisation of its file, and
//!   `with_line_ending` is its exact inverse: an untouched body written
//!   back gives the file byte for byte.

mod support;

use mae_org::{
    node_source, normalise, parse, semantics, with_line_ending, LineEnding, ParseOptions,
    SemanticDiagnostic, Semantics,
};
use proptest::prelude::*;
use std::path::Path;

/// `s` with `eol` line endings read as `\n`.
fn lf(s: &str, eol: LineEnding) -> String {
    match eol {
        LineEnding::Lf => s.to_string(),
        LineEnding::CrLf => s.replace("\r\n", "\n"),
        LineEnding::Cr => s.replace('\r', "\n"),
    }
}

/// Everything the semantics say except spans, line endings as `\n`.
fn fingerprint(text: &str) -> Vec<String> {
    let doc = parse(text, &ParseOptions::default());
    let s: Semantics = semantics(&doc);
    let eol = doc.line_ending();
    let mut out = vec![format!("tags {:?}", s.file_tags)];
    for n in &s.nodes {
        out.push(lf(
            &format!(
                "node {} {:?} {} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
                n.id,
                n.origin,
                n.level,
                n.kind,
                n.title,
                n.aliases,
                n.own_tags,
                n.properties,
                n.todo,
                n.priority
            ),
            eol,
        ));
    }
    for l in &s.links {
        out.push(lf(
            &format!(
                "link {:?} {:?} {} {:?} {:?} {} {} {} {:?} {:?}",
                l.from,
                l.kind,
                l.target,
                l.fragment,
                l.search,
                l.rel,
                l.weight,
                l.confidence,
                l.attrs,
                l.description
            ),
            eol,
        ));
    }
    for t in &s.transclusions {
        out.push(format!(
            "tr {:?} {} {} {:?}",
            t.from, t.member, t.role, t.args
        ));
    }
    for d in &s.diagnostics {
        let what = match d {
            SemanticDiagnostic::IdInUnrenderedRegion { id, region, .. } => {
                format!("{id} {region:?}")
            }
            SemanticDiagnostic::LinkInUnrenderedRegion { path, region, .. } => {
                format!("{} {region:?}", lf(path, eol))
            }
            SemanticDiagnostic::TransclusionInUnrenderedRegion { member, region, .. } => {
                format!("{member} {region:?}")
            }
            SemanticDiagnostic::DuplicateId { id, .. } => id.clone(),
            SemanticDiagnostic::SetupFileNotRead { file, .. } => file.clone(),
            _ => d.kind_name().to_string(),
        };
        out.push(format!("diag {} {what}", d.kind_name()));
    }
    for u in &s.unrendered {
        out.push(format!("unrendered {:?} {:?}", u.element, u.region));
    }
    for f in &s.footnotes {
        out.push(format!("note {}", f.label));
    }
    out
}

fn check(text: &str) -> Result<(), String> {
    let n = normalise(text);
    if normalise(&n) != n {
        return Err(format!("not idempotent: {text:?} -> {n:?}"));
    }
    if fingerprint(&n) != fingerprint(text) {
        return Err(format!(
            "semantics changed: {text:?}\n{:#?}\n{:#?}",
            fingerprint(text),
            fingerprint(&n)
        ));
    }
    bodies(text)
}

/// Every node's edit form: inverse of `with_line_ending`, and the same in
/// the normalised file (but a final line ending normalisation adds).
fn bodies(text: &str) -> Result<(), String> {
    let doc = parse(text, &ParseOptions::default());
    let sem = semantics(&doc);
    let ndoc = parse(&normalise(text), &ParseOptions::default());
    let nsem = semantics(&ndoc);
    for node in &sem.nodes {
        let src = node_source(&doc, node);
        let back = with_line_ending(&src, doc.line_ending());
        if back != text[node.body_span.range()] {
            return Err(format!("{}: body does not write back: {back:?}", node.id));
        }
        let Some(other) = nsem.nodes.iter().find(|n| n.id == node.id) else {
            return Err(format!("{} lost by normalising", node.id));
        };
        let nsrc = node_source(&ndoc, other);
        let at_end = node.body_span.end as usize == text.len();
        if nsrc != src && !(at_end && nsrc.strip_suffix('\n') == Some(src.as_str())) {
            return Err(format!(
                "{}: edit form changed: {src:?} vs {nsrc:?}",
                node.id
            ));
        }
    }
    Ok(())
}

fn variants(text: &str) -> Vec<String> {
    vec![
        text.to_string(),
        text.replace('\n', "\r\n"),
        text.replace('\n', "\r"),
        format!("\u{feff}{}", text.trim_end_matches('\n')),
    ]
}

#[test]
fn corpus_normalises_without_changing_its_semantics() {
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
        for v in variants(&t) {
            if let Err(e) = check(&v) {
                failures.push(format!("{}: {}", support::rel(f), &e[..e.len().min(1500)]));
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn the_decisions_are_pinned() {
    // Line endings, a BOM and a final newline; nothing else.
    assert_eq!(
        normalise("\u{feff}* a\r\n:PROPERTIES:\r\n:id: x\r\n:END:"),
        "* a\n:PROPERTIES:\n:id: x\n:END:\n"
    );
    assert_eq!(normalise("a\rb\r"), "a\nb\n");
    assert_eq!(normalise("a  \n\n\n"), "a  \n\n\n");
    assert_eq!(normalise(""), "");
    // A content CR in a unix file stays content.
    assert_eq!(normalise("a\r\nb\n"), "a\r\nb\n");
    // A DOS file whose every line ending follows a content CR has no LF
    // spelling with the same meaning: it keeps CRLF.
    assert_eq!(normalise("a\r\r\nb\r\r\n"), "a\r\r\nb\r\r\n");
    assert_eq!(with_line_ending("a\nb\n", LineEnding::CrLf), "a\r\nb\r\n");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn normalising_arbitrary_text_keeps_its_semantics(text in any::<String>()) {
        prop_assert!(check(&text).is_ok(), "{:?}", check(&text));
    }

    #[test]
    fn normalising_org_shaped_text_keeps_its_semantics(
        text in prop::collection::vec(prop::sample::select(PIECES), 0..16).prop_map(|v| v.concat())
    ) {
        prop_assert!(check(&text).is_ok(), "{:?}", check(&text));
    }
}

const PIECES: &[&str] = &[
    "* H\n",
    ":PROPERTIES:\n:ID: a\n:END:\n",
    ":PROPERTIES:\r\n:ID: b\r\n:END:\r\n",
    "text\r",
    "\r\n",
    "\n",
    "\r",
    "x\r\r\n",
    "- i\n  :PROPERTIES:\n  :ID: c\n  :END:\n",
    "[[id:z]]\n",
    "#+title: T\n",
    "\u{feff}",
    "** H2 :t:\n",
    "#+begin_src\nx\r\n#+end_src\n",
    "[fn:1] n\n",
    "r[fn:1]\n",
];
