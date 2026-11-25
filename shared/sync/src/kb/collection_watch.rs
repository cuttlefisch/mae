//! `KbCollectionDoc`: a watch on what a KB's encryption mode is read from, so a
//! store can tell, from the write itself, whether a write could have changed
//! it, without reading the collection before and after.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use yrs::types::{Event, PathSegment};
use yrs::{DeepObservable, Doc, Subscription};

use super::*;

/// The collection keys whether a KB is E2E is read from: the signed op-log
/// ([`crate::membership::derive_encryption`]), the unsigned display flag, and
/// the owner, whose key a store that owns the KB uses as its anchor when none
/// is recorded.
const ENCRYPTION_INPUT_KEYS: [&str; 3] = [COLL_OPLOG_KEY, COLL_ENCRYPTION_KEY, COLL_OWNER_KEY];

/// Set when a transaction on the watched document changes anything an
/// encryption reading depends on. See [`KbCollectionDoc::watch_encryption_inputs`].
pub struct EncryptionInputsWatch {
    touched: Arc<AtomicBool>,
    _subscription: Subscription,
}

impl EncryptionInputsWatch {
    /// Whether a transaction since the watch began changed an encryption input.
    pub fn touched(&self) -> bool {
        self.touched.load(Ordering::Acquire)
    }
}

impl KbCollectionDoc {
    /// Watch `doc`, a collection document held as a raw CRDT (a store that does
    /// not decode it), for a change to what its encryption mode is read from:
    /// the op-log and anything under it, the display flag, the owner. Until it
    /// is dropped, every transaction on `doc` that changes one of them sets
    /// [`EncryptionInputsWatch::touched`], including blocks that were pending
    /// and are integrated by a later update, and deletions.
    ///
    /// Conservative: a change it cannot place under a key (an event on the
    /// root that is not a map event) counts as touched. A watch that is not
    /// touched proves the encryption reading is the same before and after, for
    /// the same anchor.
    pub fn watch_encryption_inputs(doc: &Doc) -> EncryptionInputsWatch {
        let touched = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&touched);
        let root = doc.get_or_insert_map(COLLECTION_MAP);
        let subscription = root.observe_deep(move |txn, events| {
            if events.iter().any(|e| event_touches_inputs(txn, e)) {
                flag.store(true, Ordering::Release);
            }
        });
        EncryptionInputsWatch {
            touched,
            _subscription: subscription,
        }
    }
}

fn is_input(key: &str) -> bool {
    ENCRYPTION_INPUT_KEYS.contains(&key)
}

/// Whether one deep event, relative to the collection root, is under an input.
fn event_touches_inputs(txn: &yrs::TransactionMut, event: &Event) -> bool {
    match event.path().front() {
        Some(PathSegment::Key(key)) => is_input(key),
        Some(PathSegment::Index(_)) => true,
        None => match event {
            Event::Map(m) => m.keys(txn).keys().any(|k| is_input(k)),
            _ => true,
        },
    }
}
