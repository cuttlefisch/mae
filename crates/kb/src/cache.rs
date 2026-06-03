//! NodeCache — LRU cache for KB nodes.
//!
//! Sits between `KbQueryLayer` and `CozoKbStore`, caching recently accessed
//! nodes to avoid Datalog round-trips on hot paths (help buffer rendering,
//! link resolution).

use crate::query::KbQueryLayer;
use crate::store::{HealthReport, Link, SearchHit, SubGraph};
use crate::Node;
use std::collections::HashMap;
use std::sync::Mutex;

/// LRU node cache with configurable capacity.
pub struct NodeCache {
    /// Cached nodes keyed by ID. Uses insertion order for LRU eviction.
    entries: Mutex<LruMap>,
}

struct LruMap {
    map: HashMap<String, (Node, u64)>,
    generation: u64,
    capacity: usize,
}

impl LruMap {
    fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::with_capacity(capacity),
            generation: 0,
            capacity,
        }
    }

    fn get(&mut self, id: &str) -> Option<&Node> {
        if let Some(entry) = self.map.get_mut(id) {
            self.generation += 1;
            entry.1 = self.generation;
            Some(&entry.0)
        } else {
            None
        }
    }

    fn put(&mut self, node: Node) {
        self.generation += 1;
        if self.map.len() >= self.capacity && !self.map.contains_key(&node.id) {
            // Evict least recently used
            if let Some(lru_key) = self
                .map
                .iter()
                .min_by_key(|(_, (_, gen))| *gen)
                .map(|(k, _)| k.clone())
            {
                self.map.remove(&lru_key);
            }
        }
        let id = node.id.clone();
        self.map.insert(id, (node, self.generation));
    }

    fn invalidate(&mut self, id: &str) {
        self.map.remove(id);
    }

    fn invalidate_all(&mut self) {
        self.map.clear();
    }
}

impl NodeCache {
    /// Create a new cache with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(LruMap::new(capacity)),
        }
    }

    /// Get a cached node (returns clone).
    pub fn get(&self, id: &str) -> Option<Node> {
        self.entries.lock().unwrap().get(id).cloned()
    }

    /// Insert or update a cached node.
    pub fn put(&self, node: Node) {
        self.entries.lock().unwrap().put(node);
    }

    /// Remove a specific node from cache.
    pub fn invalidate(&self, id: &str) {
        self.entries.lock().unwrap().invalidate(id);
    }

    /// Clear all cached entries.
    pub fn invalidate_all(&self) {
        self.entries.lock().unwrap().invalidate_all();
    }

    /// Current number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().map.len()
    }

    /// Returns true if cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// CachedQueryLayer wraps a KbQueryLayer with an LRU NodeCache.
/// `get()` checks cache first, falls back to underlying layer.
/// Search and link queries always go through the underlying layer.
pub struct CachedQueryLayer {
    inner: Box<dyn KbQueryLayer>,
    cache: NodeCache,
}

impl CachedQueryLayer {
    pub fn new(inner: Box<dyn KbQueryLayer>, capacity: usize) -> Self {
        Self {
            inner,
            cache: NodeCache::new(capacity),
        }
    }

    /// Invalidate a cached node (after mutation).
    pub fn invalidate(&self, id: &str) {
        self.cache.invalidate(id);
    }

    /// Clear the entire cache.
    pub fn invalidate_all(&self) {
        self.cache.invalidate_all();
    }
}

impl KbQueryLayer for CachedQueryLayer {
    fn get(&self, id: &str) -> Option<Node> {
        // Check cache first
        if let Some(node) = self.cache.get(id) {
            return Some(node);
        }
        // Cache miss — fetch from underlying store
        let node = self.inner.get(id)?;
        self.cache.put(node.clone());
        Some(node)
    }

    fn contains(&self, id: &str) -> bool {
        if self.cache.get(id).is_some() {
            return true;
        }
        self.inner.contains(id)
    }

    fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        self.inner.search(query, limit)
    }

    fn links_from(&self, id: &str) -> Vec<Link> {
        self.inner.links_from(id)
    }

    fn links_to(&self, id: &str) -> Vec<Link> {
        self.inner.links_to(id)
    }

    fn list_ids(&self, prefix: Option<&str>) -> Vec<String> {
        self.inner.list_ids(prefix)
    }

    fn id_title_pairs(&self, prefix: Option<&str>) -> Vec<(String, String)> {
        self.inner.id_title_pairs(prefix)
    }

    fn health_report(&self) -> Option<HealthReport> {
        self.inner.health_report()
    }

    fn neighborhood(&self, id: &str, depth: u32) -> Option<SubGraph> {
        self.inner.neighborhood(id, depth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_basic_operations() {
        let cache = NodeCache::new(3);
        assert!(cache.is_empty());

        let node = Node::new("a", "Alpha", crate::NodeKind::Note, "body");
        cache.put(node);
        assert_eq!(cache.len(), 1);

        let got = cache.get("a").unwrap();
        assert_eq!(got.title, "Alpha");
        assert!(cache.get("b").is_none());

        cache.invalidate("a");
        assert!(cache.get("a").is_none());
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_lru_eviction() {
        let cache = NodeCache::new(2);

        cache.put(Node::new("a", "A", crate::NodeKind::Note, ""));
        cache.put(Node::new("b", "B", crate::NodeKind::Note, ""));
        assert_eq!(cache.len(), 2);

        // Access "a" to make "b" the LRU
        let _ = cache.get("a");

        // Adding "c" should evict "b"
        cache.put(Node::new("c", "C", crate::NodeKind::Note, ""));
        assert_eq!(cache.len(), 2);
        assert!(cache.get("a").is_some());
        assert!(cache.get("b").is_none());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn cache_invalidate_all() {
        let cache = NodeCache::new(10);
        cache.put(Node::new("a", "A", crate::NodeKind::Note, ""));
        cache.put(Node::new("b", "B", crate::NodeKind::Note, ""));
        cache.invalidate_all();
        assert!(cache.is_empty());
    }
}
