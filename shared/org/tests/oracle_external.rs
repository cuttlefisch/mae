//! The Emacs differential over an EXTERNAL directory, run locally:
//!
//! ```sh
//! MAE_ORG_ORACLE_DIR=~/src/org-mode cargo test -p mae-org --release \
//!     --test oracle_external -- --ignored --nocapture
//! ```
//!
//! It runs `emacs -Q --batch` with `conformance/org-element-dump.el` over every
//! `.org` file under the directory (one Emacs process) and compares each
//! element — kind, span, contents and parent — with mae-org's. It prints
//! agreement per kind (exact, and span-only) plus the first differences.
//! Nothing is written into the repository, so a private corpus can be checked
//! without its content leaving the machine. It reports; it does not fail on
//! disagreement (the checked-in goldens are the gate).

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use support::*;

/// Per kind: (exact matches, span-only matches, oracle total).
type Tally = BTreeMap<String, (usize, usize, usize)>;

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
    let mut tally = Tally::new();
    let (mut agree, mut skipped, mut shown) = (0, 0, 0);
    let (mut views, mut view_bad) = (0, 0);
    for (file, golden) in files.iter().zip(&goldens) {
        let text = read(file);
        let checked = golden
            .as_deref()
            .ok_or_else(|| "no output".to_string())
            .and_then(|g| check_golden_header(g, &text).map(|_| g));
        let golden = match checked {
            Ok(g) => g,
            Err(e) => {
                skipped += 1;
                eprintln!("== {}: oracle unusable: {e}", file.display());
                continue;
            }
        };
        let oracle: Vec<El> = golden_elements(golden)
            .into_iter()
            .map(|(e, _)| e)
            .collect();
        let doc = parse_default(&text);
        let mine = mae_elements(&doc);
        count(&oracle, &mine, &mut tally);
        let (counts, bad) = compare_views(&doc, golden);
        views += counts.values().sum::<usize>();
        for m in bad.iter().take(3) {
            eprintln!("== {} VIEW {m}", file.display());
        }
        view_bad += bad.len();
        let (only_oracle, only_mae) = diff(&oracle, &mine);
        if only_oracle.is_empty() && only_mae.is_empty() {
            agree += 1;
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
    eprintln!(
        "files fully agreeing: {agree}/{} ({skipped} skipped: oracle error)",
        files.len() - skipped
    );
    eprintln!("view elements compared: {views}, field mismatches: {view_bad}");
    eprintln!("  {:22} {:>13} {:>13}", "kind", "exact", "span only");
    for (k, (exact, span, total)) in tally {
        eprintln!("  {k:22} {exact:6}/{total:6} {span:6}/{total:6}");
    }
}

fn first(v: &[El]) -> &[El] {
    &v[..v.len().min(6)]
}

fn span_only(v: &[El]) -> Vec<El> {
    v.iter()
        .map(|e| El {
            contents: None,
            parent: (String::new(), 0),
            ..e.clone()
        })
        .collect()
}

fn count(oracle: &[El], mine: &[El], tally: &mut Tally) {
    let (exact_miss, _) = diff(oracle, mine);
    let (span_miss, _) = diff(&span_only(oracle), &span_only(mine));
    for e in oracle {
        let t = tally.entry(e.kind.clone()).or_default();
        t.0 += 1;
        t.1 += 1;
        t.2 += 1;
    }
    for e in &exact_miss {
        if let Some(t) = tally.get_mut(&e.kind) {
            t.0 -= 1;
        }
    }
    for e in &span_miss {
        if let Some(t) = tally.get_mut(&e.kind) {
            t.1 -= 1;
        }
    }
}

/// Runs the oracle over `files` in one Emacs process; `None` where it wrote
/// nothing. A file Emacs itself fails on yields a `#error` line instead.
fn run_oracle(files: &[std::path::PathBuf], out: &Path) -> Vec<Option<String>> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/org-element-dump.el");
    std::fs::create_dir_all(out).expect("scratch dir");
    let manifest = out.join("manifest");
    let lines: String = files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            format!(
                "{}\t{}\n",
                f.display(),
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

/// Prints mae-org's elements for one file, for investigating a difference:
/// `MAE_ORG_DUMP=path cargo test -p mae-org --test oracle_external
/// dump_one_file -- --ignored --nocapture`.
#[test]
#[ignore = "debugging aid; needs MAE_ORG_DUMP"]
fn dump_one_file() {
    let Ok(path) = std::env::var("MAE_ORG_DUMP") else {
        return;
    };
    for e in mae_elements(&parse_default(&read(Path::new(&path)))) {
        println!("{e:?}");
    }
}
