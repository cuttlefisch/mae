//! The scenarios of the ADR-074 C1 write-cost benchmark (see the parent module).

use std::time::{Duration, Instant};

use tokio::sync::Barrier;

use super::*;

/// One scenario's shape.
#[derive(Clone, Copy)]
struct Load {
    writers: usize,
    writes_each: usize,
    /// `Some(rate)`: each writer issues `rate` writes/s (open loop); `None`: as fast as it can.
    per_sec: Option<u64>,
    /// Through `DocStore::apply_update` (true) or `wal_append` alone.
    via_doc_store: bool,
}

/// Per-scenario stream readers (C1's fan-out side).
#[derive(Clone, Copy)]
struct Fanout {
    streams: usize,
    coalesce: Option<Duration>,
}

struct Measured {
    lat: Vec<Duration>,
    /// Writes refused with SQLITE_BUSY ("database is locked") after `busy_timeout`.
    busy: usize,
    wall: Duration,
    reads: usize,
    read_lat: Vec<Duration>,
}

/// One result row, kept for the across-runs summary.
struct Row {
    run: usize,
    key: String,
    mode: Mode,
    p50: f64,
    p99: f64,
    tput: f64,
}

fn pct(sorted: &[Duration], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i].as_secs_f64() * 1e6
}

fn loadavg() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| "?".into())
}

/// Production `DocStore` settings (`main.rs` + config defaults).
fn store(backend: Arc<IndexedBackend>) -> DocStore {
    DocStore::new(backend, 500)
        .with_max_documents(4096)
        .with_max_wal_entries(5000)
        .with_max_document_size(10_485_760)
        .with_max_update_size(4_194_304)
}

/// One writer: load its hot docs (untimed), wait for the start, then time each
/// write. A write refused as "database is locked" is counted, not fatal: that
/// refusal is a measured outcome. Any other error is a bug and panics.
async fn writer(
    w: usize,
    load: Load,
    store: Arc<DocStore>,
    backend: Arc<IndexedBackend>,
    start: Arc<Barrier>,
) -> (Vec<Duration>, usize) {
    let docs = hot_docs(w);
    let mut replicas = Vec::new();
    for (j, doc) in docs.iter().enumerate() {
        let state = store.encode_state(doc).await.unwrap();
        let cid = 1_000_000 + (w * HOT_PER_WRITER + j) as u64;
        replicas.push(TextSync::from_state_with_client_id(&state, cid).unwrap());
    }
    let principal = format!("user-{w}");
    let mut rng = Rng(0xA5A5_0000 + w as u64);
    let mut tick = load.per_sec.map(|r| {
        let mut t = tokio::time::interval(Duration::from_micros(1_000_000 / r));
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        t
    });
    start.wait().await;
    let (mut lat, mut busy) = (Vec::with_capacity(load.writes_each), 0);
    for _ in 0..load.writes_each {
        if let Some(t) = tick.as_mut() {
            t.tick().await;
        }
        let j = rng.below(docs.len());
        let at = rng.below(replicas[j].rope().len_chars()) as u32;
        let update = replicas[j].insert(at, &"x".repeat(1 + rng.below(8)));
        let by = Attribution::Principal(&principal);
        let t0 = Instant::now();
        let written = if load.via_doc_store {
            let done = store.apply_update(&docs[j], &update, Some(w as u64), by);
            done.await.map(drop)
        } else {
            let stamp = EditStamp::new(by.into(), 0);
            let done = backend.wal_append(&docs[j], &update, Some(w as u64), &stamp);
            done.await.map(drop)
        };
        lat.push(t0.elapsed());
        match written {
            Ok(()) => {}
            Err(StorageError::Sqlite(e)) if e.contains("database is locked") => busy += 1,
            Err(e) => panic!("write failed: {e:?}"),
        }
    }
    (lat, busy)
}

