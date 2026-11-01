//! The generative leak differential: anything Emacs 30.2's ox-html
//! (`emacs -Q --batch`, body only, babel never evaluating) does not show
//! must appear in NO mae-org output channel, in any render mode (see
//! `leak_oracle/channels.rs` for the channels and modes).
//!
//! ```sh
//! cargo test -p mae-org --release --test render_leak_oracle -- --ignored --nocapture
//! ```
//!
//! Environment (all optional):
//! `LEAK_ORACLE_SEED` (default 1), `LEAK_ORACLE_COUNT` generated documents
//! (default 2000), `LEAK_ORACLE_DEPTH` maximum nesting (default 3),
//! `LEAK_ORACLE_JOBS` parallel Emacs processes (default 4),
//! `LEAK_ORACLE_CHUNK` documents per Emacs process (default 100),
//! `LEAK_ORACLE_TIMEOUT` seconds per Emacs process (default 20, at most
//! 120; a chunk exports in about 2 s, so this bounds what a document that
//! makes Emacs loop costs), `LEAK_ORACLE_MINIMISE` minimised repros per category (default 1;
//! 0 for none; one for all of a citation's contents), `LEAK_ORACLE_BUDGET`
//! Emacs runs per minimisation (default 40, each with a 5 s timeout), `LEAK_ORACLE_FIXED=0` to skip the fixed cases,
//! `LEAK_ORACLE_NO_SCOPE=1` to use `ulimit -v` instead of `systemd-run`.
//!
//! Each document plants unique markers (`qz<n>qz`), one per site, and the
//! generator records each site's intent ([`leak_oracle::gen::Allow`]).
//! Emacs exports an `id:` link as a link to an opaque target with its
//! description (`conformance/leak-export.el`: Org cannot export a KB link
//! without ID tracking, and nothing of its path is Org's to show). A LEAK
//! is a marker
//! MAE shows in any channel that Emacs's HTML (attributes included) does
//! not, except a plain property value, alias or title in a metadata
//! channel, and an id link's `#fragment` inside an `href`. Where Emacs's
//! export fails, MAE must render nothing, except for the errors the
//! register lists ([`leak_oracle::judge::REGISTERED_ERRORS`]). MAE hiding
//! more is counted (lost, fail-closed), never failed. Every failure
//! category is minimised by delta debugging and printed with both outputs.
//! The corpus, scratch HOME and report go under `target/leak-oracle/`.

mod leak_oracle;
mod support;

use leak_oracle::channels::mae_view;
use leak_oracle::emacs::{Emacs, EmacsOut};
use leak_oracle::gen::Doc;
use leak_oracle::judge::{judge, Verdict};
use leak_oracle::minimise::{minimise, Target};
use leak_oracle::report::{around, Outcome, Tally};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Instant;

#[derive(Debug, Clone)]
struct Config {
    seed: u64,
    count: u64,
    depth: usize,
    jobs: usize,
    chunk: usize,
    timeout: u64,
    minimise: usize,
    budget: usize,
    fixed: bool,
}

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn config() -> Config {
    Config {
        seed: env_or("LEAK_ORACLE_SEED", 1),
        count: env_or("LEAK_ORACLE_COUNT", 2000),
        depth: env_or("LEAK_ORACLE_DEPTH", 3),
        jobs: env_or("LEAK_ORACLE_JOBS", 4usize).max(1),
        chunk: env_or("LEAK_ORACLE_CHUNK", 100usize).max(1),
        timeout: env_or("LEAK_ORACLE_TIMEOUT", 20),
        minimise: env_or("LEAK_ORACLE_MINIMISE", 1),
        budget: env_or("LEAK_ORACLE_BUDGET", 40),
        fixed: env_or("LEAK_ORACLE_FIXED", 1u8) != 0,
    }
}

fn scratch_root() -> PathBuf {
    let root = support::repo_root().join("target/leak-oracle");
    std::fs::create_dir_all(&root).expect("target/leak-oracle");
    root
}

