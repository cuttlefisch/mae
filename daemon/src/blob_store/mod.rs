//! ADR-115 P1: a KB's attachments, in a per-KB, content-addressed store.
//!
//! **Layout** (D1), under the collab data directory's `blobs/`:
//!
//! ```text
//! blobs/blobs.db                    metadata (SQLite): the `kb_blob` table
//! blobs/<kb_id>/<sha256[0:2]>/<sha256>   one immutable file per blob
//! blobs/.tmp/                       uploads being received or committed
//! ```
//!
//! A `kb_id` is `^[a-z0-9][a-z0-9-]{0,62}$` (checked before any path is built),
//! so it can never be `.tmp`, `blobs.db`, or anything that differs only in case.
//!
//! **Deduplication is per KB only.** The same bytes in two KBs are two rows and
//! two files: nothing about another KB's blobs is ever consulted, so neither an
//! upload's timing nor its answer can say what another KB holds.
//!
//! **The server picks the address.** [`process::process`] runs first and the
//! sha256 is of its output, so a client cannot choose the address, and a
//! re-upload of the same file maps to the same blob.
//!
//! **A commit** ([`BlobStore::commit`]) is: the processed bytes written to a
//! temporary file and `fsync`ed ([`BlobStore::stage`]); then, under the KB's
//! binding lock and the metadata lock, the quota checked, the file renamed into
//! place and its directory `fsync`ed, and only then the row inserted. So a crash
//! at any step leaves no row naming a missing file. It can leave a file with no
//! row, which no quota counts (a tenant's usage is the sum of its rows) and the
//! next upload of the same bytes simply replaces. Temporary files are removed at
//! the next [`BlobStore::open`].
//!
//! **Quota** (D3a's storage quota): each row records the tenant it was charged
//! to (`charged_to`, beyond D1's columns: the quota needs it). A deduplicated
//! upload adds no row and so charges nothing. The check and the insert happen
//! under one lock, so concurrent uploads cannot jointly overshoot.
//!
//! **No garbage collection yet.** Without P2's `attachments` map nothing
//! references a blob, so P1 never deletes one. P2's sweep (D5) is to run under
//! [`BlobStore::binding_lock`] for the KB, the same lock a commit holds, and drop
//! what is outside the live set with [`BlobStore::remove_unreferenced`], which
//! deletes the row before the file. P2 also adds D1's `blob_ref` table to this
//! database; P1 creates only `kb_blob`.

pub mod access;
pub mod process;
pub mod sniff;
pub mod strip;
pub mod svg;
pub mod upload;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod svg_tests;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub use sniff::MediaType;

/// `oauth.max_attachment_bytes`'s default: 10 MiB (see `oauth_config`, where
/// the reasoning is pinned). Here so that every way into the store, the
/// HTTPS route and `kb seed`, starts from one value.
pub const DEFAULT_MAX_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;
/// `oauth.max_attachment_pixels`'s default: D3a's 40 megapixels.
pub const DEFAULT_MAX_ATTACHMENT_PIXELS: u64 = 40_000_000;

/// The blob root's name under the collab data directory.
pub const DIR: &str = "blobs";
/// The metadata database, inside the blob root.
pub const DB_FILE: &str = "blobs.db";
/// Where uploads are received and staged, inside the blob root: on the same
/// filesystem as the blobs, so the final rename is atomic.
pub const TMP_DIR: &str = ".tmp";

const SCHEMA: &str = "PRAGMA journal_mode=WAL;
     PRAGMA busy_timeout=5000;
     CREATE TABLE IF NOT EXISTS kb_blob (
         kb_id      TEXT NOT NULL,
         sha256     TEXT NOT NULL,
         size       INTEGER NOT NULL,
         media_type TEXT NOT NULL,
         width      INTEGER,
         height     INTEGER,
         stored_at  INTEGER NOT NULL,
         charged_to TEXT,
         PRIMARY KEY (kb_id, sha256)
     );
     CREATE INDEX IF NOT EXISTS kb_blob_charged_to ON kb_blob (charged_to);";

/// One stored blob's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobMeta {
    pub sha256: String,
    pub size: u64,
    pub media_type: MediaType,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Unix seconds. P2's grace period counts from it.
    pub stored_at: u64,
}

