//! Measures org-parse latency over a real corpus — the input to ADR-100 D4's
//! decision about where the browser gets org structure from.
//!
//! D4 asks whether compiling MAE's own org parser to WASM is viable, and the
//! interactive budget is the load-bearing half of that: a live-preview editor
//! re-parses on (debounced) keystrokes, so a parse that costs milliseconds per
//! keypress rules the approach out regardless of bundle size.
//!
//! This measures the **native** cost. WASM is typically 1–3× native for this
//! kind of pure byte-scanning work, so a native figure is a lower bound, not an
//! answer — but if native is already too slow, no WASM build will save it.
//!
//! Usage: `cargo run --release --example org_parse_latency -p mae-kb -- <dir>...`
//! Defaults to the bundled practice KBs when given no arguments.

use std::time::Instant;

/// Reps per file — enough that a sub-millisecond parse is measurable above
/// timer noise, without the run itself taking long.
const REPS: u32 = 200;

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dirs: Vec<String> = if args.is_empty() {
        vec![
            "assets/devpractices".to_string(),
            "assets/practices".to_string(),
        ]
    } else {
        args
    };

    let mut corpus: Vec<(String, String)> = Vec::new();
    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            eprintln!("skipping unreadable dir: {dir}");
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("org") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                corpus.push((path.display().to_string(), text));
            }
        }
    }

    if corpus.is_empty() {
        eprintln!("no .org files found in {dirs:?} — run from the repo root");
        std::process::exit(1);
    }

    let total_bytes: usize = corpus.iter().map(|(_, t)| t.len()).sum();
    println!(
        "corpus: {} files, {} bytes total, largest {} bytes\n",
        corpus.len(),
        total_bytes,
        corpus.iter().map(|(_, t)| t.len()).max().unwrap_or(0)
    );

    let hot_nodes = HotNodes::default();
    let ops = ops(&hot_nodes);

    for (name, op) in &ops {
        let mut per_file_us: Vec<f64> = Vec::with_capacity(corpus.len());
        let mut worst = (0.0f64, String::new(), 0usize);

        for (path, text) in &corpus {
            // Warm up so the first-touch page faults don't land in the sample.
            op(text);
            let start = Instant::now();
            for _ in 0..REPS {
                op(text);
            }
            let us = start.elapsed().as_secs_f64() * 1e6 / REPS as f64;
            per_file_us.push(us);
            if us > worst.0 {
                worst = (us, path.clone(), text.len());
            }
        }

        per_file_us.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("{name}");
        println!(
            "  p50 {:>8.1} us   p95 {:>8.1} us   max {:>8.1} us",
            percentile(&per_file_us, 0.50),
            percentile(&per_file_us, 0.95),
            percentile(&per_file_us, 1.0),
        );
        println!(
            "  slowest: {} ({} bytes) at {:.1} us\n",
            worst.1, worst.2, worst.0
        );
    }
}

/// One node per file, reused across reps (see the hot-loop op).
type HotNodes = std::cell::RefCell<std::collections::HashMap<usize, mae_kb::Node>>;

/// A measured operation: its name and what it does to one file's text.
type Op<'a> = (&'static str, Box<dyn Fn(&str) + 'a>);

/// Each measured operation is one a live-preview decorator, or the KB's own
/// graph code (#941 G3), actually needs, rather than a synthetic
/// microbenchmark.
fn ops(hot_nodes: &HotNodes) -> Vec<Op<'_>> {
    vec![
        (
            "parse_org_multi_result (full structure)",
            Box::new(|t: &str| {
                let _ = mae_kb::org::parse_org_multi_result(t);
            }),
        ),
        (
            "org_ingest::parse_file (mae-org ingest, full structure)",
            Box::new(|t: &str| {
                let _ = mae_kb::org_ingest::parse_file(t);
            }),
        ),
        (
            "rewrite_links_with_types (link scan)",
            Box::new(|t: &str| {
                let _ = mae_kb::org::rewrite_links_with_types(t);
            }),
        ),
        (
            "compute_code_block_ranges",
            Box::new(|t: &str| {
                let _ = mae_kb::compute_code_block_ranges(t);
            }),
        ),
        (
            "parse_typed_links",
            Box::new(|t: &str| {
                let _ = mae_kb::org::parse_typed_links(t, "spike:node");
            }),
        ),
        (
            "org_ingest::edges_of_body (mae-org)",
            Box::new(|t: &str| {
                let _ = mae_kb::org_ingest::edges_of_body(t);
            }),
        ),
        (
            "Node::links, fresh node per call (cold)",
            Box::new(|t: &str| {
                let n = mae_kb::Node::new("spike:node", "", mae_kb::NodeKind::Note, t);
                let _ = n.links();
            }),
        ),
        (
            "Node::links, same node every call (hot loop)",
            Box::new(move |t: &str| {
                // One node per file, reused across reps: what a graph scan over
                // an in-memory KB does (#941 G3).
                let mut nodes = hot_nodes.borrow_mut();
                let n = nodes.entry(t.as_ptr() as usize).or_insert_with(|| {
                    mae_kb::Node::new("spike:node", "", mae_kb::NodeKind::Note, t)
                });
                let _ = n.links();
                let _ = n.links_typed();
            }),
        ),
    ]
}
