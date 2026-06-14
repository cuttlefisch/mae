//! Performance / scale validation for KB search & traversal.
//!
//! Complements the accuracy dipstick (`kb_search_grading.rs`) with the other
//! half of the user's ask: numerical support for *performance*. Builds a large
//! synthetic KB and asserts that per-query work is bounded — both in latency
//! (generous wall-clock ceilings, well above observed, to stay CI-stable) and
//! in result-set size (never unbounded). Run with:
//!
//!   cargo test -p mae --test kb_search_perf -- --nocapture
//!
//! Latency numbers are printed for visibility; the *assertions* are coarse
//! ceilings (so a genuine O(n²) regression trips them) rather than tight
//! benchmarks (which would flake under CI load).

use std::time::Instant;

use mae_kb::{KnowledgeBase, Node, NodeKind};

const N: usize = 5_000;

/// Build a synthetic KB of `N` interlinked, tagged nodes. Each node links to a
/// few neighbors (a ring + a hub) so the link graph is non-trivial for
/// `related`, and carries a couple of tags drawn from a small vocabulary.
fn build_synthetic_kb(n: usize) -> KnowledgeBase {
    let mut kb = KnowledgeBase::new();
    let topics = ["alpha", "beta", "gamma", "delta", "epsilon"];
    for i in 0..n {
        let id = format!("note:n{i}");
        // Link to the next two nodes (ring) and a shared hub every 50 nodes.
        let next1 = (i + 1) % n;
        let next2 = (i + 2) % n;
        let hub = (i / 50) * 50;
        let body = format!(
            "Node {i} about buffer window command. See [[note:n{next1}]] and \
             [[note:n{next2}]] and hub [[note:n{hub}]].",
        );
        let mut node = Node::new(&id, format!("Note {i} title"), NodeKind::Note, &body);
        node.tags = vec![
            topics[i % topics.len()].to_string(),
            topics[(i / 7) % topics.len()].to_string(),
        ];
        kb.insert(node);
    }
    kb
}

/// p50/p95 of a slice of millisecond timings.
fn percentiles(mut ms: Vec<f64>) -> (f64, f64) {
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pick = |p: f64| ms[((ms.len() as f64 * p) as usize).min(ms.len() - 1)];
    (pick(0.50), pick(0.95))
}

#[test]
fn search_ranked_scales_to_thousands_of_nodes() {
    let kb = build_synthetic_kb(N);
    assert_eq!(kb.len(), N);

    let queries = [
        "buffer",
        "window command",
        "note title",
        "buffer window command",
        "alpha",
        "n1234",
        "nonexistent term here",
    ];

    let mut timings = Vec::new();
    for _ in 0..5 {
        for q in &queries {
            let start = Instant::now();
            let results = kb.search_ranked(q, 20);
            timings.push(start.elapsed().as_secs_f64() * 1000.0);
            // Result set is always bounded by the limit — never unbounded.
            assert!(results.len() <= 20, "search_ranked exceeded its limit");
        }
    }

    let (p50, p95) = percentiles(timings);
    println!("\n=== search_ranked over {N} nodes ===\n  p50 = {p50:.2} ms   p95 = {p95:.2} ms");
    // search_ranked is O(nodes × terms) — linear in corpus size. These tests
    // run in a DEBUG build (no opt), where a linear pass over 5k nodes is tens
    // of ms; release is ~10× faster. The ceiling is therefore coarse: it exists
    // to trip an *algorithmic* regression (an accidental O(n²) join would be
    // multiple seconds at this N), not to benchmark — so it stays well above
    // observed debug latency to avoid CI flake. (Per-keystroke completion at
    // this scale is Phase 6's debounce/server-top-N job, not this path's.)
    assert!(
        p95 < 600.0,
        "search_ranked p95 too slow over {N} nodes: {p95:.2} ms (possible O(n²) regression)"
    );
}

#[test]
fn related_scales_to_thousands_of_nodes() {
    let kb = build_synthetic_kb(N);

    // Sample seeds spread across the corpus.
    let seeds: Vec<String> = (0..N)
        .step_by(N / 20)
        .map(|i| format!("note:n{i}"))
        .collect();

    let mut timings = Vec::new();
    for seed in &seeds {
        let start = Instant::now();
        let related = kb.related(seed, 10);
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(related.len() <= 10, "related exceeded its limit");
        // Scores are sorted descending and the seed never appears.
        let scores: Vec<f64> = related.iter().map(|(_, s)| *s).collect();
        assert!(
            scores.windows(2).all(|w| w[0] >= w[1]),
            "related not sorted"
        );
        assert!(
            related.iter().all(|(id, _)| id != seed),
            "related includes the seed itself"
        );
    }

    let (p50, p95) = percentiles(timings);
    println!("\n=== related over {N} nodes ===\n  p50 = {p50:.2} ms   p95 = {p95:.2} ms");
    // `related` is a bounded graph walk (2-hop) + one linear tag scan — linear
    // in corpus size. Same debug-build/coarse-ceiling reasoning as above: the
    // bound catches a quadratic regression, not normal debug latency.
    assert!(
        p95 < 800.0,
        "related p95 too slow over {N} nodes: {p95:.2} ms (possible O(n²) regression)"
    );
}
