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
    json!({
        "total": editor.buffers.len(),
        "by_kind": by_kind,
        "total_rope_bytes": total_bytes,
        "shell_buffers": shell_count,
        "buffer_details": buffers,
    })
}

fn build_shell_section(editor: &Editor) -> serde_json::Value {
    json!({
        "viewport_count": editor.shell_viewports.len(),
        "cwd_count": editor.shell_cwds.len(),
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

fn build_ai_section(editor: &Editor) -> serde_json::Value {
    let conv_entries = editor.conversation().map(|c| c.entries.len()).unwrap_or(0);
    let context_usage_pct = if editor.ai_context_window > 0 {
        (editor.ai_context_used_tokens as f64 / editor.ai_context_window as f64 * 100.0) as u64
    } else {
        0
    };
    let cache_hit_pct = {
        let total = editor.ai_cache_read_tokens + editor.ai_cache_creation_tokens;
        if total > 0 {
            (editor.ai_cache_read_tokens as f64 / total as f64 * 100.0) as u64
        } else {
            0
        }
    };
    json!({
        "mode": editor.ai_mode,
        "profile": editor.ai_profile,
        "streaming": editor.ai_streaming,
        "input_lock": format!("{:?}", editor.input_lock),
        "conversation_entries": conv_entries,
        "current_round": editor.ai_current_round,
        "transaction_start_idx": editor.ai_transaction_start_idx,
        "session_cost_usd": editor.ai_session_cost_usd,
        "session_tokens_in": editor.ai_session_tokens_in,
        "session_tokens_out": editor.ai_session_tokens_out,
        "cache_read_tokens": editor.ai_cache_read_tokens,
        "cache_creation_tokens": editor.ai_cache_creation_tokens,
        "cache_hit_pct": cache_hit_pct,
        "context_window": editor.ai_context_window,
        "context_used_tokens": editor.ai_context_used_tokens,
        "context_usage_pct": context_usage_pct,
    })
}
