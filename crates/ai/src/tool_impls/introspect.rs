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
    for b in &editor.buffers {
        *by_kind.entry(format!("{:?}", b.kind)).or_default() += 1;
        total_bytes += b.rope().len_bytes();
        if b.kind == BufferKind::Shell {
            shell_count += 1;
        }
    }
    json!({
        "total": editor.buffers.len(),
        "by_kind": by_kind,
        "total_rope_bytes": total_bytes,
        "shell_buffers": shell_count,
    })
}

fn build_shell_section(editor: &Editor) -> serde_json::Value {
    json!({
        "viewport_count": editor.shell_viewports.len(),
        "cwd_count": editor.shell_cwds.len(),
    })
}

fn build_ai_section(editor: &Editor) -> serde_json::Value {
    let conv_entries = editor.conversation().map(|c| c.entries.len()).unwrap_or(0);
    json!({
        "streaming": editor.ai_streaming,
        "input_lock": format!("{:?}", editor.input_lock),
        "conversation_entries": conv_entries,
        "current_round": editor.ai_current_round,
        "transaction_start_idx": editor.ai_transaction_start_idx,
        "session_cost_usd": editor.ai_session_cost_usd,
        "session_tokens_in": editor.ai_session_tokens_in,
        "session_tokens_out": editor.ai_session_tokens_out,
    })
}