/// One stream: wake on its KB's watch, read the index from its cursor through
/// the store's own connections (as a `/events` handler would), until stopped;
/// then drain once more. Returns (reads, query latencies, final cursor).
async fn reader(
    backend: Arc<IndexedBackend>,
    kb: String,
    coalesce: Option<Duration>,
    mut stop: watch::Receiver<bool>,
) -> (usize, Vec<Duration>, u64) {
    let mut rx = backend.feeds[&kb].subscribe();
    let mut cursor = *rx.borrow_and_update();
    let (mut reads, mut lat) = (0, Vec::new());
    loop {
        let stopping = tokio::select! {
            r = rx.changed() => r.is_err(),
            _ = stop.changed() => true,
        };
        if let (false, Some(c)) = (stopping, coalesce) {
            tokio::time::sleep(c).await;
        }
        let t0 = Instant::now();
        cursor = read_from(&backend.inner, &kb, cursor);
        lat.push(t0.elapsed());
        reads += 1;
        if stopping {
            return (reads, lat, cursor);
        }
    }
}

/// Page through `kb_changes` after `cursor`; returns the new cursor.
fn read_from(backend: &SqliteBackend, kb: &str, mut cursor: u64) -> u64 {
    let conn = backend.pool.lock_shard(&format!("kbc:{kb}"));
    loop {
        let mut stmt = conn.prepare(READ_CHANGES).unwrap();
        stmt.bind((1, kb)).unwrap();
        stmt.bind((2, cursor as i64)).unwrap();
        let mut rows = 0;
        while let sqlite::State::Row = stmt.next().unwrap() {
            cursor = stmt.read::<i64, _>("seq").unwrap() as u64;
            rows += 1;
        }
        if rows < 1000 {
            return cursor;
        }
    }
}

async fn scenario(sqlite: Arc<SqliteBackend>, mode: Mode, load: Load, fanout: Fanout) -> Measured {
    let backend = Arc::new(IndexedBackend::new(sqlite, mode));
    let store = Arc::new(store(backend.clone()));
    let start = Arc::new(Barrier::new(load.writers + 1));
    let (stop_tx, stop_rx) = watch::channel(false);
    let readers: Vec<_> = (0..fanout.streams)
        .map(|s| {
            let (b, kb) = (backend.clone(), kb_name(s % KBS));
            tokio::spawn(reader(b, kb, fanout.coalesce, stop_rx.clone()))
        })
        .collect();
    let writers: Vec<_> = (0..load.writers)
        .map(|w| {
            let (s, b, st) = (store.clone(), backend.clone(), start.clone());
            tokio::spawn(writer(w, load, s, b, st))
        })
        .collect();
    let checked = if mode.indexes() { mode } else { Mode::C1 };
    let before = index_state(&backend.inner, checked);
    start.wait().await;
    let t0 = Instant::now();
    let (mut lat, mut busy) = (Vec::new(), 0);
    for w in writers {
        let (l, b) = w.await.unwrap();
        lat.extend(l);
        busy += b;
    }
    let wall = t0.elapsed();
    stop_tx.send(true).unwrap();
    let (reads, read_lat) = check_readers(&backend, readers).await;
    check_index(
        mode,
        before,
        index_state(&backend.inner, checked),
        lat.len() - busy,
    );
    Measured {
        lat,
        busy,
        wall,
        reads,
        read_lat,
    }
}

/// The oracle: a variant that claims to index must have indexed every write,
/// and the baseline must have indexed nothing. A no-op prototype costs nothing,
/// so without this the comparison could pass for the wrong reason.
fn check_index(mode: Mode, (h0, t0): (i64, i64), (h1, t1): (i64, i64), writes: usize) {
    match mode {
        Mode::Base | Mode::BaseSerial => {
            assert_eq!((h1, t1), (h0, t0), "baseline wrote the index")
        }
        Mode::C1 | Mode::C1NoRowid | Mode::C1Serial => {
            assert_eq!(
                h1 - h0,
                writes as i64,
                "{mode:?}: head did not advance once per write"
            );
            assert_eq!(t1, h1, "{mode:?}: change rows disagree with the heads");
        }
        Mode::WalSeq => assert!(t1 == h1 && t1 > t0, "walseq: last WAL id not indexed"),
    }
}

