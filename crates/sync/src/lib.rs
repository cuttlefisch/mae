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
    fn doc_address_display() {
        let addr = DocAddress::Shared {
            name: "test".to_string(),
        };
        assert_eq!(format!("{addr}"), "shared:test");
    }
}
