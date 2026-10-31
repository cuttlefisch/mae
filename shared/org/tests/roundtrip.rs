//! Gate 1 (ADR-112 D5): printing an unedited tree reproduces the input byte
//! for byte, for every bundled `.org` file, every synthetic fixture, their
//! CRLF variants, and generated large inputs — with the structural
//! invariants of `support::check_tree` on every tree.

mod support;

use mae_org::{parse, ParseOptions, DEFAULT_MAX_DEPTH};
use support::*;

fn assert_round_trip(name: &str, text: &str) {
    let doc = parse(text, &ParseOptions::default());
    if let Err(e) = check_tree(&doc, text, DEFAULT_MAX_DEPTH) {
        panic!("{name}: {e}");
    }
    assert_eq!(unparsed_count(&doc), 0, "{name}: UNPARSED tokens");
}

#[test]
fn every_bundled_org_file_and_its_crlf_variant_round_trips() {
    let files = bundled_corpus();
    // The bundled corpus had 340 files on 2026-10-31; a walk that silently
    // finds nothing must not pass.
    assert!(
        files.len() >= 300,
        "corpus walk found only {} files",
        files.len()
    );
    for f in &files {
        let text = read(f);
        assert_round_trip(&rel(f), &text);
        assert_round_trip(&format!("{} (CRLF)", rel(f)), &to_crlf(&text));
    }
    eprintln!("round trip: {} bundled files x 2 (LF, CRLF)", files.len());
}

#[test]
fn every_fixture_and_its_crlf_and_bom_variants_round_trip() {
    let files = fixtures();
    assert!(
        files.len() >= 30,
        "fixture walk found only {} files",
        files.len()
    );
    for f in &files {
        let text = read(f);
        assert_round_trip(&rel(f), &text);
        assert_round_trip(&format!("{} (CRLF)", rel(f)), &to_crlf(&text));
        assert_round_trip(&format!("{} (BOM)", rel(f)), &format!("\u{feff}{text}"));
    }
    eprintln!(
        "round trip: {} fixtures x 3 (as stored, CRLF, BOM)",
        files.len()
    );
}

/// Generated rather than checked in: they are large and wholly regular.
#[test]
fn generated_large_inputs_round_trip() {
    let cases: Vec<(&str, String)> = vec![
        (
            "20k headings",
            (0..20_000)
                .map(|i| format!("{} heading {i}\n", "*".repeat(1 + i % 5)))
                .collect(),
        ),
        (
            "2000 nested same-name quotes",
            format!(
                "{}x\n{}",
                "#+begin_quote\n".repeat(2000),
                "#+end_quote\n".repeat(2000)
            ),
        ),
        ("2000 nested distinct blocks", nested_blocks(2000)),
        (
            "5000-deep list",
            (0..5000)
                .map(|i| format!("{}- {i}\n", " ".repeat(i)))
                .collect(),
        ),
        ("1 MB single line", "word *b* /i/ [[l]] ".repeat(55_000)),
        ("*a x 20k one line", "*a ".repeat(20_000)),
        ("[[ x 50k", "[[".repeat(50_000)),
        ("only newlines", "\n".repeat(10_000)),
        ("only CRs", "\r".repeat(10_000)),
    ];
    for (name, text) in &cases {
        assert_round_trip(name, text);
    }
}

pub fn nested_blocks(n: usize) -> String {
    let open: String = (0..n).map(|i| format!("#+begin_b{i}\n")).collect();
    let close: String = (0..n).rev().map(|i| format!("#+end_b{i}\n")).collect();
    format!("{open}core\n{close}")
}
