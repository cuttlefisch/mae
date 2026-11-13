//! ADR-074 Amendment 1 (C1) write-cost measurement: what does writing a per-KB
//! change index inside `wal_append`'s own transaction cost on the real write path?
//!
//! Not a feature. `IndexedBackend` is the real `SqliteBackend` with an index
//! write (see [`Mode`]) optionally added to its `wal_append` transaction; in
//! `Mode::Base` it calls the real `SqliteBackend::wal_append` and nothing else.
//! Writes go through the real `DocStore::apply_update`, configured as `main.rs`
//! configures it, on a file-backed store opened with the production shard count
//! and PRAGMAs (WAL, `synchronous=NORMAL`, `secure_delete=ON`).
//!
//! ```text
//! cd daemon && cargo test --release --lib storage::change_index_bench::run::change_index_bench -- --exact --ignored --nocapture
//! ```
//! Env: `MAE_BENCH_DIR` (default `~/.cache/mae-bench-074a`; never tmpfs),
//! `MAE_BENCH_RUNS` (5), `MAE_BENCH_NODES` (100000), `MAE_BENCH_KEEP`, and
//! `MAE_BENCH_ONLY` (comma-separated scenario-key substrings, e.g.
//! `apply w=8,apply_paced`). The causal checkpoint test is in
//! `storage_change_index_bench_ckpt_tests.rs`. Method and results:
//! `docs/research/074a-write-cost-measurement.md`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use mae_sync::text::TextSync;
use mae_sync::DocAddress;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use super::{
    editor_record, Checkpoint, DocumentState, Listed, SqliteBackend, StorageBackend, StorageError,
};
use crate::doc_editors::{Attribution, EditStamp, EditorKind, EditorsSince};
use crate::doc_store::DocStore;
use mae_sync::kb::history::{HistoryEntry, HistoryKey, SignedHead, Tip};

const KBS: usize = 4;
const HOT_PER_WRITER: usize = 64;

/// What the write transaction records besides the WAL row and editor record.
/// Each variant has its own tables, so one cannot disturb another's oracle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Nothing: the real `SqliteBackend::wal_append`, untouched.
    Base,
    /// C1 as drafted: bump `kb_feed.head`, upsert `kb_changes` (rowid table:
    /// table + PK index + seq index).
    C1,
    /// C1 with the change table `WITHOUT ROWID` (one B-tree fewer).
    C1NoRowid,
    /// No counter row: `seq` is the WAL row's own id, already monotonic in
    /// commit order because SQLite has one writer per file.
    WalSeq,
    /// `Base` with every `wal_append` taken through one in-process FIFO lock,
    /// so writers queue instead of colliding on SQLite's file lock (whose busy
    /// handler resolves a collision by sleeping 1, 2, 5, ... ms).
    BaseSerial,
    /// `C1` behind the same in-process writer lock.
    C1Serial,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Base => "base",
            Mode::C1 => "c1",
            Mode::C1NoRowid => "c1_norowid",
            Mode::WalSeq => "walseq",
            Mode::BaseSerial => "base_serial",
            Mode::C1Serial => "c1_serial",
        }
    }

    fn indexes(self) -> bool {
        !matches!(self, Mode::Base | Mode::BaseSerial)
    }

    fn serial(self) -> bool {
        matches!(self, Mode::BaseSerial | Mode::C1Serial)
    }

    /// `(feed table, change table)`.
    fn tables(self) -> (Option<&'static str>, &'static str) {
        match self {
            Mode::Base | Mode::C1 | Mode::BaseSerial | Mode::C1Serial => {
                (Some("kb_feed"), "kb_changes")
            }
            Mode::C1NoRowid => (Some("kb_feed_wr"), "kb_changes_wr"),
            Mode::WalSeq => (None, "kb_changes_ws"),
        }
    }
}

