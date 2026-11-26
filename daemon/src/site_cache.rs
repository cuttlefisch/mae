//! The live site's derivation cache (ADR-119 D6, first step; #1036).
//!
//! The live site used to read every node of a KB and derive the whole site on
//! every request: about 2 s a page at 1,560 nodes on one CPU. This keeps one
//! derived site per KB and serves later requests from it until the KB changes.
//!
//! **What a cached site is keyed on.** Everything deriving it reads:
//! - the KB's content generation (`DocStore::kb_generation`), which moves after
//!   every committed write to the KB's collection or node documents;
//! - the attachment store's change count, since a page shows whether each
//!   attachment resolves;
//! - the derivation options (hidden namespaces, whether source is highlighted)
//!   and the per-request bounds (D21), since they change the output or whether
//!   there is one.
//!
//! A caller takes the generation BEFORE it reads any content, so a write racing
//! the read can only make the cached site newer than its key, never older. A
//! hit accepts a cached site at least as new as the key asked for.
//!
//! **What is not cached.** Authorization. The read gate and whether the reader
//! may copy the whole KB are decided per request, before the cache is asked,
//! and the derived site is the same for every reader the gate admits (only
//! plaintext, review-off KBs reach it: ADR-119 D8). One KB's site is never
//! served for another: the slot is the KB's own.
//!
//! **One derivation at a time per KB.** Concurrent misses on one KB wait for
//! the first instead of each paying the derivation.
//!
//! **Bounded.** At most `capacity` KBs are held; the least recently used is
//! dropped. Incremental re-derivation per node, with ADR-119's property test
//! that it equals a full derivation, is the rest of D6.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// How many KBs' sites are held when the operator sets nothing.
pub const DEFAULT_CAPACITY: usize = 8;

/// What a derived site was derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivationKey {
    /// `DocStore::kb_generation`, taken before reading.
    pub generation: u64,
    /// `BlobStore::changes`, taken before reading (0 without a store).
    pub blob_changes: u64,
    pub hidden_namespaces: Vec<String>,
    pub highlighted: bool,
    pub max_scan_nodes: usize,
    pub max_body_bytes: usize,
}

impl DerivationKey {
    /// Whether a site derived under `self` answers a request keyed `want`:
    /// the same options and bounds, and content at least as new.
    fn serves(&self, want: &DerivationKey) -> bool {
        self.hidden_namespaces == want.hidden_namespaces
            && self.highlighted == want.highlighted
            && self.max_scan_nodes == want.max_scan_nodes
            && self.max_body_bytes == want.max_body_bytes
            && self.generation >= want.generation
            && self.blob_changes >= want.blob_changes
    }
}

/// A KB's derived site, that it is over the bounds, or that deriving it
/// failed. A failure is never held: the next request derives again.
pub enum Derived {
    Site(mae_site::Site),
    TooLarge,
    Failed,
}

type Held = Option<(DerivationKey, Arc<Derived>)>;

struct Slot {
    held: Arc<tokio::sync::Mutex<Held>>,
    used: AtomicU64,
}

/// One derived site per KB, for the life of the process.
pub struct SiteCache {
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    capacity: AtomicUsize,
    tick: AtomicU64,
    derivations: AtomicU64,
}

impl Default for SiteCache {
    fn default() -> Self {
        SiteCache {
            slots: Mutex::new(HashMap::new()),
            capacity: AtomicUsize::new(DEFAULT_CAPACITY),
            tick: AtomicU64::new(0),
            derivations: AtomicU64::new(0),
        }
    }
}

impl SiteCache {
    /// Hold at most `n` KBs (`0`: hold none, derive every request).
    pub fn set_capacity(&self, n: usize) {
        self.capacity.store(n, Ordering::SeqCst);
    }

    /// How many derivations ran since the process started.
    pub fn derivations(&self) -> u64 {
        self.derivations.load(Ordering::SeqCst)
    }

    /// KB `kb_id`'s site for `key`: the held one if it serves, otherwise
    /// `derive`'s, which is then held. `derive` reads the KB itself, after
    /// the caller took `key`.
    pub async fn get_or_derive<F, Fut>(
        &self,
        kb_id: &str,
        key: DerivationKey,
        derive: F,
    ) -> Arc<Derived>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Derived>,
    {
        let Some(slot) = self.slot(kb_id) else {
            self.derivations.fetch_add(1, Ordering::SeqCst);
            return Arc::new(derive().await);
        };
        let mut held = slot.held.lock().await;
        if let Some((k, site)) = held.as_ref() {
            if k.serves(&key) {
                return Arc::clone(site);
            }
        }
        self.derivations.fetch_add(1, Ordering::SeqCst);
        let site = Arc::new(derive().await);
        if !matches!(*site, Derived::Failed) {
            *held = Some((key, Arc::clone(&site)));
        }
        site
    }

    /// KB `kb_id`'s slot, made (dropping the least recently used KB when
    /// full) if it has none; `None` when the cache holds nothing.
    fn slot(&self, kb_id: &str) -> Option<Arc<Slot>> {
        let cap = self.capacity.load(Ordering::SeqCst);
        if cap == 0 {
            return None;
        }
        let now = self.tick.fetch_add(1, Ordering::SeqCst);
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = slots.get(kb_id) {
            s.used.store(now, Ordering::SeqCst);
            return Some(Arc::clone(s));
        }
        while slots.len() >= cap {
            let oldest = slots
                .iter()
                .min_by_key(|(_, s)| s.used.load(Ordering::SeqCst))
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => slots.remove(&k),
                None => break,
            };
        }
        let slot = Arc::new(Slot {
            held: Arc::new(tokio::sync::Mutex::new(None)),
            used: AtomicU64::new(now),
        });
        slots.insert(kb_id.to_string(), Arc::clone(&slot));
        Some(slot)
    }

    /// How many KBs are held (tests, status).
    pub fn held_kbs(&self) -> usize {
        self.slots.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

#[cfg(test)]
#[path = "site_cache_tests.rs"]
mod tests;
