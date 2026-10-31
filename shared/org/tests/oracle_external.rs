//! The Emacs differential over an EXTERNAL directory, run locally:
//!
//! ```sh
//! MAE_ORG_ORACLE_DIR=~/src/org-mode cargo test -p mae-org --release \
//!     --test oracle_external -- --ignored --nocapture
//! ```
//!
//! It runs `emacs -Q --batch` with `conformance/org-element-dump.el` over every
//! `.org` file under the directory and compares each
//! element — kind, span, contents and parent — with mae-org's. It prints
//! agreement per kind (exact, and span-only) plus the first differences.
//! Nothing is written into the repository, so a private corpus can be checked
//! without its content leaving the machine.
//!
//! Emacs is bounded (see `support::run_emacs_bounded`): files run in chunks
//! of 25 under a virtual-memory cap (`MAE_ORG_ORACLE_VMEM_KB`, default 2 GiB)
//! and a time limit (`MAE_ORG_ORACLE_TIMEOUT` seconds per file, default 60;
//! a chunk gets five files' time), and the files of a chunk that does not
//! finish run one at a time. A file that hits a limit is counted as "oracle:
//! Emacs did not terminate"; it is not a crash and does not stop the run. It reports; it does not fail on
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
    let (mut agree, mut skipped, mut shown, mut nonterminating) = (0, 0, 0, 0);
    let (mut views, mut view_bad) = (0, 0);
    for (file, golden) in files.iter().zip(&goldens) {
        let text = read(file);
        let checked = golden
            .as_deref()
            .ok_or_else(|| "no output".to_string())
            .and_then(|g| check_golden_header(g, &text).map(|_| g));
        let golden = match checked {
            Ok(g) if is_nonterminating(g) => {
                nonterminating += 1;
                eprintln!("== {}: oracle: Emacs did not terminate", file.display());
                continue;
            }
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
        "files fully agreeing: {agree}/{} ({skipped} skipped: oracle error; \
         {nonterminating} skipped: Emacs did not terminate)",
        files.len() - skipped - nonterminating
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

fn env_num(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Runs the oracle over `files`, bounded; `None` where it wrote nothing. A
/// file Emacs itself fails on yields a `#error` line instead, and one on
/// which it hits a cap a `#nonterminating` line.
fn run_oracle(files: &[std::path::PathBuf], out: &Path) -> Vec<Option<String>> {
    const CHUNK: usize = 25;
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/org-element-dump.el");
    let vmem_kb = env_num("MAE_ORG_ORACLE_VMEM_KB", 2 * 1024 * 1024);
    let per_file = std::time::Duration::from_secs(env_num("MAE_ORG_ORACLE_TIMEOUT", 60));
    std::fs::create_dir_all(out).expect("scratch dir");
    let dest = |i: usize| out.join(format!("{i}.jsonl"));
    let base = || -> Vec<std::ffi::OsString> {
        vec![
            "-Q".into(),
            "--batch".into(),
            "-l".into(),
            script.clone().into(),
        ]
    };
    // Preflight: a run that dies is recorded as "did not terminate", so the
    // dump must first be shown to work on a trivial file.
    let pre = out.join("preflight.org");
    std::fs::write(&pre, "* a\n").expect("preflight file");
    let pre_out = out.join("preflight.jsonl");
    let mut args = base();
    args.push(pre.into());
    let sink = std::fs::File::create(&pre_out).expect("preflight output");
    assert_eq!(
        run_emacs_bounded(&args, vmem_kb, per_file, sink.into()),
        EmacsExit::Ok,
        "the dump script fails on a trivial file"
    );
    assert!(read(&pre_out).contains("\"type\":\"#file\""));
    for (c, chunk) in files.chunks(CHUNK).enumerate() {
        let first = c * CHUNK;
        let manifest = out.join(format!("manifest-{c}"));
        let lines: String = chunk
            .iter()
            .enumerate()
            .map(|(k, f)| format!("{}\t{}\n", f.display(), dest(first + k).display()))
            .collect();
        std::fs::write(&manifest, lines).expect("manifest");
        let mut args = base();
        args.extend(["--manifest".into(), manifest.into()]);
        let _ = run_emacs_bounded(&args, vmem_kb, per_file * 5, std::process::Stdio::null());
        for (k, f) in chunk.iter().enumerate() {
            let d = dest(first + k);
            if d.exists() {
                continue;
            }
            let mut args = base();
            args.push(f.clone().into());
            let sink = std::fs::File::create(&d).expect("golden file");
            let why = match run_emacs_bounded(&args, vmem_kb, per_file, sink.into()) {
                EmacsExit::Ok => continue,
                EmacsExit::OutOfMemory => "memory".to_string(),
                EmacsExit::TimedOut => "time".to_string(),
                EmacsExit::Failed(code) => format!("exit {code:?}"),
            };
            std::fs::write(
                &d,
                format!("{{\"type\":\"#nonterminating\",\"limit\":\"{why}\"}}\n"),
            )
            .expect("golden file");
        }
    }
    (0..files.len())
        .map(|i| std::fs::read_to_string(dest(i)).ok())
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

/// The runner's caps hold on an input where org-element never terminates
/// (it allocates without bound; an unbounded oracle reached 16 GB on it): the
/// file is recorded as "Emacs did not terminate", within the limits.
#[test]
#[ignore = "needs emacs; run locally"]
fn oracle_runner_bounds_an_input_emacs_never_finishes() {
    let out = std::env::temp_dir().join(format!("mae-org-oracle-cap-{}", std::process::id()));
    std::fs::create_dir_all(&out).expect("scratch dir");
    let input = out.join("loop.org");
    std::fs::write(&input, "<<<x>>>\n\nx[[x]]\n").expect("input");
    let started = std::time::Instant::now();
    let goldens = run_oracle(&[input], &out.join("run"));
    let took = started.elapsed();
    let _ = std::fs::remove_dir_all(&out);
    let golden = goldens[0].as_deref().expect("a golden");
    assert!(is_nonterminating(golden), "{golden}");
    let limit = std::time::Duration::from_secs(env_num("MAE_ORG_ORACLE_TIMEOUT", 60));
    assert!(
        took < limit * 6 + std::time::Duration::from_secs(10),
        "{took:?}"
    );
    eprintln!("bounded in {took:?}: {golden}");
}
