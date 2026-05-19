//! mae-sync: Collaborative state synchronization via yrs (YATA CRDT).
//!
//! Wraps yrs with MAE-specific document schemas and provides a bridge
//! between yrs YText and ropey Rope for rendering.

pub mod encoding;
pub mod kb;
pub mod text;

pub use yrs;

use std::fmt;

/// Errors from sync operations.
#[derive(Debug)]
pub enum SyncError {
    Encoding(String),
    RopeRebuild(String),
    Schema(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encoding(msg) => write!(f, "yrs encoding error: {msg}"),
            Self::RopeRebuild(msg) => write!(f, "rope rebuild failed: {msg}"),
            Self::Schema(msg) => write!(f, "schema violation: {msg}"),
        }
    }
}

impl std::error::Error for SyncError {}

/// Structured document address for cross-session stability.
///
/// Documents can be identified by project-relative file path, KB node ID,
/// or arbitrary shared name. The string form uses URI-like prefixes:
/// `file:{project_hash}/{rel_path}`, `kb:{node_id}`, `shared:{name}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DocAddress {
    /// A file within a project, identified by project hash and relative path.
    File {
        project_hash: String,
        rel_path: String,
    },
    /// A knowledge-base node.
    KbNode { node_id: String },
    /// An arbitrary shared document (e.g. scratch buffers, REPL).
    Shared { name: String },
}

impl DocAddress {
    /// Convert to the canonical doc_name string used in storage / sync protocol.
    pub fn to_doc_name(&self) -> String {
        match self {
            DocAddress::File {
                project_hash,
                rel_path,
            } => format!("file:{project_hash}/{rel_path}"),
            DocAddress::KbNode { node_id } => format!("kb:{node_id}"),
            DocAddress::Shared { name } => format!("shared:{name}"),
        }
    }

    /// Parse a doc_name string back into a DocAddress.
    pub fn parse(s: &str) -> Option<Self> {
        if let Some(rest) = s.strip_prefix("file:") {
            let slash = rest.find('/')?;
            let project_hash = rest[..slash].to_string();
            let rel_path = rest[slash + 1..].to_string();
            if project_hash.is_empty() || rel_path.is_empty() {
                return None;
            }
            Some(DocAddress::File {
                project_hash,
                rel_path,
            })
        } else if let Some(rest) = s.strip_prefix("kb:") {
            if rest.is_empty() {
                return None;
            }
            Some(DocAddress::KbNode {
                node_id: rest.to_string(),
            })
        } else if let Some(rest) = s.strip_prefix("shared:") {
            if rest.is_empty() {
                return None;
            }
            Some(DocAddress::Shared {
                name: rest.to_string(),
            })
        } else {
            None
        }
    }
}

/// Save policy derived from `DocAddress` type.
///
/// Determines how `:w` behaves for collaborative documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavePolicy {
    /// Each client writes to their own `{project_root}/{rel_path}`.
    LocalFirst,
    /// KB owner client persists CRDT to SQLite.
    ServerAuthoritative,
    /// `:w` prompts for file path (scratch buffer).
    Ephemeral,
}

impl DocAddress {
    /// Derive the save policy for this document type.
    pub fn save_policy(&self) -> SavePolicy {
        match self {
            DocAddress::File { .. } => SavePolicy::LocalFirst,
            DocAddress::KbNode { .. } => SavePolicy::ServerAuthoritative,
            DocAddress::Shared { .. } => SavePolicy::Ephemeral,
        }
    }
}

/// Per-client clock comparison result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockStatus {
    /// Both sides have the same clock for this client_id.
    Aligned,
    /// Local is ahead of remote by the given number of operations.
    Ahead(u32),
    /// Local is behind remote by the given number of operations.
    Behind(u32),
    /// Only exists on one side.
    LocalOnly,
    /// Only exists on remote side.
    RemoteOnly,
}

/// Diagnosis of sync state between two state vectors.
#[derive(Debug, Clone)]
pub struct SyncDiagnosis {
    /// Per-client_id comparison.
    pub clocks: Vec<(u64, ClockStatus)>,
    /// Overall status.
    pub status: SyncOverallStatus,
}

/// Summary of sync state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOverallStatus {
    Aligned,
    Diverged,
}