/// A tenant's storage quota, as the upload's principal resolves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageQuota {
    /// The bucket's label, recorded in `charged_to`: [`StorageQuota::tenant`]
    /// or [`StorageQuota::unassigned`].
    pub tenant: String,
    /// Bytes the tenant's blobs may total. `0`: unlimited, still recorded.
    pub max_bytes: u64,
}

/// The `charged_to` label of the bucket principals in no configured tenant
/// share. A configured tenant's label is always `tenant:<name>`, so no tenant
/// name, whatever an operator writes, can share this bucket's usage.
pub const UNASSIGNED: &str = "unassigned";

impl StorageQuota {
    /// A configured tenant's quota.
    pub fn tenant(name: &str, max_bytes: u64) -> Self {
        StorageQuota {
            tenant: format!("tenant:{name}"),
            max_bytes,
        }
    }

    /// The quota every principal in no configured tenant shares.
    pub fn unassigned(max_bytes: u64) -> Self {
        StorageQuota {
            tenant: UNASSIGNED.to_string(),
            max_bytes,
        }
    }
}

#[derive(Debug)]
pub enum BlobError {
    /// `tenant_quota_exceeded`: storing this blob would take the tenant past
    /// its storage quota.
    TenantFull { used: u64, size: u64, limit: u64 },
    /// The store could not be read or written.
    Storage(String),
    /// A fault injected through [`CommitStep`]: the commit stopped there,
    /// leaving the disk as a crash at that step would.
    Interrupted(CommitStep),
}

impl std::fmt::Display for BlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobError::TenantFull { used, size, limit } => write!(
                f,
                "the tenant holds {used} bytes; {size} more would exceed its {limit}-byte storage quota"
            ),
            BlobError::Storage(e) => write!(f, "blob storage: {e}"),
            BlobError::Interrupted(s) => write!(f, "commit interrupted at {s:?}"),
        }
    }
}

impl From<std::io::Error> for BlobError {
    fn from(e: std::io::Error) -> Self {
        BlobError::Storage(e.to_string())
    }
}

impl From<sqlite::Error> for BlobError {
    fn from(e: sqlite::Error) -> Self {
        BlobError::Storage(e.to_string())
    }
}

/// The steps of a commit, in order, where a test can stop it (the seam the
/// crash tests use; production installs no hook).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitStep {
    /// The staged file is written, not yet `fsync`ed.
    BeforeFsync,
    /// The staged file is durable, not yet renamed into place.
    BeforeRename,
    /// The blob file is in place, its row not yet inserted.
    BeforeRow,
    /// The row is inserted.
    AfterRow,
}

/// Consulted at each [`CommitStep`]; `true` stops the commit there.
pub type FaultHook = Arc<dyn Fn(CommitStep) -> bool + Send + Sync>;

/// A file under `.tmp`, removed when dropped unless it was moved or kept.
#[derive(Debug)]
pub struct TempPath(Option<PathBuf>);

impl TempPath {
    pub fn path(&self) -> &Path {
        self.0
            .as_deref()
            .expect("a TempPath holds its path until dropped")
    }

