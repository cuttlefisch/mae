//! mae-sync: Collaborative state synchronization via yrs (YATA CRDT).
//!
//! Wraps yrs with MAE-specific document schemas and provides a bridge
//! between yrs YText and ropey Rope for rendering.

pub mod awareness;
pub mod content_ops;
pub mod encoding;
pub mod kb;
pub mod membership;
pub mod text;
pub mod wire;

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
/// KB collection manifest, or arbitrary shared name. The string form uses
/// URI-like prefixes: `file:{project_hash}/{rel_path}`, `kb:{node_id}`,
/// `kbc:{kb_id}`, `shared:{name}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DocAddress {
    /// A file within a project, identified by project hash and relative path.
    File {
        project_hash: String,
        rel_path: String,
    },
    /// A knowledge-base node.
    KbNode { node_id: String },
    /// A KB collection manifest (node inventory for a shared KB).
    KbCollection { kb_id: String },
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
            DocAddress::KbCollection { kb_id } => format!("kbc:{kb_id}"),
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
        } else if let Some(rest) = s.strip_prefix("kbc:") {
            if rest.is_empty() {
                return None;
            }
            Some(DocAddress::KbCollection {
                kb_id: rest.to_string(),
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
            DocAddress::KbNode { .. } | DocAddress::KbCollection { .. } => {
                SavePolicy::ServerAuthoritative
            }
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

    use yrs::block::ClientID;

    let mut all_ids: std::collections::BTreeSet<ClientID> = std::collections::BTreeSet::new();
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
        clocks.push((cid.get(), status));
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

// --- WU4: Git-based project identity ---

/// Compute a stable project identity string from a project root directory.
///
/// Precedence:
/// 1. `git remote get-url origin` → normalize URL → FNV-1a hash
/// 2. `.project` TOML file `name` field
/// 3. Directory basename
/// 4. FNV-1a of absolute path (backward compat)
///
/// The returned string is suitable as the `project_hash` component of `DocAddress::File`.
pub fn compute_project_identity(project_root: &std::path::Path) -> String {
    // 1. Try git remote origin URL.
    if let Some(hash) = git_remote_identity(project_root) {
        return hash;
    }
    // 2. Try .project TOML name field.
    if let Some(name) = dotproject_name(project_root) {
        return fnv1a_hash(name.as_bytes());
    }
    // 3. Directory basename.
    if let Some(basename) = project_root.file_name() {
        let s = basename.to_string_lossy();
        if !s.is_empty() {
            return fnv1a_hash(s.as_bytes());
        }
    }
    // 4. Fallback: FNV-1a of absolute path.
    fnv1a_hash(project_root.to_string_lossy().as_bytes())
}

/// Normalize a git remote URL for stable identity:
/// - Strip `.git` suffix
/// - Strip auth (user@, user:pass@)
/// - Lowercase host
/// - Handle SSH `git@host:path` → `host/path`
fn normalize_git_url(url: &str) -> String {
    let mut s = url.trim().to_string();
    // Strip trailing .git
    if s.ends_with(".git") {
        s.truncate(s.len() - 4);
    }
    // SSH format: git@github.com:user/repo → github.com/user/repo
    if let Some(rest) = s.strip_prefix("git@") {
        s = rest.replacen(':', "/", 1);
    }
    // HTTPS: https://user:pass@host/path → host/path
    if let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
    {
        // Strip auth
        let rest = if let Some(at_pos) = rest.find('@') {
            &rest[at_pos + 1..]
        } else {
            rest
        };
        s = rest.to_string();
    }
    // Lowercase the host portion (everything before first /).
    if let Some(slash_pos) = s.find('/') {
        let (host, path) = s.split_at(slash_pos);
        s = format!("{}{}", host.to_lowercase(), path);
    } else {
        s = s.to_lowercase();
    }
    s
}

fn git_remote_identity(project_root: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(project_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout);
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let normalized = normalize_git_url(url);
    Some(fnv1a_hash(normalized.as_bytes()))
}

fn dotproject_name(project_root: &std::path::Path) -> Option<String> {
    let path = project_root.join(".project");
    let content = std::fs::read_to_string(path).ok()?;
    // Simple TOML parsing: look for `name = "..."` line.
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name") {
            let rest = rest.trim();
            if let Some(rest) = rest.strip_prefix('=') {
                let val = rest.trim().trim_matches('"').trim_matches('\'');
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

fn fnv1a_hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:012x}")
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
    fn doc_address_kb_collection_roundtrip() {
        let addr = DocAddress::KbCollection {
            kb_id: "a7f3c2d1".to_string(),
        };
        let s = addr.to_doc_name();
        assert_eq!(s, "kbc:a7f3c2d1");
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
        assert!(DocAddress::parse("kbc:").is_none());
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

    // --- WU4: Git identity tests ---

    #[test]
    fn normalize_git_url_https() {
        let url = "https://github.com/user/repo.git";
        assert_eq!(normalize_git_url(url), "github.com/user/repo");
    }

    #[test]
    fn normalize_git_url_ssh() {
        let url = "git@github.com:user/repo.git";
        assert_eq!(normalize_git_url(url), "github.com/user/repo");
    }

    #[test]
    fn normalize_git_url_with_auth() {
        let url = "https://token:x-oauth@github.com/user/repo.git";
        assert_eq!(normalize_git_url(url), "github.com/user/repo");
    }

    #[test]
    fn normalize_git_url_lowercase_host() {
        let url = "https://GitHub.COM/User/Repo";
        assert_eq!(normalize_git_url(url), "github.com/User/Repo");
    }

    #[test]
    fn same_remote_different_paths_same_identity() {
        // Two users with the same git remote should get the same identity.
        // We test normalize + hash directly since git_remote_identity requires a real repo.
        let url1 = "git@github.com:cuttlefisch/mae.git";
        let url2 = "https://github.com/cuttlefisch/mae.git";
        let h1 = fnv1a_hash(normalize_git_url(url1).as_bytes());
        let h2 = fnv1a_hash(normalize_git_url(url2).as_bytes());
        assert_eq!(h1, h2, "SSH and HTTPS should produce same identity");
    }

    #[test]
    fn dotproject_name_parses() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".project"),
            "name = \"my-project\"\nversion = \"1.0\"\n",
        )
        .unwrap();
        assert_eq!(dotproject_name(dir.path()), Some("my-project".to_string()));
    }

    #[test]
    fn dotproject_name_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(dotproject_name(dir.path()), None);
    }

    #[test]
    fn compute_project_identity_uses_basename_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("my-project");
        std::fs::create_dir(&sub).unwrap();
        let identity = compute_project_identity(&sub);
        let expected = fnv1a_hash(b"my-project");
        assert_eq!(identity, expected);
    }

    #[test]
    fn compute_project_identity_uses_dotproject() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".project"), "name = \"test-proj\"\n").unwrap();
        let identity = compute_project_identity(dir.path());
        let expected = fnv1a_hash(b"test-proj");
        assert_eq!(identity, expected);
    }

    // --- Branch coverage: compare_state_vectors ---

    #[test]
    fn compare_state_vectors_both_empty() {
        use yrs::{updates::encoder::Encode, ReadTxn, Transact};
        let doc = yrs::Doc::with_client_id(1);
        let sv = doc.transact().state_vector().encode_v1();
        let diag = compare_state_vectors(&sv, &sv).unwrap();
        assert_eq!(diag.status, SyncOverallStatus::Aligned);
        assert!(diag.clocks.is_empty());
    }

    #[test]
    fn compare_state_vectors_one_empty() {
        use yrs::{updates::encoder::Encode, ReadTxn, Text, Transact};
        let doc = yrs::Doc::with_client_id(1);
        let text = doc.get_or_insert_text("t");
        {
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, "data");
        }
        let full_sv = doc.transact().state_vector().encode_v1();
        let empty_doc = yrs::Doc::with_client_id(2);
        let empty_sv = empty_doc.transact().state_vector().encode_v1();

        // Full vs empty.
        let diag = compare_state_vectors(&full_sv, &empty_sv).unwrap();
        assert_eq!(diag.status, SyncOverallStatus::Diverged);
        assert!(diag
            .clocks
            .iter()
            .any(|(_, s)| *s == ClockStatus::LocalOnly));

        // Empty vs full.
        let diag2 = compare_state_vectors(&empty_sv, &full_sv).unwrap();
        assert_eq!(diag2.status, SyncOverallStatus::Diverged);
        assert!(diag2
            .clocks
            .iter()
            .any(|(_, s)| *s == ClockStatus::RemoteOnly));
    }

    #[test]
    fn compare_state_vectors_same_clients_different_versions() {
        use yrs::{updates::encoder::Encode, ReadTxn, Text, Transact};
        let doc_a = yrs::Doc::with_client_id(1);
        let doc_b = yrs::Doc::with_client_id(1);
        let text_a = doc_a.get_or_insert_text("t");
        let text_b = doc_b.get_or_insert_text("t");
        {
            let mut txn = doc_a.transact_mut();
            text_a.insert(&mut txn, 0, "more text");
        }
        {
            let mut txn = doc_b.transact_mut();
            text_b.insert(&mut txn, 0, "x");
        }
        let sv_a = doc_a.transact().state_vector().encode_v1();
        let sv_b = doc_b.transact().state_vector().encode_v1();
        let diag = compare_state_vectors(&sv_a, &sv_b).unwrap();
        assert_eq!(diag.status, SyncOverallStatus::Diverged);
        // Client 1 should be Ahead on A's side (more text inserted).
        assert!(diag
            .clocks
            .iter()
            .any(|(_, s)| matches!(s, ClockStatus::Ahead(_))));
    }

    // --- Branch coverage: DocAddress::parse ---

    #[test]
    fn doc_address_parse_multi_slash_path() {
        let addr = DocAddress::parse("file:hash123/src/nested/file.rs").unwrap();
        assert_eq!(
            addr,
            DocAddress::File {
                project_hash: "hash123".to_string(),
                rel_path: "src/nested/file.rs".to_string(),
            }
        );
    }

    #[test]
    fn doc_address_parse_colon_in_kb_id() {
        let addr = DocAddress::parse("kb:concept:buffer").unwrap();
        assert_eq!(
            addr,
            DocAddress::KbNode {
                node_id: "concept:buffer".to_string(),
            }
        );
    }

    #[test]
    fn doc_address_parse_very_long_string() {
        let long_path = "x".repeat(10_000);
        let input = format!("file:hash/{}", long_path);
        let addr = DocAddress::parse(&input).unwrap();
        match addr {
            DocAddress::File { rel_path, .. } => assert_eq!(rel_path.len(), 10_000),
            _ => panic!("expected File"),
        }
    }

    // --- Branch coverage: dotproject_name ---

    #[test]
    fn dotproject_name_malformed_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".project"),
            "garbage content\nno name field\n",
        )
        .unwrap();
        assert_eq!(dotproject_name(dir.path()), None);
    }

    #[test]
    fn dotproject_name_empty_value() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".project"), "name = \"\"\n").unwrap();
        assert_eq!(dotproject_name(dir.path()), None);
    }

    // --- Branch coverage: normalize_git_url ---

    #[test]
    fn normalize_git_url_no_dot_git() {
        assert_eq!(
            normalize_git_url("https://github.com/user/repo"),
            "github.com/user/repo"
        );
    }

    #[test]
    fn normalize_git_url_bare_host() {
        assert_eq!(normalize_git_url("GITHUB.COM"), "github.com");
    }

    // --- SavePolicy coverage ---

    #[test]
    fn save_policy_from_doc_address() {
        assert_eq!(
            DocAddress::File {
                project_hash: "x".into(),
                rel_path: "y".into(),
            }
            .save_policy(),
            SavePolicy::LocalFirst
        );
        assert_eq!(
            DocAddress::KbNode {
                node_id: "x".into(),
            }
            .save_policy(),
            SavePolicy::ServerAuthoritative
        );
        assert_eq!(
            DocAddress::KbCollection { kb_id: "x".into() }.save_policy(),
            SavePolicy::ServerAuthoritative
        );
        assert_eq!(
            DocAddress::Shared { name: "x".into() }.save_policy(),
            SavePolicy::Ephemeral
        );
    }
}