/// Compare two yrs state vectors (v1-encoded) and produce a per-client diagnosis.
///
/// Used by `collab-doctor` to report sync health.
pub fn compare_state_vectors(
    local_sv: &[u8],
    remote_sv: &[u8],
) -> Result<SyncDiagnosis, SyncError> {
    use yrs::{updates::decoder::Decode, StateVector};

    let local = StateVector::decode_v1(local_sv)
        .map_err(|e| SyncError::Encoding(format!("local sv decode: {e}")))?;
    let remote = StateVector::decode_v1(remote_sv)
        .map_err(|e| SyncError::Encoding(format!("remote sv decode: {e}")))?;

    let mut all_ids: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
    for (&cid, _) in local.iter() {
        all_ids.insert(cid);
    }
    for (&cid, _) in remote.iter() {
        all_ids.insert(cid);
    }

    let mut clocks = Vec::new();
    let mut all_aligned = true;

    for cid in all_ids {
        let l_present = local.contains_client(&cid);
        let r_present = remote.contains_client(&cid);
        let status = match (l_present, r_present) {
            (true, true) => {
                let lv = local.get(&cid);
                let rv = remote.get(&cid);
                if lv == rv {
                    ClockStatus::Aligned
                } else if lv > rv {
                    all_aligned = false;
                    ClockStatus::Ahead(lv - rv)
                } else {
                    all_aligned = false;
                    ClockStatus::Behind(rv - lv)
                }
            }
            (true, false) => {
                all_aligned = false;
                ClockStatus::LocalOnly
            }
            (false, true) => {
                all_aligned = false;
                ClockStatus::RemoteOnly
            }
            (false, false) => unreachable!(),
        };
        clocks.push((cid, status));
    }

    Ok(SyncDiagnosis {
        clocks,
        status: if all_aligned {
            SyncOverallStatus::Aligned
        } else {
            SyncOverallStatus::Diverged
        },
    })
}

impl fmt::Display for DocAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_doc_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_address_file_roundtrip() {
        let addr = DocAddress::File {
            project_hash: "abc123".to_string(),
            rel_path: "src/main.rs".to_string(),
        };
        let s = addr.to_doc_name();
        assert_eq!(s, "file:abc123/src/main.rs");
        let parsed = DocAddress::parse(&s).unwrap();
        assert_eq!(parsed, addr);
    }

    #[test]
    fn doc_address_kb_roundtrip() {
        let addr = DocAddress::KbNode {
            node_id: "concept:buffer".to_string(),
        };
        let s = addr.to_doc_name();
        assert_eq!(s, "kb:concept:buffer");
        let parsed = DocAddress::parse(&s).unwrap();
        assert_eq!(parsed, addr);
    }

    #[test]
    fn doc_address_shared_roundtrip() {
        let addr = DocAddress::Shared {
            name: "scratch-1".to_string(),
        };
        let s = addr.to_doc_name();
        assert_eq!(s, "shared:scratch-1");
        let parsed = DocAddress::parse(&s).unwrap();
        assert_eq!(parsed, addr);
    }

    #[test]
    fn doc_address_parse_invalid() {
        assert!(DocAddress::parse("").is_none());
        assert!(DocAddress::parse("unknown:foo").is_none());
        assert!(DocAddress::parse("file:").is_none());
        assert!(DocAddress::parse("file:hash").is_none()); // no slash
        assert!(DocAddress::parse("file:/path").is_none()); // empty hash
        assert!(DocAddress::parse("file:hash/").is_none()); // empty path
        assert!(DocAddress::parse("kb:").is_none());
        assert!(DocAddress::parse("shared:").is_none());
    }

    #[test]
    fn compare_state_vectors_aligned() {
        use yrs::{updates::encoder::Encode, ReadTxn, Text, Transact};
        let doc = yrs::Doc::with_client_id(1);
        let text = doc.get_or_insert_text("t");
        {
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, "hello");
        }
        let sv = {
            let txn = doc.transact();
            txn.state_vector().encode_v1()
        };
        // Same sv on both sides → aligned.
        let diag = compare_state_vectors(&sv, &sv).unwrap();
        assert_eq!(diag.status, SyncOverallStatus::Aligned);
        assert!(!diag.clocks.is_empty());
    }

    #[test]
    fn compare_state_vectors_diverged() {
        use yrs::{updates::encoder::Encode, ReadTxn, Text, Transact};
        let doc_a = yrs::Doc::with_client_id(1);
        let doc_b = yrs::Doc::with_client_id(2);
        let text_a = doc_a.get_or_insert_text("t");
        let text_b = doc_b.get_or_insert_text("t");
        {
            let mut txn = doc_a.transact_mut();
            text_a.insert(&mut txn, 0, "aaa");
        }
        {
            let mut txn = doc_b.transact_mut();
            text_b.insert(&mut txn, 0, "bbb");
        }
        let sv_a = doc_a.transact().state_vector().encode_v1();
        let sv_b = doc_b.transact().state_vector().encode_v1();

        let diag = compare_state_vectors(&sv_a, &sv_b).unwrap();
        assert_eq!(diag.status, SyncOverallStatus::Diverged);
        // Should have entries for both client IDs.
        assert!(diag.clocks.len() >= 2);
    }

    #[test]
    fn doc_address_display() {
        let addr = DocAddress::Shared {
            name: "test".to_string(),
        };
        assert_eq!(format!("{addr}"), "shared:test");
    }
}
