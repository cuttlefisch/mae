//! LruQueryLayer — bounded LRU cache over daemon RPC.
//!
//! Implements `KbQueryLayer` with a bounded in-process LRU cache. Cache
//! misses go to `mae-daemon` via `DaemonClient` JSON-RPC. The cache
//! provides <1μs hits for recently accessed nodes, with 5-15ms cold
//! misses (round-trip to daemon).
//!
//! Memory budget: ~400KB for 200 nodes (default capacity).

use crate::query::KbQueryLayer;
use crate::store::{HealthReport, Link, SearchHit, SubGraph};
use crate::Node;
use mae_mcp::daemon_client::{DaemonClient, DaemonClientError};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Mutex;

/// LRU cache entry for a node lookup result.
/// Only found nodes are cached — missing lookups are not cached to avoid
/// stale negatives when nodes are created later.
#[derive(Clone)]
enum CacheEntry {
    /// Node was found.
    Found(Box<Node>),
}

/// Bounded LRU cache implementing `KbQueryLayer` via daemon RPC.
pub struct LruQueryLayer {
    client: Mutex<DaemonClient>,
    /// Node cache: id → CacheEntry
    node_cache: Mutex<lru::LruCache<String, CacheEntry>>,
    /// Links-from cache: id → Vec<Link>
    links_from_cache: Mutex<lru::LruCache<String, Vec<Link>>>,
    /// Links-to cache: id → Vec<Link>
    links_to_cache: Mutex<lru::LruCache<String, Vec<Link>>>,
}

impl LruQueryLayer {
    /// Create a new LRU query layer with the given daemon client and cache capacity.
    /// Capacity of 0 means unbounded (uses usize::MAX).
    pub fn new(client: DaemonClient, capacity: usize) -> Self {
        let cap = if capacity == 0 {
            NonZeroUsize::new(usize::MAX).unwrap()
        } else {
            NonZeroUsize::new(capacity).unwrap()
        };
        Self {
            client: Mutex::new(client),
            node_cache: Mutex::new(lru::LruCache::new(cap)),
            links_from_cache: Mutex::new(lru::LruCache::new(cap)),
            links_to_cache: Mutex::new(lru::LruCache::new(cap)),
        }
    }

    /// Evict a single node and its associated link caches.
    /// Acquires all three locks atomically to prevent races where another
    /// thread repopulates caches between individual evictions.
    pub fn invalidate(&self, node_id: &str) {
        let mut nc = self.node_cache.lock().unwrap();
        let mut lfc = self.links_from_cache.lock().unwrap();
        let mut ltc = self.links_to_cache.lock().unwrap();
        nc.pop(node_id);
        lfc.pop(node_id);
        ltc.pop(node_id);
    }

    /// Evict all cached entries.
    pub fn invalidate_all(&self) {
        self.node_cache.lock().unwrap().clear();
        self.links_from_cache.lock().unwrap().clear();
        self.links_to_cache.lock().unwrap().clear();
    }

    /// Current number of cached nodes.
    pub fn cached_node_count(&self) -> usize {
        self.node_cache.lock().unwrap().len()
    }

    /// Whether the daemon client is connected.
    pub fn is_connected(&self) -> bool {
        self.client.lock().unwrap().is_connected()
    }

    /// Attempt to reconnect the daemon client.
    pub fn reconnect(&self) -> Result<(), DaemonClientError> {
        self.client.lock().unwrap().connect()
    }

    /// Fetch a node from cache or daemon.
    fn fetch_node(&self, id: &str) -> Option<Node> {
        // Check cache first
        {
            let mut cache = self.node_cache.lock().unwrap();
            if let Some(CacheEntry::Found(node)) = cache.get(id) {
                return Some(*node.clone());
            }
        }

        // Cache miss → RPC to daemon
        let result = {
            let mut client = self.client.lock().unwrap();
            client.call("kb/get", json!({"id": id}))
        };

        match result {
            Ok(Value::Null) => {
                // Don't cache missing entries — if a node is created later,
                // the stale "Missing" would suppress future lookups until
                // invalidate_all(). Cache hits are the common case; cache
                // misses for nonexistent nodes are rare.
                None
            }
            Ok(val) => {
                let node = parse_node_from_json(&val);
                if let Some(ref n) = node {
                    let mut cache = self.node_cache.lock().unwrap();
                    cache.put(id.to_string(), CacheEntry::Found(Box::new(n.clone())));
                }
                node
            }
            Err(e) => {
                tracing::debug!(error = %e, id, "LruQueryLayer: daemon fetch failed");
                None
            }
        }
    }
}