#[test]
#[ignore = "needs Emacs 30.2; run locally (see the file header)"]
fn leak_oracle() {
    let cfg = config();
    let root = scratch_root();
    let el = support::repo_root().join("shared/org/conformance/leak-export.el");
    let emacs = Emacs::new(&root, &el, cfg.timeout);
    let mut docs = if cfg.fixed {
        leak_oracle::fixed::docs()
    } else {
        Vec::new()
    };
    let fixed = docs.len();
    docs.extend((0..cfg.count).map(|i| leak_oracle::gen_doc::document(cfg.seed, i, cfg.depth)));
    write_corpus(&root, &cfg, &docs);
    eprintln!("leak oracle: {cfg:?}, systemd scope: {}", emacs.scoped());
    let start = Instant::now();
    let outcomes = run_all(&docs, &emacs, &cfg, start);
    let tally = Tally::of(&outcomes, fixed);
    let mut report = tally.render(cfg.count as usize, start.elapsed());
    report.push_str(&fixed_summary(&docs[..fixed], &outcomes[..fixed]));
    report.push_str(&hang_list(&docs, &outcomes));
    let quick = Emacs::new(&root, &el, 5);
    report.push_str(&minimise_all(&docs, &outcomes, fixed, &quick, &cfg));
    let _ = writeln!(report, "== total {:.1?}", start.elapsed());
    let path = root.join(format!("report-seed{}-n{}.txt", cfg.seed, cfg.count));
    std::fs::write(&path, &report).expect("report");
    eprintln!("{report}\nreport: {}", path.display());
    assert_eq!(
        tally.failures(),
        0,
        "leaks or unregistered Emacs-error renders: see {}",
        path.display()
    );
}

fn write_corpus(root: &std::path::Path, cfg: &Config, docs: &[Doc]) {
    let dir = root.join(format!("corpus-seed{}", cfg.seed));
    std::fs::create_dir_all(&dir).expect("corpus dir");
    for d in docs {
        std::fs::write(dir.join(format!("{}.org", d.name)), &d.text).expect("corpus doc");
    }
}

/// Exports and judges every document, `cfg.jobs` Emacs processes at a
/// time, each over `cfg.chunk` documents.
fn run_all(docs: &[Doc], emacs: &Emacs, cfg: &Config, start: Instant) -> Vec<Outcome> {
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel();
    let mut slots: Vec<Option<Outcome>> = vec![None; docs.len()];
    std::thread::scope(|s| {
        for _ in 0..cfg.jobs {
            let (tx, next) = (tx.clone(), &next);
            s.spawn(move || loop {
                let lo = next.fetch_add(1, Ordering::Relaxed) * cfg.chunk;
                if lo >= docs.len() {
                    break;
                }
                let hi = (lo + cfg.chunk).min(docs.len());
                run_chunk(&docs[lo..hi], lo, emacs, start, &tx);
            });
        }
        drop(tx);
        for (k, o) in rx.into_iter().enumerate() {
            if k % 500 == 499 {
                eprintln!("  {} judged, {:.1?}", k + 1, start.elapsed());
            }
            let i = o.doc;
            slots[i] = Some(o);
        }
    });
    slots
        .into_iter()
        .map(|o| o.expect("every document judged"))
        .collect()
}

fn run_chunk(docs: &[Doc], lo: usize, emacs: &Emacs, start: Instant, tx: &mpsc::Sender<Outcome>) {
    let refs: Vec<&str> = docs.iter().map(|d| d.text.as_str()).collect();
    for (k, e) in emacs.export(&refs).into_iter().enumerate() {
        let d = &docs[k];
        let verdict = judge(&e, &mae_view(&d.text), &d.sites, &d.text);
        let at = start.elapsed();
        let _ = tx.send(Outcome {
            doc: lo + k,
            verdict,
            at,
        });
    }
}

/// The documents Emacs did not finish (each cost a whole timeout).
fn hang_list(docs: &[Doc], outcomes: &[Outcome]) -> String {
    let hung: Vec<String> = outcomes
        .iter()
        .filter_map(|o| match o.verdict {
            Verdict::EmacsHang(why) => Some(format!("{} ({why})", docs[o.doc].name)),
            _ => None,
        })
        .collect();
    format!(
        "-- Emacs did not finish ({}): {}\n",
        hung.len(),
        hung.join(", ")
    )
}

fn fixed_summary(docs: &[Doc], outcomes: &[Outcome]) -> String {
    let mut s = String::from("-- fixed cases:\n");
    for (d, o) in docs.iter().zip(outcomes) {
        let v = match &o.verdict {
            Verdict::Compared { leaks, .. } if leaks.is_empty() => "clean".to_string(),
            Verdict::Compared { leaks, .. } => {
                let ls: Vec<String> = leaks
                    .iter()
                    .map(|l| format!("{}@{}", l.category, l.channels[0].0))
                    .collect();
                format!("LEAK {}", ls.join(", "))
            }
            other => format!("{other:?}"),
        };
        let _ = writeln!(s, "   {:<48} {v}", d.name);
    }
    s
}

