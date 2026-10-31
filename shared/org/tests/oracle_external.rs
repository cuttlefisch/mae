//! The Emacs differential over an EXTERNAL directory, run locally:
//!
//! ```sh
//! MAE_ORG_ORACLE_DIR=~/src/org-mode cargo test -p mae-org --release \
//!     --test oracle_external -- --ignored --nocapture
//! ```
//!
//! It runs `emacs -Q --batch` with `conformance/org-element-dump.el` over every
//! `.org` file under the directory (one Emacs process), compares element kinds
//! and spans with mae-org's, and prints agreement per kind plus the first
//! differences. Nothing is written into the repository, so a private corpus
//! can be checked without its content leaving the machine. It reports; it does
//! not fail on disagreement (the checked-in goldens are the gate).

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use support::*;

#[test]
#[ignore = "needs emacs and MAE_ORG_ORACLE_DIR; run locally"]
fn oracle_diff_external_dir() {
    let Ok(dir) = std::env::var("MAE_ORG_ORACLE_DIR") else {
        eprintln!("MAE_ORG_ORACLE_DIR not set; nothing to do");
        return;
    };
    let files = org_files(Path::new(&dir));
    let out = std::env::temp_dir().join(format!("mae-org-oracle-{}", std::process::id()));
    let goldens = run_oracle(&files, &out);
    let mut per_kind: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let (mut agree_files, mut shown) = (0, 0);
    for (file, golden) in files.iter().zip(&goldens) {
        let Some(golden) = golden else { continue };
        let text = read(file);
        let checked = if is_pure_crlf(&text) {
            text.replace("\r\n", "\n")
        } else {
            text.clone()
        };
        if let Err(e) = check_golden_header(golden, &checked) {
            eprintln!("== {}: oracle self-check failed: {e}", file.display());
            continue;
        }
        let lf = text.replace("\r\n", "\n");
        let oracle: Vec<El> = golden_elements(golden)
            .into_iter()
            .map(|(e, _)| e)
            .collect();
        // A pure-CRLF file was fed to the oracle as LF (see run_oracle).
        let oracle: Vec<El> = if is_pure_crlf(&text) {
            oracle.iter().map(|e| lf_to_crlf(&lf, e)).collect()
        } else {
            oracle
        };
        let mine = mae_elements(&parse_default(&text));
        tally(&oracle, &mine, &mut per_kind);
        let (only_oracle, only_mae) = diff(&oracle, &mine);
        if only_oracle.is_empty() && only_mae.is_empty() {
            agree_files += 1;
        } else if shown < 40 {
            shown += 1;
            eprintln!(
                "== {}\n  oracle only: {:?}\n  mae only:    {:?}",
                file.display(),
                first(&only_oracle),
                first(&only_mae)
            );
        }
    }
    let _ = std::fs::remove_dir_all(&out);
    eprintln!("files fully agreeing: {agree_files}/{}", files.len());
    for (k, (m, t)) in per_kind {
        eprintln!("  {k:22} {m:6}/{t:6}");
    }
}

fn first(v: &[El]) -> &[El] {
    &v[..v.len().min(6)]
}

/// Per kind: (oracle elements matched exactly by mae-org, oracle total).
fn tally(oracle: &[El], mine: &[El], per_kind: &mut BTreeMap<String, (usize, usize)>) {
    let (only_oracle, _) = diff(oracle, mine);
    for e in oracle {
        let entry = per_kind.entry(e.kind.clone()).or_default();
        entry.0 += 1;
        entry.1 += 1;
    }
    for e in &only_oracle {
        if let Some(entry) = per_kind.get_mut(&e.kind) {
            entry.0 -= 1;
        }
    }
}

/// Runs the oracle over `files` in one Emacs process; `None` where it failed.
fn run_oracle(files: &[std::path::PathBuf], out: &Path) -> Vec<Option<String>> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/org-element-dump.el");
    std::fs::create_dir_all(out).expect("scratch dir");
    let manifest = out.join("manifest");
    // Emacs decodes a pure-CRLF file with DOS line endings for a user; the
    // oracle reads `utf-8-unix` (exact bytes), so it is given the LF form and
    // the spans are mapped back. Lone CRs are left as they are.
    let lines: String = files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let text = std::fs::read_to_string(f).unwrap_or_default();
            let input = if is_pure_crlf(&text) {
                let lf = out.join(format!("{i}.lf.org"));
                std::fs::write(&lf, text.replace("\r\n", "\n")).expect("LF copy");
                lf
            } else {
                f.clone()
            };
            format!(
                "{}\t{}\n",
                input.display(),
                out.join(format!("{i}.jsonl")).display()
            )
        })
        .collect();
    std::fs::write(&manifest, lines).expect("manifest");
    let status = std::process::Command::new("emacs")
        .args(["-Q", "--batch", "-l"])
        .arg(&script)
        .arg("--manifest")
        .arg(&manifest)
        .status()
        .expect("emacs runs");
    assert!(status.success(), "oracle failed");
    (0..files.len())
        .map(|i| std::fs::read_to_string(out.join(format!("{i}.jsonl"))).ok())
        .collect()
}

/// Prints mae-org's elements for one file, in the oracle's format, for
/// investigating a difference: `MAE_ORG_DUMP=path cargo test -p mae-org
/// --test oracle_external dump_one_file -- --ignored --nocapture`.
#[test]
#[ignore = "debugging aid; needs MAE_ORG_DUMP"]
fn dump_one_file() {
    let Ok(path) = std::env::var("MAE_ORG_DUMP") else {
        return;
    };
    for e in mae_elements(&parse_default(&read(Path::new(&path)))) {
        println!("{} {} {}", e.kind, e.begin, e.end);
    }
}