impl KbQueryLayer for LruQueryLayer {
    fn get(&self, id: &str) -> Option<Node> {
        self.fetch_node(id)
    }

    fn contains(&self, id: &str) -> bool {
        // Check node cache first
        {
            let mut cache = self.node_cache.lock().unwrap();
            if cache.get(id).is_some() {
                return true; // Only Found entries are cached
            }
        }
        // Fetch from daemon (populates cache on hit)
        self.fetch_node(id).is_some()
    }

    fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        // Search is not cacheable — always goes to daemon
        let result = {
            let mut client = self.client.lock().unwrap();
            client.call("kb/search", json!({"query": query, "limit": limit}))
        };
        match result {
            Ok(val) => parse_search_hits(&val),
            Err(e) => {
                tracing::debug!(error = %e, "LruQueryLayer: search failed");
                Vec::new()
            }
        }
    }

    fn links_from(&self, id: &str) -> Vec<Link> {
        // Check cache
        {
            let mut cache = self.links_from_cache.lock().unwrap();
            if let Some(links) = cache.get(id) {
                return links.clone();
            }
        }
        // RPC
        let result = {
            let mut client = self.client.lock().unwrap();
            client.call("kb/links_from", json!({"id": id}))
        };
        match result {
            Ok(val) => {
                let links = parse_links(&val);
                let mut cache = self.links_from_cache.lock().unwrap();
                cache.put(id.to_string(), links.clone());
                links
            }
            Err(e) => {
                tracing::debug!(error = %e, id, "LruQueryLayer: links_from failed");
                Vec::new()
            }
        }
    }

    fn links_to(&self, id: &str) -> Vec<Link> {
        // Check cache
        {
            let mut cache = self.links_to_cache.lock().unwrap();
            if let Some(links) = cache.get(id) {
                return links.clone();
            }
        }
        // RPC
        let result = {
            let mut client = self.client.lock().unwrap();
            client.call("kb/links_to", json!({"id": id}))
        };
        match result {
            Ok(val) => {
                let links = parse_links(&val);
                let mut cache = self.links_to_cache.lock().unwrap();
                cache.put(id.to_string(), links.clone());
                links
            }
            Err(e) => {
                tracing::debug!(error = %e, id, "LruQueryLayer: links_to failed");
                Vec::new()
            }
        }
    }

    fn list_ids(&self, prefix: Option<&str>) -> Vec<String> {
        // Listing is cheap via RPC — don't cache
        let params = match prefix {
            Some(p) => json!({"prefix": p}),
            None => json!({}),
        };
        let result = {
            let mut client = self.client.lock().unwrap();
            client.call("kb/list_ids", params)
        };
        match result {
            Ok(val) => val
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            Err(e) => {
                tracing::debug!(error = %e, "LruQueryLayer: list_ids failed");
                Vec::new()
            }
        }
    }

    fn id_title_pairs(&self, prefix: Option<&str>) -> Vec<(String, String)> {
        // Delegate to daemon — this is used for completion, not worth caching all
        let params = match prefix {
            Some(p) => json!({"prefix": p}),
            None => json!({}),
        };
        let result = {
            let mut client = self.client.lock().unwrap();
            client.call("kb/id_title_pairs", params)
        };
        match result {
            Ok(val) => val
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| {
                            let id = v.get(0)?.as_str()?;
                            let title = v.get(1)?.as_str()?;
                            Some((id.to_string(), title.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Err(e) => {
                tracing::debug!(error = %e, "LruQueryLayer: id_title_pairs failed");
                Vec::new()
            }
        }
    }

    fn id_title_body_triples(
        &self,
        prefix: Option<&str>,
        body_limit: usize,
    ) -> Vec<(String, String, String)> {
        let mut params = json!({"body_limit": body_limit});
        if let Some(p) = prefix {
            params["prefix"] = json!(p);
        }
        let result = {
            let mut client = self.client.lock().unwrap();
            client.call("kb/id_title_body_triples", params)
        };
        match result {
            Ok(val) => val
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| {
                            let id = v.get(0)?.as_str()?;
                            let title = v.get(1)?.as_str()?;
                            let body = v.get(2)?.as_str().unwrap_or("");
                            Some((id.to_string(), title.to_string(), body.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Err(e) => {
                tracing::debug!(error = %e, "LruQueryLayer: id_title_body_triples failed");
                Vec::new()
            }
        }
    }

    fn health_report(&self) -> Option<HealthReport> {
        let result = {
            let mut client = self.client.lock().unwrap();
            client.call("kb/health", json!({}))
        };
        match result {
            Ok(val) => {
                // Parse the subset the daemon returns
                Some(HealthReport {
                    total_nodes: val["total_nodes"].as_u64().unwrap_or(0) as usize,
                    total_links: val["total_links"].as_u64().unwrap_or(0) as usize,
                    namespace_counts: HashMap::new(),
                    by_kind: HashMap::new(),
                    by_rel_type: HashMap::new(),
                    orphan_ids: Vec::new(), // Daemon returns count, not full list
                    broken_links: Vec::new(),
                    hub_nodes: Vec::new(),
                })
            }
            Err(e) => {
                tracing::debug!(error = %e, "LruQueryLayer: health failed");
                None
            }
        }
    }

    fn neighborhood(&self, _id: &str, _depth: u32) -> Option<SubGraph> {
        // Neighborhood is complex — not yet implemented via daemon RPC
        // Falls back to None (caller can use local store if available)
        None
    }
}

// --- JSON parsing helpers ---

fn parse_node_from_json(val: &Value) -> Option<Node> {
    let id = val.get("id")?.as_str()?;
    let title = val.get("title")?.as_str().unwrap_or("");
    let kind_str = val.get("kind")?.as_str().unwrap_or("Note");
    let body = val.get("body")?.as_str().unwrap_or("");
    let tags: Vec<String> = val
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let kind = parse_node_kind(kind_str);
    let mut node = Node::new(id, title, kind, body);
    node.tags = tags;
    Some(node)
}

fn parse_node_kind(s: &str) -> crate::NodeKind {
    use crate::NodeKind;
    // Daemon sends lowercase via NodeKind::as_str(); accept both cases
    // for robustness (Debug format is title-case).
    match s.to_ascii_lowercase().as_str() {
        "index" => NodeKind::Index,
        "command" => NodeKind::Command,
        "concept" => NodeKind::Concept,
        "key" => NodeKind::Key,
        "project" => NodeKind::Project,
        "category" => NodeKind::Category,
        "lesson" => NodeKind::Lesson,
        "tutorial" => NodeKind::Tutorial,
        "meta" => NodeKind::Meta,
        "block" => NodeKind::Block,
        "scheme_api" | "schemeapi" => NodeKind::SchemeApi,
        "task" => NodeKind::Task,
        "view" => NodeKind::View,
        _ => NodeKind::Note,
    }
}

fn parse_search_hits(val: &Value) -> Vec<SearchHit> {
    val.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let id = v.get("id")?.as_str()?.to_string();
                    let score = v.get("score")?.as_f64().unwrap_or(0.0);
                    Some(SearchHit { id, score })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_links(val: &Value) -> Vec<Link> {
    val.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let src = v.get("src")?.as_str()?.to_string();
                    let dst = v.get("dst")?.as_str()?.to_string();
                    let rel_type = v.get("rel_type")?.as_str()?.to_string();
                    Some(Link {
                        src,
                        dst,
                        rel_type,
                        display: None,
                        weight: 1.0,
                        confidence: 1.0,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_node_from_json_basic() {
        let val = json!({
            "id": "concept:buffer",
            "title": "Buffer",
            "kind": "concept",
            "body": "A buffer holds text.",
            "tags": ["core", "editing"],
        });
        let node = parse_node_from_json(&val).unwrap();
        assert_eq!(node.id, "concept:buffer");
        assert_eq!(node.title, "Buffer");
        assert_eq!(node.tags, vec!["core", "editing"]);
    }

    #[test]
    fn parse_node_null_returns_none() {
        assert!(parse_node_from_json(&Value::Null).is_none());
    }

    #[test]
    fn parse_search_hits_basic() {
        let val = json!([
            {"id": "a", "score": 0.9},
            {"id": "b", "score": 0.5},
        ]);
        let hits = parse_search_hits(&val);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "a");
    }

    #[test]
    fn parse_links_basic() {
        let val = json!([
            {"src": "a", "dst": "b", "rel_type": "references"},
        ]);
        let links = parse_links(&val);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].src, "a");
        assert_eq!(links[0].dst, "b");
    }

    #[test]
    fn parse_node_kind_variants() {
        // Lowercase (as_str format — canonical)
        assert!(matches!(
            parse_node_kind("command"),
            crate::NodeKind::Command
        ));
        assert!(matches!(
            parse_node_kind("concept"),
            crate::NodeKind::Concept
        ));
        // Title-case (Debug format — also accepted)
        assert!(matches!(
            parse_node_kind("Command"),
            crate::NodeKind::Command
        ));
        assert!(matches!(
            parse_node_kind("scheme_api"),
            crate::NodeKind::SchemeApi
        ));
        assert!(matches!(parse_node_kind("unknown"), crate::NodeKind::Note));
    }
}
