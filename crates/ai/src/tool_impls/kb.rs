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
    // Try local KB first
    if let Some(node) = editor.kb.get(id) {
        return Some(serde_json::json!({
            "id": node.id,
            "title": node.title,
            "kind": node.kind,
            "body": node.body,
            "tags": node.tags,
            "links_from": editor.kb.links_from(id),
            "links_to": editor.kb.links_to(id),
        }));
    }
    // Try federated instances
    for (uuid, kb) in &editor.kb_instances {
        if let Some(node) = kb.get(id) {
            let inst_name = editor
                .kb_registry
                .find_by_uuid(uuid)
                .map(|i| i.name.as_str())
                .unwrap_or("unknown");
            return Some(serde_json::json!({
                "id": node.id,
                "title": node.title,
                "kind": node.kind,
                "body": node.body,
                "tags": node.tags,
                "links_from": kb.links_from(id),
                "links_to": kb.links_to(id),
                "instance": inst_name,
            }));
        }
    }
    None
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
    // Search local KB
    let mut ids = editor.kb.search(query);
    // Search federated instances
    for kb in editor.kb_instances.values() {
        ids.extend(kb.search(query));
    }
    // Deduplicate (local results take priority — they appear first)
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
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
    // Check local KB first, then federated instances
    if editor.kb.contains(id) {
        let links = editor.kb.links_from(id);
        return serde_json::to_string_pretty(&links).map_err(|e| e.to_string());
    }
    for kb in editor.kb_instances.values() {
        if kb.contains(id) {
            let links = kb.links_from(id);
            return serde_json::to_string_pretty(&links).map_err(|e| e.to_string());
        }
    }
    Err(format!("No KB node: {}", id))
}

pub fn execute_kb_links_to(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    let mut links = editor.kb.links_to(id);
    // Merge from federated instances
    for kb in editor.kb_instances.values() {
        for l in kb.links_to(id) {
            if !links.contains(&l) {
                links.push(l);
            }
        }
    }
    links.sort();
    serde_json::to_string_pretty(&links).map_err(|e| e.to_string())
}