fn index_schema(feed: Option<&str>, changes: &str, rowid: bool) -> String {
    let feed = feed.map_or(String::new(), |f| {
        format!(
            "CREATE TABLE {f} (kb_id TEXT PRIMARY KEY, epoch INTEGER NOT NULL,
                 head INTEGER NOT NULL, floor INTEGER NOT NULL);"
        )
    });
    let without = if rowid { "" } else { " WITHOUT ROWID" };
    format!(
        "{feed}
         CREATE TABLE {changes} (kb_id TEXT NOT NULL, subject TEXT NOT NULL,
             seq INTEGER NOT NULL, op TEXT NOT NULL, PRIMARY KEY (kb_id, subject)){without};
         CREATE INDEX idx_{changes}_seq ON {changes}(kb_id, seq);"
    )
}

const READ_CHANGES: &str = "SELECT subject, seq, op FROM kb_changes
     WHERE kb_id = ?1 AND seq > ?2 ORDER BY seq LIMIT 1000";

/// Deterministic xorshift, so every run writes the same shape of workload.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn kb_name(k: usize) -> String {
    format!("kb{k}")
}

fn node_doc(k: usize, i: usize) -> String {
    format!("kbn:{}:node-{i:06}", kb_name(k))
}

fn bump_head(conn: &sqlite::Connection, feed: &str, kb: &str) -> Result<i64, sqlite::Error> {
    let mut bump = conn.prepare(format!(
        "INSERT INTO {feed} (kb_id, epoch, head, floor)
         VALUES (?1, abs(random()) % 4294967296, 1, 0)
         ON CONFLICT (kb_id) DO UPDATE SET head = head + 1 RETURNING head"
    ))?;
    bump.bind((1, kb))?;
    bump.next()?;
    bump.read::<i64, _>(0)
}

/// The index statements for `mode`, inside the caller's transaction (which has
/// just written WAL row `wal_id`). Returns the sequence the change was given.
fn record_change(
    conn: &sqlite::Connection,
    mode: Mode,
    (kb, node): (&str, &str),
    wal_id: u64,
) -> Result<u64, sqlite::Error> {
    let (feed, changes) = mode.tables();
    let seq = match feed {
        Some(feed) => bump_head(conn, feed, kb)?,
        None => wal_id as i64,
    };
    let mut change = conn.prepare(format!(
        "INSERT INTO {changes} (kb_id, subject, seq, op) VALUES (?1, ?2, ?3, 'upsert')
         ON CONFLICT (kb_id, subject) DO UPDATE SET seq = excluded.seq, op = excluded.op"
    ))?;
    change.bind((1, kb))?;
    change.bind((2, node))?;
    change.bind((3, seq))?;
    change.next()?;
    Ok(seq as u64)
}

struct IndexedBackend {
    inner: Arc<SqliteBackend>,
    mode: Mode,
    /// C1's only in-memory signal: a per-KB watch holding `head`.
    feeds: HashMap<String, watch::Sender<u64>>,
    /// The in-process writer queue of the `*Serial` modes (tokio's is FIFO).
    writer: tokio::sync::Mutex<()>,
}

impl IndexedBackend {
    fn new(inner: Arc<SqliteBackend>, mode: Mode) -> Self {
        let feeds = (0..KBS)
            .map(|k| {
                (
                    kb_name(k),
                    watch::channel(head(&inner, "kb_feed", &kb_name(k))).0,
                )
            })
            .collect();
        IndexedBackend {
            inner,
            mode,
            feeds,
            writer: tokio::sync::Mutex::new(()),
        }
    }