    /// Leave the file where it is (a simulated crash, or a completed rename).
    fn keep(mut self) {
        self.0 = None;
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Processed bytes staged durably under `.tmp`, ready to commit.
#[derive(Debug)]
pub struct Staged {
    file: TempPath,
    pub sha256: String,
    pub size: u64,
}

pub struct BlobStore {
    root: PathBuf,
    conn: Mutex<sqlite::Connection>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    faults: Option<FaultHook>,
}

/// Holds one KB's binding lock (D5): a commit, and P2's binds and sweep.
pub struct KbGuard(#[allow(dead_code)] tokio::sync::OwnedMutexGuard<()>);

impl BlobStore {
    /// Open (creating) the store at `root`, and remove whatever an interrupted
    /// upload left under `.tmp`.
    pub fn open(root: &Path) -> Result<Self, BlobError> {
        std::fs::create_dir_all(root)?;
        let tmp = root.join(TMP_DIR);
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp)?;
        }
        std::fs::create_dir_all(&tmp)?;
        let conn = sqlite::Connection::open(root.join(DB_FILE))?;
        conn.execute(SCHEMA)?;
        Ok(BlobStore {
            root: root.to_path_buf(),
            conn: Mutex::new(conn),
            locks: Mutex::new(HashMap::new()),
            faults: None,
        })
    }

    /// Install a fault hook (tests: the crash seam).
    pub fn with_fault_hook(mut self, hook: FaultHook) -> Self {
        self.faults = Some(hook);
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn fault(&self, step: CommitStep) -> bool {
        self.faults.as_ref().is_some_and(|f| f(step))
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, sqlite::Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Where blob `sha256` of `kb_id` lives. Both must already be validated
    /// ([`valid_kb_id`], [`valid_sha256`]).
    pub fn blob_path(&self, kb_id: &str, sha256: &str) -> PathBuf {
        self.root.join(kb_id).join(&sha256[..2]).join(sha256)
    }

    /// A new, empty file under `.tmp`, with a random name.
    pub fn temp_file(&self) -> std::io::Result<(TempPath, std::fs::File)> {
        let mut name = [0u8; 16];
        rand::Rng::fill_bytes(&mut rand::rng(), &mut name);
        let path = self.root.join(TMP_DIR).join(hex::encode(name));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok((TempPath(Some(path)), file))
    }

    /// Write processed bytes to `.tmp` and `fsync` them: the first step of a
    /// commit, done before any lock is taken.
    pub fn stage(&self, bytes: &[u8]) -> Result<Staged, BlobError> {
        use sha2::Digest;
        let (path, mut file) = self.temp_file()?;
        file.write_all(bytes)?;
        if self.fault(CommitStep::BeforeFsync) {
            path.keep();
            return Err(BlobError::Interrupted(CommitStep::BeforeFsync));
        }
        file.sync_all()?;
        Ok(Staged {
            file: path,
            sha256: hex::encode(sha2::Sha256::digest(bytes)),
            size: bytes.len() as u64,
        })
    }

    /// The binding lock for `kb_id` (D5), blocking until it is free. Call from
    /// a blocking context (a `spawn_blocking` worker), never on the runtime.
    pub fn binding_lock(&self, kb_id: &str) -> KbGuard {
        let lock = {
            let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(locks.entry(kb_id.to_string()).or_default())
        };
        KbGuard(lock.blocking_lock_owned())
    }

    /// The row of blob `sha256` in `kb_id`, if this KB holds it.
    pub fn get(&self, kb_id: &str, sha256: &str) -> Result<Option<BlobMeta>, BlobError> {
        select(&self.conn(), kb_id, sha256)
    }

    /// Bytes charged to `tenant`: the sum of its rows.
    pub fn usage(&self, tenant: &str) -> Result<u64, BlobError> {
        usage(&self.conn(), tenant)
    }

    /// Commit a staged blob to `kb_id` (see the module docs). An existing row
    /// for the same bytes is returned as it is: no second file, no charge.
    pub fn commit(
        &self,
        kb_id: &str,
        staged: Staged,
        media: (MediaType, Option<u32>, Option<u32>),
        quota: Option<&StorageQuota>,
    ) -> Result<BlobMeta, BlobError> {
        let _kb = self.binding_lock(kb_id);
        let conn = self.conn();
        if let Some(existing) = select(&conn, kb_id, &staged.sha256)? {
            return Ok(existing);
        }
        if let Some(q) = quota.filter(|q| q.max_bytes > 0) {
            let used = usage(&conn, &q.tenant)?;
            if used.saturating_add(staged.size) > q.max_bytes {
                return Err(BlobError::TenantFull {
                    used,
                    size: staged.size,
                    limit: q.max_bytes,
                });
            }
        }
        if self.fault(CommitStep::BeforeRename) {
            staged.file.keep();
            return Err(BlobError::Interrupted(CommitStep::BeforeRename));
        }
        let dest = self.blob_path(kb_id, &staged.sha256);
        let dir = dest.parent().expect("a blob path has a directory");
        std::fs::create_dir_all(dir)?;
        std::fs::rename(staged.file.path(), &dest)?;
        staged.file.keep();
        sync_dir(dir)?;
        if self.fault(CommitStep::BeforeRow) {
            return Err(BlobError::Interrupted(CommitStep::BeforeRow));
        }
        let meta = BlobMeta {
            sha256: staged.sha256,
            size: staged.size,
            media_type: media.0,
            width: media.1,
            height: media.2,
            stored_at: now_unix(),
        };
        insert(&conn, kb_id, &meta, quota.map(|q| q.tenant.as_str()))?;
        if self.fault(CommitStep::AfterRow) {
            return Err(BlobError::Interrupted(CommitStep::AfterRow));
        }
        Ok(meta)
    }

    /// P2's sweep primitive: drop blob `sha256` from `kb_id`, the row first
    /// and then the file, so no row ever names a missing file. The caller holds
    /// the KB's binding lock and has established that nothing references it.
    pub fn remove_unreferenced(
        &self,
        _held: &KbGuard,
        kb_id: &str,
        sha256: &str,
    ) -> Result<(), BlobError> {
        {
            let conn = self.conn();
            let mut st = conn.prepare("DELETE FROM kb_blob WHERE kb_id = ? AND sha256 = ?")?;
            st.bind((1, kb_id))?;
            st.bind((2, sha256))?;
            while st.next()? != sqlite::State::Done {}
        }
        match std::fs::remove_file(self.blob_path(kb_id, sha256)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// Every `(kb_id, sha256)` row, for backup.
    pub fn list_all(&self) -> Result<Vec<(String, String)>, BlobError> {
        list_all(&self.conn())
    }
}

/// Every `(kb_id, sha256)` row of the database at `db`, opened read-only: what
/// `mae-daemon backup` copies the files of, from its own snapshot.
pub fn list_rows(db: &Path) -> Result<Vec<(String, String)>, BlobError> {
    let conn = sqlite::Connection::open_with_flags(db, sqlite::OpenFlags::new().with_read_only())?;
    list_all(&conn)
}

fn list_all(conn: &sqlite::Connection) -> Result<Vec<(String, String)>, BlobError> {
    let mut st = conn.prepare("SELECT kb_id, sha256 FROM kb_blob ORDER BY kb_id, sha256")?;
    let mut out = Vec::new();
    while st.next()? == sqlite::State::Row {
        out.push((st.read::<String, _>(0)?, st.read::<String, _>(1)?));
    }
    Ok(out)
}

fn select(
    conn: &sqlite::Connection,
    kb_id: &str,
    sha256: &str,
) -> Result<Option<BlobMeta>, BlobError> {
    let mut st = conn.prepare(
        "SELECT size, media_type, width, height, stored_at FROM kb_blob
         WHERE kb_id = ? AND sha256 = ?",
    )?;
    st.bind((1, kb_id))?;
    st.bind((2, sha256))?;
    if st.next()? != sqlite::State::Row {
        return Ok(None);
    }
    let dim = |v: Option<i64>| v.and_then(|v| u32::try_from(v).ok());
    Ok(Some(BlobMeta {
        sha256: sha256.to_string(),
        size: st.read::<i64, _>(0)? as u64,
        media_type: MediaType::parse(&st.read::<String, _>(1)?),
        width: dim(st.read::<Option<i64>, _>(2)?),
        height: dim(st.read::<Option<i64>, _>(3)?),
        stored_at: st.read::<i64, _>(4)? as u64,
    }))
}

fn usage(conn: &sqlite::Connection, tenant: &str) -> Result<u64, BlobError> {
    let mut st = conn.prepare("SELECT COALESCE(SUM(size), 0) FROM kb_blob WHERE charged_to = ?")?;
    st.bind((1, tenant))?;
    st.next()?;
    Ok(st.read::<i64, _>(0)? as u64)
}

fn insert(
    conn: &sqlite::Connection,
    kb_id: &str,
    m: &BlobMeta,
    charged_to: Option<&str>,
) -> Result<(), BlobError> {
    let mut st = conn.prepare(
        "INSERT INTO kb_blob (kb_id, sha256, size, media_type, width, height, stored_at, charged_to)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )?;
    st.bind((1, kb_id))?;
    st.bind((2, m.sha256.as_str()))?;
    st.bind((3, m.size as i64))?;
    st.bind((4, m.media_type.as_str()))?;
    st.bind((5, m.width.map(i64::from)))?;
    st.bind((6, m.height.map(i64::from)))?;
    st.bind((7, m.stored_at as i64))?;
    st.bind((8, charged_to))?;
    while st.next()? != sqlite::State::Done {}
    Ok(())
}

/// Make a rename in `dir` durable. Windows has no directory `fsync`; its
/// rename is already flushed with the file system's metadata.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// A blob address: 64 lowercase hex digits.
pub fn valid_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A KB identifier as every route takes it (ADR-119 P-04).
pub fn valid_kb_id(s: &str) -> bool {
    mae_site::address::valid_kb_id(s)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
