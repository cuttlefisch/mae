//! Benchmarked concurrent-session capacity for the KB Unix socket (ADR-054),
//! replacing `docs/adr/004-kb-scaling.md`'s unverified "5-10 concurrent
//! editors" claim with a measured number.
//!
//! Spawns the real, compiled `mae-daemon` binary (a bench target, like
//! `daemon/tests/*.rs`, only sees the library crate's public re-exports —
//! `handler`/`accept_loop`/`DaemonState` are deliberately bin-crate-private,
//! see `daemon/src/tests/mod.rs`'s doc comment) against a pre-seeded store,
//! then drives increasing numbers of concurrent real Unix-socket clients
//! issuing `kb/search` calls, recording p50/p99 latency per level. "Capacity"
//! is reported as the largest concurrent-session count whose p99 stays
//! within 2x the single-client baseline — a *concurrent-session* count
//! (VS Code/other-editor clients count same as `mae` sessions), matching
//! ADR-054's own framing.
//!
//! Run: `cargo bench -p mae-daemon --bench kb_dispatch_concurrency`
//! (not part of default `cargo test`/CI — the in-process
//! `kb_socket_concurrency_tests.rs` satisfies the "runs in default CI"
//! Verification bullet; this satisfies the separate "published capacity
//! number" bullet.)

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use mae_kb::{CozoKbStore, KbStore, Node, NodeKind};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UnixStream;

/// Matches ADR-004's own "~20K nodes" framing for a single-machine KB.
const NODE_COUNT: usize = 20_000;

struct DaemonHandle {
    child: std::process::Child,
    socket_path: PathBuf,
    _tmp: tempfile::TempDir,
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Seed a realistic-sized store BEFORE the daemon starts (the daemon holds
/// exclusive access to its own store once running) at the exact path
/// `main.rs` computes (`effective_data_dir().join("daemon-kb.cozo")`).
fn seed_store(data_dir: &std::path::Path) {
    let db_path = data_dir.join("daemon-kb.cozo");
    let store = CozoKbStore::open_with_engine(&db_path, "sqlite").expect("seed store opens");
    let topics = [
        "rust",
        "scheme",
        "cozo",
        "mesh",
        "concurrency",
        "daemon",
        "collab",
        "kb",
    ];
    for i in 0..NODE_COUNT {
        let node = Node::new(
            format!("bench:node-{i}"),
            format!("Bench node {i}"),
            NodeKind::Note,
            format!(
                "body content for benchmark node {i} covering {} and related topics",
                topics[i % topics.len()]
            ),
        );
        store.insert_node(&node).expect("seed insert");
    }
}

/// Spawn a real `mae-daemon` subprocess, isolated XDG dirs, pre-seeded KB.
fn spawn_daemon(rt: &tokio::runtime::Runtime) -> DaemonHandle {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    seed_store(&data_dir);

    let socket_path = tmp.path().join("mae-daemon.sock");
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args(["--data-dir", data_dir.to_str().unwrap()])
        .env("XDG_RUNTIME_DIR", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn mae-daemon");

    rt.block_on(async {
        for _ in 0..100 {
            if UnixStream::connect(&socket_path).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("mae-daemon did not bind its KB socket within 10s");
    });

    DaemonHandle {
        child,
        socket_path,
        _tmp: tmp,
    }
}

/// One real `kb/search` round trip; returns its wall-clock latency.
async fn kb_search(socket_path: &std::path::Path, query: &str) -> Duration {
    let mut stream = UnixStream::connect(socket_path)
        .await
        .expect("connect to kb socket");
    let (r, mut w) = stream.split();
    let mut reader = tokio::io::BufReader::new(r);
    let req = json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/search",
        "params": {"query": query, "limit": 20},
    });
    let body = serde_json::to_vec(&req).unwrap();
    let start = Instant::now();
    mae_mcp::write_framed(&mut w, &body, Duration::from_secs(10))
        .await
        .expect("write request");
    let msg = mae_mcp::read_message(&mut reader)
        .await
        .expect("read response")
        .expect("response before EOF");
    let elapsed = start.elapsed();
    let resp: serde_json::Value = serde_json::from_str(&msg).expect("parse response");
    assert!(resp.get("error").is_none(), "kb/search failed: {resp:?}");
    elapsed
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = (((sorted.len() - 1) as f64) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn bench_kb_dispatch_concurrency(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let daemon = spawn_daemon(&rt);
    let queries = [
        "rust",
        "scheme",
        "cozo",
        "mesh",
        "concurrency",
        "daemon",
        "collab",
        "kb",
    ];

    let mut group = c.benchmark_group("kb_dispatch_concurrency");
    group.sample_size(10);

    let mut levels: Vec<(usize, Duration, Duration)> = Vec::new(); // (n, p50, p99)

    for &n in &[1usize, 4, 8, 16, 32, 64] {
        let socket_path = daemon.socket_path.clone();
        let samples: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));

        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.to_async(&rt).iter(|| {
                let socket_path = socket_path.clone();
                let samples = Arc::clone(&samples);
                async move {
                    let mut handles = Vec::with_capacity(n);
                    for i in 0..n {
                        let socket_path = socket_path.clone();
                        let query = queries[i % queries.len()].to_string();
                        handles.push(tokio::spawn(async move {
                            kb_search(&socket_path, &query).await
                        }));
                    }
                    let mut batch = Vec::with_capacity(n);
                    for h in handles {
                        batch.push(h.await.expect("client task panicked"));
                    }
                    samples.lock().unwrap().extend(batch);
                }
            });
        });

        let mut collected = samples.lock().unwrap().clone();
        collected.sort();
        let p50 = percentile(&collected, 0.50);
        let p99 = percentile(&collected, 0.99);
        eprintln!(
            "kb_dispatch_concurrency: N={n:3} p50={p50:?} p99={p99:?} (samples={})",
            collected.len()
        );
        levels.push((n, p50, p99));
    }
    group.finish();

    if let Some(&(_, _, baseline_p99)) = levels.first() {
        let slo = baseline_p99 * 2;
        let capacity = levels
            .iter()
            .filter(|&&(_, _, p99)| p99 <= slo)
            .map(|&(n, _, _)| n)
            .max()
            .unwrap_or(1);
        eprintln!(
            "kb_dispatch_concurrency: SLO (p99 <= 2x single-client baseline {baseline_p99:?}) \
             holds up to N={capacity} concurrent sessions against a {NODE_COUNT}-node KB — \
             record this figure in docs/adr/004-kb-scaling.md"
        );
    }
}

criterion_group!(benches, bench_kb_dispatch_concurrency);
criterion_main!(benches);
