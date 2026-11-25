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
//! **How it decides.** The daemon does not interpret the update: it asks
//! whether the KB is E2E after the write and before it, by
//! [`DocStore::collection_is_e2e`] (the signed op-log or the collection's own
//! flag). A state that does not decode as a collection is not E2E to any
//! reader either, so it is not a switch. On `apply_update` it asks only when
//! the write changed something that reading depends on (the op-log, the flag,
//! the owner: [`KbCollectionDoc::watch_encryption_inputs`], which also sees
//! blocks a later update integrates from pending); otherwise the answer is
//! the same before and after and nothing is decoded. The state before is then
//! read back from storage, which holds exactly the document minus this update
//! (`restore_from_storage`).
//!
//! **The anchor.** "E2E by the signed op-log" needs the KB's content anchor,
//! which lives outside the collection: while it is unknown, an owner-signed
//! `SetEncryption` reads as plaintext, and a write carrying it without the
//! display flag is accepted. Recording the anchor afterwards would switch the
//! KB with no write passing this guard, so both registrations
//! ([`DocStore::set_kb_anchor`], [`DocStore::record_kb_anchor_if_absent`])
//! meet the same check under the same gate: an anchor that would make a KB
//! holding plaintext read as E2E is refused.
//!
//! **Logging.** Every refusal is counted; the warning is rate-limited per KB
//! ([`REFUSAL_LOG_INTERVAL`]) and carries the number suppressed since the last
//! one, so a client or peer that keeps offering a refused switch cannot flood
//! the log. A mesh session ends at its first refusal (`dialer`).
//!
//! @ai-caution: [security] Lock order: the KB's switch gate, then the
//! document's replace gate, then its entry lock. A review request takes the
//! same switch gate across its E2E re-check, its freezes and its commit, so a
//! request and a switch serialize: the switch sees every revision a request
//! commits, and a request sees every switch that lands first. An anchor
//! registration takes it too, and takes the anchors lock only after reading
//! the collection (never an entry lock under the anchors lock). Never take the
//! switch gate while holding a replace gate, an entry lock or the anchors lock.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mae_sync::kb::KbCollectionDoc;
use mae_sync::text::TextSync;
use tokio::sync::MutexGuard;
use tracing::warn;

use super::DocStore;
use crate::storage::{PlaintextHeld, StorageError};

/// Number of [`DocStore`] switch-gate stripes.
pub(super) const SWITCH_GATE_STRIPES: usize = 64;

/// At most one refusal warning per KB per interval. A minute: a refusal that
/// keeps recurring is still seen within one, and a peer or client re-offering
/// it every second logs a sixtieth of it.
pub(super) const REFUSAL_LOG_INTERVAL: Duration = Duration::from_secs(60);

/// KBs the rate limit remembers. Past it, entries older than the interval go
/// first, then all of them: at worst one extra warning per KB.
const REFUSAL_LOG_KBS: usize = 1024;

/// The per-KB rate limit on refusal warnings, and how many it let through.
#[derive(Default)]
pub(super) struct RefusalLog {
    /// KB -> (when its last warning was logged, refusals suppressed since).
    last: std::sync::Mutex<HashMap<String, (Instant, u64)>>,
    logged: AtomicU64,
}

