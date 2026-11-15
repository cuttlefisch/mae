//! Why the index moves single-writer p99 by a step rather than a slope: a
//! causal test, not an inference. Each index variant writes more WAL frames
//! per commit, so the 1000-page auto-checkpoint (which fsyncs) lands on more
//! commits. If that is the mechanism, turning auto-checkpoint off must remove
//! the step while leaving the p50 cost in place.
//!
//! ```text
//! cd daemon && cargo test --release --lib change_index_checkpoint_cause -- --ignored --nocapture
//! ```

use super::*;

fn wal_bytes(dir: &std::path::Path) -> u64 {
    std::fs::metadata(dir.join("collab.db-wal")).map_or(0, |m| m.len())
}

/// Checkpoint and truncate the WAL file; asserts it emptied, since every
/// frame count below is read from its size.
fn truncate_wal(sqlite: &SqliteBackend, dir: &std::path::Path) {
    sqlite
        .pool
        .lock_primary()
        .execute("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    assert_eq!(
        wal_bytes(dir),
        0,
        "WAL did not truncate; frame counts would be wrong"
    );
}

fn set_autocheckpoint(sqlite: &SqliteBackend, pages: u32) {
    for shard in &sqlite.pool.shards {
        let conn = shard.lock().unwrap();
        conn.execute(format!("PRAGMA wal_autocheckpoint={pages};"))
            .unwrap();
    }
}

/// WAL frames one `wal_append` commit writes, per mode (50 commits, well under
/// the 1000-page auto-checkpoint, so no checkpoint resets the file meanwhile).
async fn frames_per_commit(sqlite: &Arc<SqliteBackend>, dir: &std::path::Path) {
    const COMMITS: usize = 50;
    let page = scalar(&sqlite.pool.lock_primary(), "PRAGMA page_size") as u64;
    for mode in [Mode::Base, Mode::C1, Mode::C1NoRowid, Mode::WalSeq] {
        truncate_wal(sqlite, dir);
        scenario(sqlite.clone(), mode, closed(1, COMMITS, false), NO_FANOUT).await;
        let frames = (wal_bytes(dir) - 32) / (page + 24);
        let per = frames as f64 / COMMITS as f64;
        println!(
            "FRAMES mode={} per_commit={per:.2} commits_per_autocheckpoint={:.0}",
            mode.name(),
            1000.0 / per
        );
    }
    truncate_wal(sqlite, dir);
}

#[test]
#[ignore = "benchmark: a ~1 GB store, minutes of wall time; run by hand on an idle machine"]
fn change_index_checkpoint_cause() {
    let dir = bench_dir("ckpt");
    std::fs::create_dir_all(&dir).unwrap();
    let nodes = env_usize("MAE_BENCH_NODES", 100_000);
    println!("MACHINE nodes={nodes} load1={}", loadavg());
    let sqlite = Arc::new(SqliteBackend::open(&dir.join("collab.db")).unwrap());
    populate(&sqlite, nodes);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(frames_per_commit(&sqlite, &dir));
    let mut rows = Vec::new();
    for run in 0..env_usize("MAE_BENCH_RUNS", 5) {
        for autockpt in [1000, 0] {
            set_autocheckpoint(&sqlite, autockpt);
            let name = format!("wal_append_autockpt{autockpt}");
            for mode in [[Mode::Base, Mode::C1], [Mode::C1, Mode::Base]][run % 2] {
                let load = closed(1, 4000, false);
                let m = rt.block_on(scenario(sqlite.clone(), mode, load, NO_FANOUT));
                rows.push(report(run, &name, mode, load, NO_FANOUT, m));
                truncate_wal(&sqlite, &dir); // untimed: start each from an empty WAL
            }
        }
    }
    set_autocheckpoint(&sqlite, 1000);
    summarize(&rows);
    drop(sqlite);
    if std::env::var_os("MAE_BENCH_KEEP").is_none() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
