//! Sealing an entry onto a node's chain, verifying a chain, and the KB head.

use sha2::{Digest, Sha256};

use super::codec::{entry_preimage, is_genesis, Writer};
use super::{
    decode_content, encode_content, Editor, Hash, HistoryEntry, HistoryKey, MaterializedNode,
    VersionKind, GENESIS,
};

/// What a caller supplies for a new version; [`seal`] adds the sequence and
/// the hashes.
#[derive(Debug, Clone)]
pub struct Draft {
    pub kind: VersionKind,
    pub created_at: i64,
    pub editors: Vec<Editor>,
    pub wal_from: u64,
    pub wal_to: u64,
    pub summary: Option<String>,
    pub name: Option<String>,
    pub restored_from: Option<u64>,
    /// The node after the edits the version covers. `None` exactly on a
    /// `Pruned` audit entry.
    pub content: Option<MaterializedNode>,
}

/// Why [`seal`] refused a draft: it would make an entry no verifier accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealError(pub String);

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What failed, at which `seq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError {
    pub seq: u64,
    pub broken: Broken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Broken {
    /// The entry is not the next in sequence: one was removed, reordered or
    /// renumbered.
    Seq,
    /// The entry names another KB or node than its chain.
    Ids,
    /// `prev_hash` is not the predecessor's `entry_hash`.
    Link,
    /// `entry_hash` is not the hash of the entry's fields.
    EntryHash,
    /// The content is not a canonical node encoding, or is another node.
    Content,
    /// The content's keyed hash is not `content_hash`.
    ContentHash,
    /// A combination [`seal`] never writes (a `Named` version with no name, a
    /// `Pruned` audit entry holding content, unsorted editors …).
    Shape,
    /// The chain does not end at the head it was checked against.
    Head,
}

impl Broken {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Seq => "seq",
            Self::Ids => "ids",
            Self::Link => "link",
            Self::EntryHash => "entry_hash",
            Self::Content => "content",
            Self::ContentHash => "content_hash",
            Self::Shape => "shape",
            Self::Head => "head",
        }
    }
}

/// The entry `draft` makes as the next version of `node_id` in `kb_id`, after
/// `prev` (`None` for the node's first), its content hashed under the KB's
/// history `key`.
pub fn seal(
    kb_id: &str,
    node_id: &str,
    prev: Option<&HistoryEntry>,
    draft: Draft,
    key: &HistoryKey,
) -> Result<HistoryEntry, SealError> {
    if let Some(p) = prev {
        if p.kb_id != kb_id || p.node_id != node_id {
            return Err(SealError("the previous entry is another node's".into()));
        }
    }
    if let Some(node) = &draft.content {
        if node.id != node_id {
            return Err(SealError(format!("the content is node {:?}", node.id)));
        }
    }
    let mut editors = draft.editors;
    editors.sort();
    editors.dedup();
    let (content, content_hash) = match &draft.content {
        Some(node) => (Some(encode_content(node)), key.content_hash(node)),
        None => (None, GENESIS),
    };
    let mut entry = HistoryEntry {
        kb_id: kb_id.to_string(),
        node_id: node_id.to_string(),
        seq: prev.map_or(1, |p| p.seq + 1),
        kind: draft.kind,
        created_at: draft.created_at,
        editors,
        wal_from: draft.wal_from,
        wal_to: draft.wal_to,
        summary: draft.summary,
        name: draft.name,
        restored_from: draft.restored_from,
        content,
        content_hash,
        prev_hash: prev.map_or(GENESIS, |p| p.entry_hash),
        entry_hash: GENESIS,
    };
    if let Some(why) = shape_fault(&entry) {
        return Err(SealError(why.to_string()));
    }
    entry.entry_hash = entry.compute_hash();
    Ok(entry)
}

impl HistoryEntry {
    /// What `entry_hash` must be for the entry's fields and `prev_hash`. Not a
    /// secret: anyone holding the store can recompute it, which is why a chain
    /// is checked against a head kept elsewhere (D4, "the honest limit").
    pub fn compute_hash(&self) -> Hash {
        Sha256::digest(entry_preimage(self)).into()
    }
}

/// Why `e` is a combination [`seal`] never writes, if it is one. Content may
/// be absent from any entry (pruning); what it may not do is contradict the
/// entry's kind.
fn shape_fault(e: &HistoryEntry) -> Option<&'static str> {
    let audit = e.kind == VersionKind::Pruned;
    if audit && (e.content.is_some() || !is_genesis(&e.content_hash)) {
        return Some("a pruned audit entry holds no content");
    }
    if !audit && is_genesis(&e.content_hash) {
        return Some("a version needs content");
    }
    if (e.kind == VersionKind::Named) != e.name.is_some() {
        return Some("a name is set exactly on a named version");
    }
    if (e.kind == VersionKind::Rollback) != e.restored_from.is_some() {
        return Some("restored_from is set exactly on a rollback");
    }
    if e.restored_from.is_some_and(|s| s == 0 || s >= e.seq) {
        return Some("a rollback restores an earlier version");
    }
    if e.wal_from > e.wal_to {
        return Some("the WAL range is reversed");
    }
    if !e.editors.windows(2).all(|w| w[0] < w[1]) {
        return Some("editors are sorted and distinct");
    }
    None
}

/// Checks a node's whole chain, in `seq` order, and that it ends at `head`
/// (`(seq, entry_hash)` of its last entry, `(0, GENESIS)` for none) when one
/// is given. Without a head, a removed last entry cannot be seen.
///
/// With the KB's history `key`, each held content is checked too: canonical,
/// this node's, and hashing to its `content_hash`. Without it (an E2E KB after
/// the switch, whose content the checker cannot read) the chain is checked and
/// the content is not.
pub fn verify_chain(
    kb_id: &str,
    node_id: &str,
    entries: &[HistoryEntry],
    head: Option<(u64, Hash)>,
    key: Option<&HistoryKey>,
) -> Result<(), VerifyError> {
    let mut prev = GENESIS;
    for (i, e) in entries.iter().enumerate() {
        let fail = |broken| VerifyError { seq: e.seq, broken };
        if e.seq != i as u64 + 1 {
            return Err(fail(Broken::Seq));
        }
        if e.kb_id != kb_id || e.node_id != node_id {
            return Err(fail(Broken::Ids));
        }
        if e.prev_hash != prev {
            return Err(fail(Broken::Link));
        }
        if shape_fault(e).is_some() {
            return Err(fail(Broken::Shape));
        }
        if e.compute_hash() != e.entry_hash {
            return Err(fail(Broken::EntryHash));
        }
        if let (Some(bytes), Some(key)) = (&e.content, key) {
            let node = decode_content(bytes).map_err(|_| fail(Broken::Content))?;
            if node.id != node_id {
                return Err(fail(Broken::Content));
            }
            if key.content_hash(&node) != e.content_hash {
                return Err(fail(Broken::ContentHash));
            }
        }
        prev = e.entry_hash;
    }
    match head {
        Some((seq, hash)) if (seq, hash) != (entries.len() as u64, prev) => Err(VerifyError {
            seq,
            broken: Broken::Head,
        }),
        _ => Ok(()),
    }
}

/// The KB's history head (D4): one digest over every node's
/// `(node_id, seq, entry_hash)`, in node order, whatever order they are given
/// in. The daemon signs this; a head kept elsewhere is what shows a
/// rewritten history.
pub fn kb_head<'a>(
    kb_id: &str,
    nodes: impl IntoIterator<Item = (&'a str, u64, Hash)>,
) -> Result<Hash, SealError> {
    let mut nodes: Vec<(&str, u64, Hash)> = nodes.into_iter().collect();
    nodes.sort_by(|a, b| a.0.cmp(b.0));
    if nodes.windows(2).any(|w| w[0].0 == w[1].0) {
        return Err(SealError("a node appears twice in the head".into()));
    }
    let mut w = Writer::default();
    w.bytes(b"maehistoryhead/v1");
    w.bytes(kb_id.as_bytes());
    w.u64(nodes.len() as u64);
    for (node_id, seq, hash) in nodes {
        w.bytes(node_id.as_bytes());
        w.u64(seq);
        w.bytes(&hash);
    }
    Ok(Sha256::digest(&w.0).into())
}
