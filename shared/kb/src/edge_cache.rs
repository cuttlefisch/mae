//! A node's graph edges, cached against the body they were derived from
//! (ADR-112 O4, #941 G3).
//!
//! `Node::links()` sits in hot loops (graph BFS, neighbours, orphan and
//! dead-end scans, the health fold), and deriving edges through `mae-org` is
//! a full parse: roughly ten times the old hand scanner's cost. So a node
//! keeps the edges of its body, and the cache is keyed by the body ITSELF
//! (length plus a per-process randomly seeded hash), never by a flag a writer
//! must remember to clear: `Node::body` is a public field assigned all over
//! the workspace, and any such assignment changes the key, so a stale entry
//! is never returned.
//! @ai-caution: [kb-links] Do not replace the body key with a dirty flag or
//! a version counter: nothing forces a writer of `node.body` to bump it, and a
//! stale edge set is silent (the graph just misses an edge).

use crate::org::ParsedLink;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::{Arc, Mutex, OnceLock};

/// What a cached edge set was derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BodyKey {
    len: usize,
    hash: u64,
}

fn body_key(body: &str) -> BodyKey {
    // Seeded once per process: a body cannot be crafted to collide with
    // another's key without knowing the seed.
    static SEED: OnceLock<RandomState> = OnceLock::new();
    BodyKey {
        len: body.len(),
        hash: SEED.get_or_init(RandomState::new).hash_one(body),
    }
}

/// One node's cached edges. Cloning a node clones the entry, which stays
/// valid for the clone for as long as their bodies agree, and no longer.
#[derive(Default)]
pub struct EdgeCache(Mutex<Option<(BodyKey, Arc<[ParsedLink]>)>>);

impl EdgeCache {
    /// The edges of `body` ([`edges_of`]), from the cache when it holds
    /// `body`'s.
    pub(crate) fn edges(&self, body: &str) -> Arc<[ParsedLink]> {
        let key = body_key(body);
        if let Some((k, edges)) = self.lock().as_ref() {
            if *k == key {
                return Arc::clone(edges);
            }
        }
        // Derived outside the lock: two threads racing on one node both parse,
        // and the last write wins with an identical value.
        let edges = edges_of(body);
        *self.lock() = Some((key, Arc::clone(&edges)));
        edges
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<(BodyKey, Arc<[ParsedLink]>)>> {
        // A panic while holding the lock can only have left a whole entry or
        // none: the value is still a valid cache.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Clone for EdgeCache {
    fn clone(&self) -> Self {
        EdgeCache(Mutex::new(self.lock().clone()))
    }
}

impl std::fmt::Debug for EdgeCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EdgeCache")
    }
}

/// The graph edges a body declares, in document order: its KB links
/// ([`crate::org_ingest::edges_of_body`]) that name a node. A fragment-only
/// KB link names no node to draw an edge to.
pub fn edges_of(body: &str) -> Arc<[ParsedLink]> {
    crate::org_ingest::edges_of_body(body)
        .into_iter()
        .filter(|l| !l.target.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::{Node, NodeKind};

    #[test]
    fn an_edited_body_is_never_answered_from_the_cache() {
        let mut n = Node::new("x", "x", NodeKind::Note, "[[a]]");
        assert_eq!(n.links(), vec!["a".to_string()]);
        // Same length: a key of the length alone would serve the old edges.
        n.body = "[[b]]".to_string();
        assert_eq!(n.links(), vec!["b".to_string()]);
        n.body.push_str(" [[c?rel=r&w=0.5]]");
        assert_eq!(
            n.links_typed(),
            vec![
                ("b".to_string(), "references".to_string(), 1.0),
                ("c".to_string(), "r".to_string(), 0.5),
            ]
        );
        n.body.clear();
        assert!(n.links().is_empty());
    }

    #[test]
    fn a_clone_keeps_its_own_edges_once_its_body_diverges() {
        let mut a = Node::new("x", "x", NodeKind::Note, "[[a]]");
        assert_eq!(a.links(), vec!["a".to_string()]);
        let mut b = a.clone();
        assert_eq!(b.links(), vec!["a".to_string()]);
        b.body = "[[b]]".to_string();
        a.body = "[[c]]".to_string();
        assert_eq!(b.links(), vec!["b".to_string()]);
        assert_eq!(a.links(), vec!["c".to_string()]);
    }

    #[test]
    fn a_repeated_call_returns_the_cached_derivation() {
        let n = Node::new("x", "x", NodeKind::Note, "[[a]] [[b]]");
        assert!(std::sync::Arc::ptr_eq(&n.edges(), &n.edges()));
    }
}