/// Join the readers; each must have caught up to its KB's head (nothing missed).
async fn check_readers(
    backend: &IndexedBackend,
    readers: Vec<tokio::task::JoinHandle<(usize, Vec<Duration>, u64)>>,
) -> (usize, Vec<Duration>) {
    let (mut reads, mut lat) = (0, Vec::new());
    for (s, r) in readers.into_iter().enumerate() {
        let (n, l, cursor) = r.await.unwrap();
        let head = head(&backend.inner, "kb_feed", &kb_name(s % KBS));
        assert_eq!(cursor, head, "stream {s} missed a notice");
        reads += n;
        lat.extend(l);
    }
    (reads, lat)
}

fn report(run: usize, name: &str, mode: Mode, load: Load, fanout: Fanout, mut m: Measured) -> Row {
    m.lat.sort();
    let tput = m.lat.len() as f64 / m.wall.as_secs_f64();
    let key = format!("{name} w={} s={}", load.writers, fanout.streams);
    let (p50, p99) = (pct(&m.lat, 0.50), pct(&m.lat, 0.99));
    let mut line = format!(
        "RESULT run={run} {key} mode={} n={} p50_us={p50:.1} p95_us={:.1} p99_us={p99:.1} \
         max_us={:.1} tput_per_s={tput:.0} busy={} load1={}",
        mode.name(),
        m.lat.len(),
        pct(&m.lat, 0.95),
        pct(&m.lat, 1.0),
        m.busy,
        loadavg(),
    );
    if !m.read_lat.is_empty() {
        m.read_lat.sort();
        line += &format!(
            " reads_per_s={:.0} read_p50_us={:.1} read_p99_us={:.1}",
            m.reads as f64 / m.wall.as_secs_f64(),
            pct(&m.read_lat, 0.50),
            pct(&m.read_lat, 0.99)
        );
    }
    println!("{line}");
    Row {
        run,
        key,
        mode,
        p50,
        p99,
        tput,
    }
}

const NO_FANOUT: Fanout = Fanout {
    streams: 0,
    coalesce: None,
};

fn closed(writers: usize, writes_each: usize, via_doc_store: bool) -> Load {
    Load {
        writers,
        writes_each,
        per_sec: None,
        via_doc_store,
    }
}

/// 32 writers at `rate` writes/s each, open loop.
fn paced(rate: u64, writes_each: usize) -> Load {
    Load {
        writers: 32,
        writes_each,
        per_sec: Some(rate),
        via_doc_store: true,
    }
}

const PACED: Load = Load {
    writers: 32,
    writes_each: 60,
    per_sec: Some(20),
    via_doc_store: true,
};

/// `MAE_BENCH_ONLY=a,b`: run only scenarios whose key contains one of them.
fn wanted(name: &str, load: Load, fanout: Fanout) -> bool {
    let key = format!("{name} w={} s={}", load.writers, fanout.streams);
    std::env::var("MAE_BENCH_ONLY").map_or(true, |only| only.split(',').any(|t| key.contains(t)))
}

