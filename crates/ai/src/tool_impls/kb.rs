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
    // Use query layer (CozoDB-first) when available. A *miss* here must fall
    // through to the in-memory KB — a node joined over collab lives in
    // `primary` but may not be in the CozoDB query layer yet, so short-
    // circuiting on a query-layer miss made `kb_get` fail for joined nodes
    // even though `kb_update` could resolve them (I-9 read/write asymmetry).
    if let Some(q) = editor.kb.query_layer() {
        if let Some(node) = q.get(id) {
            let links_from: Vec<String> = q.links_from(id).into_iter().map(|l| l.dst).collect();
            let links_to: Vec<String> = q.links_to(id).into_iter().map(|l| l.src).collect();
            return Some(serde_json::json!({
                "id": node.id,
                "title": node.title,
                "kind": node.kind,
                "body": node.body,
                "tags": node.tags,
                "links_from": links_from,
                "links_to": links_to,
            }));
        }
    }
    // Fallback: in-memory KB
    if let Some(node) = editor.kb.primary.get(id) {
        return Some(serde_json::json!({
            "id": node.id,
            "title": node.title,
            "kind": node.kind,
            "body": node.body,
            "tags": node.tags,
            "links_from": editor.kb.primary.links_from(id),
            "links_to": editor.kb.primary.links_to(id),
        }));
    }
    // Try federated instances
    for (uuid, kb) in &editor.kb.instances {
        if let Some(node) = kb.get(id) {
            let inst_name = editor
                .kb
                .registry
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
        Some(v) => {
            let mut result = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
            if editor.kb.ai_visited_ids.contains(id) {
                result.push_str("\n\n[Note: You already visited this node. Use kb_graph with depth=2 for neighborhood traversal instead of manual link-following.]");
            }
            Ok(result)
        }
        None => Err(format!("No KB node: {}", id)),
    }
}

/// Record a KB node ID as visited by the AI agent (for cycle detection and
/// recency ordering).
pub fn record_kb_visit(editor: &mut Editor, id: &str) {
    editor.kb.ai_visited_ids.insert(id.to_string());
    editor.kb.record_visit(id);
}

pub fn execute_kb_search(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
    // Optional `scope` ("all" | "local" | "remote" | "<instance-name>") selects
    // which federated layers participate; an explicit arg wins, else the
    // `kb_search_scope` option default. Optional `limit` caps the returned objects.
    let scope = args
        .get("scope")
        .and_then(|v| v.as_str())
        .map(mae_kb::KbScope::parse)
        .unwrap_or_else(|| mae_kb::KbScope::parse(&editor.kb.search_scope));
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(editor.kb.search_max_results);

    // Use the scoped federated search (respects kb_search_sort). Return enriched
    // objects (id/title/kind/instance/excerpt) so the agent can choose a node
    // without a follow-up kb_get round-trip.
    let results = editor.kb_federated_search_scoped(query, &scope);
    let objs: Vec<serde_json::Value> = results
        .into_iter()
        .take(limit)
        .map(|(instance, node)| {
            serde_json::json!({
                "id": node.id,
                "title": node.title,
                "kind": node.kind.as_str(),
                "instance": instance,
                "excerpt": kb_excerpt(&node.body, 160),
            })
        })
        .collect();
    serde_json::to_string_pretty(&objs).map_err(|e| e.to_string())
}

/// First non-empty line of `body`, trimmed and truncated to `max` chars (on a
/// char boundary) with an ellipsis. Used for compact search-result previews.
fn kb_excerpt(body: &str, max: usize) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() <= max {
        return line.to_string();
    }
    let truncated: String = line.chars().take(max).collect();
    format!("{}…", truncated.trim_end())
}

pub fn execute_kb_list(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let prefix = args.get("prefix").and_then(|v| v.as_str());
    let ids = if let Some(q) = editor.kb.query_layer() {
        q.list_ids(prefix)
    } else {
        editor.kb.primary.list_ids(prefix)
    };
    serde_json::to_string_pretty(&ids).map_err(|e| e.to_string())
}

