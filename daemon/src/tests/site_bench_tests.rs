//! ADR-119 D6: where a whole-KB derivation spends its time. `#[ignore]`d and
//! non-gating: it prints numbers, asserts nothing. Point it at a bundle of
//! `.org` files:
//!
//! ```text
//! cd daemon && MAE_SITE_BENCH_BUNDLE=/path/to/bundle cargo test --release --bin mae-daemon site_bench -- --ignored --nocapture
//! ```

use std::time::Instant;

fn bundle() -> Vec<mae_site::SourcePage> {
    let Ok(root) = std::env::var("MAE_SITE_BENCH_BUNDLE") else {
        return Vec::new();
    };
    let mut files = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(root)];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "org") {
                files.push(p);
            }
        }
    }
    files.sort();
    files
        .into_iter()
        .map(|p| mae_site::SourcePage {
            origin: p.display().to_string(),
            text: std::fs::read_to_string(&p).unwrap(),
            attachments: Default::default(),
        })
        .collect()
}

#[test]
#[ignore]
fn site_bench_whole_kb_derivation() {
    let sources = bundle();
    if sources.is_empty() {
        eprintln!("set MAE_SITE_BENCH_BUNDLE");
        return;
    }
    let addressing = mae_site::Addressing::live("kb-bench").unwrap();
    for (label, hl) in [
        ("plain", None),
        ("highlighted", Some(mae_daemon::site_highlight::shared())),
    ] {
        let opts = mae_site::SiteOptions {
            hidden_namespaces: vec![],
            highlighter: hl,
        };
        for run in 0..3 {
            let t = Instant::now();
            let site = mae_site::derive_with(&sources, &addressing, &opts);
            // Equivalence across builds: the whole output, written once.
            if let (0, Ok(dir)) = (run, std::env::var("MAE_SITE_BENCH_DUMP")) {
                let path = format!("{dir}/{label}.txt");
                std::fs::write(&path, format!("{site:#?}")).unwrap();
            }
            eprintln!(
                "BENCH {label} run={run} pages={} ms={}",
                site.pages.len(),
                t.elapsed().as_millis()
            );
        }
    }
}

/// D6: one page edited, then the KB derived again with the kept state. ADR-119's
/// live target is a re-derivation within 200 ms of one node's change.
#[test]
#[ignore]
fn site_bench_one_edit_re_derived() {
    let mut sources = bundle();
    if sources.is_empty() {
        eprintln!("set MAE_SITE_BENCH_BUNDLE");
        return;
    }
    let addressing = mae_site::Addressing::live("kb-bench").unwrap();
    let opts = mae_site::SiteOptions {
        hidden_namespaces: vec![],
        highlighter: Some(mae_daemon::site_highlight::shared()),
    };
    let mut d = mae_site::incremental::Deriver::default();
    let t = Instant::now();
    d.derive(&sources, &addressing, &opts);
    eprintln!(
        "BENCH cold ms={} rendered={}",
        t.elapsed().as_millis(),
        d.last_rendered()
    );
    for run in 0..5 {
        let i = (run * 311) % sources.len();
        sources[i]
            .text
            .push_str(&format!("\nAn edit, run {run}.\n"));
        let t = Instant::now();
        d.derive(&sources, &addressing, &opts);
        eprintln!(
            "BENCH one-edit run={run} ms={} rendered={}",
            t.elapsed().as_millis(),
            d.last_rendered()
        );
    }
}
