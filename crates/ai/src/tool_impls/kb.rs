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
    // `kb_resolve_anywhere` is the single source of truth for the
    // query-layer-then-in-memory(-then-federated-instance) fallback order —
    // see its doc comment for why this must not be reimplemented locally
    // (it already had been, three times, before that consolidation, and a
    // fourth divergent copy lived right here until this fix).
    let (node, resolution) = editor.kb_resolve_anywhere(id)?;

    let (links_from, links_to): (Vec<String>, Vec<String>) = match &resolution {
        mae_core::KbResolution::Query => {
            let q = editor
                .kb
                .query_layer()
                .expect("KbResolution::Query implies a query layer is available");
            (
                q.links_from(id).into_iter().map(|l| l.dst).collect(),
                q.links_to(id).into_iter().map(|l| l.src).collect(),
            )
        }
        mae_core::KbResolution::Primary => (
            editor.kb.primary.links_from(id),
            editor.kb.primary.links_to(id),
        ),
        mae_core::KbResolution::Instance(uuid) => {
            let kb = editor
                .kb
                .instances
                .get(uuid)
                .expect("KbResolution::Instance implies that instance is loaded");
            (kb.links_from(id), kb.links_to(id))
        }
    };

    let mut val = serde_json::json!({
        "id": node.id,
        "title": node.title,
        "kind": node.kind,
        "body": node.body,
        "tags": node.tags,
        "links_from": links_from,
        "links_to": links_to,
    });
    if let mae_core::KbResolution::Instance(uuid) = &resolution {
        let inst_name = editor
            .kb
            .registry
            .find_by_uuid(uuid)
            .map(|i| i.name.as_str())
            .unwrap_or("unknown");
        val["instance"] = serde_json::json!(inst_name);
    }
    Some(val)
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

/// `requester_provider` -- the caller's AI provider, when known -- lets this
/// ScopedFederatedScanFilterable tool (ADR-048/#358) post-filter its own
/// materialized results for the AI-residency seed-content exemption, since
/// the gate (`crates/mae/src/ai_residency.rs`) allows the call through
/// unconditionally for this shape rather than pre-denying it.
pub fn execute_kb_search(
    editor: &Editor,
    args: &serde_json::Value,
    requester_provider: Option<&str>,
) -> Result<String, String> {
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
    let results =
        mae_core::ai_residency::filter_residency_exempt(editor, requester_provider, results);
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
    // Outgoing links are a property of the node's OWN tier (its body is what
    // defines them), so — unlike links_to below — this is exactly the
    // `kb_resolve_anywhere`-shaped single-tier fallback.
    let (_, resolution) = editor
        .kb_resolve_anywhere(id)
        .ok_or_else(|| format!("No KB node: {}", id))?;
    let links = match resolution {
        mae_core::KbResolution::Query => {
            let q = editor
                .kb
                .query_layer()
                .expect("KbResolution::Query implies a query layer is available");
            serde_json::to_value(
                q.links_from(id)
                    .into_iter()
                    .map(|l| serde_json::json!({ "dst": l.dst, "rel_type": l.rel_type }))
                    .collect::<Vec<_>>(),
            )
        }
        mae_core::KbResolution::Primary => serde_json::to_value(editor.kb.primary.links_from(id)),
        mae_core::KbResolution::Instance(uuid) => {
            let kb = editor
                .kb
                .instances
                .get(&uuid)
                .expect("KbResolution::Instance implies that instance is loaded");
            serde_json::to_value(kb.links_from(id))
        }
    }
    .map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&links).map_err(|e| e.to_string())
}

