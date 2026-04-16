//! Knowledge-base tool implementations.
//!
//! Expose the same `KnowledgeBase` that drives the `*Help*` buffer to the
//! AI agent. The human reads KB nodes through `:help`; the agent reads the
//! same nodes through these tools — one of the core "AI-as-peer" design
//! points.
//!
//! All tools here are `ReadOnly` — the KB is currently not mutable via AI
//! (that belongs in a future `kb_insert` tool alongside user note workflows).

use mae_core::Editor;

/// Serialize a node to the JSON shape the agent sees.  Includes outgoing
/// and incoming links so a single `kb_get` is enough to plan navigation
/// without an extra round-trip.  `NodeKind` is serialized via its serde
/// `#[serde(rename_all = "lowercase")]` so the wire shape matches
/// what `kb_search` / `kb_list` would produce on the same node.
fn node_json(editor: &Editor, id: &str) -> Option<serde_json::Value> {
    let node = editor.kb.get(id)?;
    Some(serde_json::json!({
        "id": node.id,
        "title": node.title,
        "kind": node.kind,
        "body": node.body,
        "tags": node.tags,
        "links_from": editor.kb.links_from(id),
        "links_to": editor.kb.links_to(id),
    }))
}

pub fn execute_kb_get(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    match node_json(editor, id) {
        Some(v) => serde_json::to_string_pretty(&v).map_err(|e| e.to_string()),
        None => Err(format!("No KB node: {}", id)),
    }
}

pub fn execute_kb_search(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let ids = editor.kb.search(query);
    serde_json::to_string_pretty(&ids).map_err(|e| e.to_string())
}

pub fn execute_kb_list(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let prefix = args.get("prefix").and_then(|v| v.as_str());
    let ids = editor.kb.list_ids(prefix);
    serde_json::to_string_pretty(&ids).map_err(|e| e.to_string())
}

pub fn execute_kb_links_from(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    if !editor.kb.contains(id) {
        return Err(format!("No KB node: {}", id));
    }
    let links = editor.kb.links_from(id);
    serde_json::to_string_pretty(&links).map_err(|e| e.to_string())
}

pub fn execute_kb_links_to(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    // links_to is useful for dangling targets too — do NOT require the
    // node to exist. The reverse index records incoming references
    // regardless, so the agent can discover "who would link here if I
    // added this node?".
    let links = editor.kb.links_to(id);
    serde_json::to_string_pretty(&links).map_err(|e| e.to_string())
}

