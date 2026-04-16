//! mae-kb — in-memory knowledge base (graph store).
//!
//! The knowledge base is the shared data model for:
//!
//! 1. The built-in help system (command, concept, and keybinding docs).
//! 2. User-authored notes (org-roam-style bidirectional links).
//! 3. An AI-facing query surface — the agent is a *peer actor* that can
//!    read the same nodes the human reads via `:help`.
//!
//! ## Design
//!
//! - A **node** is a typed, named document with a markdown body.
//! - Links are embedded in the body as `[[id]]` or `[[id|display text]]`.
//! - The store keeps a reverse index so "what links to X?" is O(1).
//! - No persistence layer yet — everything is in-memory. The Phase-5
//!   SQLite-backed kb.db will replace the storage but preserve this API.
//!
//! This crate depends on no MAE internals — it's a pure data library
//! callable from `mae-core`, `mae-ai`, and eventually `mae-kb-persist`.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub mod org;
pub mod persist;
pub mod watch;
pub use org::IngestReport;
pub use persist::PersistError;

/// Kind of a node. Controls how the node is surfaced to the user
/// (e.g. command nodes show up in `describe-command`) and styled by
/// the renderer (e.g. concept nodes get a different sigil).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    /// The help index page (there is usually exactly one of these).
    Index,
    /// An editor command — seeded from `CommandRegistry` at startup.
    Command,
    /// An architectural concept (buffer, window, mode, AI-as-peer, …).
    Concept,
    /// A keybinding or key sequence documentation entry.
    Key,
    /// Free-form user note (org-roam-style).
    Note,
}

/// A single node in the knowledge base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Stable identifier — e.g. `"cmd:delete-line"`, `"concept:buffer"`,
    /// `"index"`. Slugs use `:` as namespace separator by convention.
    pub id: String,
    /// Human-readable title shown at the top of the help buffer.
    pub title: String,
    pub kind: NodeKind,
    /// Markdown body. May contain `[[link]]` markers that the renderer
    /// styles as hyperlinks.
    pub body: String,
    /// Freeform tags for filtering (e.g. `["movement", "vi"]`).
    pub tags: Vec<String>,
}

impl Node {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        kind: NodeKind,
        body: impl Into<String>,
    ) -> Self {
        Node {
            id: id.into(),
            title: title.into(),
            kind,
            body: body.into(),
            tags: Vec::new(),
        }
    }

    pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    /// Extract all `[[link]]` and `[[link|display]]` targets from the body.
    /// Returns the target ids in document order, deduplicated.
    pub fn links(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for (target, _) in parse_links(&self.body) {
            if seen.insert(target.clone()) {
                out.push(target);
            }
        }
        out
    }
}