    /// The real `SqliteBackend::wal_append` body plus `record_change`.
    fn append_indexed(
        &self,
        doc: &str,
        update: &[u8],
        client_id: Option<u64>,
        by: &EditStamp,
    ) -> Result<u64, StorageError> {
        let Some(DocAddress::KbNode { kb_id, node_id }) = DocAddress::parse(doc) else {
            panic!("bench writes node docs only: {doc}");
        };
        let (id, seq) = {
            let conn = self.inner.pool.lock_shard(doc);
            conn.execute("BEGIN IMMEDIATE")?;
            let written = editor_record::write_attributed(&conn, doc, update, client_id, by)
                .and_then(|id| {
                    let seq = record_change(&conn, self.mode, (&kb_id, &node_id), id)?;
                    Ok((id, seq))
                });
            editor_record::commit_or_rollback(&conn, written)
                .map_err(|e| StorageError::Sqlite(format!("wal append: {e}")))?
        };
        if let (Mode::C1 | Mode::C1Serial, Some(tx)) = (self.mode, self.feeds.get(&kb_id)) {
            tx.send_if_modified(|h| (seq > *h).then(|| *h = seq).is_some());
        }
        Ok(id)
    }
}

#[async_trait]
impl StorageBackend for IndexedBackend {
    async fn wal_append(
        &self,
        doc: &str,
        update: &[u8],
        client_id: Option<u64>,
        by: &EditStamp,
    ) -> Result<u64, StorageError> {
        let _queued = match self.mode.serial() {
            true => Some(self.writer.lock().await),
            false => None,
        };
        if !self.mode.indexes() {
            return self.inner.wal_append(doc, update, client_id, by).await;
        }
        self.append_indexed(doc, update, client_id, by)
    }
    async fn editors_since_wal(&self, d: &str, w: u64) -> Result<EditorsSince, StorageError> {
        self.inner.editors_since_wal(d, w).await
    }
    async fn wal_high_water(&self, d: &str) -> Result<u64, StorageError> {
        self.inner.wal_high_water(d).await
    }
    async fn forget_editors(&self, d: &str) -> Result<(), StorageError> {
        self.inner.forget_editors(d).await
    }
    async fn load_document(&self, d: &str) -> Result<Option<DocumentState>, StorageError> {
        self.inner.load_document(d).await
    }
    async fn compact(&self, d: &str, s: &[u8], w: u64) -> Result<(), StorageError> {
        self.inner.compact(d, s, w).await
    }
    async fn list_documents(&self) -> Result<Vec<String>, StorageError> {
        self.inner.list_documents().await
    }
    async fn doc_exists(&self, d: &str) -> Result<bool, StorageError> {
        self.inner.doc_exists(d).await
    }
    async fn delete_document(&self, d: &str) -> Result<(), StorageError> {
        self.inner.delete_document(d).await
    }
    // History (ADR-116) is not what this benchmark measures: passed through.
    async fn history_session(
        &self,
        kb: &str,
        node: &str,
    ) -> Result<Vec<(EditorKind, String)>, StorageError> {
        self.inner.history_session(kb, node).await
    }
    async fn history_cut(
        &self,
        cp: Checkpoint,
        now: i64,
    ) -> Result<Option<HistoryEntry>, StorageError> {
        self.inner.history_cut(cp, now).await
    }
    async fn history_holds_content(&self, kb: &str) -> Result<bool, StorageError> {
        self.inner.history_holds_content(kb).await
    }
    async fn history_nodes(&self, kb: &str) -> Result<Vec<String>, StorageError> {
        self.inner.history_nodes(kb).await
    }
    async fn history_kbs(&self) -> Result<Vec<String>, StorageError> {
        self.inner.history_kbs().await
    }
    async fn history_tips(&self, kb: &str) -> Result<Vec<Tip>, StorageError> {
        self.inner.history_tips(kb).await
    }
    async fn history_append_head(&self, head: &SignedHead) -> Result<(), StorageError> {
        self.inner.history_append_head(head).await
    }
    async fn history_latest_head(&self, kb: &str) -> Result<Option<SignedHead>, StorageError> {
        self.inner.history_latest_head(kb).await
    }
    async fn history_key(&self, kb: &str) -> Result<Option<HistoryKey>, StorageError> {
        self.inner.history_key(kb).await
    }
    async fn history_list(&self, kb: &str, node: &str) -> Result<Vec<HistoryEntry>, StorageError> {
        self.inner.history_list(kb, node).await
    }
    async fn history_page(
        &self,
        kb: &str,
        node: &str,
        before: Option<u64>,
        limit: u32,
    ) -> Result<Vec<Listed>, StorageError> {
        self.inner.history_page(kb, node, before, limit).await
    }
}

