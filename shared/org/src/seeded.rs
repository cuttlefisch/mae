//! Hash maps keyed with a secret.
//!
//! mae-org's maps are keyed by document text (names, labels, ids, titles),
//! and a document may come from someone else: a KB member's note, opened in
//! everyone's browser. A hash table stays fast only while its keys stay
//! unpredictable, so every map here hashes with SipHash under keys nobody
//! outside the process knows.
//!
//! Natively the keys come from std's `RandomState`, seeded by the operating
//! system. On wasm32-unknown-unknown `RandomState` is not seeded (its keys
//! are the same on every load), so the host supplies them with [`seed`]
//! (mae-org-wasm's loader does, from `crypto.getRandomValues`); until it
//! does, [`is_seeded`] is false there, and mae-org-wasm refuses to run.
//!
//! A map takes the keys current when it is made and keeps them: seeding
//! again changes the keys of later maps only.

use std::hash::BuildHasher;
use std::sync::Mutex;

/// A hash map keyed with the process's secret (see the module doc).
#[allow(clippy::disallowed_types)]
pub type HashMap<K, V> = std::collections::HashMap<K, V, Seeded>;
/// A hash set keyed with the process's secret (see the module doc).
#[allow(clippy::disallowed_types)]
pub type HashSet<K> = std::collections::HashSet<K, Seeded>;

/// The keys, and whether the host gave them.
static KEYS: Mutex<Option<((u64, u64), bool)>> = Mutex::new(None);

/// Sets the keys every map made from now on hashes with: 128 bits the host
/// draws from a secure source.
pub fn seed(k0: u64, k1: u64) {
    *KEYS.lock().unwrap_or_else(|e| e.into_inner()) = Some(((k0, k1), true));
}

/// Whether the keys are secret: always natively; on wasm32 once the host
/// has called [`seed`].
pub fn is_seeded() -> bool {
    !cfg!(target_arch = "wasm32") || KEYS.lock().is_ok_and(|k| k.is_some_and(|k| k.1))
}

fn keys() -> (u64, u64) {
    let mut k = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    k.get_or_insert_with(|| {
        let r = std::collections::hash_map::RandomState::new();
        ((r.hash_one(0u64), r.hash_one(1u64)), false)
    })
    .0
}

/// SipHash under the keys current when the map was made.
#[derive(Debug, Clone)]
pub struct Seeded {
    k0: u64,
    k1: u64,
}

impl Default for Seeded {
    fn default() -> Seeded {
        let (k0, k1) = keys();
        Seeded { k0, k1 }
    }
}

impl BuildHasher for Seeded {
    // SipHash-2-4: std's keyed SipHash; deprecated only in favour of crates
    // mae-org does not depend on (ADR-112 D1).
    #[allow(deprecated)]
    type Hasher = std::hash::SipHasher;

    #[allow(deprecated)]
    fn build_hasher(&self) -> std::hash::SipHasher {
        std::hash::SipHasher::new_with_keys(self.k0, self.k1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::BuildHasher;

    /// A map keeps the keys it was made with: seeding again neither loses
    /// what it holds nor changes how it hashes; a later map uses the new
    /// keys, and the hasher is keyed (other keys hash differently).
    #[test]
    fn a_map_keeps_its_keys_across_a_new_seed() {
        seed(1, 2);
        let mut m: HashMap<String, u32> = HashMap::default();
        for i in 0..1000 {
            m.insert(format!("k{i}"), i);
        }
        let before = m.hasher().hash_one("k7");
        seed(3, 4);
        assert!((0..1000).all(|i| m.get(&format!("k{i}")) == Some(&i)));
        assert_eq!(m.hasher().hash_one("k7"), before);
        let later: HashMap<String, u32> = HashMap::default();
        assert_ne!(later.hasher().hash_one("k7"), before);
        assert!(is_seeded());
    }
}
