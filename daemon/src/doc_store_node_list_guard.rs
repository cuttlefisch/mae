//! ADR-036 Amendment 2, G3: once the hub requires signed ops, a session's
//! write to a KB's collection may not change its node list. The node list then
//! changes only through `kb/collection_node_add`/`_remove` with a signed
//! manifest op, whose update the daemon applies as [`Writer::manifest_op`],
//! and then by exactly the change signed: a held-back raw change that the
//! daemon's update happens to release is refused with it.
//!
//! **Where it runs.** Inside `DocStore::apply_update` and `replace_doc`, under
//! the document's lock, beside the E7 delete guard (`doc_store_delete_guard`):
//! one guard covers every session path into a collection (`kb/collection_op`,
//! `sync/update`, and any route added later), and it reads the live document
//! after the write, so an update whose list change yrs held back as pending is
//! seen in whichever later write integrates it.
//!
//! **What it covers.** Writes by a session ([`Writer::guards_node_list`]). Not
//! a mesh relay (#727 part 3) and not the daemon acting for no session (seed,
//! migration, the operator's control socket).
//!
//! @ai-caution: [security] Compare the document the write leaves, never a copy
//! rebuilt from `encode_state` with the update applied: a rebuilt copy can
//! differ from what the real apply integrates (pending structs), and a check
//! outside the document's lock races the write it judges. The independent
//! review of #727 part 2 found exactly that in the first version.

use std::collections::BTreeMap;

use mae_sync::kb::KbCollectionDoc;
use mae_sync::text::TextSync;
use tracing::warn;

use super::DocStore;
use crate::doc_editors::Writer;
use crate::storage::StorageError;
use mae_sync::kb::Transport;

/// The node list KB `kb`'s collection must hold after a session's write: the
/// list before it, with the write's verified manifest change, if any.
pub(super) struct ListBaseline {
    kb: String,
    nodes: BTreeMap<String, String>,
}

impl DocStore {
    /// Before a write by `by` to KB `kb`'s collection (`None`: not a
    /// collection), held in `sync`: the node list it must leave, if guarded.
    pub(super) fn list_baseline(
        &self,
        kb: Option<&str>,
        by: &Writer<'_>,
        sync: &TextSync,
    ) -> Option<ListBaseline> {
        let kb = kb?;
        if !self.content_op_policy().require_signed(Transport::Hub) || !by.guards_node_list() {
            return None;
        }
        let before = KbCollectionDoc::nodes_in(sync.doc());
        let nodes = match by.manifest_op {
            Some(change) => change.applied_to(before),
            None => before,
        };
        Some(ListBaseline {
            kb: kb.to_string(),
            nodes,
        })
    }

    /// The refusal for a write that left the collection as `after`, if its
    /// node list is not the one `base` expects.
    pub(super) fn list_refusal(
        base: Option<ListBaseline>,
        after: &TextSync,
    ) -> Option<StorageError> {
        let base = base?;
        if KbCollectionDoc::nodes_in(after.doc()) == base.nodes {
            return None;
        }
        warn!(kb_id = %base.kb, "refused a session's unsigned change to a KB's node list");
        Some(StorageError::UnsignedNodeListChange { kb_id: base.kb })
    }

    /// For a replace by `by` of KB `kb`'s collection (`None`: not a collection)
    /// holding `before` (`None`: no document yet, a KB's creation) by `probe`.
    pub(super) fn refuse_replacing_list(
        &self,
        kb: Option<&str>,
        by: &Writer<'_>,
        before: Option<&[u8]>,
        probe: &TextSync,
    ) -> Result<(), StorageError> {
        let Some(before) = before else { return Ok(()) };
        let mut old = TextSync::empty_relay();
        old.apply_update(before)
            .map_err(|e| StorageError::Sqlite(format!("node-list guard: stored state: {e}")))?;
        match Self::list_refusal(self.list_baseline(kb, by, &old), probe) {
            Some(refused) => Err(refused),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "doc_store_node_list_guard_tests.rs"]
mod tests;
