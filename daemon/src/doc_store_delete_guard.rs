//! ADR-113 E7: a node that holds a publication-review revision, in any state,
//! is never deleted; a document leaves force only by derogation or a successor.
//!
//! **Where it runs.** Inside `DocStore::apply_update` and `replace_doc`, on
//! every write to a `kbc:` collection, under the KB's switch gate and the
//! document's locks, like the E2E-switch guard beside it (`doc_store_e2e_switch`).
//! Removing a node from a KB is removing it from the collection's node list,
//! and every write to a collection passes one of those two, so one guard covers
//! every delete path: the hub's `kb/collection_node_remove`, a client's raw
//! collection update (the editor's delete), generic `sync/update` and
//! `sync/share`, a mesh peer's update, and seed removal (which also checks
//! first, so an update is refused before it writes anything).
//!
//! **What a refusal does.** The whole write is refused before it is logged,
//! and the in-memory document is reloaded from storage: it is never
//! half-applied. A mesh session ends at it (`dialer`), as at a refused switch.
//!
//! **How it decides.** Only when the KB holds any revision does it look at the
//! node list (no decode on a KB that has never been reviewed). It compares the
//! node ids before and after the write; the ids the write removes are checked
//! against the revisions table. A failed read refuses (fail closed).
//!
//! @ai-caution: [security] A review request takes the same switch gate across
//! its freeze and commit, so a request and a removal serialize: the check
//! below sees every revision a request commits first. Do not move the guard out
//! from under that gate.

use std::collections::BTreeSet;

use mae_sync::kb::KbCollectionDoc;
use mae_sync::text::TextSync;
use tracing::warn;

use super::{DocEntry, DocStore};
use crate::storage::StorageError;

/// The node list of KB `kb`'s collection before a write, when there is
/// anything to guard.
pub(super) struct RemovalBaseline {
    kb: String,
    nodes: BTreeSet<String>,
}

impl DocStore {
    /// Before a write to KB `kb`'s collection (`None`: not a collection), held
    /// in `sync`: its node list, if the KB holds any revision. A failed read
    /// of the revisions guards anyway.
    pub(super) async fn removal_baseline(
        &self,
        kb: Option<&str>,
        sync: &TextSync,
    ) -> Option<RemovalBaseline> {
        let kb = kb?.to_string();
        let probe = kb.clone();
        let held = self
            .review_read(move |c| crate::review::store::has_revisions(c, &probe))
            .await;
        if let Ok(false) = held {
            return None;
        }
        Some(RemovalBaseline {
            kb,
            nodes: KbCollectionDoc::node_ids_in(sync.doc()),
        })
    }

    /// The refusal for a write that left the collection as `after`, if it
    /// removes a node holding a revision.
    async fn removal_refusal(
        &self,
        base: RemovalBaseline,
        after: &TextSync,
    ) -> Option<StorageError> {
        let after = KbCollectionDoc::node_ids_in(after.doc());
        let removed: Vec<String> = base.nodes.difference(&after).cloned().collect();
        if removed.is_empty() {
            return None;
        }
        let kb = base.kb.clone();
        let held = self
            .review_read(move |c| crate::review::store::nodes_with_revisions(c, &kb, &removed))
            .await;
        let nodes = match held {
            Ok(held) if held.is_empty() => return None,
            Ok(held) => held,
            Err(e) => {
                warn!(kb_id = %base.kb, error = %e, "delete guard: revisions unreadable; refused");
                Vec::new()
            }
        };
        warn!(kb_id = %base.kb, ?nodes, "refused a write removing nodes that hold revisions");
        Some(StorageError::NodeHasRevisions {
            kb_id: base.kb,
            nodes,
        })
    }

    /// After an update was applied in memory to `doc`: refuse it, putting
    /// `doc` back to what storage holds, if it removed a guarded node.
    pub(super) async fn refuse_applied_removal(
        &self,
        doc_name: &str,
        doc: &mut DocEntry,
        base: Option<RemovalBaseline>,
    ) -> Result<(), StorageError> {
        let Some(base) = base else { return Ok(()) };
        match self.removal_refusal(base, &doc.sync).await {
            Some(refused) => {
                self.restore_from_storage(doc_name, doc).await?;
                Err(refused)
            }
            None => Ok(()),
        }
    }

    /// For a replace of KB `kb`'s collection (`None`: not a collection) by
    /// `probe`, over `before` (`None`: no document yet): the switch refusal
    /// (`doc_store_e2e_switch`), then the removal refusal.
    pub(super) async fn refuse_replacing(
        &self,
        kb: Option<&str>,
        before: Option<&[u8]>,
        probe: &TextSync,
    ) -> Result<(), StorageError> {
        self.refuse_replacing_switch(kb, before, probe).await?;
        self.refuse_replacing_removal(kb, before, probe).await
    }

    /// [`Self::refuse_replacing`]'s removal half.
    async fn refuse_replacing_removal(
        &self,
        kb: Option<&str>,
        before: Option<&[u8]>,
        probe: &TextSync,
    ) -> Result<(), StorageError> {
        let Some(before) = before else { return Ok(()) };
        let mut old = TextSync::empty_relay();
        old.apply_update(before)
            .map_err(|e| StorageError::Sqlite(format!("delete guard: stored state: {e}")))?;
        match self.removal_baseline(kb, &old).await {
            Some(base) => match self.removal_refusal(base, probe).await {
                Some(refused) => Err(refused),
                None => Ok(()),
            },
            None => Ok(()),
        }
    }
}