/// One target per failure category: the smallest document showing it.
fn targets(docs: &[Doc], outcomes: &[Outcome], fixed: usize, per: usize) -> Vec<(usize, Target)> {
    let mut by: BTreeMap<String, Vec<(usize, Target)>> = BTreeMap::new();
    for o in outcomes.iter().filter(|o| o.doc >= fixed) {
        let ts: Vec<Target> = match &o.verdict {
            Verdict::Compared { leaks, .. } => leaks
                .iter()
                .filter(|l| !l.category.starts_with("citation>") || l.leaf.ends_with("cite-key"))
                .map(|l| Target::Leak(l.category.clone()))
                .collect(),
            Verdict::EmacsErrorMaeRenders {
                class,
                registered: None,
            } => vec![Target::ErrorRenders(class.clone())],
            _ => Vec::new(),
        };
        for t in ts {
            by.entry(format!("{t:?}")).or_default().push((o.doc, t));
        }
    }
    let mut out = Vec::new();
    for (_, mut v) in by {
        v.sort_by_key(|(i, _)| docs[*i].text.len());
        v.dedup_by_key(|(i, _)| *i);
        out.extend(v.into_iter().take(per));
    }
    out
}

fn minimise_all(
    docs: &[Doc],
    outcomes: &[Outcome],
    fixed: usize,
    emacs: &Emacs,
    cfg: &Config,
) -> String {
    let ts = targets(docs, outcomes, fixed, cfg.minimise);
    let next = AtomicUsize::new(0);
    let mut parts: Vec<(usize, String)> = Vec::new();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..cfg.jobs)
            .map(|_| {
                let (ts, next) = (&ts, &next);
                s.spawn(move || {
                    let mut mine = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some((i, t)) = ts.get(k) else {
                            break;
                        };
                        mine.push((k, repro(&docs[*i], t, emacs, cfg.budget)));
                    }
                    mine
                })
            })
            .collect();
        for h in handles {
            parts.extend(h.join().expect("minimiser thread"));
        }
    });
    parts.sort();
    let mut s = format!("== minimised repros ({}):\n", parts.len());
    for (_, p) in parts {
        s.push_str(&p);
    }
    s
}

/// The minimised document for `target`, with Emacs's output and the
/// channels that leak.
fn repro(doc: &Doc, target: &Target, emacs: &Emacs, budget: usize) -> String {
    let t0 = Instant::now();
    let (min, runs) = minimise(&doc.text, &doc.sites, target, emacs, budget);
    let e = emacs.export(&[min.as_str()]).remove(0);
    let view = mae_view(&min);
    let verdict = judge(&e, &view, &doc.sites, &min);
    let mut s = format!(
        "\n---- {target:?} (from {}, {} -> {} bytes, {runs} Emacs runs, {:.1?})\n{min}",
        doc.name,
        doc.text.len(),
        min.len(),
        t0.elapsed()
    );
    let emacs_text = match &e {
        EmacsOut::Html(h) => h.clone(),
        other => format!("{other:?}"),
    };
    let _ = writeln!(s, "-- Emacs:\n{}", emacs_text.trim_end());
    let body = view
        .channels
        .iter()
        .find(|c| c.name.ends_with("res=false:body.html"));
    let _ = writeln!(
        s,
        "-- MAE body (pd=false, explicit): {}",
        body.map_or("", |c| c.text.as_str())
    );
    describe_leaks(&mut s, &verdict, &view);
    s
}

fn describe_leaks(s: &mut String, verdict: &Verdict, view: &leak_oracle::channels::MaeView) {
    let Verdict::Compared { leaks, .. } = verdict else {
        let _ = writeln!(s, "-- verdict: {verdict:?}");
        return;
    };
    for l in leaks {
        let names: Vec<&str> = l.channels.iter().map(|c| c.0.as_str()).collect();
        let _ = writeln!(
            s,
            "-- LEAK qz{}qz [{}] {} in {} channels: {:?}",
            l.marker,
            l.category,
            l.path,
            names.len(),
            &names[..names.len().min(4)]
        );
        if let Some(ch) = view.channels.iter().find(|c| c.name == l.channels[0].0) {
            let _ = writeln!(s, "   {}: …{}…", ch.name, around(&ch.text, l.marker));
        }
    }
}