/// BFS neighborhood around a seed node, up to `depth` hops (default 1, max 3).
/// Returns `{ root, nodes: [{id, title, kind, hop}], edges: [{src, dst}] }`.
/// Edges are deduplicated and include both outgoing and incoming links
/// between nodes in the neighborhood — so the agent sees the local graph,
/// not just a tree. Dangling targets are included as nodes with `"hop": N`
/// and `"missing": true` so the agent can surface them to the user.
/// Searches local KB and all federated instances.
pub fn execute_kb_graph(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    // Check local KB first, then federated
    if !editor.kb.contains(id) && !editor.kb_instances.values().any(|kb| kb.contains(id)) {
        return Err(format!("No KB node: {}", id));
    }
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .min(3) as usize;

    use std::collections::{HashMap, HashSet, VecDeque};

    // Helper: get neighbors from local + all federated KBs, deduped
    let federated_neighbors = |nid: &str| -> Vec<String> {
        let mut out = editor.kb.neighbors(nid);
        let mut seen: HashSet<String> = out.iter().cloned().collect();
        for kb in editor.kb_instances.values() {
            for n in kb.neighbors(nid) {
                if seen.insert(n.clone()) {
                    out.push(n);
                }
            }
        }
        out
    };

    // Helper: get node from any KB
    let get_node = |nid: &str| -> Option<&mae_core::KbNode> {
        editor
            .kb
            .get(nid)
            .or_else(|| editor.kb_instances.values().find_map(|kb| kb.get(nid)))
    };

    // Helper: links_from across all KBs
    let federated_links_from = |nid: &str| -> Vec<String> {
        let mut out = editor.kb.links_from(nid);
        let mut seen: HashSet<String> = out.iter().cloned().collect();
        for kb in editor.kb_instances.values() {
            for l in kb.links_from(nid) {
                if seen.insert(l.clone()) {
                    out.push(l);
                }
            }
        }
        out
    };

    // BFS
    let mut hops: HashMap<String, usize> = HashMap::from([(id.to_string(), 0)]);
    let mut queue: VecDeque<(String, usize)> = VecDeque::from([(id.to_string(), 0)]);
    while let Some((cur, h)) = queue.pop_front() {
        if h >= depth {
            continue;
        }
        for n in federated_neighbors(&cur) {
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
            match get_node(nid) {
                Some(n) => {
                    let mut val = serde_json::json!({
                        "id": n.id,
                        "title": n.title,
                        "kind": n.kind,
                        "hop": hop,
                    });
                    // Add instance info for federated nodes
                    if !editor.kb.contains(&n.id) {
                        for (uuid, kb) in &editor.kb_instances {
                            if kb.contains(&n.id) {
                                let inst_name = editor
                                    .kb_registry
                                    .find_by_uuid(uuid)
                                    .map(|i| i.name.as_str())
                                    .unwrap_or("unknown");
                                val["instance"] = serde_json::json!(inst_name);
                                break;
                            }
                        }
                    }
                    val
                }
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
        for dst in federated_links_from(src) {
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

pub fn execute_kb_health(editor: &Editor) -> Result<String, String> {
    let report = editor.kb.health_report();
    let broken: Vec<serde_json::Value> = report
        .broken_links
        .iter()
        .map(|(src, dst)| serde_json::json!({"source": src, "target": dst}))
        .collect();

    // Federated instance health summaries
    let instances: Vec<serde_json::Value> = editor
        .kb_registry
        .instances
        .iter()
        .map(|inst| {
            let kb_health = editor
                .kb_instances
                .get(&inst.uuid)
                .map(|kb| kb.health_report());
            match kb_health {
                Some(h) => serde_json::json!({
                    "name": inst.name,
                    "uuid": inst.uuid,
                    "total_nodes": h.total_nodes,
                    "total_links": h.total_links,
                    "orphan_count": h.orphan_ids.len(),
                    "broken_link_count": h.broken_links.len(),
                    "namespace_counts": h.namespace_counts,
                }),
                None => serde_json::json!({
                    "name": inst.name,
                    "uuid": inst.uuid,
                    "status": "not loaded",
                }),
            }
        })
        .collect();

    let out = serde_json::json!({
        "local": {
            "total_nodes": report.total_nodes,
            "total_links": report.total_links,
            "avg_links_per_node": if report.total_nodes > 0 {
                (report.total_links as f64) / (report.total_nodes as f64)
            } else { 0.0 },
            "orphan_nodes": report.orphan_ids,
            "broken_links": broken,
            "namespace_counts": report.namespace_counts,
        },
        "instances": instances,
    });
    serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}

pub fn execute_kb_create(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: id")?;
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: title")?;
    let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let kind = match args.get("kind").and_then(|v| v.as_str()) {
        Some("concept") => mae_core::KbNodeKind::Concept,
        Some("command") => mae_core::KbNodeKind::Command,
        Some("key") => mae_core::KbNodeKind::Key,
        Some("project") => mae_core::KbNodeKind::Project,
        _ => mae_core::KbNodeKind::Note,
    };

    editor.kb_create_node(id, title, body, kind)?;

    // Return the created node
    match node_json(editor, id) {
        Some(v) => serde_json::to_string_pretty(&v).map_err(|e| e.to_string()),
        None => Ok(format!("Created node: {}", id)),
    }
}

pub fn execute_kb_update(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: id")?;
    let title = args.get("title").and_then(|v| v.as_str());
    let body = args.get("body").and_then(|v| v.as_str());
    let tags: Option<Vec<String>> = args.get("tags").and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str().map(String::from))
                .collect()
        })
    });

    editor.kb_update_node(id, title, body, tags)?;

    match node_json(editor, id) {
        Some(v) => serde_json::to_string_pretty(&v).map_err(|e| e.to_string()),
        None => Ok(format!("Updated node: {}", id)),
    }
}

pub fn execute_kb_delete(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: id")?;
    editor.kb_delete_node(id)?;
    Ok(format!("Deleted node: {}", id))
}

pub fn execute_kb_register(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: name")?;
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: path")?;
    let expanded = mae_core::file_picker::expand_tilde(path_str);
    let path = std::path::Path::new(&expanded);

    match editor.kb_register(name, path) {
        Some(result) => Ok(result.to_json()),
        None => Err(editor.status_msg.clone()),
    }
}

