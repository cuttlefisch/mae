//! **ADR-115's Performance section, the routes' half: kept runnable.**
//!
//! `#[ignore]`d and non-gating: each prints its numbers, nothing asserts on
//! time. Upload throughput and peak memory per size (with ten times the cap
//! refused), download p50/p95 per size and the `304` path, and a backup with
//! blobs. Run in release on a disk-backed `TMPDIR`:
//!
//! ```text
//! cd daemon && TMPDIR=~/.cache/mae-bench cargo test --release --bin mae-daemon blob_bench -- --ignored --nocapture --test-threads 1
//! ```
//!
//! Peak memory is `VmHWM`, reset per case through `/proc/self/clear_refs`
//! (Linux only). The processing and SQLite-crossover numbers are the store's
//! half, `mae_daemon::blob_store::bench`.

use std::time::Instant;

use super::blob_route_tests::{harness_cap, TestQuota};

const MIB: usize = 1024 * 1024;

fn peak_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))?
                .split_whitespace()
                .nth(1)?
                .parse::<f64>()
                .ok()
        })
        .unwrap_or(0.0)
        / 1024.0
}

fn reset_peak() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

fn body(size: usize, seed: u8) -> Vec<u8> {
    // Not an image, so the numbers are transport, hashing and the commit.
    let mut v: Vec<u8> = (0..size)
        .map(|i| (i as u8).wrapping_mul(31) ^ seed)
        .collect();
    v[..4].copy_from_slice(b"BIN\0");
    v
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "blob_bench: uploads and downloads up to 100 MB"]
async fn blob_bench_upload_and_download() {
    let cap = crate::oauth_config::DEFAULT_MAX_ATTACHMENT_BYTES as usize;
    let h = harness_cap(
        TestQuota {
            storage: None,
            refuse_requests: false,
        },
        40_000_000,
        cap as u64,
    )
    .await;
    let sizes = [10 * 1024, 100 * 1024, MIB, 5 * MIB, cap];
    let mut stored = Vec::new();
    for (i, &size) in sizes.iter().enumerate() {
        let b = body(size, i as u8);
        reset_peak();
        let base = peak_mib();
        let t = Instant::now();
        let r = h.upload(&h.people.editor, "kb-a", &b, "").await;
        let secs = t.elapsed().as_secs_f64();
        assert_eq!(r.status, 200);
        println!(
            "upload   {:>9} B  {:>8.1} ms  {:>7.1} MB/s  peak +{:>6.1} MiB (client buffer included)",
            size,
            secs * 1000.0,
            size as f64 / secs / 1e6,
            peak_mib() - base
        );
        stored.push((size, r.json()["sha256"].as_str().unwrap().to_string()));
    }
    let big = body(cap * 10, 9);
    reset_peak();
    let base = peak_mib();
    let t = Instant::now();
    let r = h.upload(&h.people.editor, "kb-a", &big, "").await;
    println!(
        "upload   {:>9} B  {:>8.1} ms  refused {}  server read {} B  peak +{:>6.1} MiB",
        big.len(),
        t.elapsed().as_secs_f64() * 1000.0,
        r.status,
        h.body_bytes() - sizes.iter().sum::<usize>() as u64,
        peak_mib() - base
    );
    for (size, sha) in &stored {
        if *size == cap {
            continue;
        }
        let mut ms = Vec::new();
        for _ in 0..40 {
            let t = Instant::now();
            let r = h.fetch("GET", &h.people.viewer, "kb-a", sha, "").await;
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(r.body.len(), *size);
        }
        ms.sort_by(f64::total_cmp);
        println!(
            "download {:>9} B  p50 {:>7.2} ms  p95 {:>7.2} ms",
            size,
            pct(&ms, 0.5),
            pct(&ms, 0.95)
        );
    }
    let (_, sha) = &stored[2];
    let inm = format!("\r\nIf-None-Match: \"{sha}\"");
    let mut ms = Vec::new();
    for _ in 0..40 {
        let t = Instant::now();
        let r = h.fetch("GET", &h.people.viewer, "kb-a", sha, &inm).await;
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(r.status, 304);
    }
    ms.sort_by(f64::total_cmp);
    println!(
        "download 304          p50 {:>7.2} ms  p95 {:>7.2} ms",
        pct(&ms, 0.5),
        pct(&ms, 0.95)
    );
}

/// A backup of a blob store holding `BENCH_BLOB_MB` (default 1024) MB, then a
/// second run. `backup create` writes a whole archive each time: there is no
/// incremental mode, so the second figure is a full run over a warm cache.
#[test]
#[ignore = "blob_bench: writes a gigabyte"]
fn blob_bench_backup() {
    use mae_daemon::blob_store::{BlobStore, MediaType, DIR};
    let mb: usize = std::env::var("BENCH_BLOB_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024);
    let tmp = tempfile::TempDir::new().unwrap();
    let config = super::isolated(tmp.path());
    let data_dir = config.effective_data_dir();
    std::fs::create_dir_all(&data_dir).unwrap();
    mae_kb::CozoKbStore::open_with_engine(data_dir.join("daemon-kb.cozo"), "sqlite").unwrap();
    let store = BlobStore::open(&config.instance_paths().collab_data_dir.join(DIR)).unwrap();
    let per = 4 * MIB;
    for i in 0..(mb * MIB / per) {
        let staged = store.stage(&body(per, i as u8 ^ (i >> 8) as u8)).unwrap();
        store
            .commit("kb-a", staged, (MediaType::Other, None, None), None)
            .unwrap();
    }
    let blobs = store.list_all().unwrap().len();
    for run in 1..=2 {
        let out = tmp.path().join(format!("b{run}.tar"));
        let t = Instant::now();
        crate::backup::run_create(&config, &out).unwrap();
        println!(
            "backup run {run}: {blobs} blobs, {mb} MB  {:>7.1} s  archive {:>7.1} MB",
            t.elapsed().as_secs_f64(),
            std::fs::metadata(&out).unwrap().len() as f64 / 1e6
        );
    }
}
