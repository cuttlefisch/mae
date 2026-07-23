//! Reference lazy-fetch client (ADR-053/Phase G, #382) — the "no full
//! replication" mechanism decision 4/7 describes is unambiguously
//! **client-side**: only a genuine KB member ever holds the per-KB
//! `ContentKey`, so only a client can decrypt what `kb/query.get` returns
//! for an `Encryption::E2e` KB; the daemon never caches plaintext by
//! construction. This module is that client-side half — a thin, bounded,
//! evictable cache a "thin client" (a paired external editor, a scripted
//! test harness, etc.) uses after fetching `kb/query.get`'s raw
//! `ciphertext_b64` payload.
//!
//! Deliberately reuses `mae_kb::cache::NodeCache` unmodified (principle #8)
//! rather than a parallel cache type — see ADR-053's Implementation-note
//! addendum. The only new logic here is decrypting/materializing an op-set
//! (`mae_sync::op_set::materialize`, itself promoted to `pub` from a
//! previously test-only helper for this reuse) and principal-scoping the
//! cache key, so one client process serving multiple authenticated
//! identities never leaks one principal's decrypted content to another.
//!
//! `#[cfg(test)]`-only for now: this proves the mechanism (bounded,
//! evictable, no cross-principal leak, decrypts real content) with real
//! crypto rather than describing it in prose, but nothing in this
//! repository yet ships a production "thin client" process to wire it into
//! (that's Phase I's VS Code extension, or a future generic client SDK) —
//! not silently claiming broader production usage than exists today.

use mae_kb::cache::NodeCache;
use mae_kb::{Node, NodeKind};
use mae_sync::content_crypto::ContentKey;
use mae_sync::{encoding, op_set};

/// A bounded, evictable, principal-scoped decrypt cache — the reference
/// implementation of ADR-053 decision 4/7's lazy-fetch primitive.
pub struct LazyFetchClient {
    cache: NodeCache,
}

impl LazyFetchClient {
    pub fn new(capacity: usize) -> Self {
        LazyFetchClient {
            cache: NodeCache::new(capacity),
        }
    }

    /// Cache key composition: principal-prefixed, never bare `node_id` —
    /// decision 7's "keyed per authenticated principal," and the actual
    /// defense against a multi-tenant client cross-serving decrypted
    /// content between two different authenticated identities.
    fn cache_key(principal: &str, node_id: &str) -> String {
        format!("{principal}:{node_id}")
    }

    /// A cached hit, if present, for `(principal, node_id)`.
    pub fn get_cached(&self, principal: &str, node_id: &str) -> Option<Node> {
        self.cache
            .get(&Self::cache_key(principal, node_id))
            .map(|mut n| {
                n.id = node_id.to_string(); // the cache-storage key, not the node's own identity
                n
            })
    }

    /// Decrypt a `kb/query.get` E2E response's raw op-set ciphertext
    /// (`ciphertext_b64`, exactly the field that RPC returns) with `key`,
    /// materialize it into a plaintext `Node`, and cache it under
    /// `(principal, node_id)`. `None` if the ciphertext doesn't open under
    /// `key` (wrong/rotated key, tamper, or genuinely not a member) — never
    /// populates the cache in that case.
    pub fn decrypt_and_cache(
        &self,
        principal: &str,
        node_id: &str,
        ciphertext_b64: &str,
        key: &ContentKey,
    ) -> Option<Node> {
        let op_set_state = encoding::base64_to_update(ciphertext_b64).ok()?;
        let doc = op_set::materialize(&op_set_state, key);
        if doc.title().is_empty() && doc.body().is_empty() {
            return None;
        }
        let node = Node::new(node_id, doc.title(), NodeKind::Note, doc.body());
        let mut cache_entry = node.clone();
        cache_entry.id = Self::cache_key(principal, node_id);
        self.cache.put(cache_entry);
        Some(node)
    }

    /// Current cache size — test/introspection only.
    pub fn len(&self) -> usize {
        self.cache.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_node(id: &str) -> Node {
        Node::new(id, format!("Title {id}"), NodeKind::Note, "body")
    }

    #[test]
    fn bounded_capacity_never_exceeded_under_sustained_pressure() {
        let client = LazyFetchClient::new(10);
        for i in 0..500 {
            let key = ContentKey::generate();
            // Directly exercise the cache (bypassing real encryption — this
            // test is about capacity bookkeeping, not crypto) via the same
            // NodeCache instance decrypt_and_cache uses internally.
            let mut n = fake_node(&format!("node-{i}"));
            n.id = format!("principal-a:node-{i}");
            client.cache.put(n);
            let _ = key; // silence unused in this capacity-only test
            assert!(
                client.len() <= 10,
                "cache size {} exceeded configured capacity 10 after {i} inserts",
                client.len()
            );
        }
        assert_eq!(
            client.len(),
            10,
            "cache should be at capacity after 500 inserts"
        );
    }

    #[test]
    fn a_denied_or_undecryptable_fetch_never_populates_the_cache() {
        let client = LazyFetchClient::new(10);
        let key = ContentKey::generate();
        // Ciphertext sealed under a DIFFERENT key -- simulates a non-member
        // or wrong/rotated key, exactly op_set.rs's own adversarial case.
        let other_key = ContentKey::generate();
        let mut node = mae_sync::kb::KbNodeDoc::new_with_client_id("n1", "", "", &[], 7);
        let mut state = Vec::new();
        for pt in [
            node.encode_state(),
            node.set_title("Secret"),
            node.set_body("private"),
        ] {
            let (_id, outer) = op_set::seal_op(&state, &other_key, &pt, 7).unwrap();
            state = op_set::merge(&state, &outer).unwrap();
        }
        let ciphertext_b64 = encoding::update_to_base64(&state);

        let result = client.decrypt_and_cache("principal-a", "n1", &ciphertext_b64, &key);
        assert!(result.is_none(), "wrong key must not materialize a node");
        assert_eq!(
            client.len(),
            0,
            "a failed decrypt must never populate the cache"
        );
    }

    #[test]
    fn two_principals_decrypted_entries_never_cross_contaminate() {
        let client = LazyFetchClient::new(10);
        let key = ContentKey::generate();
        let mut node = mae_sync::kb::KbNodeDoc::new_with_client_id("shared-node", "", "", &[], 7);
        let mut state = Vec::new();
        for pt in [
            node.encode_state(),
            node.set_title("Real Title"),
            node.set_body("real body"),
        ] {
            let (_id, outer) = op_set::seal_op(&state, &key, &pt, 7).unwrap();
            state = op_set::merge(&state, &outer).unwrap();
        }
        let ciphertext_b64 = encoding::update_to_base64(&state);

        // Principal A fetches and decrypts.
        let a_result =
            client.decrypt_and_cache("principal-a", "shared-node", &ciphertext_b64, &key);
        assert!(a_result.is_some());

        // Principal B, a DIFFERENT authenticated identity on the same client
        // process, must NOT see a cache hit for the same node_id just
        // because principal A already decrypted it -- each principal's
        // access is independently gated server-side per-request; the
        // client-side cache must not shortcut that.
        let b_cached = client.get_cached("principal-b", "shared-node");
        assert!(
            b_cached.is_none(),
            "principal B must not get a cache hit from principal A's decrypted entry"
        );

        // Principal A's own cache hit still works.
        let a_cached = client.get_cached("principal-a", "shared-node");
        assert!(a_cached.is_some());
        assert_eq!(a_cached.unwrap().title, "Real Title");
    }
}