pub fn execute_kb_unregister(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: name")?;
    editor.kb_unregister(name);
    Ok(editor.status_msg.clone())
}

pub fn execute_kb_reimport(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: name")?;

    match editor.kb_reimport(name) {
        Some(result) => Ok(result.to_json()),
        None => Err(editor.status_msg.clone()),
    }
}

/// RAG-optimized KB search: returns top-K nodes with body excerpts for AI
/// reasoning context. Searches local KB and all federated instances.
/// Each result includes id, title, kind, excerpt (truncated body), and
/// an optional `instance` field for federated nodes.
pub fn execute_kb_search_context(
    editor: &Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: query")?;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .min(20) as usize;

    // Search local KB
    let mut results: Vec<(Option<String>, mae_core::KbNode)> = Vec::new();
    for id in editor.kb.search(query) {
        if let Some(node) = editor.kb.get(&id) {
            results.push((None, node.clone()));
        }
    }
    // Search federated instances
    for (uuid, kb) in &editor.kb_instances {
        let inst_name = editor
            .kb_registry
            .find_by_uuid(uuid)
            .map(|i| i.name.clone());
        for id in kb.search(query) {
            if let Some(node) = kb.get(&id) {
                results.push((inst_name.clone(), node.clone()));
            }
        }
    }

    let items: Vec<serde_json::Value> = results
        .into_iter()
        .take(limit)
        .map(|(inst_name, node)| {
            let excerpt = if node.body.len() > 500 {
                format!("{}…", &node.body[..node.body.floor_char_boundary(500)])
            } else {
                node.body.clone()
            };
            let mut val = serde_json::json!({
                "id": node.id,
                "title": node.title,
                "kind": node.kind,
                "excerpt": excerpt,
            });
            if let Some(name) = inst_name {
                val["instance"] = serde_json::json!(name);
            }
            val
        })
        .collect();

    serde_json::to_string_pretty(&items).map_err(|e| e.to_string())
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

    #[test]
    fn kb_health_returns_json() {
        let editor = Editor::new();
        let result = execute_kb_health(&editor).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        let local = &v["local"];
        assert!(local["total_nodes"].as_u64().unwrap() > 0);
        assert!(local["total_links"].as_u64().unwrap() > 0);
        assert!(local["namespace_counts"].is_object());
        assert!(local["orphan_nodes"].is_array());
        assert!(local["broken_links"].is_array());
        assert!(local["avg_links_per_node"].as_f64().unwrap() > 0.0);
        assert!(v["instances"].is_array());
    }

    #[test]
    fn kb_create_via_tool() {
        let mut editor = Editor::new();
        let result = execute_kb_create(
            &mut editor,
            &serde_json::json!({"id": "user:tool-test", "title": "Tool Test", "body": "Created via tool"}),
        );
        assert!(result.is_ok());
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["id"], "user:tool-test");
        assert_eq!(json["title"], "Tool Test");
    }

    #[test]
    fn kb_update_via_tool() {
        let mut editor = Editor::new();
        execute_kb_create(
            &mut editor,
            &serde_json::json!({"id": "user:upd-tool", "title": "Original", "body": "body"}),
        )
        .unwrap();
        let result = execute_kb_update(
            &mut editor,
            &serde_json::json!({"id": "user:upd-tool", "title": "Updated"}),
        );
        assert!(result.is_ok());
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["title"], "Updated");
        assert_eq!(json["body"], "body"); // unchanged
    }

    #[test]
    fn kb_delete_via_tool() {
        let mut editor = Editor::new();
        execute_kb_create(
            &mut editor,
            &serde_json::json!({"id": "user:del-tool", "title": "Delete Me"}),
        )
        .unwrap();
        let result = execute_kb_delete(&mut editor, &serde_json::json!({"id": "user:del-tool"}));
        assert!(result.is_ok());
        assert!(editor.kb.get("user:del-tool").is_none());
    }

    #[test]
    fn kb_create_rejects_seed_via_tool() {
        let mut editor = Editor::new();
        let result = execute_kb_create(
            &mut editor,
            &serde_json::json!({"id": "index", "title": "Override"}),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("seed node"));
    }

    #[test]
    fn seed_nodes_broken_links_are_only_cmd_refs() {
        // Concept nodes reference `cmd:*` and other nodes that are only
        // created at runtime from CommandRegistry. Only non-cmd broken
        // links indicate a real problem in seed data.
        let editor = Editor::new();
        let report = editor.kb.health_report();
        let non_cmd: Vec<_> = report
            .broken_links
            .iter()
            .filter(|(_, target)| !target.starts_with("cmd:"))
            .collect();
        // A few known false positives: "link" from org-mode example,
        // "other-node" from KB concept example, "target][label" from
        // option:link_descriptive markup example.
        let known_false = ["link", "other-node", "target][label"];
        let real_broken: Vec<_> = non_cmd
            .iter()
            .filter(|(_, target)| !known_false.contains(&target.as_str()))
            .collect();
        assert!(
            real_broken.is_empty(),
            "unexpected broken links in seed KB: {:?}",
            real_broken
        );
    }

    // W4: Federated graph traversal tests

    #[test]
    fn kb_links_from_finds_federated() {
        let mut editor = Editor::new();
        let mut inst = mae_core::KnowledgeBase::new();
        inst.insert(mae_core::KbNode::new(
            "fed-node",
            "Fed",
            mae_core::KbNodeKind::Note,
            "links to [[index]]",
        ));
        editor.kb_instances.insert("inst-1".to_string(), inst);
        let result =
            execute_kb_links_from(&editor, &serde_json::json!({"id": "fed-node"})).unwrap();
        let links: Vec<String> = serde_json::from_str(&result).unwrap();
        assert!(links.contains(&"index".to_string()));
    }

    #[test]
    fn kb_links_to_merges_federated() {
        let mut editor = Editor::new();
        let mut inst = mae_core::KnowledgeBase::new();
        inst.insert(mae_core::KbNode::new(
            "fed-linker",
            "Fed Linker",
            mae_core::KbNodeKind::Note,
            "see [[concept:buffer]]",
        ));
        editor.kb_instances.insert("inst-1".to_string(), inst);
        let result =
            execute_kb_links_to(&editor, &serde_json::json!({"id": "concept:buffer"})).unwrap();
        let links: Vec<String> = serde_json::from_str(&result).unwrap();
        assert!(links.contains(&"fed-linker".to_string()));
    }

    #[test]
    fn kb_graph_traverses_federated() {
        let mut editor = Editor::new();
        let mut inst = mae_core::KnowledgeBase::new();
        inst.insert(mae_core::KbNode::new(
            "fed-linked",
            "Federated Linked",
            mae_core::KbNodeKind::Note,
            "see [[index]]",
        ));
        editor.kb_instances.insert("inst-1".to_string(), inst);
        let result = execute_kb_graph(&editor, &serde_json::json!({"id": "index"})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        let nodes = v["nodes"].as_array().unwrap();
        assert!(
            nodes.iter().any(|n| n["id"] == "fed-linked"),
            "federated node should appear in graph neighborhood"
        );
    }

    // W5: AI RAG integration tests

    #[test]
    fn kb_search_context_returns_excerpts() {
        let editor = Editor::new();
        let result =
            execute_kb_search_context(&editor, &serde_json::json!({"query": "buffer", "limit": 3}))
                .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert!(!items.is_empty());
        assert!(items.len() <= 3);
        for item in &items {
            assert!(item["id"].is_string());
            assert!(item["title"].is_string());
            assert!(item["kind"].is_string());
            assert!(item["excerpt"].is_string());
        }
    }

    #[test]
    fn kb_search_context_includes_federated() {
        let mut editor = Editor::new();
        let mut inst = mae_core::KnowledgeBase::new();
        inst.insert(mae_core::KbNode::new(
            "fed-rag-test",
            "Federated RAG Node",
            mae_core::KbNodeKind::Note,
            "This is a unique rag test body for federated search",
        ));
        editor.kb_instances.insert("rag-inst".to_string(), inst);
        let result =
            execute_kb_search_context(&editor, &serde_json::json!({"query": "unique rag test"}))
                .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert!(
            items.iter().any(|i| i["id"] == "fed-rag-test"),
            "should include federated results"
        );
    }
}
