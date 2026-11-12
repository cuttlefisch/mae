//! Node version history (ADR-116): the record, its canonical encoding, the
//! hash chain, verification and the retention evaluation. No I/O: the daemon's
//! `state.db` tables and the editor's Cozo `node_versions` are adapters over
//! this one module (D1, principle #8).
//!
//! A version is a checkpoint of a node's whole materialized content (D2). Its
//! entry records who changed the node and when (D3) and names its
//! predecessor by hash (D4):
//!
//! `entry_hash = SHA-256(canonical("maehistory/v1", prev_hash, every field
//! but content and entry_hash))`
//!
//! The content enters only through `content_hash`, the full digest of the
//! materialized node ([`MaterializedNode::digest`]). That is what lets
//! pruning drop an entry's content and keep the chain verifying (D5).
//!
//! What the chain alone cannot show: a removed LAST entry, or a whole history
//! rewritten by someone who can recompute every hash. The first needs a head
//! ([`verify_chain`] takes one; [`kb_head`] makes it), the second a head kept
//! outside the store (ADR-116 D4, "the honest limit").

mod chain;
mod codec;
mod diff;
mod head;
mod retention;

pub use chain::{kb_head, seal, verify_chain, Broken, Draft, SealError, VerifyError};
pub use codec::{decode_content, encode_content, ContentError};
pub use diff::{diff, FieldChange};
pub use head::{verify_against_head, HeadError, SignedHead, Tip};
pub use retention::{
    prune_candidates, Exemptions, Keep, Policy, PolicyError, MAX_VERSIONS_FLOOR, MIN_DAYS_FLOOR,
};

use super::MaterializedNode;

/// A SHA-256 digest.
pub type Hash = [u8; 32];

/// The `prev_hash` of a node's first entry.
pub const GENESIS: Hash = [0; 32];

/// The class of an editor (ADR-113 A1): how far the author of a change is
/// known. Stored as text; see [`EditorKind::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EditorKind {
    Principal,
    Unauthenticated,
    /// A relay whose update carried NO verified signer: the author is unknown.
    MeshPeer,
    /// A relay whose update was signed; the signer has its own `Principal` row.
    SignedRelay,
    Internal,
}

impl EditorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Principal => "principal",
            Self::Unauthenticated => "unauthenticated",
            Self::MeshPeer => "mesh-peer",
            Self::SignedRelay => "signed-relay",
            Self::Internal => "internal",
        }
    }

    /// The inverse of [`Self::as_str`]. `None` for a value this build does not
    /// know, which a reader reports as an error rather than guessing a kind.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "principal" => Some(Self::Principal),
            "unauthenticated" => Some(Self::Unauthenticated),
            "mesh-peer" => Some(Self::MeshPeer),
            "signed-relay" => Some(Self::SignedRelay),
            "internal" => Some(Self::Internal),
            _ => None,
        }
    }

    /// Whether a row of this kind leaves the authors of a change unknown.
    /// Only a principal (a known author) and a signed relay (whose signer is
    /// recorded as a principal) do not.
    pub fn hides_an_author(self) -> bool {
        !matches!(self, Self::Principal | Self::SignedRelay)
    }
}

/// Why a version was cut (D2), or a `Pruned` audit entry (D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VersionKind {
    Edit,
    Named,
    Rollback,
    Import,
    Seed,
    Rebirth,
    Delete,
    /// An audit entry recording a pruning pass. It carries no content.
    Pruned,
}

impl VersionKind {
    pub const ALL: [VersionKind; 8] = [
        Self::Edit,
        Self::Named,
        Self::Rollback,
        Self::Import,
        Self::Seed,
        Self::Rebirth,
        Self::Delete,
        Self::Pruned,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Named => "named",
            Self::Rollback => "rollback",
            Self::Import => "import",
            Self::Seed => "seed",
            Self::Rebirth => "rebirth",
            Self::Delete => "delete",
            Self::Pruned => "pruned",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// One principal who edited the node since the previous version.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Editor {
    pub kind: EditorKind,
    pub principal: String,
}

/// One version of one node (D3). Made by [`seal`], checked by [`verify_chain`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub kb_id: String,
    pub node_id: String,
    /// Per node, gapless, from 1.
    pub seq: u64,
    pub kind: VersionKind,
    /// Unix seconds, UTC, from the daemon clock. Surfaces render it as RFC 3339;
    /// the record keeps the instant, which has one encoding.
    pub created_at: i64,
    /// Sorted and without duplicates.
    pub editors: Vec<Editor>,
    /// The WAL range the version covers.
    pub wal_from: u64,
    pub wal_to: u64,
    pub summary: Option<String>,
    /// Set exactly on a `Named` version.
    pub name: Option<String>,
    /// Set exactly on a `Rollback`: the `seq` it restored.
    pub restored_from: Option<u64>,
    /// The canonical encoding of the node ([`encode_content`]). `None` once
    /// pruned, and always on a `Pruned` audit entry.
    pub content: Option<Vec<u8>>,
    /// [`MaterializedNode::digest`] of the content; [`GENESIS`] on a `Pruned`
    /// audit entry.
    pub content_hash: Hash,
    pub prev_hash: Hash,
    pub entry_hash: Hash,
}

impl HistoryEntry {
    /// Whether the entry's content is gone (pruned, or never held).
    pub fn is_pruned(&self) -> bool {
        self.content.is_none()
    }

    /// Drops the content and keeps everything else: the chain still verifies,
    /// and who changed what and when stays (D5).
    pub fn prune(&mut self) {
        self.content = None;
    }

    /// The node as this version recorded it. `None` once pruned.
    pub fn node(&self) -> Option<Result<MaterializedNode, ContentError>> {
        self.content.as_deref().map(decode_content)
    }
}
