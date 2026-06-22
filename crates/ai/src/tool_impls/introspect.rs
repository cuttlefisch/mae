//! AI `introspect` tool — comprehensive MAE diagnostics.
//!
//! Returns structured JSON covering threads, performance, locks,
//! buffers, shell, and AI state.

use mae_core::Editor;
use serde_json::json;

pub fn execute_introspect(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let section = args
        .get("section")
        .and_then(|v| v.as_str())
        .unwrap_or("all");

    let mut result = serde_json::Map::new();

    // Always include version for diagnostic context
    if section == "all" || section == "version" {
        result.insert(
            "version".into(),
            json!({
                "mae": env!("CARGO_PKG_VERSION"),
                "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            }),
        );
    }
    if section == "all" || section == "modules" {
        let loaded: Vec<&str> = editor
            .active_modules
            .iter()
            .filter(|m| m.status == "loaded")
            .map(|m| m.name.as_str())
            .collect();
        let failed: Vec<&str> = editor
            .active_modules
            .iter()
            .filter(|m| m.status != "loaded")
            .map(|m| m.name.as_str())
            .collect();
        result.insert(
            "modules".into(),
            json!({
                "total": editor.active_modules.len(),
                "loaded_count": loaded.len(),
                "loaded": loaded,
                "failed_count": failed.len(),
                "failed": failed,
            }),
        );
    }

    if section == "all" || section == "threads" {
        result.insert("threads".into(), build_threads_section());
    }
    if section == "all" || section == "perf" {
        result.insert("perf".into(), build_perf_section(editor));
    }
    if section == "all" || section == "locks" {
        result.insert("locks".into(), build_locks_section());
    }
    if section == "all" || section == "buffers" {
        result.insert("buffers".into(), build_buffers_section(editor));
    }
    if section == "all" || section == "shell" {
        result.insert("shell".into(), build_shell_section(editor));
    }
    if section == "all" || section == "ai" {
        result.insert("ai".into(), build_ai_section(editor));
    }
    if section == "all" || section == "kb" {
        result.insert("kb".into(), build_kb_section(editor));
    }
    if section == "all" || section == "lsp" {
        result.insert("lsp".into(), build_lsp_section(editor));
    }
    if section == "all" || section == "collaboration" {
        result.insert("collaboration".into(), build_collaboration_section(editor));
    }
    if section == "all" || section == "scheme" {
        result.insert("scheme".into(), build_scheme_section(editor));
    }
    if section == "frame" {
        result.insert("frame".into(), build_frame_section(editor));
    }

    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON serialization error: {}", e))
}

fn build_threads_section() -> serde_json::Value {
    // Read /proc/self/status for thread count
    let thread_count = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Threads:"))
                .map(|l| l.split_whitespace().nth(1).unwrap_or("0").to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    json!({
        "os_thread_count": thread_count,
    })
}

fn build_perf_section(editor: &Editor) -> serde_json::Value {
    let ps = &editor.perf_stats;
    json!({
        "frame_time_us": ps.frame_time_us,
        "avg_frame_time_us": ps.avg_frame_time_us,
        "fps": ps.fps(),
        "rss_bytes": ps.rss_bytes,
        "cpu_percent": ps.cpu_percent,
        "stall_count": ps.stall_count,
        "jank_count": ps.jank_count,
        "last_command_us": ps.last_command_us,
        "last_command_name": ps.last_command_name,
        "cache_miss_count": ps.cache_miss_count,
        "render_syntax_us": ps.render_syntax_us,
        "render_layout_us": ps.render_layout_us,
        "render_draw_us": ps.render_draw_us,
        "syntax_cache_hits": ps.syntax_cache_hits,
        "syntax_cache_misses": ps.syntax_cache_misses,
        "markup_cache_hits": ps.markup_cache_hits,
        "markup_cache_misses": ps.markup_cache_misses,
        "visual_rows_cache_hits": ps.visual_rows_cache_hits,
        "visual_rows_cache_misses": ps.visual_rows_cache_misses,
        "recent_anomalies": ps.anomaly_log.iter().map(|a| {
            json!({
                "frame": a.frame_number,
                "duration_us": a.duration_us,
                "kind": format!("{:?}", a.kind),
            })
        }).collect::<Vec<_>>(),
    })
}

fn build_locks_section() -> serde_json::Value {
    let stats = mae_core::lock_stats::snapshot();
    let entries: serde_json::Map<String, serde_json::Value> = stats
        .into_iter()
        .map(|(name, entry)| {
            (
                name,
                json!({
                    "acquisitions": entry.acquisitions,
                    "total_wait_us": entry.total_wait_us,
                    "max_wait_us": entry.max_wait_us,
                    "currently_held": entry.currently_held,
                }),
            )
        })
        .collect();
    serde_json::Value::Object(entries)
}

fn build_buffers_section(editor: &Editor) -> serde_json::Value {
    use mae_core::buffer::BufferKind;
    let mut by_kind: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut total_bytes: usize = 0;
    let mut shell_count = 0;
    let mut buffers = Vec::new();
    for b in &editor.buffers {
        *by_kind.entry(format!("{:?}", b.kind)).or_default() += 1;
        total_bytes += b.rope().len_bytes();
        if b.kind == BufferKind::Shell {
            shell_count += 1;
        }

        let mut b_info = json!({
            "name": b.name,
            "kind": format!("{:?}", b.kind),
            "modified": b.modified,
            "line_count": b.line_count(),
            "folded_ranges_count": b.folded_ranges.len(),
            "has_git_status": b.git_status_view().is_some(),
        });

        if b.kind == BufferKind::GitStatus {
            if let Some(gs) = b.git_status_view() {
                b_info["git_repo_root"] = json!(gs.repo_root);
            }
        }

        buffers.push(b_info);
    }
    let hover_info = editor.lsp.hover_popup.as_ref().map(|p| {
        json!({
            "buffer_idx": p.buffer_idx,
            "anchor_row": p.anchor_row,
            "anchor_col": p.anchor_col,
            "content_len": p.contents.len(),
        })
    });

    json!({
        "total": editor.buffers.len(),
        "by_kind": by_kind,
        "total_rope_bytes": total_bytes,
        "shell_buffers": shell_count,
        "buffer_details": buffers,
        "hover_popup": hover_info,
        "code_action_menu": editor.lsp.code_action_menu.is_some(),
        "completion_items": editor.lsp.completion_items.len(),
    })
}

fn build_shell_section(editor: &Editor) -> serde_json::Value {
    json!({
        "viewport_count": editor.shell.viewports.len(),
        "cwd_count": editor.shell.viewport_cwds.len(),
    })
}

fn build_frame_section(editor: &Editor) -> serde_json::Value {
    let ps = &editor.perf_stats;
    let mut visible_buffers = Vec::new();
    for win in editor.window_mgr.iter_windows() {
        if let Some(buf) = editor.buffers.get(win.buffer_idx) {
            visible_buffers.push(json!({
                "idx": win.buffer_idx,
                "name": buf.name,
                "lines": buf.line_count(),
                "degraded": editor.should_degrade_features(win.buffer_idx),
                "scroll_offset": win.scroll_offset,
                "viewport_height": editor.viewport_height,
            }));
        }
    }
    json!({
        "frame_time_us": ps.frame_time_us,
        "total_render_us": ps.total_render_us,
        "render_phase_us": {
            "syntax": ps.render_syntax_us,
            "layout": ps.render_layout_us,
            "draw": ps.render_draw_us,
        },
        "caches": {
            "syntax": { "hits": ps.syntax_cache_hits, "misses": ps.syntax_cache_misses },
            "markup": { "hits": ps.markup_cache_hits, "misses": ps.markup_cache_misses },
            "visual_rows": { "hits": ps.visual_rows_cache_hits, "misses": ps.visual_rows_cache_misses },
        },
        "last_command": {
            "name": ps.last_command_name,
            "us": ps.last_command_us,
        },
        "visible_buffers": visible_buffers,
    })
}

fn build_kb_section(editor: &Editor) -> serde_json::Value {
    let local_nodes = if let Some(q) = editor.kb.query_layer() {
        q.list_ids(None).len()
    } else {
        editor.kb.primary.len()
    };
    let federated_instances = editor.kb.instances.len();
    let total_federated_nodes: usize = editor.kb.instances.values().map(|kb| kb.len()).sum();
    let watcher_count = editor.kb.watchers.len();
    let ws = &editor.kb.watcher_stats;

    // Check for non-default KB options
    let mut option_overrides = serde_json::Map::new();
    if !editor.kb.watcher_enabled {
        option_overrides.insert("kb_watcher_enabled".into(), json!(false));
    }
    if editor.kb.watcher_debounce_ms != 500 {
        option_overrides.insert(
            "kb_watcher_debounce_ms".into(),
            json!(editor.kb.watcher_debounce_ms),
        );
    }
    if editor.kb.max_drain_events != 100 {
        option_overrides.insert(
            "kb_max_drain_events".into(),
            json!(editor.kb.max_drain_events),
        );
    }
    if editor.kb.search_excerpt_length != 500 {
        option_overrides.insert(
            "kb_search_excerpt_length".into(),
            json!(editor.kb.search_excerpt_length),
        );
    }
    if editor.kb.search_max_results != 20 {
        option_overrides.insert(
            "kb_search_max_results".into(),
            json!(editor.kb.search_max_results),
        );
    }

    json!({
        "local_nodes": local_nodes,
        "federated_instances": federated_instances,
        "total_federated_nodes": total_federated_nodes,
        "watcher_count": watcher_count,
        "watcher_stats": {
            "events_upserted": ws.events_upserted,
            "events_removed": ws.events_removed,
            "suppressed_debounce": ws.suppressed_debounce,
            "suppressed_timebox": ws.suppressed_timebox,
            "events_suppressed": ws.events_suppressed,
            "reimports_total": ws.reimports_total,
            "errors": ws.errors,
            "last_drain_us": ws.last_drain_us,
            "last_drain_event_count": ws.last_drain_event_count,
            "drain_us_sum": ws.drain_us_sum,
            "drain_count": ws.drain_count,
        },
        "search_latency_us": editor.perf_stats.kb_search_latency_us,
        "option_overrides": option_overrides,
    })
}

fn build_lsp_section(editor: &Editor) -> serde_json::Value {
    let servers: Vec<serde_json::Value> = editor
        .lsp
        .servers
        .iter()
        .map(|(lang, info)| {
            json!({
                "language": lang,
                "status": format!("{:?}", info.status),
                "command": info.command,
                "binary_found": info.binary_found,
            })
        })
        .collect();
    let any_connected = editor
        .lsp
        .servers
        .values()
        .any(|i| matches!(i.status, mae_core::editor::LspServerStatus::Connected));
    let any_starting = editor
        .lsp
        .servers
        .values()
        .any(|i| matches!(i.status, mae_core::editor::LspServerStatus::Starting));
    json!({
        "server_count": editor.lsp.servers.len(),
        "servers": servers,
        "any_connected": any_connected,
        "any_starting": any_starting,
    })
}

fn build_ai_section(editor: &Editor) -> serde_json::Value {
    let conv_entries = editor.conversation().map(|c| c.entries.len()).unwrap_or(0);
    let context_usage_pct = if editor.ai.context_window > 0 {
        (editor.ai.context_used_tokens as f64 / editor.ai.context_window as f64 * 100.0) as u64
    } else {
        0
    };
    let cache_hit_pct = {
        let total = editor.ai.cache_read_tokens + editor.ai.cache_creation_tokens;
        if total > 0 {
            (editor.ai.cache_read_tokens as f64 / total as f64 * 100.0) as u64
        } else {
            0
        }
    };
    json!({
        "mode": editor.ai.mode,
        "profile": editor.ai.profile,
        "streaming": editor.ai.streaming,
        "input_lock": format!("{:?}", editor.ai.input_lock),
        "conversation_entries": conv_entries,
        "current_round": editor.ai.current_round,
        "transaction_start_idx": editor.ai.transaction_start_idx,
        "session_cost_usd": editor.ai.session_cost_usd,
        "session_tokens_in": editor.ai.session_tokens_in,
        "session_tokens_out": editor.ai.session_tokens_out,
        "cache_read_tokens": editor.ai.cache_read_tokens,
        "cache_creation_tokens": editor.ai.cache_creation_tokens,
        "cache_hit_pct": cache_hit_pct,
        "context_window": editor.ai.context_window,
        "context_used_tokens": editor.ai.context_used_tokens,
        "context_usage_pct": context_usage_pct,
    })
}

fn build_scheme_section(editor: &Editor) -> serde_json::Value {
    let s = &editor.scheme_stats;
    json!({
        "eval_count": s.eval_count,
        "gc_collections": s.collections_count,
        "globals_count": s.globals_count,
        "function_count": s.function_count,
        "stack_hwm": s.stack_hwm,
        "error_count": s.error_count,
    })
}

fn build_collaboration_section(editor: &Editor) -> serde_json::Value {
    let collab_status = editor.collab.status.as_str();
    let collab_server = editor.collab.server_address.clone();

    // Shared-KB sync visibility (ADR-019): the transient broadcast-gate set, the
    // durable owning-instance markers, and the pending-update queue depth. The
    // key diagnostic is divergence — a KB with a durable `shared`/`collab_id`
    // marker but an empty `shared_kbs` entry means edits won't broadcast.
    let mut shared_kbs: Vec<serde_json::Value> = editor
        .collab
        .shared_kbs
        .iter()
        .map(|(kb_id, nodes)| json!({ "kb_id": kb_id, "node_count": nodes.len() }))
        .collect();
    shared_kbs.sort_by(|a, b| a["kb_id"].as_str().cmp(&b["kb_id"].as_str()));

    let owning_instances: Vec<serde_json::Value> = editor
        .kb
        .registry
        .instances
        .iter()
        .filter(|i| i.shared || i.collab_id.is_some())
        .map(|i| {
            json!({
                "name": i.name,
                "uuid": i.uuid,
                "shared": i.shared,
                "collab_id": i.collab_id,
                "gate_present": editor.collab.shared_kbs.contains_key(&i.name)
                    || i
                        .collab_id
                        .as_ref()
                        .is_some_and(|c| editor.collab.shared_kbs.contains_key(c)),
            })
        })
        .collect();

    // ADR-020 observability: an edit to a shared node is persisted to the DURABLE
    // SQLite pending queue at edit time (even offline) — the in-memory queue is empty
    // when store-backed (B-16 single-source emit). Report both so a user/agent can
    // answer "do I have unsynced offline edits?" (the in-mem count alone reads 0
    // offline, which is misleading).
    let durable_pending = editor
        .kb
        .store
        .as_ref()
        .and_then(|s| s.count_pending_updates().ok())
        .unwrap_or(0);
    json!({
        "collab_status": collab_status,
        "collab_server": collab_server,
        "synced_buffers": editor.collab.synced_docs,
        "pending_collab_intent": editor.collab.pending_intent.is_some(),
        "kb_sync_mode": editor.collab.kb_sync_mode,
        "shared_kbs": shared_kbs,
        // Total unsynced edits = transient in-memory (no-store fallback) + durable queue.
        "pending_kb_updates": editor.collab.pending_kb_updates.len() + durable_pending,
        "durable_pending_kb_updates": durable_pending,
        "owning_instances": owning_instances,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_core::editor::{LspServerInfo, LspServerStatus};
    use mae_core::Editor;

    #[test]
    fn introspect_lsp_section_empty() {
        let editor = Editor::new();
        let result = execute_introspect(&editor, &json!({"section": "lsp"})).unwrap();
        let val: serde_json::Value = serde_json::from_str(&result).unwrap();
        let lsp = &val["lsp"];
        assert_eq!(lsp["server_count"], 0);
        assert_eq!(lsp["any_connected"], false);
        assert_eq!(lsp["any_starting"], false);
        assert!(lsp["servers"].as_array().unwrap().is_empty());
    }

    #[test]
    fn introspect_lsp_section_with_servers() {
        let mut editor = Editor::new();
        editor.lsp.servers.insert(
            "rust".to_string(),
            LspServerInfo {
                status: LspServerStatus::Connected,
                command: "rust-analyzer".to_string(),
                binary_found: true,
            },
        );
        editor.lsp.servers.insert(
            "python".to_string(),
            LspServerInfo {
                status: LspServerStatus::Starting,
                command: "pyright".to_string(),
                binary_found: true,
            },
        );
        let result = execute_introspect(&editor, &json!({"section": "lsp"})).unwrap();
        let val: serde_json::Value = serde_json::from_str(&result).unwrap();
        let lsp = &val["lsp"];
        assert_eq!(lsp["server_count"], 2);
        assert_eq!(lsp["any_connected"], true);
        assert_eq!(lsp["any_starting"], true);
        let servers = lsp["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 2);
    }

    #[test]
    fn introspect_collaboration_section() {
        let editor = Editor::new();
        let result = execute_introspect(&editor, &json!({"section": "collaboration"})).unwrap();
        let val: serde_json::Value = serde_json::from_str(&result).unwrap();
        let collab = &val["collaboration"];
        assert_eq!(collab["collab_status"], "off");
        assert!(collab["collab_server"].as_str().is_some());
        assert_eq!(collab["synced_buffers"], 0);
        assert_eq!(collab["pending_collab_intent"], false);
        // ADR-019 observability fields present.
        assert!(collab["kb_sync_mode"].as_str().is_some());
        assert!(collab["shared_kbs"].as_array().unwrap().is_empty());
        assert_eq!(collab["pending_kb_updates"], 0);
        assert!(collab["owning_instances"].as_array().unwrap().is_empty());
    }

    /// ADR-019: the introspect surface must make the broadcast-gate divergence
    /// visible — a KB with a durable `shared`/`collab_id` marker but no
    /// `shared_kbs` entry reports `gate_present: false` (edits won't broadcast).
    #[test]
    fn introspect_surfaces_shared_kb_gate_divergence() {
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(mae_kb::federation::KbInstance {
                uuid: "uuid-collabtest".into(),
                name: "collabtest".into(),
                org_dir: std::path::PathBuf::from("/tmp/collabtest"),
                db_path: std::path::PathBuf::from("/tmp/collabtest.db"),
                primary: false,
                enabled: true,
                last_import: None,
                collab_id: Some("collabtest".into()),
                shared: true,
                remote_peers: Vec::new(),
                last_sync: None,
            });
        // shared_kbs intentionally left empty → divergence.
        let result = execute_introspect(&editor, &json!({"section": "collaboration"})).unwrap();
        let val: serde_json::Value = serde_json::from_str(&result).unwrap();
        let owning = &val["collaboration"]["owning_instances"];
        assert_eq!(owning.as_array().unwrap().len(), 1);
        assert_eq!(owning[0]["name"], "collabtest");
        assert_eq!(owning[0]["shared"], true);
        assert_eq!(
            owning[0]["gate_present"], false,
            "durable marker present but gate empty must surface as gate_present=false"
        );
    }

    #[test]
    fn introspect_all_includes_collaboration() {
        let editor = Editor::new();
        let result = execute_introspect(&editor, &json!({})).unwrap();
        let val: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(
            val.get("collaboration").is_some(),
            "all sections should include collaboration"
        );
        assert_eq!(val["collaboration"]["collab_status"], "off");
    }
}