pub fn execute_kb_links_from(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    if let Some(q) = editor.kb.query_layer() {
        if !q.contains(id) {
            return Err(format!("No KB node: {}", id));
        }
        let links: Vec<serde_json::Value> = q
            .links_from(id)
            .into_iter()
            .map(|l| serde_json::json!({ "dst": l.dst, "rel_type": l.rel_type }))
            .collect();
        return serde_json::to_string_pretty(&links).map_err(|e| e.to_string());
    }
    // Fallback: in-memory KB
    if editor.kb.primary.contains(id) {
        let links = editor.kb.primary.links_from(id);
        return serde_json::to_string_pretty(&links).map_err(|e| e.to_string());
    }
    for kb in editor.kb.instances.values() {
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
    if let Some(q) = editor.kb.query_layer() {
        let links: Vec<serde_json::Value> = q
            .links_to(id)
            .into_iter()
            .map(|l| serde_json::json!({ "src": l.src, "rel_type": l.rel_type }))
            .collect();
        return serde_json::to_string_pretty(&links).map_err(|e| e.to_string());
    }
    // Fallback: in-memory KB
    let mut links = editor.kb.primary.links_to(id);
    for kb in editor.kb.instances.values() {
        for l in kb.links_to(id) {
            if !links.contains(&l) {
                links.push(l);
            }
        }
    }
    links.sort();
    serde_json::to_string_pretty(&links).map_err(|e| e.to_string())
}

/// Graph-relatedness: nodes structurally related to `id` (co-citation /
/// bibliographic coupling / shared tags), distinct from lexical `kb_search`.
/// Prefers the query layer (Cozo Datalog) and falls back to the in-memory KB.
/// Returns `[{id, title, kind, score}]` sorted by relatedness, capped to
/// `limit` (default 10). Relatedness is per-instance (graph edges don't cross
/// federated instances), matching `kb_graph`/`kb_links_from`.
pub fn execute_kb_related(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(10);

    // In-memory fallback: the owning instance computes relatedness (primary
    // wins, else whichever federated instance contains the node).
    let related_in_memory = |id: &str| -> Vec<(String, f64)> {
        if editor.kb.primary.contains(id) {
            return editor.kb.primary.related(id, limit);
        }
        for kb in editor.kb.instances.values() {
            if kb.contains(id) {
                return kb.related(id, limit);
            }
        }
        Vec::new()
    };

    let scored: Vec<(String, f64)> = match editor.kb.query_layer() {
        Some(q) if q.contains(id) => q.related(id, limit),
        _ => related_in_memory(id),
    };

    // Resolve title/kind for each result without an extra round-trip.
    let lookup = |rid: &str| -> (String, String) {
        if let Some(n) = editor.kb.primary.get(rid) {
            return (n.title.clone(), n.kind.as_str().to_string());
        }
        for kb in editor.kb.instances.values() {
            if let Some(n) = kb.get(rid) {
                return (n.title.clone(), n.kind.as_str().to_string());
            }
        }
        (String::new(), String::new())
    };

    let objs: Vec<serde_json::Value> = scored
        .into_iter()
        .map(|(rid, score)| {
            let (title, kind) = lookup(&rid);
            serde_json::json!({ "id": rid, "title": title, "kind": kind, "score": score })
        })
        .collect();
    serde_json::to_string_pretty(&objs).map_err(|e| e.to_string())
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

    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .min(3) as usize;

    use std::collections::{HashMap, HashSet, VecDeque};

    // Use query layer when available (CozoDB-first)
    if let Some(q) = editor.kb.query_layer() {
        if !q.contains(id) {
            return Err(format!("No KB node: {}", id));
        }

        // BFS using query layer
        let mut hops: HashMap<String, usize> = HashMap::from([(id.to_string(), 0)]);
        let mut queue: VecDeque<(String, usize)> = VecDeque::from([(id.to_string(), 0)]);
        while let Some((cur, h)) = queue.pop_front() {
            if h >= depth {
                continue;
            }
            // Neighbors = union of links_from destinations + links_to sources
            let mut neighbors: Vec<String> =
                q.links_from(&cur).into_iter().map(|l| l.dst).collect();
            let incoming: Vec<String> = q.links_to(&cur).into_iter().map(|l| l.src).collect();
            let mut seen_n: HashSet<String> = neighbors.iter().cloned().collect();
            for n in incoming {
                if seen_n.insert(n.clone()) {
                    neighbors.push(n);
                }
            }
            for n in neighbors {
                if !hops.contains_key(&n) {
                    hops.insert(n.clone(), h + 1);
                    queue.push_back((n, h + 1));
                }
            }
        }

        let mut ids: Vec<String> = hops.keys().cloned().collect();
        ids.sort_by(|a, b| hops[a].cmp(&hops[b]).then_with(|| a.cmp(b)));
        let nodes: Vec<serde_json::Value> = ids
            .iter()
            .map(|nid| {
                let hop = hops[nid];
                match q.get(nid) {
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

        let in_set: HashSet<&String> = hops.keys().collect();
        let mut edges: Vec<(String, String)> = Vec::new();
        let mut seen = HashSet::new();
        for src in &ids {
            for link in q.links_from(src) {
                if in_set.contains(&link.dst) && seen.insert((src.clone(), link.dst.clone())) {
                    edges.push((src.clone(), link.dst));
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
        return serde_json::to_string_pretty(&out).map_err(|e| e.to_string());
    }

    // Fallback: in-memory KB
    if !editor.kb.primary.contains(id) && !editor.kb.instances.values().any(|kb| kb.contains(id)) {
        return Err(format!("No KB node: {}", id));
    }

    let federated_neighbors = |nid: &str| -> Vec<String> {
        let mut out = editor.kb.primary.neighbors(nid);
        let mut seen: HashSet<String> = out.iter().cloned().collect();
        for kb in editor.kb.instances.values() {
            for n in kb.neighbors(nid) {
                if seen.insert(n.clone()) {
                    out.push(n);
                }
            }
        }
        out
    };

    let get_node = |nid: &str| -> Option<&mae_core::KbNode> {
        editor
            .kb
            .primary
            .get(nid)
            .or_else(|| editor.kb.instances.values().find_map(|kb| kb.get(nid)))
    };

    let federated_links_from = |nid: &str| -> Vec<String> {
        let mut out = editor.kb.primary.links_from(nid);
        let mut seen: HashSet<String> = out.iter().cloned().collect();
        for kb in editor.kb.instances.values() {
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
                    if !editor.kb.primary.contains(&n.id) {
                        for (uuid, kb) in &editor.kb.instances {
                            if kb.contains(&n.id) {
                                let inst_name = editor
                                    .kb
                                    .registry
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

fn broken_links_json(links: &[mae_core::BrokenLink]) -> serde_json::Value {
    use mae_core::BrokenLinkKind;

    // Classify and group for structured output.
    let items: Vec<serde_json::Value> = links
        .iter()
        .map(|b| {
            serde_json::json!({
                "source": b.source,
                "target": b.target,
                "display": b.display,
                "kind": match b.kind {
                    BrokenLinkKind::DeletedNode => "deleted_node",
                    BrokenLinkKind::MalformedId => "malformed_id",
                    BrokenLinkKind::TemplatePlaceholder => "template_placeholder",
                },
            })
        })
        .collect();

    // Summary counts by kind.
    let deleted = links
        .iter()
        .filter(|b| b.kind == BrokenLinkKind::DeletedNode)
        .count();
    let malformed = links
        .iter()
        .filter(|b| b.kind == BrokenLinkKind::MalformedId)
        .count();
    let placeholder = links
        .iter()
        .filter(|b| b.kind == BrokenLinkKind::TemplatePlaceholder)
        .count();

    serde_json::json!({
        "total": links.len(),
        "by_kind": {
            "deleted_node": deleted,
            "malformed_id": malformed,
            "template_placeholder": placeholder,
        },
        "items": items,
    })
}

pub fn execute_kb_health(editor: &Editor) -> Result<String, String> {
    // Build a cross-federation resolver: local KB checks federated instances.
    let report = editor
        .kb
        .primary
        .health_report_with(|id| editor.kb.instances.values().any(|kb| kb.contains(id)));

    // Federated instance health summaries — with full broken link detail.
    let instances: Vec<serde_json::Value> = editor
        .kb
        .registry
        .instances
        .iter()
        .map(|inst| {
            let kb_health = editor.kb.instances.get(&inst.uuid).map(|kb| {
                // Cross-federation: check local KB + other instances.
                kb.health_report_with(|id| {
                    if let Some(q) = editor.kb.query_layer() {
                        q.contains(id)
                    } else {
                        editor.kb.primary.contains(id)
                            || editor
                                .kb
                                .instances
                                .iter()
                                .any(|(uuid, other)| *uuid != inst.uuid && other.contains(id))
                    }
                })
            });
            match kb_health {
                Some(h) => serde_json::json!({
                    "name": inst.name,
                    "uuid": inst.uuid,
                    "total_nodes": h.total_nodes,
                    "total_links": h.total_links,
                    "orphan_count": h.orphan_ids.len(),
                    "broken_links": broken_links_json(&h.broken_links),
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
            "broken_links": broken_links_json(&report.broken_links),
            "namespace_counts": report.namespace_counts,
        },
        "instances": instances,
    });
    serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}

fn ghost_ids_json(ghosts: &[mae_kb::GhostNode]) -> serde_json::Value {
    serde_json::json!(ghosts
        .iter()
        .map(|g| serde_json::json!({
            "id": g.id,
            "title": g.title,
            "source_file": g.source_file.display().to_string(),
            "reason": "id_not_found_in_current_file_content",
        }))
        .collect::<Vec<_>>())
}

/// Stale nodes (`source_file` doesn't exist at all) are a DIFFERENT flavor of
/// the same "index doesn't match reality" problem `detect_ghost_ids` catches —
/// e.g. exactly what happens if a file with an already-ghosted id (from an
/// earlier in-place rename) is then itself renamed/deleted: its `source_file`
/// stops existing, so `detect_ghost_ids` (which only re-parses EXISTING
/// files) skips it, leaving it invisible to kb_id_audit unless this is
/// folded in too. Reported with the same shape plus a distinct `reason` so
/// callers can tell the two cases apart.
fn stale_nodes_json(stale: &[mae_kb::StaleNode]) -> serde_json::Value {
    serde_json::json!(stale
        .iter()
        .map(|s| serde_json::json!({
            "id": s.id,
            "title": s.title,
            "source_file": s.source_file.display().to_string(),
            "reason": "source_file_no_longer_exists",
        }))
        .collect::<Vec<_>>())
}

/// Union of ghost ids (id no longer in an existing file) and stale nodes
/// (file itself is gone) — the full set of "safe to remove" cleanup
/// candidates `kb_id_audit` surfaces per scope.
fn cleanup_candidates_json(kb: &mae_kb::KnowledgeBase) -> serde_json::Value {
    let mut out = ghost_ids_json(&kb.detect_ghost_ids())
        .as_array()
        .cloned()
        .unwrap_or_default();
    out.extend(
        stale_nodes_json(&kb.detect_stale_nodes())
            .as_array()
            .cloned()
            .unwrap_or_default(),
    );
    serde_json::json!(out)
}

/// Per-federated-instance sync/freshness diagnostics — the piece that lets you
/// self-diagnose "why didn't process B see my new node" without a source dive:
/// is `kb_notes_dir` even resolvable to a registered instance, is that
/// instance's filesystem watcher actually attached (not just "was one ever
/// expected" — `watcher_count` alone is ambiguous about that), and how long
/// since it last drained a real change.
pub fn execute_kb_sync_status(editor: &Editor) -> Result<String, String> {
    let notes_dir = editor.kb.notes_dir.clone();
    let notes_dir_canon = notes_dir
        .as_ref()
        .map(|d| d.canonicalize().unwrap_or_else(|_| d.clone()));
    let notes_dir_resolves_to = notes_dir_canon.as_ref().and_then(|dir_canon| {
        editor.kb.registry.instances.iter().find_map(|inst| {
            let inst_canon = inst
                .org_dir
                .canonicalize()
                .unwrap_or_else(|_| inst.org_dir.clone());
            (&inst_canon == dir_canon).then(|| inst.name.clone())
        })
    });

    let instances: Vec<serde_json::Value> = editor
        .kb
        .registry
        .instances
        .iter()
        .map(|inst| {
            let watcher_attached = editor.kb.watchers.contains_key(&inst.uuid);
            let attach_error = editor.kb.watcher_attach_errors.get(&inst.uuid).cloned();
            let seconds_since_last_drain = editor
                .kb
                .last_drain
                .get(&inst.uuid)
                .map(|t| t.elapsed().as_secs());
            serde_json::json!({
                "name": inst.name,
                "uuid": inst.uuid,
                "org_dir": inst.org_dir.display().to_string(),
                "watcher_attached": watcher_attached,
                "watcher_attach_error": attach_error,
                "seconds_since_last_drain": seconds_since_last_drain,
            })
        })
        .collect();

    let out = serde_json::json!({
        "kb_notes_dir": notes_dir.map(|d| d.display().to_string()),
        "kb_notes_dir_resolves_to_instance": notes_dir_resolves_to,
        "watcher_enabled": editor.kb.watcher_enabled,
        "watcher_stats": {
            "drain_count": editor.kb.watcher_stats.drain_count,
            "reimports_total": editor.kb.watcher_stats.reimports_total,
            "errors": editor.kb.watcher_stats.errors,
        },
        "instances": instances,
    });
    serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}

/// Detect ghost/stale ids across the primary KB and every federated instance —
/// see `KnowledgeBase::detect_ghost_ids`. More expensive than `kb_health`
/// (re-parses each distinct source file), so it's its own on-demand tool
/// rather than folded into the routinely-called health report.
pub fn execute_kb_id_audit(editor: &Editor) -> Result<String, String> {
    let instances: Vec<serde_json::Value> = editor
        .kb
        .registry
        .instances
        .iter()
        .map(|inst| match editor.kb.instances.get(&inst.uuid) {
            Some(kb) => serde_json::json!({
                "name": inst.name,
                "uuid": inst.uuid,
                "ghost_ids": cleanup_candidates_json(kb),
            }),
            None => serde_json::json!({
                "name": inst.name,
                "uuid": inst.uuid,
                "status": "not loaded",
            }),
        })
        .collect();

    let out = serde_json::json!({
        "local": { "ghost_ids": cleanup_candidates_json(&editor.kb.primary) },
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

pub fn execute_kb_set_role(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: id")?;
    let role = args
        .get("role")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: role")?;
    editor.kb_set_role(id, role)
}

pub fn execute_kb_set_ai_residency(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let kb = args
        .get("kb")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: kb")?;
    let policy_str = args
        .get("policy")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: policy")?;
    let policy = match policy_str {
        "open" => mae_kb::federation::AiResidency::Open,
        "local_models_only" => mae_kb::federation::AiResidency::LocalModelsOnly,
        other => {
            return Err(format!(
                "Invalid policy '{}': expected 'open' or 'local_models_only'",
                other
            ))
        }
    };
    editor.kb_set_ai_residency(kb, policy)
}

pub fn execute_kb_reimport(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: name")?;

    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .map(mae_kb::IngestMode::from_str_lossy);

    match editor.kb_reimport(name, mode) {
        Some(result) => Ok(result.to_json()),
        None => Err(editor.status_msg.clone()),
    }
}

/// Paragraph-aware excerpt: split by `\n\n`, accumulate until byte budget.
/// Falls back to `floor_char_boundary` truncation for flat bodies.
fn excerpt_body(body: &str, max_bytes: usize) -> String {
    if body.len() <= max_bytes {
        return body.to_string();
    }
    let paragraphs: Vec<&str> = body.split("\n\n").collect();
    if paragraphs.len() > 1 {
        let mut acc = String::new();
        for para in paragraphs {
            let trimmed = para.trim();
            if trimmed.is_empty() {
                continue;
            }
            if acc.len() + trimmed.len() + 2 > max_bytes {
                break;
            }
            if !acc.is_empty() {
                acc.push_str("\n\n");
            }
            acc.push_str(trimmed);
        }
        if !acc.is_empty() {
            return format!("{}…", acc);
        }
    }
    // Flat body — use char-boundary truncation
    format!("{}…", &body[..body.floor_char_boundary(max_bytes)])
}

/// Simple relevance score for RAG ranking.
fn score_node(query_lower: &str, node: &mae_core::KbNode) -> u32 {
    let mut score = 0u32;
    if node.title.to_lowercase().contains(query_lower) {
        score += 3;
    }
    for tag in &node.tags {
        if tag.to_lowercase().contains(query_lower) {
            score += 2;
        }
    }
    if score == 0 {
        score = 1; // body match (search already filtered)
    }
    score
}

/// RAG-optimized KB search: returns top-K nodes with body excerpts for AI
/// reasoning context. Searches local KB and all federated instances.
/// Deduplicated by node ID (local wins), ranked by relevance, with
/// paragraph-aware excerpts and low-result guidance.
pub fn execute_kb_search_context(
    editor: &Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: query")?;
    let configured_limit = editor.kb.search_max_results;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .min(configured_limit as u64) as usize;
    let excerpt_len = editor.kb.search_excerpt_length;

    // Deduplicated collection
    let mut seen = std::collections::HashSet::new();
    let mut results: Vec<(Option<String>, mae_core::KbNode, u32)> = Vec::new();
    let query_lower = query.to_lowercase();

    // Search using query layer (CozoDB-first) when available
    if let Some(q) = editor.kb.query_layer() {
        for hit in q.search(query, limit * 3) {
            if let Some(node) = q.get(&hit.id) {
                if seen.insert(node.id.clone()) {
                    let score = score_node(&query_lower, &node);
                    results.push((None, node, score));
                }
            }
        }
    } else {
        // Fallback: search local KB first (wins on duplicates)
        for id in editor.kb.primary.search(query) {
            if let Some(node) = editor.kb.primary.get(&id) {
                if seen.insert(node.id.clone()) {
                    let score = score_node(&query_lower, node);
                    results.push((None, node.clone(), score));
                }
            }
        }
        // Search federated instances
        for (uuid, kb) in &editor.kb.instances {
            let inst_name = editor
                .kb
                .registry
                .find_by_uuid(uuid)
                .map(|i| i.name.clone());
            for id in kb.search(query) {
                if let Some(node) = kb.get(&id) {
                    if seen.insert(node.id.clone()) {
                        let score = score_node(&query_lower, node);
                        results.push((inst_name.clone(), node.clone(), score));
                    }
                }
            }
        }
    }

    // Sort by score desc, then ID for deterministic tie-breaking
    results.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.id.cmp(&b.1.id)));

    // Context-budget-aware scaling
    let context_budget_pct = args
        .get("context_budget_pct")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    let (effective_limit, effective_excerpt_len, ids_only) = if context_budget_pct > 92 {
        (limit.min(3), 0, true)
    } else if context_budget_pct > 85 {
        (limit.min(3), excerpt_len / 2, false)
    } else {
        (limit, excerpt_len, false)
    };

    let items: Vec<serde_json::Value> = results
        .into_iter()
        .take(effective_limit)
        .map(|(inst_name, node, score)| {
            let mut val = serde_json::json!({
                "id": node.id,
                "title": node.title,
                "kind": node.kind,
                "score": score,
            });
            if !ids_only {
                val["excerpt"] = serde_json::json!(excerpt_body(&node.body, effective_excerpt_len));
            }
            if let Some(name) = inst_name {
                val["instance"] = serde_json::json!(name);
            }
            val
        })
        .collect();

    // Low-result guidance
    if items.is_empty() {
        let guidance = serde_json::json!({
            "results": [],
            "guidance": "No KB results. Try: broader query terms, `:kb-register` to add org directories, or `kb_search` for ID-only results."
        });
        return serde_json::to_string_pretty(&guidance).map_err(|e| e.to_string());
    }

    serde_json::to_string_pretty(&items).map_err(|e| e.to_string())
}

// --- Graph-native tools (delegate to KbStore trait) ---

pub fn execute_kb_shortest_path(
    editor: &Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let from = args
        .get("from")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: from".to_string())?;
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: to".to_string())?;
    let store = editor
        .kb
        .store
        .as_ref()
        .ok_or_else(|| "No KB store configured".to_string())?;
    match store.shortest_path(from, to) {
        Ok(path) => serde_json::to_string_pretty(&path).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn execute_kb_neighborhood(
    editor: &Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(2)
        .min(5) as u32;
    let store = editor
        .kb
        .store
        .as_ref()
        .ok_or_else(|| "No KB store configured".to_string())?;
    match store.neighborhood(id, depth) {
        Ok(subgraph) => {
            let out = serde_json::json!({
                "root": id,
                "depth": depth,
                "nodes": subgraph.nodes.iter().map(|(nid, title)| {
                    serde_json::json!({"id": nid, "title": title})
                }).collect::<Vec<_>>(),
                "edges": subgraph.edges.iter().map(|(src, dst, rel)| {
                    serde_json::json!({"src": src, "dst": dst, "rel_type": rel})
                }).collect::<Vec<_>>(),
            });
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

pub fn execute_kb_add_link(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let src = args
        .get("src")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: src".to_string())?;
    let dst = args
        .get("dst")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: dst".to_string())?;
    let rel_type = args
        .get("rel_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: rel_type".to_string())?;
    let weight = args.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0);

    // ADR-030: text is truth. Append the typed link into `src`'s body instead of
    // writing cozo's `links` relation directly -- the previous implementation did
    // exactly that (a direct store.add_typed_link call), producing a graph edge
    // with no corresponding source text: lost on any KB rebuild/reimport, and
    // per-peer divergent in collab mode since only the cozo projection changed,
    // never the CRDT text every peer actually converges on. Routing through
    // kb_update_node means this now round-trips through the same
    // parse_typed_links + replace_node_links projection every other write path
    // uses (fixed for the single-user case in the same change that added this).
    let current_body = node_json(editor, src)
        .and_then(|v| v.get("body").and_then(|b| b.as_str()).map(str::to_string))
        .ok_or_else(|| format!("No KB node: {}", src))?;
    let link_line = format!("\n[[{dst}?rel={rel_type}&w={weight}][{dst}]]");
    let new_body = format!("{current_body}{link_line}");
    editor.kb_update_node(src, None, Some(&new_body), None)?;

    Ok(serde_json::json!({
        "status": "ok",
        "src": src,
        "dst": dst,
        "rel_type": rel_type,
        "weight": weight,
    })
    .to_string())
}

pub fn execute_kb_raw_query(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: query".to_string())?;
    let store = editor
        .kb
        .store
        .as_ref()
        .ok_or_else(|| "No KB store configured".to_string())?;
    match store.raw_query(query) {
        Ok((headers, rows)) => {
            let out = serde_json::json!({
                "backend": store.backend_name(),
                "headers": headers,
                "rows": rows,
                "row_count": rows.len(),
            });
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

// --- v0.12.0 graph KB tools ---

pub fn execute_kb_agenda(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let filter_type = args
        .get("filter")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: filter".to_string())?;
    let value = args.get("value").and_then(|v| v.as_str()).unwrap_or("");

    let filter = match filter_type {
        "todo" => {
            if value.is_empty() {
                mae_kb::AgendaFilter::Todo(None)
            } else {
                mae_kb::AgendaFilter::Todo(Some(value.to_string()))
            }
        }
        "priority" => {
            let c = value.chars().next().unwrap_or('A');
            mae_kb::AgendaFilter::Priority(c)
        }
        "tag" => mae_kb::AgendaFilter::Tag(value.to_string()),
        "stale" => {
            let days = value.parse::<u32>().unwrap_or(30);
            mae_kb::AgendaFilter::Stale(days)
        }
        "orphan" => mae_kb::AgendaFilter::Orphan,
        "dead_end" => mae_kb::AgendaFilter::DeadEnd,
        "missing_role" => mae_kb::AgendaFilter::MissingRole,
        "weakly_linked" => {
            let n = value.parse::<u32>().unwrap_or(2);
            mae_kb::AgendaFilter::WeaklyLinked(n)
        }
        "custom" => mae_kb::AgendaFilter::Custom(value.to_string()),
        _ => return Err(format!("Unknown filter type: {filter_type}")),
    };

    let store = editor
        .kb
        .store
        .as_ref()
        .ok_or_else(|| "No KB store configured".to_string())?;
    let nodes = store.agenda_query(&filter).map_err(|e| e.to_string())?;
    let out: Vec<serde_json::Value> = nodes
        .iter()
        .map(|n| {
            serde_json::json!({
                "id": n.id,
                "title": n.title,
                "kind": format!("{:?}", n.kind),
                "todo_state": n.todo_state,
                "priority": n.priority.map(|c| c.to_string()),
                "tags": n.tags,
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "filter": filter_type,
        "count": out.len(),
        "nodes": out,
    }))
    .map_err(|e| e.to_string())
}

pub fn execute_kb_history(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

    let store = editor
        .kb
        .store
        .as_ref()
        .ok_or_else(|| "No KB store configured".to_string())?;
    let versions = store.node_history(id, limit).map_err(|e| e.to_string())?;
    let out: Vec<serde_json::Value> = versions
        .iter()
        .map(|v| {
            serde_json::json!({
                "version": v.version,
                "title": v.title,
                "change_summary": v.change_summary,
                "content_hash": v.content_hash,
                "author": v.author,
                "created_at": v.created_at,
                "integrity_ok": v.verify_integrity(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "id": id,
        "version_count": out.len(),
        "versions": out,
    }))
    .map_err(|e| e.to_string())
}

pub fn execute_kb_restore(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    let version = args
        .get("version")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "Missing required argument: version".to_string())?;

    let store = editor
        .kb
        .store
        .as_ref()
        .ok_or_else(|| "No KB store configured".to_string())?;
    store
        .restore_version(id, version)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "status": "restored",
        "id": id,
        "restored_to_version": version,
    })
    .to_string())
}

/// `CozoKbStore::raw_query` returns Debug-formatted `DataValue`s — string cells
/// come back quoted and escaped (e.g. `"?[...] kind = \"task\""`, or the
/// `Str("...")` variant). Recover the underlying string for cells we use as-is,
/// notably a stored Datalog query (running the quoted form fails at position 0
/// on the leading quote). Non-string cells pass through unchanged.
fn unquote_dv(s: &str) -> String {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix("Str(\"").and_then(|x| x.strip_suffix("\")")) {
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return s[1..s.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
    }
    s.to_string()
}

pub fn execute_kb_view_query(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let view_id = args
        .get("view_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: view_id".to_string())?;

    let store = editor
        .kb
        .store
        .as_ref()
        .ok_or_else(|| "No KB store configured".to_string())?;

    // Get the view definition from the views relation
    let (_headers, rows) = store
        .raw_query(&format!(
            "?[title, kind, query, display_config_json] := *views{{id, title, kind, query, display_config_json}}, id = \"{view_id}\""
        ))
        .map_err(|e| e.to_string())?;

    if rows.is_empty() {
        return Err(format!("View not found: {view_id}"));
    }

    // raw_query Debug-formats cells, so these come back quoted/escaped. Recover
    // the clean strings — the query in particular must be unquoted or executing
    // it fails at position 0 on the leading quote.
    let title = unquote_dv(&rows[0].first().cloned().unwrap_or_default());
    let kind = unquote_dv(&rows[0].get(1).cloned().unwrap_or_default());
    let query = unquote_dv(&rows[0].get(2).cloned().unwrap_or_default());
    let config = unquote_dv(&rows[0].get(3).cloned().unwrap_or_default());

    if query.trim().is_empty() {
        return Err(format!(
            "View '{view_id}' has no query defined (stale or unseeded KB store; try :kb-rebuild)"
        ));
    }

    // Execute the view's query
    let (result_headers, result_rows) = store.raw_query(&query).map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "view_id": view_id,
        "title": title,
        "kind": kind,
        "display_config": config,
        "headers": result_headers,
        "rows": result_rows,
        "row_count": result_rows.len(),
    })
    .to_string())
}

pub fn execute_kb_vector_search(
    editor: &Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    // Semantic/vector search is the third search modality (alongside lexical
    // `kb_search` and graph `kb_related`). It shares their contract — `scope`
    // and `limit` are accepted and validated here so the API shape is stable —
    // but the ranked path is stubbed: the HNSW index + store/search APIs and
    // the 0..1 score band are ready, yet no embedding provider is wired, so we
    // can't embed the query. Fail gracefully and steer to the modalities that
    // DO work rather than erroring opaquely.
    let _scope = args
        .get("scope")
        .and_then(|v| v.as_str())
        .map(mae_kb::KbScope::parse)
        .unwrap_or_else(|| mae_kb::KbScope::parse(&editor.kb.search_scope));
    let _limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(editor.kb.search_max_results);
    Err(
        "Semantic (vector) search is unavailable: no embedding provider is \
         configured, so the query can't be embedded. The HNSW index and 0..1 \
         score contract are ready for when one is wired. For now use kb_search \
         (lexical relevance) or kb_related (graph relatedness) instead."
            .to_string(),
    )
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
    fn kb_set_ai_residency_valid_call() {
        let mut editor = Editor::new();
        let result = execute_kb_set_ai_residency(
            &mut editor,
            &serde_json::json!({"kb": "primary", "policy": "local_models_only"}),
        )
        .unwrap();
        assert!(result.contains("local_models_only"), "result was: {result}");
        assert_eq!(
            editor.kb.registry.primary_ai_residency,
            mae_kb::federation::AiResidency::LocalModelsOnly
        );
    }

    #[test]
    fn kb_set_ai_residency_invalid_policy_is_error() {
        let mut editor = Editor::new();
        let err = execute_kb_set_ai_residency(
            &mut editor,
            &serde_json::json!({"kb": "primary", "policy": "not-a-real-policy"}),
        )
        .unwrap_err();
        assert!(err.contains("Invalid policy"), "err was: {err}");
        // Rejected before touching the registry — must not have mutated anything.
        assert_eq!(
            editor.kb.registry.primary_ai_residency,
            mae_kb::federation::AiResidency::Open
        );
    }

    #[test]
    fn kb_set_ai_residency_missing_kb_arg_is_error() {
        let mut editor = Editor::new();
        let err = execute_kb_set_ai_residency(&mut editor, &serde_json::json!({"policy": "open"}))
            .unwrap_err();
        assert!(err.contains("kb"), "err was: {err}");
    }

    #[test]
    fn kb_set_ai_residency_missing_policy_arg_is_error() {
        let mut editor = Editor::new();
        let err = execute_kb_set_ai_residency(&mut editor, &serde_json::json!({"kb": "primary"}))
            .unwrap_err();
        assert!(err.contains("policy"), "err was: {err}");
    }

    #[test]
    fn kb_set_ai_residency_unknown_instance_is_error() {
        let mut editor = Editor::new();
        let err = execute_kb_set_ai_residency(
            &mut editor,
            &serde_json::json!({"kb": "does-not-exist", "policy": "open"}),
        )
        .unwrap_err();
        assert!(err.contains("no instance found"), "err was: {err}");
    }

    #[test]
    fn kb_add_link_writes_adr030_grammar_into_body_not_direct_cozo() {
        // Regression for the ADR-030 violation this fix closes: kb_add_link used to
        // call store.add_typed_link() directly -- a graph edge with no corresponding
        // source text, lost on any KB rebuild/reimport and per-peer divergent in
        // collab mode. It must now append the typed-link grammar into the source
        // node's body and go through kb_update_node (the same path M4.1 fixed).
        let mut editor = Editor::new();
        editor
            .kb_create_node(
                "note:link-src",
                "Src",
                "Original body.",
                mae_kb::NodeKind::Note,
            )
            .unwrap();
        editor
            .kb_create_node("note:link-dst", "Dst", "", mae_kb::NodeKind::Note)
            .unwrap();

        let result = execute_kb_add_link(
            &mut editor,
            &serde_json::json!({"src": "note:link-src", "dst": "note:link-dst", "rel_type": "teaches", "weight": 0.7}),
        )
        .unwrap();
        assert!(result.contains("teaches"), "result was: {result}");

        let node = editor.kb.primary.get("note:link-src").unwrap();
        assert!(
            node.body.contains("Original body."),
            "existing body content must be preserved, not overwritten"
        );
        assert!(
            node.body.contains("note:link-dst?rel=teaches&w=0.7"),
            "typed-link grammar must be written into the body text, body was: {}",
            node.body
        );

        // And it must actually be PROJECTED correctly (target resolved, `?query`
        // stripped) -- the in-memory KnowledgeBase's links_from only tracks target
        // ids, not rel_type/weight; the typed-link grammar's actual rel_type/weight
        // projection is what `insert_node_projects_adr030_typed_link_grammar_from_body`
        // (shared/kb/src/cozo_store.rs) verifies at the store level.
        let links = editor.kb.primary.links_from("note:link-src");
        assert_eq!(links, vec!["note:link-dst".to_string()]);
    }

    #[test]
    fn kb_add_link_appends_without_clobbering_multiple_links() {
        let mut editor = Editor::new();
        editor
            .kb_create_node("note:multi-src", "Src", "Body.", mae_kb::NodeKind::Note)
            .unwrap();
        editor
            .kb_create_node("note:multi-a", "A", "", mae_kb::NodeKind::Note)
            .unwrap();
        editor
            .kb_create_node("note:multi-b", "B", "", mae_kb::NodeKind::Note)
            .unwrap();

        execute_kb_add_link(
            &mut editor,
            &serde_json::json!({"src": "note:multi-src", "dst": "note:multi-a", "rel_type": "references"}),
        )
        .unwrap();
        execute_kb_add_link(
            &mut editor,
            &serde_json::json!({"src": "note:multi-src", "dst": "note:multi-b", "rel_type": "extends"}),
        )
        .unwrap();

        let links = editor.kb.primary.links_from("note:multi-src");
        assert_eq!(links.len(), 2);
        assert!(links.contains(&"note:multi-a".to_string()));
        assert!(links.contains(&"note:multi-b".to_string()));
    }

    #[test]
    fn kb_add_link_unknown_src_is_error() {
        let mut editor = Editor::new();
        editor
            .kb_create_node("note:dst-only", "Dst", "", mae_kb::NodeKind::Note)
            .unwrap();
        let err = execute_kb_add_link(
            &mut editor,
            &serde_json::json!({"src": "note:does-not-exist", "dst": "note:dst-only", "rel_type": "teaches"}),
        )
        .unwrap_err();
        assert!(err.contains("No KB node"), "err was: {err}");
    }

    #[test]
    fn kb_add_link_missing_args_are_errors() {
        let mut editor = Editor::new();
        assert!(execute_kb_add_link(
            &mut editor,
            &serde_json::json!({"dst": "x", "rel_type": "y"})
        )
        .is_err());
        assert!(execute_kb_add_link(
            &mut editor,
            &serde_json::json!({"src": "x", "rel_type": "y"})
        )
        .is_err());
        assert!(
            execute_kb_add_link(&mut editor, &serde_json::json!({"src": "x", "dst": "y"})).is_err()
        );
    }

    #[test]
    fn kb_set_role_valid_call() {
        let mut editor = Editor::new();
        editor
            .kb_create_node(
                "note:role-tool-test",
                "Test",
                "body",
                mae_kb::NodeKind::Note,
            )
            .unwrap();
        let result = execute_kb_set_role(
            &mut editor,
            &serde_json::json!({"id": "note:role-tool-test", "role": "hub"}),
        )
        .unwrap();
        assert!(result.contains("hub"), "result was: {result}");
        assert_eq!(
            editor
                .kb
                .primary
                .get("note:role-tool-test")
                .unwrap()
                .properties
                .get("role"),
            Some(&"hub".to_string())
        );
    }

    #[test]
    fn kb_set_role_invalid_role_is_error() {
        let mut editor = Editor::new();
        editor
            .kb_create_node("note:role-tool-bad", "Test", "body", mae_kb::NodeKind::Note)
            .unwrap();
        let err = execute_kb_set_role(
            &mut editor,
            &serde_json::json!({"id": "note:role-tool-bad", "role": "not-a-real-role"}),
        )
        .unwrap_err();
        assert!(err.contains("Invalid role"), "err was: {err}");
    }

    #[test]
    fn kb_set_role_missing_id_arg_is_error() {
        let mut editor = Editor::new();
        let err =
            execute_kb_set_role(&mut editor, &serde_json::json!({"role": "atom"})).unwrap_err();
        assert!(err.contains("id"), "err was: {err}");
    }

    #[test]
    fn kb_set_role_missing_role_arg_is_error() {
        let mut editor = Editor::new();
        let err =
            execute_kb_set_role(&mut editor, &serde_json::json!({"id": "index"})).unwrap_err();
        assert!(err.contains("role"), "err was: {err}");
    }

    #[test]
    fn kb_set_role_unknown_node_is_error() {
        let mut editor = Editor::new();
        let err = execute_kb_set_role(
            &mut editor,
            &serde_json::json!({"id": "does-not-exist", "role": "atom"}),
        )
        .unwrap_err();
        assert!(err.contains("No KB node"), "err was: {err}");
    }

    #[test]
    fn unquote_dv_recovers_clean_strings() {
        // Debug-quoted string with escaped inner quotes — the shape a stored
        // view query comes back as from raw_query (this is what broke
        // kb_view_query: the leading quote made the Datalog parser fail at 0).
        assert_eq!(unquote_dv("\"a \\\"b\\\" c\""), "a \"b\" c");
        // Str("...") DataValue Debug variant.
        assert_eq!(unquote_dv("Str(\"hello\")"), "hello");
        // Already-clean / non-string values pass through unchanged.
        assert_eq!(unquote_dv("42"), "42");
        assert_eq!(unquote_dv("plain"), "plain");
    }

    /// Pull the `id` field out of each kb_search result object.
    fn kb_search_ids(result: &str) -> Vec<String> {
        let objs: Vec<serde_json::Value> = serde_json::from_str(result).unwrap();
        objs.into_iter()
            .map(|o| o["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn kb_search_finds_by_title() {
        let editor = Editor::new();
        let result = execute_kb_search(&editor, &serde_json::json!({"query": "buffer"})).unwrap();
        let ids = kb_search_ids(&result);
        // Enriched results now rank the canonical concept node first.
        assert_eq!(ids.first().map(String::as_str), Some("concept:buffer"));
        // Each result object carries the enriched fields.
        let objs: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert!(objs.iter().all(|o| o.get("title").is_some()
            && o.get("kind").is_some()
            && o.get("excerpt").is_some()));
    }

    #[test]
    fn kb_search_empty_query_returns_bounded() {
        let editor = Editor::new();
        let result = execute_kb_search(&editor, &serde_json::json!({"query": ""})).unwrap();
        let ids = kb_search_ids(&result);
        // Empty query lists nodes but is bounded by the result cap (kb_list is
        // the unbounded enumeration tool).
        assert!(!ids.is_empty());
        assert!(ids.len() <= editor.kb.search_max_results);
    }

    #[test]
    fn kb_search_respects_explicit_limit() {
        let editor = Editor::new();
        let result =
            execute_kb_search(&editor, &serde_json::json!({"query": "buffer", "limit": 3}))
                .unwrap();
        let ids = kb_search_ids(&result);
        assert!(ids.len() <= 3);
    }

    #[test]
    fn kb_search_local_scope_excludes_federated() {
        // With no federated instances, local scope behaves like all.
        let editor = Editor::new();
        let all = execute_kb_search(&editor, &serde_json::json!({"query": "buffer"})).unwrap();
        let local = execute_kb_search(
            &editor,
            &serde_json::json!({"query": "buffer", "scope": "local"}),
        )
        .unwrap();
        assert_eq!(kb_search_ids(&all), kb_search_ids(&local));
    }

    #[test]
    fn kb_related_returns_scored_objects() {
        let editor = Editor::new();
        // concept:buffer is a well-connected manual node; it should have
        // related neighbors via the seeded link graph.
        let result =
            execute_kb_related(&editor, &serde_json::json!({"id": "concept:buffer"})).unwrap();
        let objs: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert!(
            !objs.is_empty(),
            "expected related nodes for concept:buffer"
        );
        // Each object carries id/title/kind/score and excludes the seed itself.
        for o in &objs {
            assert!(o.get("id").and_then(|v| v.as_str()).is_some());
            assert!(o.get("score").and_then(|v| v.as_f64()).is_some());
            assert_ne!(o["id"].as_str(), Some("concept:buffer"));
        }
        // Scores are sorted descending.
        let scores: Vec<f64> = objs.iter().map(|o| o["score"].as_f64().unwrap()).collect();
        assert!(scores.windows(2).all(|w| w[0] >= w[1]), "scores not sorted");
    }

    #[test]
    fn kb_vector_search_fails_gracefully_and_points_to_alternatives() {
        let editor = Editor::new();
        // Accepts the shared scope/limit contract without panicking, and the
        // error steers to the working modalities rather than failing opaquely.
        let err = execute_kb_vector_search(
            &editor,
            &serde_json::json!({"query": "buffers", "scope": "local", "limit": 5}),
        )
        .unwrap_err();
        assert!(err.contains("kb_search"), "should suggest lexical search");
        assert!(
            err.contains("kb_related"),
            "should suggest graph relatedness"
        );
    }

    #[test]
    fn kb_related_respects_limit() {
        let editor = Editor::new();
        let result = execute_kb_related(
            &editor,
            &serde_json::json!({"id": "concept:buffer", "limit": 2}),
        )
        .unwrap();
        let objs: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert!(objs.len() <= 2);
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
        assert_eq!(ids.len(), editor.kb.primary.len());
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
        for t in editor.kb.primary.links_from("index") {
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
        for src in editor.kb.primary.links_to("concept:buffer") {
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
        assert!(local["broken_links"].is_object());
        assert!(local["broken_links"]["items"].is_array());
        assert!(local["broken_links"]["by_kind"].is_object());
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
        assert!(editor.kb.primary.get("user:del-tool").is_none());
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
        let report = editor.kb.primary.health_report();
        let non_cmd: Vec<_> = report
            .broken_links
            .iter()
            .filter(|b| !b.target.starts_with("cmd:"))
            .collect();
        // A few known false positives: "link" from org-mode example,
        // "other-node" from KB concept example, "target][label" from
        // option:link_descriptive markup example.
        let known_false = ["link", "other-node", "target][label"];
        let real_broken: Vec<_> = non_cmd
            .iter()
            .filter(|b| !known_false.contains(&b.target.as_str()))
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
        editor.kb.instances.insert("inst-1".to_string(), inst);
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
        editor.kb.instances.insert("inst-1".to_string(), inst);
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
        editor.kb.instances.insert("inst-1".to_string(), inst);
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
        editor.kb.instances.insert("rag-inst".to_string(), inst);
        let result =
            execute_kb_search_context(&editor, &serde_json::json!({"query": "unique rag test"}))
                .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert!(
            items.iter().any(|i| i["id"] == "fed-rag-test"),
            "should include federated results"
        );
    }

    // --- W2: RAG reliability tests ---

    #[test]
    fn kb_search_context_deduplicates() {
        let mut editor = Editor::new();
        // Insert same node locally and in federated
        editor
            .kb_create_node(
                "user:rag-dedup",
                "RAG Dedup",
                "dedup test body",
                mae_core::KbNodeKind::Note,
            )
            .unwrap();
        let mut inst = mae_core::KnowledgeBase::new();
        inst.insert(mae_core::KbNode::new(
            "user:rag-dedup",
            "RAG Dedup",
            mae_core::KbNodeKind::Note,
            "dedup test body",
        ));
        editor.kb.instances.insert("dedup-inst".to_string(), inst);
        let result =
            execute_kb_search_context(&editor, &serde_json::json!({"query": "rag dedup"})).unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        let count = items.iter().filter(|i| i["id"] == "user:rag-dedup").count();
        assert_eq!(count, 1, "same node should appear only once");
    }

    #[test]
    fn kb_search_context_deterministic_ordering() {
        let editor = Editor::new();
        let r1 =
            execute_kb_search_context(&editor, &serde_json::json!({"query": "buffer", "limit": 5}))
                .unwrap();
        let r2 =
            execute_kb_search_context(&editor, &serde_json::json!({"query": "buffer", "limit": 5}))
                .unwrap();
        assert_eq!(r1, r2, "same query should produce identical JSON");
    }

    #[test]
    fn kb_search_context_title_match_ranks_higher() {
        let mut editor = Editor::new();
        // Node with "ranking" in title
        editor
            .kb_create_node(
                "user:rank-title",
                "Ranking Test",
                "unrelated body",
                mae_core::KbNodeKind::Note,
            )
            .unwrap();
        // Node with "ranking" only in body
        editor
            .kb_create_node(
                "user:rank-body",
                "Other Node",
                "ranking test body",
                mae_core::KbNodeKind::Note,
            )
            .unwrap();
        let result = execute_kb_search_context(
            &editor,
            &serde_json::json!({"query": "ranking", "limit": 10}),
        )
        .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        let title_pos = items.iter().position(|i| i["id"] == "user:rank-title");
        let body_pos = items.iter().position(|i| i["id"] == "user:rank-body");
        if let (Some(tp), Some(bp)) = (title_pos, body_pos) {
            assert!(tp < bp, "title match should rank higher than body match");
        }
    }

    #[test]
    fn kb_search_context_utf8_cjk() {
        let mut editor = Editor::new();
        let cjk_body = "这是一个测试文档，包含中文字符。".repeat(50);
        editor
            .kb_create_node(
                "user:cjk-test",
                "CJK Test",
                &cjk_body,
                mae_core::KbNodeKind::Note,
            )
            .unwrap();
        // Should not panic on CJK truncation
        let result = execute_kb_search_context(&editor, &serde_json::json!({"query": "CJK Test"}));
        assert!(result.is_ok(), "CJK excerpt should not panic");
    }

    #[test]
    fn kb_search_context_utf8_emoji() {
        let mut editor = Editor::new();
        let emoji_body = "🎉🎊🎈🎆🎇✨🎄🎃🎁🎂".repeat(100);
        editor
            .kb_create_node(
                "user:emoji-test",
                "Emoji Test",
                &emoji_body,
                mae_core::KbNodeKind::Note,
            )
            .unwrap();
        let result =
            execute_kb_search_context(&editor, &serde_json::json!({"query": "Emoji Test"}));
        assert!(result.is_ok(), "emoji excerpt should not panic");
    }

    #[test]
    fn kb_get_revisit_appends_guidance() {
        let mut editor = Editor::new();
        // First call — no guidance
        let r1 = execute_kb_get(&editor, &serde_json::json!({"id": "index"})).unwrap();
        assert!(
            !r1.contains("already visited"),
            "first call should not have revisit guidance"
        );
        // Record the visit
        record_kb_visit(&mut editor, "index");
        // Second call — should have guidance
        let r2 = execute_kb_get(&editor, &serde_json::json!({"id": "index"})).unwrap();
        assert!(
            r2.contains("already visited"),
            "second call should have revisit guidance"
        );
    }

    // --- W3: Prompt tests ---

    #[test]
    fn prompt_mentions_kb_search_context() {
        let content = include_str!("../../../mae/src/prompts/pair-programmer.xml");
        assert!(
            content.contains("kb_search_context"),
            "pair-programmer.xml should mention kb_search_context"
        );
    }

    #[test]
    fn gemini_hints_contain_rag_example() {
        let hints = crate::context_limits::ProviderHint::Gemini
            .prompt_hints()
            .unwrap();
        assert!(
            hints.contains("kb_search_context"),
            "Gemini hints should contain kb_search_context"
        );
    }

    #[test]
    fn deepseek_hints_contain_rag_workflow() {
        let hints = crate::context_limits::ProviderHint::DeepSeek
            .prompt_hints()
            .unwrap();
        assert!(
            hints.contains("kb_search_context"),
            "DeepSeek hints should contain kb_search_context"
        );
    }

    // --- W5: Introspect KB ---

    #[test]
    fn introspect_kb_section() {
        use crate::tool_impls::execute_introspect;
        let editor = Editor::new();
        let result = execute_introspect(&editor, &serde_json::json!({"section": "kb"})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(v["kb"]["local_nodes"].as_u64().unwrap() > 0);
        assert!(v["kb"]["watcher_count"].is_number());
        assert!(v["kb"]["watcher_stats"].is_object());
    }
}
