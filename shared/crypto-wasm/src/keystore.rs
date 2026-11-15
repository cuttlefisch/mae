//! The member keys the module holds, by handle, so a seed stays inside the
//! module between calls: generated or imported keys are used by handle,
//! and the seed leaves only through `export_seed` or a backup envelope.
//!
//! Handles count up from 1 and are never reused within an instance, so a
//! stale handle names nothing rather than someone else's key. A handle is
//! not a secret (anything that can call the module can use any key in it).

use mae_crypto::member::MemberKey;
use std::cell::RefCell;

/// The most keys one instance holds at once.
pub const MAX_KEYS: usize = 16;

struct Keystore {
    next: u32,
    // Boxed so that `retain` and growth move pointers, not keys: a moved key
    // leaves its old bytes behind where nothing will ever wipe them.
    keys: Vec<(u32, Box<MemberKey>)>,
}

thread_local! {
    static KEYS: RefCell<Keystore> = const {
        RefCell::new(Keystore { next: 1, keys: Vec::new() })
    };
}

/// Why a key could not be stored.
#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    /// [`MAX_KEYS`] are held; forget one first.
    Full,
    /// Every handle has been issued.
    Exhausted,
}

/// Takes `key` and returns its handle.
pub fn insert(key: MemberKey) -> Result<u32, StoreError> {
    KEYS.with(|k| {
        let mut k = k.borrow_mut();
        if k.keys.len() >= MAX_KEYS {
            return Err(StoreError::Full);
        }
        let handle = k.next;
        k.next = handle.checked_add(1).ok_or(StoreError::Exhausted)?;
        k.keys.push((handle, Box::new(key)));
        Ok(handle)
    })
}

/// Runs `f` on the key `handle` names, or returns `None`.
pub fn with<R>(handle: u32, f: impl FnOnce(&MemberKey) -> R) -> Option<R> {
    KEYS.with(|k| {
        let k = k.borrow();
        k.keys
            .iter()
            .find(|(h, _)| *h == handle)
            .map(|(_, key)| f(key))
    })
}

/// Drops (and so wipes) the key `handle` names; whether there was one.
pub fn forget(handle: u32) -> bool {
    KEYS.with(|k| {
        let mut k = k.borrow_mut();
        let before = k.keys.len();
        k.keys.retain(|(h, _)| *h != handle);
        k.keys.len() != before
    })
}
