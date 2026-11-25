//! An upload, from the request body to a committed blob (ADR-115 D3, D3a).
//!
//! [`Receiving`] streams the body to a file under `.tmp` in constant memory,
//! refusing at the cap before writing a byte past it, and hashes what arrived
//! (for `Content-Digest`). [`store_upload`] then processes that file and
//! commits the result; it is synchronous and runs on a bounded worker.

use std::sync::atomic::{AtomicU8, Ordering};

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::process::{ProcessError, ProcessLimits, Processed};
use super::{BlobError, BlobMeta, BlobStore, StorageQuota, TempPath};

/// A body being received.
pub struct Receiving {
    file: tokio::fs::File,
    path: TempPath,
    hasher: Sha256,
    len: u64,
    cap: u64,
}

/// A body fully received: its file, length and sha256 as sent.
pub struct Received {
    pub path: TempPath,
    pub len: u64,
    pub sha256: [u8; 32],
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReceiveError {
    /// More bytes than the cap allows. Nothing past the cap was written.
    OverCap {
        cap: u64,
    },
    Io(String),
}

impl Receiving {
    /// Start receiving a body of at most `cap` bytes.
    pub fn new(store: &BlobStore, cap: u64) -> std::io::Result<Self> {
        let (path, file) = store.temp_file()?;
        Ok(Receiving {
            file: tokio::fs::File::from_std(file),
            path,
            hasher: Sha256::new(),
            len: 0,
            cap,
        })
    }

    /// Bytes written so far.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append one chunk. A chunk that would take the body past the cap is
    /// refused whole; the caller stops reading.
    pub async fn write(&mut self, chunk: &[u8]) -> Result<(), ReceiveError> {
        let next = self.len.saturating_add(chunk.len() as u64);
        if next > self.cap {
            return Err(ReceiveError::OverCap { cap: self.cap });
        }
        self.file
            .write_all(chunk)
            .await
            .map_err(|e| ReceiveError::Io(e.to_string()))?;
        self.hasher.update(chunk);
        self.len = next;
        Ok(())
    }

    /// The body is complete.
    pub async fn finish(mut self) -> Result<Received, ReceiveError> {
        self.file
            .flush()
            .await
            .map_err(|e| ReceiveError::Io(e.to_string()))?;
        Ok(Received {
            path: self.path,
            len: self.len,
            sha256: self.hasher.finalize().into(),
        })
    }
}

/// Why an upload was not stored.
#[derive(Debug)]
pub enum UploadError {
    Process(ProcessError),
    Store(BlobError),
    /// The job was cancelled ([`Job::cancel`]) before it committed: nothing
    /// was stored and nothing charged.
    Cancelled,
}

/// What turns an upload's bytes into what is stored: [`super::process::process`]
/// in production; a test installs one it controls.
pub type Processor =
    dyn Fn(Vec<u8>, &ProcessLimits) -> Result<Processed, ProcessError> + Send + Sync;

const RUNNING: u8 = 0;
const COMMITTING: u8 = 1;
const CANCELLED: u8 = 2;

/// One upload's processing, shared by the request that waits on it and the
/// worker that runs it, so that exactly one of "it timed out" and "it was
/// stored" is ever true. The worker claims the commit ([`Job::claim_commit`])
/// before it writes anything; the request, on its timeout, cancels
/// ([`Job::cancel`]). Whichever comes first wins, atomically.
#[derive(Debug, Default)]
pub struct Job(AtomicU8);

impl Job {
    pub fn new() -> Self {
        Job(AtomicU8::new(RUNNING))
    }

    /// Cancel the job. `true`: it will never commit (or was already
    /// cancelled). `false`: the worker has already claimed the commit, which
    /// is then the outcome to wait for.
    pub fn cancel(&self) -> bool {
        match self
            .0
            .compare_exchange(RUNNING, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => true,
            Err(state) => state == CANCELLED,
        }
    }

    /// Claim the commit. `false`: the job was cancelled; store nothing.
    fn claim_commit(&self) -> bool {
        self.0
            .compare_exchange(RUNNING, COMMITTING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

/// Process the received file and commit the result to `kb_id`, unless `job`
/// is cancelled first. Synchronous: run it on a blocking worker. The received
/// file, and anything staged, is removed whenever nothing is committed.
pub fn store_upload(
    store: &BlobStore,
    kb_id: &str,
    received: Received,
    limits: &ProcessLimits,
    quota: Option<&StorageQuota>,
    processor: &Processor,
    job: &Job,
) -> Result<BlobMeta, UploadError> {
    let raw =
        std::fs::read(received.path.path()).map_err(|e| UploadError::Store(BlobError::from(e)))?;
    drop(received);
    let processed = processor(raw, limits).map_err(UploadError::Process)?;
    let staged = store.stage(&processed.bytes).map_err(UploadError::Store)?;
    // The last moment to stop: past it the blob is renamed into place and
    // charged. A request that timed out has cancelled by now, or is waiting
    // for this commit (see `Job`).
    if !job.claim_commit() {
        return Err(UploadError::Cancelled);
    }
    store
        .commit(
            kb_id,
            staged,
            (processed.media_type, processed.width, processed.height),
            quota,
        )
        .map_err(UploadError::Store)
}
