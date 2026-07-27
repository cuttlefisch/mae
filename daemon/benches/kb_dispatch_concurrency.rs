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

/// ADR-060 Phase F: the same methodology as `bench_kb_dispatch_concurrency`
/// above, with an explicit N-TENANT dimension added — not just "N concurrent
/// sessions against one store" but "N tenants, each with M concurrent
/// sessions against their OWN store, running simultaneously." Made possible
/// by issue #460's fix (`main.rs` now actually loads federated instances
/// from `kb-registry.toml` at startup; before that fix there was no way to
/// get a real spawned `mae-daemon` process to serve more than one store at
/// all, so this benchmark could not have been run honestly against the real
/// binary before now).
///
/// Each tenant gets its own dedicated `NODE_COUNT`-node store (not a shared
/// store scoped by address) and an UNLIMITED quota (`budget_per_minute = 0`,
/// `max_connections = 0`) — this benchmark measures Phase B's per-instance
/// LOCK isolation capacity, not Phase C's quota-enforcement overhead; mixing
/// the two would make the resulting number answer neither question cleanly.
struct MultiTenantDaemonHandle {
    child: std::process::Child,
    socket_path: PathBuf,
    _tmp: tempfile::TempDir,
}

impl Drop for MultiTenantDaemonHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn a real `mae-daemon` with `tenant_count` independently-registered,
/// independently-seeded KB instances, each owned by its own `[[tenant]]`
/// config entry. Returns the daemon handle plus each tenant's instance UUID
/// (the `instance` address to use in `kb/search` params), in tenant order.
fn spawn_multi_tenant_daemon(
    rt: &tokio::runtime::Runtime,
    tenant_count: usize,
) -> (MultiTenantDaemonHandle, Vec<String>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    // The primary store also gets seeded (main.rs always opens it) but no
    // tenant addresses it -- only the N dedicated instance stores below are
    // exercised, keeping the primary's own unbounded federated-search
    // exposure out of this specific measurement.
    seed_store(&data_dir);

    let instances_dir = data_dir.join("instances");
    std::fs::create_dir_all(&instances_dir).expect("create instances dir");

    let mut uuids = Vec::with_capacity(tenant_count);
    let mut registry_instances = String::new();
    let mut tenant_config = String::new();
    for t in 0..tenant_count {
        let uuid = format!("bench-tenant-{t}");
        let inst_path = instances_dir.join(format!("{uuid}.cozo"));
        seed_store_at(&inst_path, t);
        uuids.push(uuid.clone());
        registry_instances.push_str(&format!(
            r#"
[[instances]]
uuid = "{uuid}"
name = "tenant-{t}-kb"
org_dir = ""
db_path = "{path}"
primary = false
enabled = true
"#,
            path = inst_path.display()
        ));
        tenant_config.push_str(&format!(
            r#"
[[tenant]]
name = "tenant-{t}"
instances = ["{uuid}"]

[tenant.quota]
max_connections = 0
budget_per_minute = 0
"#
        ));
    }
    std::fs::write(data_dir.join("kb-registry.toml"), registry_instances)
        .expect("write kb-registry.toml");

    let config_path = tmp.path().join("daemon.toml");
    std::fs::write(&config_path, tenant_config).expect("write daemon.toml");

    let socket_path = tmp.path().join("mae-daemon.sock");
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args([
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--config",
            config_path.to_str().unwrap(),
        ])
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

    (
        MultiTenantDaemonHandle {
            child,
            socket_path,
            _tmp: tmp,
        },
        uuids,
    )
}

/// Same shape as `seed_store`, at an arbitrary path, with a `seed` folded
/// into node ids/content so different tenants' stores are distinguishable
/// (useful when debugging a failed run) without changing `NODE_COUNT`.
fn seed_store_at(db_path: &std::path::Path, seed: usize) {
    let store = CozoKbStore::open_with_engine(db_path, "sqlite").expect("seed store opens");
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
            format!("bench:tenant-{seed}:node-{i}"),
            format!("Bench tenant {seed} node {i}"),
            NodeKind::Note,
            format!(
                "body content for benchmark tenant {seed} node {i} covering {} and related topics",
                topics[i % topics.len()]
            ),
        );
        store.insert_node(&node).expect("seed insert");
    }
}