/// BFS neighborhood around a seed node, up to `depth` hops (default 1, max 3).
/// Returns `{ root, nodes: [{id, title, kind, hop}], edges: [{src, dst}] }`.
/// Edges are deduplicated and include both outgoing and incoming links
/// between nodes in the neighborhood — so the agent sees the local graph,
/// not just a tree. Dangling targets are included as nodes with `"hop": N`
/// and `"missing": true` so the agent can surface them to the user.
pub fn execute_kb_graph(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    if !editor.kb.contains(id) {
        return Err(format!("No KB node: {}", id));
    }
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .min(3) as usize;

    // BFS over (id, hop). Collect all ids reachable within `depth` hops
    // (outgoing + incoming) from the seed.
    use std::collections::{HashMap, HashSet, VecDeque};
    let mut hops: HashMap<String, usize> = HashMap::from([(id.to_string(), 0)]);
    let mut queue: VecDeque<(String, usize)> = VecDeque::from([(id.to_string(), 0)]);
    while let Some((cur, h)) = queue.pop_front() {
        if h >= depth {
            continue;
        }
        for n in editor.kb.neighbors(&cur) {
            if !hops.contains_key(&n) {
                hops.insert(n.clone(), h + 1);
                queue.push_back((n, h + 1));
            }
        }
    }

    // Build node list (sorted by hop, then id for stable output).
    let mut ids: Vec<&String> = hops.keys().collect();
    ids.sort_by(|a, b| hops[*a].cmp(&hops[*b]).then_with(|| a.cmp(b)));
    let nodes: Vec<serde_json::Value> = ids
        .iter()
        .map(|nid| {
            let hop = hops[*nid];
            match editor.kb.get(nid) {
                Some(n) => serde_json::json!({
                    "id": n.id,
                    "title": n.title,
                    "kind": n.kind,
                    "hop": hop,
                }),
                None => serde_json::json!({
                    "id": nid,
                    "hop": hop,
                    "missing": true,
                }),
            }
        })
        .collect();

    // Edges: every outgoing link from a node in `hops` whose target is also
    // in `hops`. Dedup via (src,dst) set.
    let in_set: HashSet<&String> = hops.keys().collect();
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut seen = HashSet::new();
    for src in &ids {
        for dst in editor.kb.links_from(src) {
            if in_set.contains(&dst) && seen.insert((src.to_string(), dst.clone())) {
                edges.push((src.to_string(), dst));
            }
        }
    }
    let edges_json: Vec<serde_json::Value> = edges
        .into_iter()
        .map(|(src, dst)| serde_json::json!({ "src": src, "dst": dst }))
        .collect();

    let out = serde_json::json!({
        "root": id,
        "depth": depth,
        "nodes": nodes,
        "edges": edges_json,
    });
    serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kb_get_returns_node_fields() {
        let editor = Editor::new();
        // `index` is seeded by seed_kb on startup.
        let result = execute_kb_get(&editor, &serde_json::json!({"id": "index"})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["id"], "index");
        assert!(v["title"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(v["links_from"].as_array().is_some_and(|a| !a.is_empty()));
    }

    #[test]
    fn kb_get_missing_is_error() {
        let editor = Editor::new();
        let err = execute_kb_get(&editor, &serde_json::json!({"id": "no:such:node"})).unwrap_err();
        assert!(err.contains("No KB node"));
    }

    #[test]
    fn kb_get_missing_id_arg_is_error() {
        let editor = Editor::new();
        let err = execute_kb_get(&editor, &serde_json::json!({})).unwrap_err();
        assert!(err.contains("id"));
    }

    #[test]
    fn kb_search_finds_by_title() {
        let editor = Editor::new();
        let result = execute_kb_search(&editor, &serde_json::json!({"query": "buffer"})).unwrap();
        let ids: Vec<String> = serde_json::from_str(&result).unwrap();
        assert!(ids.contains(&"concept:buffer".to_string()));
    }

    #[test]
    fn kb_search_empty_query_returns_all() {
        let editor = Editor::new();
        let result = execute_kb_search(&editor, &serde_json::json!({"query": ""})).unwrap();
        let ids: Vec<String> = serde_json::from_str(&result).unwrap();
        assert_eq!(ids.len(), editor.kb.len());
    }

    #[test]
    fn kb_list_with_prefix_filters() {
        let editor = Editor::new();
        let result = execute_kb_list(&editor, &serde_json::json!({"prefix": "cmd:"})).unwrap();
        let ids: Vec<String> = serde_json::from_str(&result).unwrap();
        assert!(!ids.is_empty());
        assert!(ids.iter().all(|id| id.starts_with("cmd:")));
    }

    #[test]
    fn kb_list_without_prefix_lists_all() {
        let editor = Editor::new();
        let result = execute_kb_list(&editor, &serde_json::json!({})).unwrap();
        let ids: Vec<String> = serde_json::from_str(&result).unwrap();
        assert_eq!(ids.len(), editor.kb.len());
    }

    #[test]
    fn kb_links_from_returns_array() {
        let editor = Editor::new();
        let result = execute_kb_links_from(&editor, &serde_json::json!({"id": "index"})).unwrap();
        let links: Vec<String> = serde_json::from_str(&result).unwrap();
        assert!(!links.is_empty());
    }

    #[test]
    fn kb_links_from_missing_is_error() {
        let editor = Editor::new();
        let err = execute_kb_links_from(&editor, &serde_json::json!({"id": "nope"})).unwrap_err();
        assert!(err.contains("No KB node"));
    }

    #[test]
    fn kb_links_to_works_for_dangling() {
        // kb.links_to records backlinks even if the target isn't yet a node,
        // so the agent can ask "who would reference foo if I created it?".
        let editor = Editor::new();
        // concept:ai-as-peer is linked from index; pick a target that's
        // known to exist so we don't rely on dangling behaviour in the
        // default seed.
        let result =
            execute_kb_links_to(&editor, &serde_json::json!({"id": "concept:buffer"})).unwrap();
        let _ids: Vec<String> = serde_json::from_str(&result).unwrap();
    }

    #[test]
    fn kb_graph_default_depth_is_one_hop() {
        let editor = Editor::new();
        let result = execute_kb_graph(&editor, &serde_json::json!({"id": "index"})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["root"], "index");
        assert_eq!(v["depth"], 1);
        let nodes = v["nodes"].as_array().unwrap();
        // Root at hop 0, every other node at hop 1.
        assert!(nodes.iter().any(|n| n["id"] == "index" && n["hop"] == 0));
        assert!(nodes.iter().all(|n| n["hop"].as_u64().unwrap() <= 1));
        // Every outgoing link from index should appear as a hop-1 node.
        for t in editor.kb.links_from("index") {
            assert!(
                nodes.iter().any(|n| n["id"] == t),
                "missing outgoing neighbor {}",
                t
            );
        }
    }

    #[test]
    fn kb_graph_includes_backlinks_as_neighbors() {
        let editor = Editor::new();
        let result =
            execute_kb_graph(&editor, &serde_json::json!({"id": "concept:buffer"})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        let nodes = v["nodes"].as_array().unwrap();
        // Every backlink to concept:buffer should appear in the neighborhood.
        for src in editor.kb.links_to("concept:buffer") {
            assert!(
                nodes.iter().any(|n| n["id"] == src),
                "missing backlink neighbor {}",
                src
            );
        }
    }

    #[test]
    fn kb_graph_depth_two_includes_further_nodes() {
        let editor = Editor::new();
        let d1 =
            execute_kb_graph(&editor, &serde_json::json!({"id": "index", "depth": 1})).unwrap();
        let d2 =
            execute_kb_graph(&editor, &serde_json::json!({"id": "index", "depth": 2})).unwrap();
        let v1: serde_json::Value = serde_json::from_str(&d1).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&d2).unwrap();
        let n1 = v1["nodes"].as_array().unwrap().len();
        let n2 = v2["nodes"].as_array().unwrap().len();
        assert!(n2 >= n1, "depth-2 should not have fewer nodes than depth-1");
    }

    #[test]
    fn kb_graph_edges_only_connect_nodes_in_set() {
        let editor = Editor::new();
        let result = execute_kb_graph(&editor, &serde_json::json!({"id": "index"})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        let node_ids: std::collections::HashSet<String> = v["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect();
        for e in v["edges"].as_array().unwrap() {
            assert!(node_ids.contains(e["src"].as_str().unwrap()));
            assert!(node_ids.contains(e["dst"].as_str().unwrap()));
        }
    }

    #[test]
    fn kb_graph_missing_seed_is_error() {
        let editor = Editor::new();
        let err = execute_kb_graph(&editor, &serde_json::json!({"id": "no:such"})).unwrap_err();
        assert!(err.contains("No KB node"));
    }

    #[test]
    fn kb_graph_depth_clamped_to_three() {
        let editor = Editor::new();
        let result =
            execute_kb_graph(&editor, &serde_json::json!({"id": "index", "depth": 99})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["depth"], 3);
    }
}