pub fn execute_kb_links_to(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    // Incoming links are NOT a single-tier property — any federated instance
    // (or the primary KB) could link into this node, so — unlike links_from
    // above — this deliberately aggregates across every tier rather than
    // routing through `kb_resolve_anywhere`'s single-tier resolution. What
    // WAS missing (and is the actual duplication/drift this fixes) is an
    // existence check: the id itself must resolve somewhere, or this
    // silently returned `[]` for a typo'd id instead of erroring like
    // `execute_kb_links_from` already did.
    if editor.kb_get_node_anywhere(id).is_none() {
        return Err(format!("No KB node: {}", id));
    }
    if let Some(q) = editor.kb.query_layer() {
        let links: Vec<serde_json::Value> = q
            .links_to(id)
            .into_iter()
            .map(|l| serde_json::json!({ "src": l.src, "rel_type": l.rel_type }))
            .collect();
        return serde_json::to_string_pretty(&links).map_err(|e| e.to_string());
    }
    // Fallback: in-memory KB, aggregated across the primary + every
    // federated instance (an incoming link can originate from any of them).
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
///
/// The rank-then-enrich core is shared with the `(kb-related)` Scheme
/// primitive via `mae_kb::graph_query::related_enriched` — see that module's
/// docs (CLAUDE.md principle #8: the ranking logic exists in exactly one
/// place, not duplicated between the MCP and Scheme surfaces).
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

    let backend = mae_kb::graph_query::FederatedRelatedBackend {
        query: editor.kb.query_layer(),
        primary: &editor.kb.primary,
        instances: &editor.kb.instances,
    };
    let items = mae_kb::graph_query::related_enriched(&backend, id, limit);

    let objs: Vec<serde_json::Value> = items
        .into_iter()
        .map(|it| {
            serde_json::json!({ "id": it.id, "title": it.title, "kind": it.kind, "score": it.score })
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
///
/// The BFS walk itself is shared with the `(kb-graph)` Scheme primitive via
/// `mae_kb::graph_query::bfs_neighborhood` — see that module's docs
/// (CLAUDE.md principle #8: the walk exists in exactly one place). This
/// executor supplies the federated backends (query layer, or in-memory
/// `KnowledgeBase` federation as a fallback); Scheme supplies a
/// single-`KbStore` backend instead, since that's all it has access to.
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

    let result = if let Some(q) = editor.kb.query_layer() {
        mae_kb::graph_query::bfs_neighborhood(&mae_kb::graph_query::QueryLayerBackend(q), id, depth)
    } else {
        mae_kb::graph_query::bfs_neighborhood(
            &mae_kb::graph_query::InMemoryFederatedBackend {
                primary: &editor.kb.primary,
                instances: &editor.kb.instances,
                registry: &editor.kb.registry,
            },
            id,
            depth,
        )
    }?;

    let nodes: Vec<serde_json::Value> = result
        .nodes
        .iter()
        .map(|n| {
            if n.missing {
                serde_json::json!({ "id": n.id, "hop": n.hop, "missing": true })
            } else {
                let mut val = serde_json::json!({
                    "id": n.id,
                    "title": n.title,
                    "kind": n.kind,
                    "hop": n.hop,
                });
                if let Some(inst) = &n.instance {
                    val["instance"] = serde_json::json!(inst);
                }
                val
            }
        })
        .collect();
    let edges_json: Vec<serde_json::Value> = result
        .edges
        .into_iter()
        .map(|(src, dst)| serde_json::json!({ "src": src, "dst": dst }))
        .collect();

    let out = serde_json::json!({
        "root": result.root,
        "depth": result.depth,
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

/// Tracked source files whose on-disk mtime has drifted from what was
/// recorded at last import — a still file-tethered node's org file may
/// have changed since (a *pre*-promotion drift signal; promoted nodes have
/// no `source_files` row and never appear here — see
/// `CozoKbStore::detect_reimport_stale_files`). `None` when no durable
/// store is configured for this scope (mirrors `kb_id_audit`'s existing
/// "status: not loaded" degrade-gracefully shape).
fn reimport_stale_files_json(store: Option<&dyn mae_kb::KbStore>) -> serde_json::Value {
    let Some(store) = store else {
        return serde_json::json!([]);
    };
    let Ok(stale) = store.detect_reimport_stale_files() else {
        return serde_json::json!([]);
    };
    serde_json::json!(stale
        .iter()
        .map(|f| serde_json::json!({
            "file_path": f.file_path.display().to_string(),
            "node_ids": f.node_ids,
            "stored_mtime": f.stored_mtime,
            "current_mtime": f.current_mtime,
            "content_changed": f.content_changed,
        }))
        .collect::<Vec<_>>())
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

/// Detect ghost/stale ids and reimport-stale files across the primary KB and
/// every federated instance — see `KnowledgeBase::detect_ghost_ids` and
/// `CozoKbStore::detect_reimport_stale_files`. More expensive than
/// `kb_health` (re-parses/re-hashes each distinct source file), so it's its
/// own on-demand tool rather than folded into the routinely-called health
/// report.
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
                "reimport_stale_files": reimport_stale_files_json(
                    editor.kb.instance_stores.get(&inst.uuid).map(|s| s.as_ref() as &dyn mae_kb::KbStore),
                ),
            }),
            None => serde_json::json!({
                "name": inst.name,
                "uuid": inst.uuid,
                "status": "not loaded",
            }),
        })
        .collect();

    let out = serde_json::json!({
        "local": {
            "ghost_ids": cleanup_candidates_json(&editor.kb.primary),
            "reimport_stale_files": reimport_stale_files_json(
                editor.kb.store.as_deref(),
            ),
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

// --- Native KB graph view (Part C Phase 1) ---
//
// Each executor calls the SAME `Editor::kb_graph_view_*` method the Scheme
// primitives (`runtime/kb_graph_view.rs`) and buffer-local keybindings
// call — CLAUDE.md principle #3 (AI/human parity).

pub fn execute_kb_graph_view_open(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    editor.kb_graph_view_open(id, depth);

    let idx = editor
        .buffers
        .iter()
        .position(|b| b.kind == mae_core::BufferKind::Graph)
        .ok_or("kb_graph_view_open: failed to create the graph buffer")?;
    let gv = editor.buffers[idx]
        .graph_view()
        .ok_or("kb_graph_view_open: graph buffer has no GraphView state")?;
    serde_json::to_string_pretty(&serde_json::json!({
        "center": gv.center_node,
        "depth": gv.depth,
        "kb_instance": gv.kb_instance,
        "node_count": gv.scene.nodes.len(),
        "edge_count": gv.scene.edges.len(),
        "hidden_node_count": gv.hidden_node_count,
    }))
    .map_err(|e| e.to_string())
}

pub fn execute_kb_graph_view_close(
    editor: &mut Editor,
    _args: &serde_json::Value,
) -> Result<String, String> {
    editor.kb_graph_view_close();
    Ok("KB graph view closed".to_string())
}

pub fn execute_kb_graph_view_refresh(
    editor: &mut Editor,
    _args: &serde_json::Value,
) -> Result<String, String> {
    editor.kb_graph_view_refresh_if_open();
    Ok("KB graph view refreshed".to_string())
}

pub fn execute_kb_graph_view_set_depth(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .ok_or("Missing required parameter: depth")? as usize;
    editor.kb_graph_view_set_depth(depth);
    Ok(format!("KB graph view depth set to {}", depth))
}

pub fn execute_kb_graph_view_navigate(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let dir_str = args
        .get("direction")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: direction")?;
    let dir = mae_core::GraphNavDirection::parse(dir_str).ok_or_else(|| {
        format!(
            "Invalid direction '{}': expected up|down|left|right",
            dir_str
        )
    })?;
    editor.kb_graph_view_navigate(dir);
    Ok(format!("KB graph view navigated {}", dir_str))
}

pub fn execute_kb_graph_view_select_current(
    editor: &mut Editor,
    _args: &serde_json::Value,
) -> Result<String, String> {
    editor.kb_graph_view_select_current();
    Ok("Companion window navigated to the selected node".to_string())
}

pub fn execute_kb_graph_view_zoom_to(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let target = args
        .get("zoom")
        .and_then(|v| v.as_f64())
        .ok_or("Missing required parameter: zoom")?;
    match editor.kb_graph_view_zoom_to(target) {
        // Report the ACTUAL applied (post-clamp) zoom, not `target` echoed
        // back — an out-of-range request (e.g. 999) is silently clamped to
        // 10.0 internally, and a caller reasoning about its own action must
        // never be told the raw, un-applied value "worked."
        Some(applied) if applied == target => Ok(format!("KB graph view zoom set to {applied}")),
        Some(applied) => Ok(format!(
            "KB graph view zoom set to {applied} (clamped from requested {target})"
        )),
        None => Err("No KB graph view is open".to_string()),
    }
}

pub fn execute_kb_graph_view_set_pinned(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: id")?;
    let pinned = args
        .get("pinned")
        .and_then(|v| v.as_bool())
        .ok_or("Missing required parameter: pinned")?;
    let x = args.get("x").and_then(|v| v.as_f64());
    let y = args.get("y").and_then(|v| v.as_f64());
    let pos = match (x, y) {
        (Some(x), Some(y)) => Some((x, y)),
        (None, None) => None,
        _ => return Err("x and y must be given together, or both omitted".to_string()),
    };
    if editor.kb_graph_view_set_pinned(id, pinned, pos) {
        Ok(format!(
            "Node '{}' {}",
            id,
            if pinned { "pinned" } else { "unpinned" }
        ))
    } else {
        Err(format!(
            "No graph node with id '{}' is currently rendered",
            id
        ))
    }
}

pub fn execute_kb_graph_view_toggle_overlay(
    editor: &mut Editor,
    _args: &serde_json::Value,
) -> Result<String, String> {
    let active = editor.kb_graph_view_toggle_overlay();
    Ok(format!(
        "KB graph view overlay: {}",
        if active { "on" } else { "off" }
    ))
}

pub fn execute_kb_graph_view_state(
    editor: &mut Editor,
    _args: &serde_json::Value,
) -> Result<String, String> {
    serde_json::to_string_pretty(&editor.kb_graph_view_state()).map_err(|e| e.to_string())
}

// --- KB-link hover preview (Part D) ---
//
// Each executor calls the SAME `Editor::kb_preview_*` method the Scheme
// primitives (`runtime/kb_preview.rs`) and buffer-local keybinding call —
// CLAUDE.md principle #3 (AI/human parity).

pub fn execute_kb_preview_show(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: id")?;
    editor.kb_preview_show(id);
    editor
        .kb_preview_popup()
        .map(|popup| popup.contents.clone())
        .ok_or_else(|| format!("kb_preview_show: could not show preview for '{}'", id))
}

pub fn execute_kb_preview_dismiss(
    editor: &mut Editor,
    _args: &serde_json::Value,
) -> Result<String, String> {
    editor.kb_preview_dismiss();
    Ok("KB preview popup dismissed".to_string())
}

pub fn execute_kb_delete(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: id")?;
    editor.kb_delete_node(id)?;
    Ok(format!("Deleted node: {}", id))
}

pub fn execute_kb_promote(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: id")?;

    let result = editor.kb_promote_node(id)?;
    let dedup = match result.dedup {
        mae_core::editor::PromoteDedup::Removed => "removed",
        mae_core::editor::PromoteDedup::KeptDiverged => "kept_diverged",
    };
    let node = node_json(editor, id);
    serde_json::to_string_pretty(&serde_json::json!({
        "status": "promoted",
        "id": result.node_id,
        "promoted_from_uuid": result.promoted_from_uuid,
        "promoted_from_org_dir": result.promoted_from_org_dir.display().to_string(),
        "instance_copy": dedup,
        "node": node,
    }))
    .map_err(|e| e.to_string())
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

/// Paragraph-aware excerpt: split by `\n\n`, accumulate until byte budget,
/// starting from the paragraph containing `start_hint` (a byte offset into
/// `body` -- typically the earliest query-term match, see
/// `body_match_position`) instead of always doc-start, so a long note's
/// excerpt surfaces the actually-relevant part (#357). `start_hint ==
/// usize::MAX` (no match found) falls back to doc-start, same as before.
/// Falls back to `floor_char_boundary` truncation from doc-start for flat
/// bodies (single paragraph) or when the hinted paragraph alone exceeds the
/// budget.
fn excerpt_body(body: &str, max_bytes: usize, start_hint: usize) -> String {
    if body.len() <= max_bytes {
        return body.to_string();
    }
    let paragraphs: Vec<&str> = body.split("\n\n").collect();
    if paragraphs.len() > 1 {
        let mut offset = 0usize;
        let mut start_idx = 0usize;
        for (i, para) in paragraphs.iter().enumerate() {
            let para_end = offset + para.len();
            if start_hint < para_end {
                start_idx = i;
                break;
            }
            offset = para_end + 2; // +2 for the "\n\n" separator
        }
        let mut acc = String::new();
        for para in &paragraphs[start_idx..] {
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
    // Flat body, or the hinted paragraph alone exceeded budget — use
    // char-boundary truncation from doc-start.
    format!("{}…", &body[..body.floor_char_boundary(max_bytes)])
}

/// Relevance score for RAG ranking: tokenized, field-weighted over
/// titles/aliases/ids/tags (same field tiers as `KnowledgeBase::search_ranked`,
/// `shared/kb/src/lib.rs`), summed across query terms rather than requiring
/// the whole phrase to match verbatim -- the previous whole-phrase
/// `.contains(query_lower)` check almost never matched a natural-language
/// query, collapsing nearly every candidate to a tie (#357). A hub/meta
/// navigational node (`NodeKind::Category`/`Meta`, or `:role: hub`) is
/// down-weighted so its broad incidental keyword coverage doesn't outscore
/// a specific atom with a few precise hits.
fn score_node(query_lower: &str, node: &mae_core::KbNode) -> u32 {
    let terms: Vec<&str> = query_lower.split_whitespace().collect();
    if terms.is_empty() {
        return 1;
    }
    let title_lower = node.title.to_lowercase();
    let id_lower = node.id.to_lowercase();
    let mut score = 0u32;
    for term in &terms {
        if title_lower.contains(term) {
            score += 3;
        }
        if node.aliases.iter().any(|a| a.to_lowercase().contains(term)) {
            score += 3;
        }
        if id_lower.contains(term) {
            score += 2;
        }
        for tag in &node.tags {
            if tag.to_lowercase().contains(term) {
                score += 2;
            }
        }
    }
    // Whole-phrase bonus: reward an exact multi-word title/alias match,
    // mirroring search_ranked's whole_bonus intuition.
    if title_lower.contains(query_lower)
        || node
            .aliases
            .iter()
            .any(|a| a.to_lowercase().contains(query_lower))
    {
        score += 5;
    }
    if score == 0 {
        score = 1; // pure body match -- kb_federated_search_scoped already found it relevant
    }
    let is_hub = matches!(
        node.kind,
        mae_core::KbNodeKind::Category | mae_core::KbNodeKind::Meta
    ) || node.properties.get("role").map(String::as_str) == Some("hub");
    if is_hub {
        score = score.saturating_sub(2);
    }
    score.max(1)
}

/// Byte offset of the first case-insensitive occurrence of any query term in
/// `body`, or `usize::MAX` if no term appears -- used to pick which
/// paragraph `excerpt_body` starts from (not as a sort key; ranking trusts
/// `kb_federated_search_scoped`'s order on ties, see `execute_kb_search_context`, #357).
fn body_match_position(terms: &[&str], body: &str) -> usize {
    let body_lower = body.to_lowercase();
    terms
        .iter()
        .filter_map(|t| body_lower.find(t))
        .min()
        .unwrap_or(usize::MAX)
}

/// RAG-optimized KB search: returns top-K nodes with body excerpts for AI
/// reasoning context. Searches within `scope` (default: all local +
/// federated instances, or the `kb_search_scope` option) via
/// `kb_federated_search_scoped` -- the same scope-aware, already-deduped
/// (local wins), already-`search_sort`-ordered mechanism `kb_search`
/// itself uses (CLAUDE.md #8). This used to be a second, independently
/// hand-rolled federated scan with no `scope` parameter at all, which is
/// how it ended up unscopeable (#350). Re-ranked on top of that by a
/// tokenized, field-weighted `score_node` (same field tiers as
/// `kb_search`'s underlying `search_ranked`, with hub/meta navigational
/// nodes down-weighted); ties preserve `kb_federated_search_scoped`'s
/// already-correct order rather than falling back to alphabetical-by-id
/// (#357). Results from a `LocalModelsOnly`-restricted KB are post-filtered
/// (dropping non-seed-exempt hits) before scoring -- see
/// `mae_core::ai_residency::filter_residency_exempt` (#358). Paragraph-aware
/// excerpts are centered on the earliest query-term match in the body, and
/// low-result guidance is returned when nothing survives.
/// `requester_provider` -- see [`execute_kb_search`] for why this parameter
/// exists (ADR-048/#358: this tool is also ScopedFederatedScanFilterable).
pub fn execute_kb_search_context(
    editor: &Editor,
    args: &serde_json::Value,
    requester_provider: Option<&str>,
) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: query")?;
    let scope = args
        .get("scope")
        .and_then(|v| v.as_str())
        .map(mae_kb::KbScope::parse)
        .unwrap_or_else(|| mae_kb::KbScope::parse(&editor.kb.search_scope));
    let configured_limit = editor.kb.search_max_results;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .min(configured_limit as u64) as usize;
    let excerpt_len = editor.kb.search_excerpt_length;
    let query_lower = query.to_lowercase();
    let terms: Vec<&str> = query_lower.split_whitespace().collect();

    let scoped_results = editor.kb_federated_search_scoped(query, &scope);
    let scoped_results =
        mae_core::ai_residency::filter_residency_exempt(editor, requester_provider, scoped_results);
    let mut results: Vec<(Option<String>, mae_core::KbNode, u32, usize)> = scoped_results
        .into_iter()
        .map(|(inst_name, node)| {
            let score = score_node(&query_lower, &node);
            let position = body_match_position(&terms, &node.body);
            (inst_name, node, score, position)
        })
        .collect();

    // Sort by score desc only. `sort_by` is stable, so residual ties keep
    // `kb_federated_search_scoped`'s incoming order (already the correct
    // field-weighted/activity/alphabetical order, whichever kb_search_sort
    // selects) instead of collapsing to alphabetical-by-id (#357).
    results.sort_by_key(|r| std::cmp::Reverse(r.2));

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
        .map(|(inst_name, node, score, position)| {
            let mut val = serde_json::json!({
                "id": node.id,
                "title": node.title,
                "kind": node.kind,
                "score": score,
            });
            if !ids_only {
                val["excerpt"] =
                    serde_json::json!(excerpt_body(&node.body, effective_excerpt_len, position));
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

/// `requester_provider` -- the caller's AI provider, when known -- lets this
/// PrimaryOnlyFilterable tool (ADR-048/#358) post-filter its own
/// materialized `Node` results for the AI-residency seed-content exemption,
/// since the gate (`crates/mae/src/ai_residency.rs`) allows the call
/// through unconditionally for this shape rather than pre-denying it. Seed
/// nodes never set `todo_state`/`priority`, but DO carry tags and never set
/// `role`, so `tag`/`missing_role`/`orphan`/`dead_end`/`weakly_linked`/
/// `custom` filters can all surface real seed content today.
pub fn execute_kb_agenda(
    editor: &Editor,
    args: &serde_json::Value,
    requester_provider: Option<&str>,
) -> Result<String, String> {
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
    let nodes =
        mae_core::ai_residency::filter_residency_exempt_primary(editor, requester_provider, nodes);
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

    /// A federated (`shared: true`, collab_id present) instance registered
    /// under `uuid-lifecycle` -- the shape the #303 root-cause bug affected.
    fn lifecycle_instance() -> mae_kb::federation::KbInstance {
        mae_kb::federation::KbInstance {
            uuid: "uuid-lifecycle".into(),
            name: "LifecycleInstance".into(),
            org_dir: std::path::PathBuf::from("/tmp/mae-test-lifecycle-org-dir"),
            db_path: std::path::PathBuf::new(),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: Some("collab-lifecycle".into()),
            shared: true,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
        }
    }

    /// End-to-end regression for #303's actual promise: every CRUD path an
    /// AI agent can reach (not just crates/core::Editor methods directly)
    /// must work flawlessly against a promoted node. Drives the real
    /// tool-executor functions -- promote -> update -> add_link -> history
    /// -> health -> agenda -> delete -- exactly the layer an MCP client
    /// exercises, which Part 2's fix never tested directly.
    #[test]
    fn promoted_node_full_crud_lifecycle_through_ai_tool_layer() {
        let mut editor = Editor::new();

        let store = mae_kb::CozoKbStore::open_mem().unwrap();
        store.seed_type_system().unwrap();
        let arc = std::sync::Arc::new(store);
        editor.kb.primary_cozo = Some(arc.clone());
        editor.kb.store = Some(arc.clone());

        let mut kb = mae_kb::KnowledgeBase::new();
        let mut node = mae_kb::Node::new(
            "test:promote-lifecycle",
            "Lifecycle Node",
            mae_kb::NodeKind::Note,
            "original body",
        );
        node.source = Some(mae_kb::NodeSource::Federation);
        kb.insert(node);
        editor.kb.instances.insert("uuid-lifecycle".to_string(), kb);
        editor.kb.registry.instances.push(lifecycle_instance());

        // promote
        let promote_result = execute_kb_promote(
            &mut editor,
            &serde_json::json!({"id": "test:promote-lifecycle"}),
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&promote_result).unwrap();
        assert_eq!(v["status"], "promoted");
        assert_eq!(
            v["instance_copy"], "removed",
            "the origin instance's identical copy must dedup away"
        );
        assert!(
            editor.kb.instances["uuid-lifecycle"]
                .get("test:promote-lifecycle")
                .is_none(),
            "origin instance copy must be gone post-dedup"
        );

        // update -- this is the root-cause regression: pre-fix, this would
        // silently route to the (now-gone) stale instance copy instead.
        execute_kb_update(
            &mut editor,
            &serde_json::json!({"id": "test:promote-lifecycle", "title": "Updated Title"}),
        )
        .unwrap();
        let get_result = execute_kb_get(
            &editor,
            &serde_json::json!({"id": "test:promote-lifecycle"}),
        )
        .unwrap();
        let g: serde_json::Value = serde_json::from_str(&get_result).unwrap();
        assert_eq!(g["title"], "Updated Title");

        // add_link
        editor
            .kb_create_node(
                "test:promote-lifecycle-target",
                "Target",
                "body",
                mae_kb::NodeKind::Note,
            )
            .unwrap();
        execute_kb_add_link(
            &mut editor,
            &serde_json::json!({
                "src": "test:promote-lifecycle",
                "dst": "test:promote-lifecycle-target",
                "rel_type": "related_to",
            }),
        )
        .unwrap();
        let links_result = execute_kb_links_from(
            &editor,
            &serde_json::json!({"id": "test:promote-lifecycle"}),
        )
        .unwrap();
        assert!(links_result.contains("test:promote-lifecycle-target"));

        // history + restore
        let v1 = arc
            .snapshot_version("test:promote-lifecycle", "post-promote checkpoint")
            .unwrap();
        let history_result = execute_kb_history(
            &editor,
            &serde_json::json!({"id": "test:promote-lifecycle"}),
        )
        .unwrap();
        let h: serde_json::Value = serde_json::from_str(&history_result).unwrap();
        assert!(h["version_count"].as_u64().unwrap() >= 1);
        execute_kb_restore(
            &editor,
            &serde_json::json!({"id": "test:promote-lifecycle", "version": v1}),
        )
        .unwrap();

        // health -- must not error against a promoted node's KB.
        let health_result = execute_kb_health(&editor).unwrap();
        let health_json: serde_json::Value = serde_json::from_str(&health_result).unwrap();
        assert!(health_json["local"].is_object());

        // agenda -- must not error against a promoted node's KB.
        execute_kb_agenda(&editor, &serde_json::json!({"filter": "orphan"}), None).unwrap();

        // delete (exercised last)
        execute_kb_delete(
            &mut editor,
            &serde_json::json!({"id": "test:promote-lifecycle"}),
        )
        .unwrap();
        let get_after_delete = execute_kb_get(
            &editor,
            &serde_json::json!({"id": "test:promote-lifecycle"}),
        );
        assert!(get_after_delete.is_err());
    }

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
    fn kb_graph_view_open_creates_buffer_and_returns_summary() {
        let mut editor = Editor::new();
        let result = execute_kb_graph_view_open(
            &mut editor,
            &serde_json::json!({"id": "index", "depth": 1}),
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["center"], "index");
        assert_eq!(v["depth"], 1);
        assert!(editor
            .buffers
            .iter()
            .any(|b| b.kind == mae_core::BufferKind::Graph));
    }

    #[test]
    fn kb_graph_view_open_defaults_center_and_depth() {
        let mut editor = Editor::new();
        let result = execute_kb_graph_view_open(&mut editor, &serde_json::json!({})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["center"], "index");
        assert_eq!(v["depth"], editor.kb_graph_default_depth as u64);
    }

    #[test]
    fn kb_graph_view_close_removes_the_buffer() {
        let mut editor = Editor::new();
        execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        assert!(editor
            .buffers
            .iter()
            .any(|b| b.kind == mae_core::BufferKind::Graph));
        execute_kb_graph_view_close(&mut editor, &serde_json::json!({})).unwrap();
        assert!(!editor
            .buffers
            .iter()
            .any(|b| b.kind == mae_core::BufferKind::Graph));
    }

    #[test]
    fn kb_graph_view_set_depth_updates_in_place() {
        let mut editor = Editor::new();
        execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": "index", "depth": 1}))
            .unwrap();
        execute_kb_graph_view_set_depth(&mut editor, &serde_json::json!({"depth": 4})).unwrap();
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == mae_core::BufferKind::Graph)
            .unwrap();
        assert_eq!(editor.buffers[idx].graph_view().unwrap().depth, 4);
    }

    #[test]
    fn kb_graph_view_set_depth_missing_arg_is_error() {
        let mut editor = Editor::new();
        let err = execute_kb_graph_view_set_depth(&mut editor, &serde_json::json!({})).unwrap_err();
        assert!(err.contains("depth"));
    }

    #[test]
    fn kb_graph_view_navigate_invalid_direction_is_error() {
        let mut editor = Editor::new();
        execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        let err = execute_kb_graph_view_navigate(
            &mut editor,
            &serde_json::json!({"direction": "sideways"}),
        )
        .unwrap_err();
        assert!(err.contains("sideways"));
    }

    #[test]
    fn kb_graph_view_navigate_valid_direction_succeeds() {
        let mut editor = Editor::new();
        execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        let result =
            execute_kb_graph_view_navigate(&mut editor, &serde_json::json!({"direction": "right"}))
                .unwrap();
        assert!(result.contains("right"));
    }

    #[test]
    fn kb_graph_view_select_current_opens_a_kb_buffer() {
        let mut editor = Editor::new();
        execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        execute_kb_graph_view_select_current(&mut editor, &serde_json::json!({})).unwrap();
        assert!(editor
            .buffers
            .iter()
            .any(|b| b.kind == mae_core::BufferKind::Kb));
    }

    #[test]
    fn kb_graph_view_state_is_null_when_no_graph_is_open() {
        let mut editor = Editor::new();
        let result = execute_kb_graph_view_state(&mut editor, &serde_json::json!({})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(v.is_null());
    }

    #[test]
    fn kb_graph_view_state_reflects_open_graph() {
        let mut editor = Editor::new();
        execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": "index", "depth": 1}))
            .unwrap();
        let result = execute_kb_graph_view_state(&mut editor, &serde_json::json!({})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["center_node"], "index");
        assert_eq!(v["depth"], 1);
        assert!(v["nodes"].as_array().is_some_and(|n| !n.is_empty()));
    }

    #[test]
    fn kb_graph_view_zoom_to_sets_the_focused_windows_zoom() {
        let mut editor = Editor::new();
        execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == mae_core::BufferKind::Graph)
            .unwrap();
        let win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == idx)
            .map(|w| w.id)
            .unwrap();
        editor.window_mgr.set_focused(win_id);

        let result =
            execute_kb_graph_view_zoom_to(&mut editor, &serde_json::json!({"zoom": 3.5})).unwrap();

        assert!(result.contains("3.5"));
        assert_eq!(
            editor.buffers[idx]
                .graph_view()
                .unwrap()
                .viewports
                .get(&win_id)
                .unwrap()
                .zoom,
            3.5
        );
    }

    #[test]
    fn kb_graph_view_zoom_to_missing_arg_is_error() {
        let mut editor = Editor::new();
        let err = execute_kb_graph_view_zoom_to(&mut editor, &serde_json::json!({})).unwrap_err();
        assert!(err.contains("zoom"));
    }

    #[test]
    fn kb_graph_view_zoom_to_out_of_range_message_reports_the_clamped_value_not_the_raw_request() {
        // Regression guard (found during live MCP validation): the response
        // message used to echo the raw requested `target` verbatim even
        // when it was clamped internally — a caller (human or the AI peer
        // itself) reading "zoom set to 999" would wrongly believe zoom is
        // now 999x, when it's actually 10.0x. The message must report what
        // ACTUALLY got applied.
        let mut editor = Editor::new();
        execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == mae_core::BufferKind::Graph)
            .unwrap();
        let win_id = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == idx)
            .map(|w| w.id)
            .unwrap();
        editor.window_mgr.set_focused(win_id);

        let result =
            execute_kb_graph_view_zoom_to(&mut editor, &serde_json::json!({"zoom": 999.0}))
                .unwrap();

        assert!(
            result.contains("set to 10"),
            "message must report the ACTUAL applied zoom (10.0, the clamp ceiling), not \
             claim 999 was applied verbatim: {result}"
        );
        assert!(
            result.to_lowercase().contains("clamp"),
            "message should transparently note the request was clamped, not silently \
             substitute a different number: {result}"
        );
    }

    #[test]
    fn kb_graph_view_zoom_to_no_graph_open_is_error() {
        let mut editor = Editor::new();
        let err = execute_kb_graph_view_zoom_to(&mut editor, &serde_json::json!({"zoom": 2.0}))
            .unwrap_err();
        assert!(err.to_lowercase().contains("no") && err.to_lowercase().contains("graph"));
    }

    #[test]
    fn kb_graph_view_set_pinned_pins_a_node_and_repositions_it() {
        let mut editor = Editor::new();
        execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == mae_core::BufferKind::Graph)
            .unwrap();
        let node_id = editor.buffers[idx].graph_view().unwrap().scene.nodes[0]
            .id
            .clone();

        let result = execute_kb_graph_view_set_pinned(
            &mut editor,
            &serde_json::json!({"id": node_id, "pinned": true, "x": 5.0, "y": 6.0}),
        )
        .unwrap();

        assert!(result.contains("pinned"));
        let node = &editor.buffers[idx].graph_view().unwrap().scene.nodes[0];
        assert!(node.pinned);
        assert_eq!(node.x, 5.0);
        assert_eq!(node.y, 6.0);
    }

    #[test]
    fn kb_graph_view_set_pinned_without_position_leaves_it_in_place() {
        let mut editor = Editor::new();
        execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        let idx = editor
            .buffers
            .iter()
            .position(|b| b.kind == mae_core::BufferKind::Graph)
            .unwrap();
        let (node_id, x0, y0) = {
            let node = &editor.buffers[idx].graph_view().unwrap().scene.nodes[0];
            (node.id.clone(), node.x, node.y)
        };

        execute_kb_graph_view_set_pinned(
            &mut editor,
            &serde_json::json!({"id": node_id, "pinned": true}),
        )
        .unwrap();

        let node = &editor.buffers[idx].graph_view().unwrap().scene.nodes[0];
        assert!(node.pinned);
        assert_eq!(node.x, x0);
        assert_eq!(node.y, y0);
    }

    #[test]
    fn kb_graph_view_set_pinned_unknown_id_is_error() {
        let mut editor = Editor::new();
        execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        let err = execute_kb_graph_view_set_pinned(
            &mut editor,
            &serde_json::json!({"id": "concept:does-not-exist", "pinned": true}),
        )
        .unwrap_err();
        assert!(err.contains("does-not-exist"));
    }

    #[test]
    fn kb_graph_view_set_pinned_only_x_without_y_is_error() {
        let mut editor = Editor::new();
        execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        let node_id = editor
            .buffers
            .iter()
            .find(|b| b.kind == mae_core::BufferKind::Graph)
            .and_then(|b| b.graph_view())
            .unwrap()
            .scene
            .nodes[0]
            .id
            .clone();
        let err = execute_kb_graph_view_set_pinned(
            &mut editor,
            &serde_json::json!({"id": node_id, "pinned": true, "x": 1.0}),
        )
        .unwrap_err();
        assert!(err.contains('x') && err.contains('y'));
    }

    #[test]
    fn kb_graph_view_set_pinned_missing_required_args_is_error() {
        let mut editor = Editor::new();
        let err =
            execute_kb_graph_view_set_pinned(&mut editor, &serde_json::json!({})).unwrap_err();
        assert!(err.contains("id"));
    }

    #[test]
    fn kb_preview_show_returns_popup_contents() {
        let mut editor = Editor::new();
        editor.open_help_at("index"); // active buffer must be KB-kind
        let result =
            execute_kb_preview_show(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        assert!(result.contains("MAE Help Index"));
        assert!(editor.kb_preview_popup().is_some());
    }

    #[test]
    fn kb_preview_show_missing_id_arg_is_error() {
        let mut editor = Editor::new();
        editor.open_help_at("index");
        let err = execute_kb_preview_show(&mut editor, &serde_json::json!({})).unwrap_err();
        assert!(err.contains("id"));
    }

    #[test]
    fn kb_preview_show_missing_node_is_error() {
        let mut editor = Editor::new();
        editor.open_help_at("index");
        let err = execute_kb_preview_show(&mut editor, &serde_json::json!({"id": "no:such:node"}))
            .unwrap_err();
        assert!(err.contains("no:such:node"));
        assert!(editor.kb_preview_popup().is_none());
    }

    #[test]
    fn kb_preview_show_outside_kb_buffer_is_error() {
        let mut editor = Editor::new(); // active buffer is scratch, not KB
        let err =
            execute_kb_preview_show(&mut editor, &serde_json::json!({"id": "index"})).unwrap_err();
        assert!(err.contains("index"));
        assert!(editor.kb_preview_popup().is_none());
    }

    #[test]
    fn kb_preview_dismiss_clears_popup() {
        let mut editor = Editor::new();
        editor.open_help_at("index");
        execute_kb_preview_show(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        assert!(editor.kb_preview_popup().is_some());
        execute_kb_preview_dismiss(&mut editor, &serde_json::json!({})).unwrap();
        assert!(editor.kb_preview_popup().is_none());
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
        let result =
            execute_kb_search(&editor, &serde_json::json!({"query": "buffer"}), None).unwrap();
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
        let result = execute_kb_search(&editor, &serde_json::json!({"query": ""}), None).unwrap();
        let ids = kb_search_ids(&result);
        // Empty query lists nodes but is bounded by the result cap (kb_list is
        // the unbounded enumeration tool).
        assert!(!ids.is_empty());
        assert!(ids.len() <= editor.kb.search_max_results);
    }

    #[test]
    fn kb_search_respects_explicit_limit() {
        let editor = Editor::new();
        let result = execute_kb_search(
            &editor,
            &serde_json::json!({"query": "buffer", "limit": 3}),
            None,
        )
        .unwrap();
        let ids = kb_search_ids(&result);
        assert!(ids.len() <= 3);
    }

    #[test]
    fn kb_search_local_scope_excludes_federated() {
        // With no federated instances, local scope behaves like all.
        let editor = Editor::new();
        let all =
            execute_kb_search(&editor, &serde_json::json!({"query": "buffer"}), None).unwrap();
        let local = execute_kb_search(
            &editor,
            &serde_json::json!({"query": "buffer", "scope": "local"}),
            None,
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

    /// Regression test for the audit finding that `execute_kb_links_to`
    /// silently returned `[]` for an id that didn't exist anywhere, instead
    /// of erroring like `execute_kb_links_from` already did for the same
    /// case — a real behavioral gap surfaced while consolidating both onto
    /// the shared existence-resolution helper.
    #[test]
    fn kb_links_to_unknown_id_is_error() {
        let editor = Editor::new();
        let err =
            execute_kb_links_to(&editor, &serde_json::json!({"id": "no:such:node"})).unwrap_err();
        assert!(err.contains("No KB node"));
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
        let result = execute_kb_search_context(
            &editor,
            &serde_json::json!({"query": "buffer", "limit": 3}),
            None,
        )
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
        let result = execute_kb_search_context(
            &editor,
            &serde_json::json!({"query": "unique rag test"}),
            None,
        )
        .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert!(
            items.iter().any(|i| i["id"] == "fed-rag-test"),
            "should include federated results"
        );
    }

    // --- #350: scope param, alias-aware ranking ---
    // --- #357: tokenized scoring, hub/meta down-weight, stable-sort tiebreak ---

    #[test]
    fn score_node_checks_aliases() {
        let node = mae_core::KbNode::new(
            "user:alias-test",
            "Unrelated Title",
            mae_core::KbNodeKind::Note,
            "unrelated body",
        )
        .with_aliases(["orderless retrieval"]);
        // Matches only via alias -- must score at the same tier as a title
        // match, not fall through to the flat body-match default (1).
        // Tokenized model: "orderless" (+3) + "retrieval" (+3) per-term, plus
        // the whole-phrase bonus (+5) since the alias exactly contains the
        // full query -- 11 total (#357).
        assert_eq!(score_node("orderless retrieval", &node), 11);
    }

    #[test]
    fn score_node_checks_id() {
        let node = mae_core::KbNode::new(
            "concept:special-id-term",
            "Unrelated Title",
            mae_core::KbNodeKind::Note,
            "unrelated body",
        );
        assert_eq!(score_node("special-id-term", &node), 2);
    }

    #[test]
    fn score_node_falls_back_to_body_match_score() {
        let node = mae_core::KbNode::new(
            "user:plain",
            "Unrelated Title",
            mae_core::KbNodeKind::Note,
            "the term appears only here",
        );
        assert_eq!(score_node("the term appears", &node), 1);
    }

    #[test]
    fn kb_search_context_ties_preserve_kb_federated_search_scoped_order() {
        let mut editor = Editor::new();
        // Both nodes tie at score 1 under score_node's tokenized model (a
        // pure body-only match, no title/alias/id/tag hit for either -- the
        // Category node's hub down-weight saturates at 0 then floors back to
        // 1 via `.max(1)`, same as the non-hub node's fallback tier). They do
        // NOT tie in kb_federated_search_scoped's own order, though: "aaa-tie"
        // is a Category (hub-like) node with a lower relevance prior there
        // (no flooring in search_ranked). If execute_kb_search_context's tie
        // handling regressed back to alphabetical-by-id, "aaa-tie" would
        // incorrectly win; the correct behavior preserves the upstream,
        // prior-aware order instead (#357).
        editor
            .kb_create_node(
                "user:aaa-tie",
                "Unrelated A",
                "padding tiebreaktermxyz padding",
                mae_core::KbNodeKind::Category,
            )
            .unwrap();
        editor
            .kb_create_node(
                "user:zzz-tie",
                "Unrelated B",
                "padding tiebreaktermxyz padding",
                mae_core::KbNodeKind::Note,
            )
            .unwrap();

        let scope = mae_kb::KbScope::parse(&editor.kb.search_scope);
        let upstream: Vec<String> = editor
            .kb_federated_search_scoped("tiebreaktermxyz", &scope)
            .into_iter()
            .map(|(_, node)| node.id)
            .filter(|id| id == "user:aaa-tie" || id == "user:zzz-tie")
            .collect();
        assert_eq!(
            upstream,
            vec!["user:zzz-tie".to_string(), "user:aaa-tie".to_string()],
            "sanity check: the Category node's lower prior should already put \
             it second in kb_federated_search_scoped's own order, got {upstream:?}"
        );

        let result = execute_kb_search_context(
            &editor,
            &serde_json::json!({"query": "tiebreaktermxyz"}),
            None,
        )
        .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        let actual: Vec<String> = items
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .filter(|id| id == "user:aaa-tie" || id == "user:zzz-tie")
            .collect();

        assert_eq!(
            actual, upstream,
            "tied score_node candidates must preserve kb_federated_search_scoped's \
             order, not fall back to alphabetical-by-id: {actual:?}"
        );
    }

    #[test]
    fn kb_search_context_hub_node_does_not_outrank_specific_target() {
        // End-to-end regression for #357's actual reported symptom: a
        // hub/Category node with broad keyword coverage (mentions many topic
        // words, like the issue's "token-efficiency-evidence" example) vs. a
        // specific Note with a real but partial title/body match. The
        // specific note must rank above the hub node for a natural-language
        // query that doesn't exact-substring the note's title.
        let mut editor = Editor::new();
        editor
            .kb_create_node(
                "practice:adversarial-testing",
                "Adversarial testing philosophy",
                "Tests exist to falsify the implementation, not confirm it. \
                 Prefer property/round-trip tests over one fixed happy path.",
                mae_core::KbNodeKind::Note,
            )
            .unwrap();
        editor
            .kb_create_node(
                "hub:dev-practices",
                "Development practices hub",
                "Links out to testing philosophy, commit conventions, code review, \
                 and build tooling practices used across this project.",
                mae_core::KbNodeKind::Category,
            )
            .unwrap();

        let result = execute_kb_search_context(
            &editor,
            &serde_json::json!({"query": "testing philosophy"}),
            None,
        )
        .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
        let pos_target = ids
            .iter()
            .position(|&id| id == "practice:adversarial-testing");
        let pos_hub = ids.iter().position(|&id| id == "hub:dev-practices");
        assert!(
            pos_target.is_some(),
            "the specific target note must be present in results: {ids:?}"
        );
        if let Some(pos_hub) = pos_hub {
            assert!(
                pos_target.unwrap() < pos_hub,
                "specific target note must outrank the hub node, got {ids:?}"
            );
        }
    }

    #[test]
    fn kb_search_context_natural_language_query_with_one_unmatched_filler_word_is_not_empty() {
        // Reproduces #357's "zero results" symptom directly, calibrated to
        // what the soft-AND fallback in search_ranked actually guarantees
        // (relax by exactly one unmatched term, see shared/kb/src/lib.rs) --
        // a short natural phrasing with exactly one word absent from the
        // target node.
        let mut editor = Editor::new();
        editor
            .kb_create_node(
                "practice:caution-annotations",
                "Caution annotation convention",
                "Use @ai-caution comments to flag invariants for other AI agents.",
                mae_core::KbNodeKind::Note,
            )
            .unwrap();

        let result = execute_kb_search_context(
            &editor,
            &serde_json::json!({"query": "caution annotation guidance"}),
            None,
        )
        .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert!(
            !items.is_empty(),
            "a natural query with one unmatched filler/synonym word must not return zero results"
        );
        assert!(
            items
                .iter()
                .any(|i| i["id"] == "practice:caution-annotations"),
            "target node should be present in results: {items:?}"
        );
    }

    #[test]
    fn kb_search_context_scope_filters_out_other_instances() {
        let mut editor = Editor::new();
        let mut inst = mae_core::KnowledgeBase::new();
        inst.insert(mae_core::KbNode::new(
            "other-inst-node",
            "Scope Filter Target",
            mae_core::KbNodeKind::Note,
            "a distinctive scopefiltertermxyz body",
        ));
        editor.kb.instances.insert("uuid-other".to_string(), inst);
        editor
            .kb
            .registry
            .instances
            .push(mae_kb::federation::KbInstance {
                uuid: "uuid-other".into(),
                name: "OtherInstance".into(),
                org_dir: std::path::PathBuf::new(),
                db_path: std::path::PathBuf::new(),
                primary: false,
                enabled: true,
                last_import: None,
                collab_id: None,
                shared: false,
                remote_peers: Vec::new(),
                last_sync: None,
                ai_residency: mae_kb::federation::AiResidency::default(),
            });

        // Unscoped: the federated node is included.
        let all_result = execute_kb_search_context(
            &editor,
            &serde_json::json!({"query": "scopefiltertermxyz"}),
            None,
        )
        .unwrap();
        let all_items: Vec<serde_json::Value> = serde_json::from_str(&all_result).unwrap();
        assert!(
            all_items.iter().any(|i| i["id"] == "other-inst-node"),
            "unscoped search must include the federated instance's node"
        );

        // Scoped to "local": the federated node must be excluded.
        let local_result = execute_kb_search_context(
            &editor,
            &serde_json::json!({"query": "scopefiltertermxyz", "scope": "local"}),
            None,
        )
        .unwrap();
        let local_json: serde_json::Value = serde_json::from_str(&local_result).unwrap();
        // Either an empty-results guidance object, or an array with no
        // match from the excluded instance -- both are correct.
        let local_items = local_json.as_array().cloned().unwrap_or_default();
        assert!(
            !local_items.iter().any(|i| i["id"] == "other-inst-node"),
            "scope=local must exclude the federated instance's node: {local_json}"
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
            execute_kb_search_context(&editor, &serde_json::json!({"query": "rag dedup"}), None)
                .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        let count = items.iter().filter(|i| i["id"] == "user:rag-dedup").count();
        assert_eq!(count, 1, "same node should appear only once");
    }

    #[test]
    fn kb_search_context_deterministic_ordering() {
        let editor = Editor::new();
        let r1 = execute_kb_search_context(
            &editor,
            &serde_json::json!({"query": "buffer", "limit": 5}),
            None,
        )
        .unwrap();
        let r2 = execute_kb_search_context(
            &editor,
            &serde_json::json!({"query": "buffer", "limit": 5}),
            None,
        )
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
            None,
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
        let result =
            execute_kb_search_context(&editor, &serde_json::json!({"query": "CJK Test"}), None);
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
            execute_kb_search_context(&editor, &serde_json::json!({"query": "Emoji Test"}), None);
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

    // --- execute_kb_agenda: filter/value string→enum dispatch tests ---
    //
    // The storage-layer `AgendaFilter` semantics (MissingRole/WeaklyLinked/etc)
    // are covered end-to-end in `shared/kb/src/cozo_store.rs`
    // (`agenda_missing_role_filter`, `agenda_weakly_linked_filter`). Those
    // tests never exercise `execute_kb_agenda` itself, so the
    // filter-string/value-string parsing done here had zero direct coverage.
    // `editor.kb.store` is `None` by default (`KbContext::new`), so tests that
    // need `filter`/`value` to actually reach `agenda_query` must wire in a
    // real `CozoKbStore::open_mem()`; the two error-path tests below return
    // before `editor.kb.store` is ever consulted, so they use a bare
    // `Editor::new()` like the rest of this file's error-path tests.

    #[test]
    fn kb_agenda_missing_filter_arg_is_error() {
        let editor = Editor::new();
        let err = execute_kb_agenda(&editor, &serde_json::json!({}), None).unwrap_err();
        assert_eq!(err, "Missing required argument: filter");
    }

    #[test]
    fn kb_agenda_unknown_filter_type_is_error() {
        let editor = Editor::new();
        let err = execute_kb_agenda(
            &editor,
            &serde_json::json!({"filter": "not_a_real_filter"}),
            None,
        )
        .unwrap_err();
        assert_eq!(err, "Unknown filter type: not_a_real_filter");
    }

    #[test]
    fn kb_agenda_missing_role_dispatches_to_missing_role_filter() {
        use mae_kb::KbStore;
        let store = mae_kb::CozoKbStore::open_mem().unwrap();

        let mut has_role = mae_core::KbNode::new(
            "note:agenda-has-role",
            "Has Role",
            mae_core::KbNodeKind::Note,
            "",
        );
        has_role
            .properties
            .insert("role".to_string(), "atom".to_string());
        store.insert_node(&has_role).unwrap();
        store
            .insert_node(&mae_core::KbNode::new(
                "note:agenda-no-role",
                "No Role",
                mae_core::KbNodeKind::Note,
                "",
            ))
            .unwrap();

        let mut editor = Editor::new();
        editor.kb.store = Some(std::sync::Arc::new(store));

        let result = execute_kb_agenda(
            &editor,
            &serde_json::json!({"filter": "missing_role"}),
            None,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["filter"], "missing_role");
        let ids: Vec<&str> = v["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap())
            .collect();
        assert!(
            ids.contains(&"note:agenda-no-role"),
            "node without a role should be picked up by \"missing_role\": {ids:?}"
        );
        assert!(
            !ids.contains(&"note:agenda-has-role"),
            "node with a role must be excluded: {ids:?}"
        );
    }

    #[test]
    fn kb_agenda_weakly_linked_dispatches_with_parsed_value() {
        // Proves "value" is actually parsed into `WeaklyLinked(n)` and not
        // silently ignored: a node with 3 outgoing links is NOT weakly-linked
        // at threshold 2 (3 < 2 is false) but IS weakly-linked at threshold 5
        // (3 < 5 is true) -- so which result set we get depends on "value"
        // actually reaching the store query as 5.
        use mae_kb::KbStore;
        let store = mae_kb::CozoKbStore::open_mem().unwrap();
        store
            .insert_node(&mae_core::KbNode::new(
                "note:agenda-wl-src",
                "Src",
                mae_core::KbNodeKind::Note,
                "",
            ))
            .unwrap();
        for t in [
            "note:agenda-wl-t1",
            "note:agenda-wl-t2",
            "note:agenda-wl-t3",
        ] {
            store
                .insert_node(&mae_core::KbNode::new(t, t, mae_core::KbNodeKind::Note, ""))
                .unwrap();
            store
                .add_typed_link("note:agenda-wl-src", t, "references", 1.0)
                .unwrap();
        }

        let mut editor = Editor::new();
        editor.kb.store = Some(std::sync::Arc::new(store));

        let at_2 = execute_kb_agenda(
            &editor,
            &serde_json::json!({"filter": "weakly_linked", "value": "2"}),
            None,
        )
        .unwrap();
        let v2: serde_json::Value = serde_json::from_str(&at_2).unwrap();
        let ids2: Vec<&str> = v2["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap())
            .collect();
        assert!(
            !ids2.contains(&"note:agenda-wl-src"),
            "3 outgoing links should not be weakly-linked at threshold 2: {ids2:?}"
        );

        let at_5 = execute_kb_agenda(
            &editor,
            &serde_json::json!({"filter": "weakly_linked", "value": "5"}),
            None,
        )
        .unwrap();
        let v5: serde_json::Value = serde_json::from_str(&at_5).unwrap();
        let ids5: Vec<&str> = v5["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap())
            .collect();
        assert!(
            ids5.contains(&"note:agenda-wl-src"),
            "3 outgoing links should be weakly-linked at threshold 5 -- proves \
             \"value\": \"5\" was actually parsed and dispatched: {ids5:?}"
        );
    }

    #[test]
    fn kb_agenda_weakly_linked_malformed_value_silently_falls_back_to_2() {
        // Documents existing (deliberately silent, not an error) behavior: an
        // unparseable "value" for "weakly_linked" falls back to the literal
        // `.unwrap_or(2)` in execute_kb_agenda's dispatch match arm, rather
        // than surfacing an error to the caller. A node with exactly 2
        // outgoing links (2 < 2 is false) is excluded at threshold 2 but
        // would be included at any larger fallback (e.g. 3), so equality
        // between the malformed-value result and the explicit "2" result
        // pins down the fallback constant.
        use mae_kb::KbStore;
        let store = mae_kb::CozoKbStore::open_mem().unwrap();
        store
            .insert_node(&mae_core::KbNode::new(
                "note:agenda-fb-src",
                "Src",
                mae_core::KbNodeKind::Note,
                "",
            ))
            .unwrap();
        for t in ["note:agenda-fb-t1", "note:agenda-fb-t2"] {
            store
                .insert_node(&mae_core::KbNode::new(t, t, mae_core::KbNodeKind::Note, ""))
                .unwrap();
            store
                .add_typed_link("note:agenda-fb-src", t, "references", 1.0)
                .unwrap();
        }

        let mut editor = Editor::new();
        editor.kb.store = Some(std::sync::Arc::new(store));

        let malformed = execute_kb_agenda(
            &editor,
            &serde_json::json!({"filter": "weakly_linked", "value": "not-a-number"}),
            None,
        )
        .unwrap();
        let explicit_2 = execute_kb_agenda(
            &editor,
            &serde_json::json!({"filter": "weakly_linked", "value": "2"}),
            None,
        )
        .unwrap();
        assert_eq!(
            malformed, explicit_2,
            "a malformed value should silently fall back to the same threshold \
             as an explicit \"2\""
        );

        let v: serde_json::Value = serde_json::from_str(&malformed).unwrap();
        let ids: Vec<&str> = v["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap())
            .collect();
        assert!(
            !ids.contains(&"note:agenda-fb-src"),
            "2 outgoing links should not be weakly-linked at fallback threshold 2: {ids:?}"
        );
    }

    #[test]
    fn kb_agenda_stale_malformed_value_does_not_error() {
        // Same fallback-on-malformed-input shape as "weakly_linked" above:
        // `.unwrap_or(30)` in execute_kb_agenda's "stale" arm. Real staleness
        // is timestamp-driven (`updated_at` is set to "now" on every insert
        // with no public API to backdate it), so this can't distinguish 30
        // from another fallback constant via node data the way
        // "weakly_linked" can -- it documents the "no error, same shape as an
        // explicit numeric value" half of the behavior.
        use mae_kb::KbStore;
        let store = mae_kb::CozoKbStore::open_mem().unwrap();
        store
            .insert_node(&mae_core::KbNode::new(
                "note:agenda-stale-fresh",
                "Fresh",
                mae_core::KbNodeKind::Note,
                "",
            ))
            .unwrap();

        let mut editor = Editor::new();
        editor.kb.store = Some(std::sync::Arc::new(store));

        let malformed = execute_kb_agenda(
            &editor,
            &serde_json::json!({"filter": "stale", "value": "not-a-number"}),
            None,
        )
        .unwrap();
        let explicit_30 = execute_kb_agenda(
            &editor,
            &serde_json::json!({"filter": "stale", "value": "30"}),
            None,
        )
        .unwrap();
        assert_eq!(
            malformed, explicit_30,
            "a malformed value should silently fall back to the same threshold \
             as an explicit \"30\""
        );
        let v: serde_json::Value = serde_json::from_str(&malformed).unwrap();
        assert_eq!(v["filter"], "stale");
        assert!(
            !v["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["id"] == "note:agenda-stale-fresh"),
            "a freshly-inserted node should never be stale"
        );
    }

    // --- New: AI-residency seed-content exemption post-filter (#358) ---
    //
    // These exercise execute_kb_search/execute_kb_search_context/execute_kb_agenda's
    // own post-filter directly (mae_core::ai_residency::filter_residency_exempt(_primary)),
    // complementing crates/mae/src/ai_residency.rs's gate-level tests, which only
    // cover the SingleTarget shape's exemption (the gate now allows these three
    // ScopedFederatedScanFilterable/PrimaryOnlyFilterable tools through
    // unconditionally -- enforcement lives here).

    fn seed_node_with(id: &str, body: &str) -> mae_core::KbNode {
        mae_core::KbNode::new(id, "Seeded Node", mae_core::KbNodeKind::Concept, body)
            .with_source(mae_kb::NodeSource::Seed, 1)
    }

    fn non_seed_node_with(id: &str, body: &str) -> mae_core::KbNode {
        mae_core::KbNode::new(id, "User Node", mae_core::KbNodeKind::Note, body)
            .with_source(mae_kb::NodeSource::Manual, 1)
    }

    #[test]
    fn kb_search_keeps_seed_drops_non_seed_from_restricted_primary() {
        let mut editor = Editor::new();
        editor.kb.primary.insert(seed_node_with(
            "seed:residency-a",
            "findableresidencytestterm seed content",
        ));
        editor.kb.primary.insert(non_seed_node_with(
            "user:residency-b",
            "findableresidencytestterm user content",
        ));
        editor.kb.registry.primary_ai_residency = mae_kb::federation::AiResidency::LocalModelsOnly;

        let result = execute_kb_search(
            &editor,
            &serde_json::json!({"query": "findableresidencytestterm"}),
            Some("claude"),
        )
        .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert!(
            items.iter().any(|i| i["id"] == "seed:residency-a"),
            "seed content must stay reachable from a restricted primary: {items:?}"
        );
        assert!(
            !items.iter().any(|i| i["id"] == "user:residency-b"),
            "non-seed content must be filtered out of a restricted primary: {items:?}"
        );
    }

    #[test]
    fn kb_search_local_provider_bypasses_residency_filter_entirely() {
        let mut editor = Editor::new();
        editor.kb.primary.insert(seed_node_with(
            "seed:residency-c",
            "findableresidencytermtwo seed content",
        ));
        editor.kb.primary.insert(non_seed_node_with(
            "user:residency-d",
            "findableresidencytermtwo user content",
        ));
        editor.kb.registry.primary_ai_residency = mae_kb::federation::AiResidency::LocalModelsOnly;

        let result = execute_kb_search(
            &editor,
            &serde_json::json!({"query": "findableresidencytermtwo"}),
            Some("ollama"),
        )
        .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert!(
            items.iter().any(|i| i["id"] == "seed:residency-c"),
            "{items:?}"
        );
        assert!(
            items.iter().any(|i| i["id"] == "user:residency-d"),
            "a local provider must bypass filtering entirely: {items:?}"
        );
    }

    #[test]
    fn kb_search_no_filtering_when_primary_open() {
        let mut editor = Editor::new(); // primary defaults to Open
        editor.kb.primary.insert(non_seed_node_with(
            "user:residency-e",
            "findableresidencytermthree user content",
        ));

        let result = execute_kb_search(
            &editor,
            &serde_json::json!({"query": "findableresidencytermthree"}),
            Some("claude"),
        )
        .unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert!(
            items.iter().any(|i| i["id"] == "user:residency-e"),
            "an open KB's content must never be filtered: {items:?}"
        );
    }

    #[test]
    fn kb_search_context_keeps_seed_drops_non_seed_from_restricted_primary() {
        let mut editor = Editor::new();
        editor.kb.primary.insert(seed_node_with(
            "seed:residency-f",
            "findableresidencytermfour seed content",
        ));
        editor.kb.primary.insert(non_seed_node_with(
            "user:residency-g",
            "findableresidencytermfour user content",
        ));
        editor.kb.registry.primary_ai_residency = mae_kb::federation::AiResidency::LocalModelsOnly;

        let result = execute_kb_search_context(
            &editor,
            &serde_json::json!({"query": "findableresidencytermfour"}),
            Some("claude"),
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let items: Vec<serde_json::Value> = parsed.as_array().cloned().unwrap_or_default();
        assert!(
            items.iter().any(|i| i["id"] == "seed:residency-f"),
            "seed content must stay reachable from a restricted primary: {items:?}"
        );
        assert!(
            !items.iter().any(|i| i["id"] == "user:residency-g"),
            "non-seed content must be filtered out of a restricted primary: {items:?}"
        );
    }

    #[test]
    fn kb_search_context_all_filtered_returns_low_result_guidance_not_a_mislabeled_empty_list() {
        let mut editor = Editor::new();
        editor.kb.primary.insert(non_seed_node_with(
            "user:residency-h",
            "findableresidencytermfive user-only content",
        ));
        editor.kb.registry.primary_ai_residency = mae_kb::federation::AiResidency::LocalModelsOnly;

        let result = execute_kb_search_context(
            &editor,
            &serde_json::json!({"query": "findableresidencytermfive"}),
            Some("claude"),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(
            value.get("guidance").is_some(),
            "an all-filtered result set must hit the low-result guidance branch, not a bare empty array: {value:?}"
        );
    }

    #[test]
    fn kb_agenda_keeps_seed_drops_non_seed_from_restricted_primary() {
        // Seed nodes never set todo_state/priority, but DO carry tags -- use
        // the "tag" filter so the positive case is real, not vacuous.
        // kb_agenda reads editor.kb.store (a CozoKbStore), not editor.kb.primary
        // (the in-memory KnowledgeBase kb_search/kb_get use) -- see the
        // pre-existing kb_agenda_missing_role_dispatches_to_missing_role_filter
        // test above for the same setup pattern.
        use mae_kb::KbStore;
        let store = mae_kb::CozoKbStore::open_mem().unwrap();
        store
            .insert_node(
                &seed_node_with("seed:residency-i", "seed agenda content")
                    .with_tags(["residencytesttag"]),
            )
            .unwrap();
        store
            .insert_node(
                &non_seed_node_with("user:residency-j", "user agenda content")
                    .with_tags(["residencytesttag"]),
            )
            .unwrap();

        let mut editor = Editor::new();
        editor.kb.store = Some(std::sync::Arc::new(store));
        editor.kb.registry.primary_ai_residency = mae_kb::federation::AiResidency::LocalModelsOnly;

        let result = execute_kb_agenda(
            &editor,
            &serde_json::json!({"filter": "tag", "value": "residencytesttag"}),
            Some("claude"),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&result).unwrap();
        let nodes = value["nodes"].as_array().unwrap();
        assert!(
            nodes.iter().any(|n| n["id"] == "seed:residency-i"),
            "seed content must stay reachable from a restricted primary: {nodes:?}"
        );
        assert!(
            !nodes.iter().any(|n| n["id"] == "user:residency-j"),
            "non-seed content must be filtered out of a restricted primary: {nodes:?}"
        );
        // count must reflect the already-filtered node list, not the
        // pre-filter total -- kept consistent by construction (both derive
        // from the same filtered `nodes` vec), asserted here explicitly.
        assert_eq!(value["count"], nodes.len());
    }

    #[test]
    fn kb_agenda_local_provider_bypasses_residency_filter_entirely() {
        use mae_kb::KbStore;
        let store = mae_kb::CozoKbStore::open_mem().unwrap();
        store
            .insert_node(
                &non_seed_node_with("user:residency-k", "user agenda content")
                    .with_tags(["residencytesttagtwo"]),
            )
            .unwrap();

        let mut editor = Editor::new();
        editor.kb.store = Some(std::sync::Arc::new(store));
        editor.kb.registry.primary_ai_residency = mae_kb::federation::AiResidency::LocalModelsOnly;

        let result = execute_kb_agenda(
            &editor,
            &serde_json::json!({"filter": "tag", "value": "residencytesttagtwo"}),
            Some("ollama"),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&result).unwrap();
        let nodes = value["nodes"].as_array().unwrap();
        assert!(
            nodes.iter().any(|n| n["id"] == "user:residency-k"),
            "a local provider must bypass filtering entirely: {nodes:?}"
        );
    }
}
