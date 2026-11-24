//! An upload, from the request body to a committed blob (ADR-115 D3, D3a).
//!
//! [`Receiving`] streams the body to a file under `.tmp` in constant memory,
//! refusing at the cap before writing a byte past it, and hashes what arrived
//! (for `Content-Digest`). [`store_upload`] then processes that file and
//! commits the result; it is synchronous and runs on a bounded worker.

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::process::{self, ProcessError, ProcessLimits};
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
}

/// Process the received file and commit the result to `kb_id`. Synchronous:
/// run it on a blocking worker. The received file is removed either way.
pub fn store_upload(
    store: &BlobStore,
    kb_id: &str,
    received: Received,
    limits: &ProcessLimits,
    quota: Option<&StorageQuota>,
) -> Result<BlobMeta, UploadError> {
    let raw =
        std::fs::read(received.path.path()).map_err(|e| UploadError::Store(BlobError::from(e)))?;
    drop(received);
    let processed = process::process(raw, limits).map_err(UploadError::Process)?;
    let staged = store.stage(&processed.bytes).map_err(UploadError::Store)?;
    store
        .commit(
            kb_id,
            staged,
            (processed.media_type, processed.width, processed.height),
            quota,
        )
        .map_err(UploadError::Store)
}