fn head(backend: &SqliteBackend, feed: &str, kb: &str) -> u64 {
    let conn = backend.pool.lock_primary();
    let mut stmt = conn
        .prepare(format!("SELECT head FROM {feed} WHERE kb_id = ?1"))
        .unwrap();
    stmt.bind((1, kb)).unwrap();
    match stmt.next().unwrap() {
        sqlite::State::Row => stmt.read::<i64, _>(0).unwrap() as u64,
        sqlite::State::Done => 0,
    }
}

/// Per mode: (sum of per-KB heads, sum of per-KB max change seq). For
/// `WalSeq`, (max WAL id, max change seq), which must be equal after a run.
fn index_state(backend: &SqliteBackend, mode: Mode) -> (i64, i64) {
    let (feed, changes) = mode.tables();
    let conn = backend.pool.lock_primary();
    let tip = scalar(
        &conn,
        &format!(
            "SELECT COALESCE(SUM(m), 0) FROM (SELECT MAX(seq) m FROM {changes} GROUP BY kb_id)"
        ),
    );
    match (mode, feed) {
        (Mode::WalSeq, _) => (
            scalar(&conn, "SELECT MAX(id) FROM wal"),
            scalar(&conn, &format!("SELECT MAX(seq) FROM {changes}")),
        ),
        (_, Some(f)) => (scalar(&conn, &format!("SELECT SUM(head) FROM {f}")), tip),
        (_, None) => unreachable!(),
    }
}

fn exec(conn: &sqlite::Connection, sql: &str, binds: &[sqlite::Value]) {
    let mut stmt = conn.prepare(sql).unwrap();
    for (i, v) in binds.iter().enumerate() {
        stmt.bind((i + 1, v.clone())).unwrap();
    }
    while let sqlite::State::Row = stmt.next().unwrap() {}
}

fn scalar(conn: &sqlite::Connection, sql: &str) -> i64 {
    let mut stmt = conn.prepare(sql).unwrap();
    stmt.next().unwrap();
    stmt.read::<i64, _>(0).unwrap()
}

/// A node as storage holds it after a little editing: a ~1.2 KB body snapshot
/// (ADR-109's per-node body) and three later updates still in the WAL tail.
fn make_node(rng: &mut Rng) -> (Vec<u8>, Vec<Vec<u8>>) {
    const WORDS: [&str; 8] = ["graph", "node", "sync", "kb", "write", "index", "the", "a"];
    let mut body = String::new();
    while body.len() < 1200 {
        body.push_str(WORDS[rng.below(WORDS.len())]);
        body.push(' ');
    }
    let mut sync = TextSync::new(&body);
    let snapshot = sync.encode_state();
    let tail = (0..3)
        .map(|_| sync.insert(rng.below(body.len()) as u32, "edit "))
        .collect();
    (snapshot, tail)
}

