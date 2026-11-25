//! The blobs a KB references (ADR-115 D5): the live set a sweep marks before
//! it may drop anything, derived from CRDT truth and stored history, never
//! from the Cozo projection (which can drift).
//!
//! Holders covered here, the ones that exist today:
//! - every node document of the KB, whether or not its collection still lists
//!   it (a node removed from the manifest keeps its document until it is
//!   purged, which is D5's "a node in the bin");
//! - every ADR-116 history version that still holds content. A pruned version
//!   has dropped its content, map included, so it holds nothing.
//!
//! Not covered, because they are not built: ADR-113 revisions and operator
//! holds. Their builders must add them here before a sweep runs against a KB
//! that has them. The grace period for unbound uploads is the sweep's own
//! (it reads the store's upload times, P1).
//!
//! A node document is read by the sweep's own reader
//! ([`KbNodeDoc::referenced_hashes`]), which fails safe: an entry the display
//! reader rejects (a bad NAME, a size stored as a string, a newer peer's
//! shape) still counts when it holds a hash, wherever in the entry it is. A
//! document past that reader's bounds is listed unreadable. A history version holds only
//! entries its canonical encoding accepts, so its map is read as it is.
//!
//! @ai-caution: [data-loss] A holder that cannot be read is listed in
//! `unreadable`, never skipped: a missed reference is a deleted image. A sweep
//! must collect nothing in a KB whose set is not [`ReferencedBlobs::complete`].
//! Never read a node document here through the tolerant display reader
//! (`attachments()`): it drops what it rejects.

use std::collections::BTreeSet;

use mae_sync::kb::KbNodeDoc;
use mae_sync::DocAddress;

use crate::doc_store::DocStore;
use crate::storage::StorageError;

/// What a KB references, and what could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReferencedBlobs {
    /// Every sha256 some holder binds a NAME to.
    pub hashes: BTreeSet<String>,
    /// Holders that could not be read: a node document's name, or
    /// `history <node> #<seq>`.
    pub unreadable: Vec<String>,
}

impl ReferencedBlobs {
    /// Whether every holder was read, so the set may be trusted to collect.
    pub fn complete(&self) -> bool {
        self.unreadable.is_empty()
    }
}

impl DocStore {
    /// The node documents of `kb_id`, in memory or only on disk, sorted.
    async fn kb_node_doc_names(&self, kb_id: &str) -> Result<Vec<String>, StorageError> {
        let mut names: BTreeSet<String> = self.document_names().await.into_iter().collect();
        names.extend(self.storage.list_documents().await?);
        Ok(names
            .into_iter()
            .filter(|n| {
                matches!(DocAddress::parse(n), Some(DocAddress::KbNode { kb_id: k, .. }) if k == kb_id)
            })
            .collect())
    }

    /// Every blob `kb_id` references (ADR-115 D5), for the sweep.
    pub async fn referenced_blobs(&self, kb_id: &str) -> Result<ReferencedBlobs, StorageError> {
        let mut out = ReferencedBlobs::default();
        for name in self.kb_node_doc_names(kb_id).await? {
            let read = match self.encode_state_and_sv(&name).await {
                Ok((state, _)) => KbNodeDoc::from_bytes(&state).ok(),
                Err(_) => None,
            };
            // A document the sweep's reader cannot walk in full (too deep,
            // too many values, a subdocument) is unreadable, not empty.
            match read.map(|doc| doc.referenced_hashes()) {
                Some(Ok(hashes)) => out.hashes.extend(hashes),
                Some(Err(_)) | None => out.unreadable.push(name),
            }
        }
        for node_id in self.history_nodes(kb_id).await? {
            for entry in self.history_list(kb_id, &node_id).await? {
                match entry.node() {
                    None => {}
                    Some(Ok(node)) => out
                        .hashes
                        .extend(node.attachments.into_values().map(|a| a.sha256)),
                    Some(Err(_)) => out
                        .unreadable
                        .push(format!("history {node_id} #{}", entry.seq)),
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "attachment_refs_tests.rs"]
mod tests;
