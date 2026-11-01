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

/// The corpus walk must see exactly the `.org` files git tracks: a
/// gitignored, machine-generated file (`*-instance.org` from `:kb-register`)
/// made every corpus count differ between machines. Skipped without git.
#[test]
fn the_bundled_corpus_is_exactly_the_tracked_org_files() {
    let root = repo_root();
    let Ok(out) = std::process::Command::new("git")
        .args(["ls-files", "assets", "tests"])
        .current_dir(&root)
        .output()
    else {
        return;
    };
    let mut tracked: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.ends_with(".org"))
        .map(str::to_string)
        .collect();
    tracked.sort();
    let mut walked: Vec<String> = bundled_corpus().iter().map(|p| rel(p)).collect();
    walked.sort();
    assert_eq!(walked, tracked);
}

/// Review F12: the corpus is the files git TRACKS, not "not ignored": an
/// untracked file (a scratch note, a file `:kb-register` wrote) must not
/// enter it. A scratch repository holds one of each kind; only the tracked
/// file is listed. Skipped without git.
#[test]
fn untracked_and_ignored_files_are_not_in_the_corpus() {
    let dir = std::env::temp_dir().join(format!("mae-org-corpus-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&dir)
            .output()
            .is_ok_and(|o| o.status.success())
    };
    if !git(&["init", "-q"]) {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    for f in [
        "tests/tracked.org",
        "tests/untracked.org",
        "tests/ignored.org",
    ] {
        std::fs::write(dir.join(f), "* h\n").unwrap();
    }
    std::fs::write(dir.join(".gitignore"), "tests/ignored.org\n").unwrap();
    assert!(git(&["add", "tests/tracked.org", ".gitignore"]));
    let got = support::tracked_org_files(&dir, &["tests/*.org"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got, Some(vec![dir.join("tests/tracked.org")]));
}
