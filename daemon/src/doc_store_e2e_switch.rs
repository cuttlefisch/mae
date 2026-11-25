//! The one guard on a KB's switch to end-to-end encryption (ADR-116
//! Amendment 1, ADR-113 O17): a write that would switch `kbc:{kb_id}` to E2E
//! is refused while this daemon holds plaintext the switch would leave
//! readable on a daemon that must hold none (ADR-037): any publication-review
//! revision (its frozen source is plaintext and never deleted, ADR-113 E7), or
//! version history that still holds content.
//!
//! **Where it runs.** Inside `DocStore::apply_update` and `replace_doc` (via
//! `share_doc` and `restore_doc`), under the document's locks, on the state
//! the write would leave. Every write to a `kbc:` document goes through one of
//! those two, so every path is covered without a check of its own: the hub's
//! `kb/collection_op` and the methods that persist a collection, generic
//! `sync/update` and `sync/share`, `kb/share`, a mesh peer's update (the
//! dialer), the legacy-collection migration, seed apply and update, a
//! checkpoint import, the control socket and `anchor_cli`.
//!
//! **What a refusal does.** The whole update is refused and nothing of it is
//! logged or applied (the in-memory document is reloaded from storage, as for
//! a size refusal); the refusal is logged and counted
//! ([`DocStore::e2e_switches_refused`]). An update that carries the switch
//! together with other changes is refused with them: it is never half-applied.
//! For a mesh peer's update that means this replica's collection stops
//! converging with that peer's until the KB is dealt with, which is the price
//! of not leaving plaintext readable here.
//!
//! **How it decides.** The daemon does not interpret the update: it reads the
//! collection before and after, and asks whether the KB is E2E in each, by
//! [`DocStore::collection_is_e2e`] (the signed op-log or the collection's own
//! flag). A state that does not decode as a collection is not E2E to any
//! reader either, so it is not a switch.
//!
//! @ai-caution: [security] Lock order: the KB's switch gate, then the
//! document's replace gate, then its entry lock. A review request takes the
//! same switch gate across its E2E re-check, its freezes and its commit, so a
//! request and a switch serialize: the switch sees every revision a request
//! commits, and a request sees every switch that lands first. Never take the
//! switch gate while holding a replace gate or an entry lock.

use std::sync::atomic::Ordering;

use mae_sync::kb::KbCollectionDoc;
use mae_sync::text::TextSync;
use tokio::sync::MutexGuard;
use tracing::warn;

use super::DocStore;
use crate::storage::{PlaintextHeld, StorageError};

/// Number of [`DocStore`] switch-gate stripes.
pub(super) const SWITCH_GATE_STRIPES: usize = 64;

/// The KB a `kbc:` document belongs to; `None` for any other document.
fn collection_kb(doc_name: &str) -> Option<String> {
    match mae_sync::DocAddress::parse(doc_name) {
        Some(mae_sync::DocAddress::KbCollection { kb_id }) => Some(kb_id),
        _ => None,
    }
}

