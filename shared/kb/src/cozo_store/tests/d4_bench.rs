//! **D4 — the hosted resource budget's measurements, kept runnable.**
//!
//! ADR-109 sets a resource budget for the hosted deployment. Its numbers live
//! HERE, as benchmarks anyone can re-run, rather than only in the ADR's prose —
//! a measured number written into prose goes stale silently and then primes every
//! later reader with a false figure (CLAUDE.md's "never write a line count, or any
//! other measured number, into that prose").
//!
//! `#[ignore]`d because they build multi-megabyte stores; they are documentation
//! that executes, not part of the normal suite.
//!
//! ```text
//! cargo test --release -p mae-kb --lib d4_bench -- --ignored --nocapture
//! BENCH_N=8000 cargo test --release -p mae-kb --lib bytes_per_node -- --ignored --nocapture
//! ```

use super::*;

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))?
                .split_whitespace()
                .nth(1)?
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// On-disk cost per node, and whether it is flat in corpus size.
///
/// Flatness is the property the budget actually needs: a per-node figure that
/// drifts with N cannot be multiplied out to a capacity plan. Measured 2026-08-25
/// on a ~1.2 KB body with two tags and one link: **10,350 B/node at n=2,000 and
/// 10,364 B/node at n=8,000** — flat, and ~8.6x the raw body (FTS index, row
/// overhead, CRDT document).
#[test]
#[ignore = "d4_bench: builds a multi-MB store"]
fn bytes_per_node() {
    let n = env_usize("BENCH_N", 2000);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("kb.sqlite");
    {
        let store = crate::cozo_store::CozoKbStore::open_with_engine(&path, "sqlite").unwrap();
        store.seed_type_system().unwrap();
        for i in 0..n {
            let body = format!(
                "Node {i}. {}",
                "lorem ipsum dolor sit amet consectetur ".repeat(30)
            );
            let mut node = Node::new(
                format!("n:{i}"),
                format!("Title {i}"),
                NodeKind::Note,
                &body,
            );
            node.tags = vec!["alpha".into(), "beta".into()];
            store.insert_node(&node).unwrap();
            if i > 0 {
                store
                    .add_link(
                        &format!("n:{i}"),
                        &format!("n:{}", i - 1),
                        Some("relates_to"),
                    )
                    .ok();
            }
        }
    }
    // A sqlite store is no longer one file (WAL sidecars) -- sum the directory.
    let total: u64 = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum();
    eprintln!(
        "D4 bytes_per_node: n={n} total={total}B per_node={}B",
        total / n as u64
    );
}

/// Marginal resident memory per OPEN KB store.
///
/// This is the number ADR-108's per-tenant-process model multiplies, so it is the
/// one that decides whether that model is affordable. Measured 2026-08-25:
/// **3.85 MB/store at 4 stores, 2.44 MB/store at 16** — the fixed cost amortizes,
/// so the marginal figure converges toward ~2.4 MB.
///
/// Corroborated independently: a live `mae-daemon` with ~8 stores open measured
/// 33 MB RSS, against ~10 MB base + 8 x 2.4 MB = ~29 MB predicted here. Two
/// unrelated methods agreeing is what makes this safe to build a budget on.
#[test]
#[ignore = "d4_bench: opens many stores"]
fn rss_per_open_store() {
    let k = env_usize("BENCH_K", 16);
    let nodes = env_usize("BENCH_N", 200);
    let tmp = tempfile::tempdir().unwrap();
    let base = rss_kb();
    let mut held = Vec::new();
    for s in 0..k {
        let path = tmp.path().join(format!("kb{s}")).join("kb.sqlite");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let store = crate::cozo_store::CozoKbStore::open_with_engine(&path, "sqlite").unwrap();
        store.seed_type_system().unwrap();
        for i in 0..nodes {
            let body = format!("Node {i}. {}", "lorem ipsum dolor sit amet ".repeat(30));
            store
                .insert_node(&Node::new(
                    format!("n:{i}"),
                    format!("T{i}"),
                    NodeKind::Note,
                    &body,
                ))
                .unwrap();
        }
        held.push(store);
    }
    let after = rss_kb();
    eprintln!(
        "D4 rss_per_open_store: stores={k} nodes_each={nodes} base={base}KB after={after}KB \
         delta={}KB per_store={}KB",
        after - base,
        (after - base) / k as u64
    );
    // Hold the stores across the measurement -- dropping them early would measure
    // nothing.
    assert_eq!(held.len(), k);
}