impl RefusalLog {
    /// If a refusal for `kb_id` is to be logged now: how many were
    /// suppressed since the last one logged.
    fn admit(&self, kb_id: &str, now: Instant) -> Option<u64> {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, suppressed)) = last.get_mut(kb_id) {
            if now.duration_since(*at) < REFUSAL_LOG_INTERVAL {
                *suppressed += 1;
                return None;
            }
            *at = now;
            self.logged.fetch_add(1, Ordering::Relaxed);
            return Some(std::mem::take(suppressed));
        }
        if last.len() >= REFUSAL_LOG_KBS {
            last.retain(|_, (at, _)| now.duration_since(*at) < REFUSAL_LOG_INTERVAL);
            if last.len() >= REFUSAL_LOG_KBS {
                last.clear();
            }
        }
        last.insert(kb_id.to_string(), (now, 0));
        self.logged.fetch_add(1, Ordering::Relaxed);
        Some(0)
    }
}

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
        is_e2e_under(coll, self.content_anchor_in(kb_id, coll).await)
    }

    async fn state_is_e2e(&self, kb_id: &str, state: &[u8]) -> bool {
        match KbCollectionDoc::from_bytes(state) {
            Ok(coll) => self.collection_is_e2e(kb_id, &coll).await,
            Err(_) => false,
        }
    }

    /// The refusal for a write taking `kbc:{kb_id}` from `before` to `after`,
    /// if it switches the KB to E2E while plaintext is held.
    pub(super) async fn e2e_switch_refusal(
        &self,
        kb_id: &str,
        before: &[u8],
        after: &[u8],
    ) -> Option<StorageError> {
        // `after` first: a plaintext KB's write decodes once.
        if !self.state_is_e2e(kb_id, after).await || self.state_is_e2e(kb_id, before).await {
            return None;
        }
        self.refuse_switch(kb_id, "write").await
    }

    /// For an update just applied in memory to `doc_name`, the collection of
    /// `kb_id`, that changed an encryption input (`sync`: the state it left):
    /// the switch refusal. The state before is the stored one.
    pub(super) async fn applied_switch_refusal(
        &self,
        kb_id: &str,
        doc_name: &str,
        sync: &TextSync,
    ) -> Option<StorageError> {
        if !self.state_is_e2e(kb_id, &sync.encode_state()).await {
            return None;
        }
        let before = match self.load_from_storage(doc_name).await {
            Ok(Some((stored, _))) => stored.encode_state(),
            Ok(None) => TextSync::empty_relay().encode_state(),
            Err(e) => {
                // Taken as not E2E: the switch is refused if plaintext is held.
                warn!(doc = doc_name, error = %e, "E2E switch: stored state unreadable");
                TextSync::empty_relay().encode_state()
            }
        };
        if self.state_is_e2e(kb_id, &before).await {
            return None;
        }
        self.refuse_switch(kb_id, "write").await
    }

    /// Whether recording `anchor` as `kb_id`'s content anchor would switch the
    /// KB to E2E while plaintext is held: what it would leave readable, if so.
    /// Called under the KB's switch gate, before the anchor is recorded.
    ///
    /// The signed op-log reads as E2E only under an anchor, so a log already
    /// holding the owner's `SetEncryption` (written while the anchor was
    /// unknown, which the write guard rightly read as plaintext) becomes E2E
    /// here, at no write.
    pub(super) async fn anchor_switch_refusal(
        &self,
        kb_id: &str,
        anchor: [u8; 32],
    ) -> Option<PlaintextHeld> {
        let coll = match self.encode_state(&format!("kbc:{kb_id}")).await {
            Ok(state) => KbCollectionDoc::from_bytes(&state).ok()?,
            // No collection yet: nothing for the anchor to switch.
            Err(StorageError::DurableDocMissing(_)) => return None,
            Err(e) => {
                // Refused whenever plaintext is held (fail closed).
                warn!(kb_id, error = %e, "E2E switch: collection unreadable at anchor registration");
                return self.refuse_switch_held(kb_id, "anchor").await;
            }
        };
        if !is_e2e_under(&coll, Some(anchor)) || self.collection_is_e2e(kb_id, &coll).await {
            return None;
        }
        self.refuse_switch_held(kb_id, "anchor").await
    }

    /// Refuse a switch of `kb_id` (by `what`: a write or an anchor) if
    /// plaintext is held.
    async fn refuse_switch(&self, kb_id: &str, what: &'static str) -> Option<StorageError> {
        let held = self.refuse_switch_held(kb_id, what).await?;
        Some(StorageError::E2eSwitchRefused {
            kb_id: kb_id.to_string(),
            held,
        })
    }

    /// What a switch of `kb_id` would leave readable, if anything: then the
    /// refusal is counted, and logged at the per-KB rate, here, so every path
    /// reports it the same way.
    async fn refuse_switch_held(&self, kb_id: &str, what: &'static str) -> Option<PlaintextHeld> {
        let held = self.plaintext_held(kb_id).await?;
        self.e2e_switches_refused.fetch_add(1, Ordering::Relaxed);
        if let Some(suppressed) = self.e2e_refusal_log.admit(kb_id, Instant::now()) {
            warn!(
                kb_id,
                ?held,
                by = what,
                suppressed,
                "refused a switch of the KB to E2E over plaintext (suppressed: refusals \
                 for this KB not logged since the last warning)"
            );
        }
        Some(held)
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

    /// How many of those refusals were logged (the rest were rate-limited).
    pub fn e2e_switch_refusals_logged(&self) -> u64 {
        self.e2e_refusal_log.logged.load(Ordering::Relaxed)
    }
}

/// Whether `coll` is E2E with `anchor` as its content anchor: by its signed
/// op-log, when there is an anchor, or by its own flag.
fn is_e2e_under(coll: &KbCollectionDoc, anchor: Option<[u8; 32]>) -> bool {
    let signed_e2e = anchor.is_some_and(|anchor| {
        matches!(
            mae_sync::membership::derive_encryption(&coll.oplog_ops(), &anchor),
            mae_sync::kb::Encryption::E2e
        )
    });
    signed_e2e || matches!(coll.encryption(), mae_sync::kb::Encryption::E2e)
}

#[cfg(test)]
#[path = "doc_store_e2e_switch_tests.rs"]
mod tests;