/// Build `nodes` node docs across `KBS` KBs through the real record writer,
/// with their WAL tails interleaved across docs, plus a full change index.
fn populate(backend: &SqliteBackend, nodes: usize) {
    const CHUNK: usize = 5_000;
    let conn = backend.pool.lock_primary();
    let seed_by = Attribution::Principal("seed-user");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for start in (0..nodes).step_by(CHUNK) {
        conn.execute("BEGIN IMMEDIATE").unwrap();
        let mut tails = Vec::new();
        for i in start..(start + CHUNK).min(nodes) {
            let doc = node_doc(i % KBS, i / KBS);
            let (snap, tail) = make_node(&mut rng);
            let hash = hex::encode(Sha256::digest(&snap));
            exec(
                &conn,
                "INSERT INTO snapshots (doc_name, state, wal_id, hash) VALUES (?1, ?2, 0, ?3)",
                &[doc.clone().into(), snap.into(), hash.into()],
            );
            tails.extend(tail.into_iter().map(|u| (doc.clone(), u)));
        }
        // Interleave the tails across documents, as concurrent editing leaves them.
        for j in (1..tails.len()).rev() {
            tails.swap(j, rng.below(j + 1));
        }
        for (doc, update) in tails {
            let stamp = EditStamp::new(seed_by.into(), 1);
            editor_record::write_attributed(&conn, &doc, &update, Some(7), &stamp).unwrap();
        }
        conn.execute("COMMIT").unwrap();
    }
    let db_pages = scalar(&conn, "PRAGMA page_count");
    for mode in [Mode::C1, Mode::C1NoRowid, Mode::WalSeq] {
        populate_index(&conn, mode, nodes);
    }
    let page = scalar(&conn, "PRAGMA page_size");
    println!(
        "FIXTURE nodes={nodes} store_bytes_without_index={} sqlite={}",
        db_pages * page,
        version(&conn)
    );
}

/// One change row per node and a feed row per KB, as a store that has seen
/// every node change once would hold; prints what that costs on disk.
fn populate_index(conn: &sqlite::Connection, mode: Mode, nodes: usize) {
    let (feed, changes) = mode.tables();
    conn.execute(index_schema(feed, changes, mode != Mode::C1NoRowid))
        .unwrap();
    let pages_before = scalar(conn, "PRAGMA page_count");
    conn.execute("BEGIN IMMEDIATE").unwrap();
    let insert =
        format!("INSERT INTO {changes} (kb_id, subject, seq, op) VALUES (?1, ?2, ?3, 'upsert')");
    for i in 0..nodes {
        let (kb, node) = (kb_name(i % KBS), format!("node-{:06}", i / KBS));
        exec(
            conn,
            &insert,
            &[kb.into(), node.into(), ((i / KBS + 1) as i64).into()],
        );
    }
    for k in (0..KBS).filter(|_| feed.is_some()) {
        let sql = format!(
            "INSERT INTO {} (kb_id, epoch, head, floor) VALUES (?1, 1, ?2, 0)",
            feed.unwrap()
        );
        exec(
            conn,
            &sql,
            &[kb_name(k).into(), (nodes.div_ceil(KBS) as i64).into()],
        );
    }
    conn.execute("COMMIT").unwrap();
    let grown =
        (scalar(conn, "PRAGMA page_count") - pages_before) * scalar(conn, "PRAGMA page_size");
    println!(
        "FIXTURE index={} rows={nodes} bytes={grown} bytes_per_row={}",
        mode.name(),
        grown / nodes as i64
    );
}

fn version(conn: &sqlite::Connection) -> String {
    let mut stmt = conn.prepare("SELECT sqlite_version()").unwrap();
    stmt.next().unwrap();
    stmt.read::<String, _>(0).unwrap()
}

/// A fresh store directory per test and process, so the two benchmarks never
/// share (or delete) each other's database.
fn bench_dir(test: &str) -> PathBuf {
    let base = std::env::var_os("MAE_BENCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".cache/mae-bench-074a")
        });
    base.join(format!("{test}-{}", std::process::id()))
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// The writer's hot set: disjoint per writer, its KB chosen round-robin, so
/// writers spread over KBs and (by doc-name hash) over all four shards.
fn hot_docs(writer: usize) -> Vec<String> {
    let (k, slot) = (writer % KBS, writer / KBS);
    (0..HOT_PER_WRITER)
        .map(|j| node_doc(k, slot * HOT_PER_WRITER + j))
        .collect()
}

#[path = "storage_change_index_bench_run_tests.rs"]
mod run;