/// One real, instance-addressed `kb/search` round trip against a specific
/// tenant's store.
async fn kb_search_tenant(socket_path: &std::path::Path, instance: &str, query: &str) -> Duration {
    let mut stream = UnixStream::connect(socket_path)
        .await
        .expect("connect to kb socket");
    let (r, mut w) = stream.split();
    let mut reader = tokio::io::BufReader::new(r);
    let req = json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/search",
        "params": {"query": query, "limit": 20, "instance": instance},
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

fn bench_kb_dispatch_multi_tenant_concurrency(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
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

    let mut group = c.benchmark_group("kb_dispatch_multi_tenant_concurrency");
    group.sample_size(10);

    // (tenant_count, sessions_per_tenant) levels. Kept smaller than the
    // single-tenant sweep's 1..64 range -- each level here seeds
    // tenant_count SEPARATE NODE_COUNT-node stores (real disk + CozoDB
    // schema-creation cost per store), so the product needs to stay
    // reasonable for a benchmark that's expected to actually be re-run by a
    // human, not just once in CI (this bench is explicitly not part of
    // default CI -- see the module doc).
    let levels_to_run: &[(usize, usize)] = &[(1, 1), (2, 4), (4, 4), (8, 4)];

    let mut results: Vec<(usize, usize, Duration, Duration)> = Vec::new(); // (tenants, per_tenant, p50, p99)

    for &(tenant_count, per_tenant) in levels_to_run {
        let (daemon, uuids) = spawn_multi_tenant_daemon(&rt, tenant_count);
        let socket_path = daemon.socket_path.clone();
        let uuids = Arc::new(uuids);
        let samples: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
        let total = tenant_count * per_tenant;

        group.bench_with_input(
            BenchmarkId::new("tenants", format!("{tenant_count}x{per_tenant}={total}")),
            &(tenant_count, per_tenant),
            |b, &(tenant_count, per_tenant)| {
                b.to_async(&rt).iter(|| {
                    let socket_path = socket_path.clone();
                    let uuids = Arc::clone(&uuids);
                    let samples = Arc::clone(&samples);
                    async move {
                        let mut handles = Vec::with_capacity(tenant_count * per_tenant);
                        for t in 0..tenant_count {
                            let instance = uuids[t].clone();
                            for s in 0..per_tenant {
                                let socket_path = socket_path.clone();
                                let query = queries[s % queries.len()].to_string();
                                let instance = instance.clone();
                                handles.push(tokio::spawn(async move {
                                    kb_search_tenant(&socket_path, &instance, &query).await
                                }));
                            }
                        }
                        let mut batch = Vec::with_capacity(handles.len());
                        for h in handles {
                            batch.push(h.await.expect("client task panicked"));
                        }
                        samples.lock().unwrap().extend(batch);
                    }
                });
            },
        );

        let mut collected = samples.lock().unwrap().clone();
        collected.sort();
        let p50 = percentile(&collected, 0.50);
        let p99 = percentile(&collected, 0.99);
        eprintln!(
            "kb_dispatch_multi_tenant_concurrency: tenants={tenant_count:2} per_tenant={per_tenant:2} \
             total={total:3} p50={p50:?} p99={p99:?} (samples={})",
            collected.len()
        );
        results.push((tenant_count, per_tenant, p50, p99));
    }
    group.finish();

    if let Some(&(_, _, _, single_tenant_baseline_p99)) = results.first() {
        eprintln!(
            "kb_dispatch_multi_tenant_concurrency: single-tenant-equivalent baseline (1 tenant x \
             1 session) p99={single_tenant_baseline_p99:?} -- cross-reference this run's own \
             bench_kb_dispatch_concurrency single-tenant N=1 figure as the regression check \
             ADR-060 Phase F requires BEFORE trusting the multi-tenant numbers below it; the two \
             should be close (same dispatch path, same store size), not compared as if measuring \
             the same thing."
        );
        for &(tenants, per_tenant, p50, p99) in &results {
            eprintln!(
                "kb_dispatch_multi_tenant_concurrency: RECORD tenants={tenants} \
                 per_tenant={per_tenant} p50={p50:?} p99={p99:?} — record in \
                 docs/adr/004-kb-scaling.md as the multi-tenant figure, distinct from (not \
                 replacing) the existing single-tenant one"
            );
        }
    }
}

criterion_group!(
    benches,
    bench_kb_dispatch_concurrency,
    bench_kb_dispatch_multi_tenant_concurrency
);
criterion_main!(benches);