impl DocStore {
    /// The switch gate of `kb_id`: held by every write to its collection and
    /// by a review request from its E2E re-check to its commit. Striped, so it
    /// stays bounded; two KBs on one stripe only wait on each other.
    pub(crate) async fn e2e_switch_gate(&self, kb_id: &str) -> MutexGuard<'_, ()> {
        let hash = kb_id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
        });
        self.e2e_switch_gates[hash as usize % self.e2e_switch_gates.len()]
            .lock()
            .await
    }

    /// For a `kbc:` document: its KB, and that KB's switch gate, held.
    pub(super) async fn collection_switch_gate(
        &self,
        doc_name: &str,
    ) -> Option<(String, MutexGuard<'_, ()>)> {
        let kb = collection_kb(doc_name)?;
        let gate = self.e2e_switch_gate(&kb).await;
        Some((kb, gate))
    }

    /// The content trust anchor of `kb_id`, given its collection: the
    /// registered anchor, else this daemon's own key when it owns the KB.
    pub async fn content_anchor_in(&self, kb_id: &str, coll: &KbCollectionDoc) -> Option<[u8; 32]> {
        if let Some(a) = self.kb_anchor(kb_id).await {
            return Some(a);
        }
        let signer = self.signer()?;
        (!coll.owner().is_empty() && coll.owner() == signer.fingerprint())
            .then(|| signer.public().to_bytes())
    }

    /// Whether `kb_id`'s content is E2E encrypted, by its signed op-log or
    /// its collection's own flag (either one: fail closed).
    pub async fn collection_is_e2e(&self, kb_id: &str, coll: &KbCollectionDoc) -> bool {
        let signed_e2e = match self.content_anchor_in(kb_id, coll).await {
            Some(anchor) => matches!(
                mae_sync::membership::derive_encryption(&coll.oplog_ops(), &anchor),
                mae_sync::kb::Encryption::E2e
            ),
            None => false,
        };
        signed_e2e || matches!(coll.encryption(), mae_sync::kb::Encryption::E2e)
    }

    async fn state_is_e2e(&self, kb_id: &str, state: &[u8]) -> bool {
        match KbCollectionDoc::from_bytes(state) {
            Ok(coll) => self.collection_is_e2e(kb_id, &coll).await,
            Err(_) => false,
        }
    }

    /// The refusal for a write taking `kbc:{kb_id}` from `before` to `after`,
    /// if it switches the KB to E2E while plaintext is held. Counted and
    /// logged here, so every path reports it the same way.
    pub(super) async fn e2e_switch_refusal(
        &self,
        kb_id: &str,
        before: &[u8],
        after: &[u8],
    ) -> Option<StorageError> {
        // `after` first: a plaintext KB's write, the common case, decodes once.
        if !self.state_is_e2e(kb_id, after).await || self.state_is_e2e(kb_id, before).await {
            return None;
        }
        let held = self.plaintext_held(kb_id).await?;
        self.e2e_switches_refused.fetch_add(1, Ordering::Relaxed);
        warn!(
            kb_id,
            ?held,
            "refused a write that would switch the KB to E2E over plaintext"
        );
        Some(StorageError::E2eSwitchRefused {
            kb_id: kb_id.to_string(),
            held,
        })
    }

    /// For a replace of KB `kb`'s collection (`None`: not a collection) by
    /// `probe`, over `before` (`None`: no document yet): the switch refusal.
    pub(super) async fn refuse_replacing_switch(
        &self,
        kb: Option<&str>,
        before: Option<&[u8]>,
        probe: &TextSync,
    ) -> Result<(), StorageError> {
        let Some(kb) = kb else { return Ok(()) };
        let empty = TextSync::empty_relay().encode_state();
        let before = before.unwrap_or(&empty);
        match self
            .e2e_switch_refusal(kb, before, &probe.encode_state())
            .await
        {
            Some(refused) => Err(refused),
            None => Ok(()),
        }
    }

    /// What plaintext `kb_id` holds that a switch would leave readable.
    async fn plaintext_held(&self, kb_id: &str) -> Option<PlaintextHeld> {
        let kb = kb_id.to_string();
        match self
            .review_read(move |c| crate::review::store::has_revisions(c, &kb))
            .await
        {
            Ok(true) => return Some(PlaintextHeld::Revisions),
            Ok(false) => {}
            Err(e) => {
                warn!(kb_id, error = %e, "E2E switch: review revisions unreadable; refusing");
                return Some(PlaintextHeld::Unreadable);
            }
        }
        match self.storage.history_holds_content(kb_id).await {
            Ok(false) => None,
            Ok(true) => Some(PlaintextHeld::History),
            Err(e) => {
                warn!(kb_id, error = %e, "E2E switch: version history unreadable; refusing");
                Some(PlaintextHeld::Unreadable)
            }
        }
    }

    /// How many writes this store refused for switching a KB to E2E over
    /// plaintext since it opened (a mesh peer's included).
    pub fn e2e_switches_refused(&self) -> u64 {
        self.e2e_switches_refused.load(Ordering::Relaxed)
    }
}