/// A parsed link from a body: `(target_id, display_text)`.
/// Display text defaults to the target id if no `|display` override exists.
pub fn parse_links(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(end_rel) = body[i + 2..].find("]]") {
                let inner = &body[i + 2..i + 2 + end_rel];
                // Split on '|' for display-text override.
                let (target, display) = match inner.find('|') {
                    Some(bar) => (&inner[..bar], &inner[bar + 1..]),
                    None => (inner, inner),
                };
                let target = target.trim();
                if !target.is_empty() {
                    out.push((target.to_string(), display.trim().to_string()));
                }
                i += 2 + end_rel + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Pre-lowercased search cache for a single node. Populated at insert
/// time so `search()` doesn't re-allocate on every query — the dominant
/// cost in the naive implementation.
#[derive(Debug, Clone, Default)]
struct LowerCache {
    lowered_id: String,
    title: String,
    body: String,
    tags: Vec<String>,
}

impl LowerCache {
    fn from_node(n: &Node) -> Self {
        Self {
            lowered_id: n.id.to_lowercase(),
            title: n.title.to_lowercase(),
            body: n.body.to_lowercase(),
            tags: n.tags.iter().map(|t| t.to_lowercase()).collect(),
        }
    }
}

/// The in-memory knowledge base.
///
/// Stores nodes keyed by id and maintains a reverse index so
/// `links_to(id)` is cheap. The forward index is recomputed from the
/// body on every `insert` (cheap — bodies are small).
///
/// Also caches lowercased title/body/tags per node so `search()` is a
/// tight byte-scan with zero per-query allocation. At ~1500 nodes with
/// typical 500-byte bodies this keeps search sub-millisecond; a proper
/// FTS5 backend replaces this in Phase 5.
#[derive(Debug, Default, Clone)]
pub struct KnowledgeBase {
    nodes: HashMap<String, Node>,
    /// Reverse index: `links_in[target] = [source_ids…]`.
    links_in: HashMap<String, Vec<String>>,
    /// Pre-lowercased searchable fields, keyed by node id.
    lower: HashMap<String, LowerCache>,
}

impl KnowledgeBase {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn get(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// Insert (or overwrite) a node. Returns the previous node, if any.
    /// Rebuilds the reverse index entries for this node's links.
    pub fn insert(&mut self, node: Node) -> Option<Node> {
        let id = node.id.clone();
        // Remove old reverse edges (if any) before rebuilding.
        if let Some(prev) = self.nodes.get(&id) {
            for target in prev.links() {
                if let Some(sources) = self.links_in.get_mut(&target) {
                    sources.retain(|s| s != &id);
                    if sources.is_empty() {
                        self.links_in.remove(&target);
                    }
                }
            }
        }
        // Install new reverse edges.
        for target in node.links() {
            let entry = self.links_in.entry(target).or_default();
            if !entry.contains(&id) {
                entry.push(id.clone());
            }
        }
        self.lower.insert(id.clone(), LowerCache::from_node(&node));
        self.nodes.insert(id, node)
    }

    /// Remove a node. Also drops its outgoing reverse-index entries.
    pub fn remove(&mut self, id: &str) -> Option<Node> {
        let prev = self.nodes.remove(id)?;
        self.lower.remove(id);
        for target in prev.links() {
            if let Some(sources) = self.links_in.get_mut(&target) {
                sources.retain(|s| s != id);
                if sources.is_empty() {
                    self.links_in.remove(&target);
                }
            }
        }
        Some(prev)
    }

    /// All node ids, sorted. If `prefix` is provided, only ids starting
    /// with it are returned (useful for `cmd:` namespace listings).
    pub fn list_ids(&self, prefix: Option<&str>) -> Vec<String> {
        let mut ids: Vec<String> = self
            .nodes
            .keys()
            .filter(|k| prefix.is_none_or(|p| k.starts_with(p)))
            .cloned()
            .collect();
        ids.sort();
        ids
    }

    /// Case-insensitive substring search over title + body + tags.
    /// Returns matching ids sorted with title matches before body matches.
    ///
    /// Scans the pre-lowercased `LowerCache` populated at insert time —
    /// no per-query allocations, no per-node `to_lowercase()`.
    pub fn search(&self, query: &str) -> Vec<String> {
        if query.is_empty() {
            return self.list_ids(None);
        }
        let q = query.to_lowercase();
        let mut title_hits = Vec::new();
        let mut body_hits = Vec::new();
        for (id, cache) in self.lower.iter() {
            if cache.title.contains(&q) || cache.lowered_id.contains(&q) {
                title_hits.push(id.clone());
            } else if cache.body.contains(&q) || cache.tags.iter().any(|t| t.contains(&q)) {
                body_hits.push(id.clone());
            }
        }
        title_hits.sort();
        body_hits.sort();
        title_hits.extend(body_hits);
        title_hits
    }

    /// Outgoing links from a node (targets of `[[…]]` markers in its body).
    /// Returns link targets in document order. Dangling links (to missing
    /// nodes) are included — callers decide how to render them.
    pub fn links_from(&self, id: &str) -> Vec<String> {
        self.nodes.get(id).map(|n| n.links()).unwrap_or_default()
    }

    /// Combined outgoing + incoming neighbors of a node, deduplicated,
    /// with outgoing order preserved and backlinks appended after.
    /// Shared by the terminal-help "Tab cycles through all reachable
    /// nodes" UX and the AI's `kb_graph` BFS.
    pub fn neighbors(&self, id: &str) -> Vec<String> {
        let mut out = self.links_from(id);
        let mut seen: HashSet<String> = out.iter().cloned().collect();
        for src in self.links_to(id) {
            if seen.insert(src.clone()) {
                out.push(src);
            }
        }
        out
    }

    /// Iterator over all nodes (value-references) — used by persistence
    /// layers. Order is arbitrary; callers that need a stable order should
    /// collect and sort by id.
    pub(crate) fn nodes_values(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// Incoming links — node ids whose body references `target`.
    pub fn links_to(&self, target: &str) -> Vec<String> {
        let mut v = self.links_in.get(target).cloned().unwrap_or_default();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb_with(nodes: Vec<Node>) -> KnowledgeBase {
        let mut kb = KnowledgeBase::new();
        for n in nodes {
            kb.insert(n);
        }
        kb
    }

    #[test]
    fn empty_kb() {
        let kb = KnowledgeBase::new();
        assert_eq!(kb.len(), 0);
        assert!(kb.is_empty());
        assert!(kb.get("nope").is_none());
    }

    #[test]
    fn insert_and_get() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "Alpha", NodeKind::Note, "body"));
        assert_eq!(kb.len(), 1);
        assert_eq!(kb.get("a").unwrap().title, "Alpha");
    }

    #[test]
    fn insert_overwrites() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "first", NodeKind::Note, ""));
        kb.insert(Node::new("a", "second", NodeKind::Note, ""));
        assert_eq!(kb.len(), 1);
        assert_eq!(kb.get("a").unwrap().title, "second");
    }

    #[test]
    fn remove_drops_node() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "x", NodeKind::Note, "see [[b]]"));
        kb.insert(Node::new("b", "y", NodeKind::Note, ""));
        assert_eq!(kb.links_to("b"), vec!["a".to_string()]);
        kb.remove("a");
        assert!(kb.links_to("b").is_empty());
        assert!(kb.get("a").is_none());
    }

    #[test]
    fn parse_links_basic() {
        let links = parse_links("see [[foo]] and [[bar|Bar!]]");
        assert_eq!(
            links,
            vec![
                ("foo".to_string(), "foo".to_string()),
                ("bar".to_string(), "Bar!".to_string())
            ]
        );
    }

    #[test]
    fn parse_links_empty_target_ignored() {
        assert!(parse_links("[[]] and [[   ]]").is_empty());
    }

    #[test]
    fn parse_links_unclosed_bracket() {
        assert!(parse_links("[[foo").is_empty());
    }

    #[test]
    fn node_links_dedup() {
        let n = Node::new("x", "x", NodeKind::Note, "[[a]] [[a]] [[b]]");
        assert_eq!(n.links(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn links_to_reverse_index() {
        let kb = kb_with(vec![
            Node::new("a", "A", NodeKind::Note, "goto [[b]]"),
            Node::new("c", "C", NodeKind::Note, "also [[b]]"),
            Node::new("b", "B", NodeKind::Note, ""),
        ]);
        let mut incoming = kb.links_to("b");
        incoming.sort();
        assert_eq!(incoming, vec!["a", "c"]);
    }

    #[test]
    fn links_to_updates_on_overwrite() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "A", NodeKind::Note, "[[b]]"));
        assert_eq!(kb.links_to("b"), vec!["a".to_string()]);
        // Overwrite to point elsewhere.
        kb.insert(Node::new("a", "A", NodeKind::Note, "[[c]]"));
        assert!(kb.links_to("b").is_empty());
        assert_eq!(kb.links_to("c"), vec!["a".to_string()]);
    }

    #[test]
    fn links_from_returns_targets_in_order() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("x", "X", NodeKind::Note, "[[one]] and [[two]]"));
        assert_eq!(kb.links_from("x"), vec!["one", "two"]);
    }

    #[test]
    fn links_from_missing_node() {
        let kb = KnowledgeBase::new();
        assert!(kb.links_from("nope").is_empty());
    }

    #[test]
    fn list_ids_sorted() {
        let kb = kb_with(vec![
            Node::new("b", "", NodeKind::Note, ""),
            Node::new("a", "", NodeKind::Note, ""),
            Node::new("c", "", NodeKind::Note, ""),
        ]);
        assert_eq!(kb.list_ids(None), vec!["a", "b", "c"]);
    }

    #[test]
    fn list_ids_with_prefix() {
        let kb = kb_with(vec![
            Node::new("cmd:a", "", NodeKind::Command, ""),
            Node::new("cmd:b", "", NodeKind::Command, ""),
            Node::new("concept:x", "", NodeKind::Concept, ""),
        ]);
        assert_eq!(kb.list_ids(Some("cmd:")), vec!["cmd:a", "cmd:b"]);
    }

    #[test]
    fn search_finds_by_title() {
        let kb = kb_with(vec![
            Node::new("a", "Buffer concept", NodeKind::Concept, ""),
            Node::new("b", "Window concept", NodeKind::Concept, ""),
        ]);
        assert_eq!(kb.search("buffer"), vec!["a"]);
    }

    #[test]
    fn search_finds_by_body() {
        let kb = kb_with(vec![
            Node::new("a", "X", NodeKind::Note, "contains widget"),
            Node::new("b", "Y", NodeKind::Note, "nothing here"),
        ]);
        assert_eq!(kb.search("widget"), vec!["a"]);
    }

    #[test]
    fn search_title_beats_body() {
        let kb = kb_with(vec![
            Node::new("a", "Other", NodeKind::Note, "mentions foo"),
            Node::new("b", "Foo bar", NodeKind::Note, "unrelated"),
        ]);
        // Title match b should come before body match a.
        assert_eq!(kb.search("foo"), vec!["b", "a"]);
    }

    #[test]
    fn search_by_tag() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "A", NodeKind::Note, "").with_tags(["movement"]));
        assert_eq!(kb.search("movement"), vec!["a"]);
    }

    #[test]
    fn search_empty_returns_all() {
        let kb = kb_with(vec![
            Node::new("a", "", NodeKind::Note, ""),
            Node::new("b", "", NodeKind::Note, ""),
        ]);
        assert_eq!(kb.search(""), vec!["a", "b"]);
    }

    #[test]
    fn search_lower_cache_is_maintained_on_overwrite() {
        // Regression test for the LowerCache invariant: if a node's title
        // changes, the old title must no longer match searches.
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "Banana", NodeKind::Note, ""));
        assert_eq!(kb.search("banana"), vec!["a"]);
        kb.insert(Node::new("a", "Cherry", NodeKind::Note, ""));
        assert!(kb.search("banana").is_empty());
        assert_eq!(kb.search("cherry"), vec!["a"]);
    }

    #[test]
    fn search_lower_cache_dropped_on_remove() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "Banana", NodeKind::Note, ""));
        kb.remove("a");
        assert!(kb.search("banana").is_empty());
    }

    #[test]
    fn search_scales_to_two_thousand_nodes() {
        // Smoke-test that search returns under 50ms at 2000 nodes with
        // 500-char bodies. Primary value: catches accidental O(n²) regressions
        // when the cache is bypassed.
        let mut kb = KnowledgeBase::new();
        let body = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(10);
        for i in 0..2000 {
            let title = if i % 97 == 0 {
                format!("needle-{i}")
            } else {
                format!("generic title {i}")
            };
            kb.insert(Node::new(
                format!("n:{i}"),
                title,
                NodeKind::Note,
                body.clone(),
            ));
        }
        let start = std::time::Instant::now();
        let hits = kb.search("needle");
        let elapsed = start.elapsed();
        assert!(!hits.is_empty(), "should find needle entries");
        assert!(
            elapsed.as_millis() < 50,
            "search took {elapsed:?} over 2000 nodes; cache may be bypassed"
        );
    }

    #[test]
    fn dangling_link_is_listed() {
        let kb = kb_with(vec![Node::new("a", "A", NodeKind::Note, "[[missing]]")]);
        // links_from returns the dangling target — callers handle rendering.
        assert_eq!(kb.links_from("a"), vec!["missing"]);
        // And the reverse index records it too (so if you later add 'missing',
        // backlinks appear retroactively).
        assert_eq!(kb.links_to("missing"), vec!["a".to_string()]);
    }
}