/// Every mode of a scenario back to back, rotating the order each run so drift
/// (page cache, WAL growth, other load) lands on all of them alike.
async fn one_run(sqlite: &Arc<SqliteBackend>, run: usize, rows: &mut Vec<Row>) {
    const ALL: [Mode; 4] = [Mode::Base, Mode::C1, Mode::C1NoRowid, Mode::WalSeq];
    const SER: [Mode; 4] = [Mode::Base, Mode::C1, Mode::BaseSerial, Mode::C1Serial];
    let paired = [
        ("wal_append", closed(1, 4000, false), &ALL[..]),
        ("apply", closed(1, 4000, true), &ALL[..2]),
        ("apply", closed(8, 1000, true), &SER[..]),
        ("apply", closed(32, 300, true), &SER[..]),
        ("apply_paced20", PACED, &SER[..]),
        ("apply_paced5", paced(5, 60), &SER[..]),
    ];
    for (name, load, modes) in paired {
        if !wanted(name, load, NO_FANOUT) {
            continue;
        }
        for i in 0..modes.len() {
            let mode = modes[(i + run) % modes.len()];
            let m = scenario(sqlite.clone(), mode, load, NO_FANOUT).await;
            rows.push(report(run, name, mode, load, NO_FANOUT, m));
        }
    }
    let fanouts = [
        (64, None),
        (256, None),
        (256, Some(Duration::from_millis(250))),
    ];
    for (streams, coalesce) in fanouts {
        let f = Fanout { streams, coalesce };
        let name = if coalesce.is_some() {
            "fanout250ms"
        } else {
            "fanout"
        };
        for load in [PACED, closed(8, 1000, true)]
            .into_iter()
            .filter(|l| wanted(name, *l, f))
        {
            let m = scenario(sqlite.clone(), Mode::C1, load, f).await;
            rows.push(report(run, name, Mode::C1, load, f, m));
        }
    }
}

fn median_range(mut v: Vec<f64>) -> String {
    v.sort_by(|a, b| a.total_cmp(b));
    format!("{:.0} [{:.0}-{:.0}]", v[v.len() / 2], v[0], v[v.len() - 1])
}

/// Medians over runs, and each mode's per-run ratio to the base run beside it.
fn summarize(rows: &[Row]) {
    let mut keys: Vec<(&str, Mode)> = rows.iter().map(|r| (r.key.as_str(), r.mode)).collect();
    keys.dedup();
    keys.sort_by_key(|(k, m)| (k.to_string(), m.name()));
    keys.dedup();
    for (key, mode) in keys {
        let mine: Vec<&Row> = rows
            .iter()
            .filter(|r| r.key == key && r.mode == mode)
            .collect();
        let pick = |f: fn(&Row) -> f64| median_range(mine.iter().map(|r| f(r)).collect());
        let mut line = format!(
            "SUMMARY {key} mode={} runs={} p50_us={} p99_us={} tput={}",
            mode.name(),
            mine.len(),
            pick(|r| r.p50),
            pick(|r| r.p99),
            pick(|r| r.tput),
        );
        let ratios = |f: fn(&Row) -> f64| -> Vec<f64> {
            mine.iter()
                .filter_map(|r| {
                    let b = rows
                        .iter()
                        .find(|b| b.key == key && b.mode == Mode::Base && b.run == r.run)?;
                    Some(100.0 * f(r) / f(b))
                })
                .collect()
        };
        if mode != Mode::Base && !ratios(|r| r.p99).is_empty() {
            line += &format!(
                " vs_base_pct p50={} p99={}",
                median_range(ratios(|r| r.p50)),
                median_range(ratios(|r| r.p99))
            );
        }
        println!("{line}");
    }
}

#[test]
#[ignore = "benchmark: a ~1 GB store, minutes of wall time; run by hand on an idle machine"]
fn change_index_bench() {
    let dir = bench_dir("bench");
    std::fs::create_dir_all(&dir).unwrap();
    let nodes = env_usize("MAE_BENCH_NODES", 100_000);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    println!(
        "MACHINE threads={threads} nodes={nodes} dir={} load1={}",
        dir.display(),
        loadavg()
    );
    let sqlite = Arc::new(SqliteBackend::open(&dir.join("collab.db")).unwrap());
    let t0 = Instant::now();
    populate(&sqlite, nodes);
    println!("FIXTURE built_in_s={:.1}", t0.elapsed().as_secs_f64());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .build()
        .unwrap();
    let mut rows = Vec::new();
    for run in 0..env_usize("MAE_BENCH_RUNS", 5) {
        rt.block_on(one_run(&sqlite, run, &mut rows));
    }
    summarize(&rows);
    drop(sqlite);
    if std::env::var_os("MAE_BENCH_KEEP").is_none() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[path = "storage_change_index_bench_ckpt_tests.rs"]
mod ckpt;
