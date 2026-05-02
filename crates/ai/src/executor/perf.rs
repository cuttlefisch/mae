//! Performance tools: `perf_stats` and `perf_benchmark`.

use mae_core::Editor;

pub(crate) fn execute_perf_stats(editor: &mut Editor) -> Result<String, String> {
    editor.perf_stats.sample_process_stats();
    let buffer_count = editor.buffers.len();
    let total_lines: usize = editor.buffers.iter().map(|b| b.line_count()).sum();
    let stats = serde_json::json!({
        "rss_bytes": editor.perf_stats.rss_bytes,
        "cpu_percent": editor.perf_stats.cpu_percent,
        "frame_time_us": editor.perf_stats.frame_time_us,
        "avg_frame_time_us": editor.perf_stats.avg_frame_time_us,
        "buffer_count": buffer_count,
        "total_lines": total_lines,
        "debug_mode": editor.debug_mode,
    });
    Ok(serde_json::to_string_pretty(&stats).unwrap())
}

pub(crate) fn execute_perf_benchmark(
    _editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let benchmark = args
        .get("benchmark")
        .and_then(|v| v.as_str())
        .unwrap_or("buffer_insert");
    let size = args.get("size").and_then(|v| v.as_u64()).unwrap_or(1000) as usize;

    let (duration_us, ops_per_sec) = match benchmark {
        "buffer_insert" => {
            let mut buf = mae_core::Buffer::new();
            let start = std::time::Instant::now();
            let mut win = mae_core::WindowManager::new(0);
            for i in 0..size {
                let line = format!("line {} — benchmark test content\n", i);
                for ch in line.chars() {
                    buf.insert_char(win.focused_window_mut(), ch);
                }
            }
            let elapsed = start.elapsed().as_micros() as u64;
            let ops = if elapsed > 0 {
                (size as f64 / (elapsed as f64 / 1_000_000.0)) as u64
            } else {
                0
            };
            (elapsed, ops)
        }
        "buffer_delete" => {
            // Set up a buffer with `size` lines, then measure deletion.
            let mut buf = mae_core::Buffer::new();
            let mut win = mae_core::WindowManager::new(0);
            for i in 0..size {
                let line = format!("line {} — content to delete\n", i);
                for ch in line.chars() {
                    buf.insert_char(win.focused_window_mut(), ch);
                }
            }
            let start = std::time::Instant::now();
            for _ in 0..size {
                if buf.line_count() > 1 {
                    win.focused_window_mut().cursor_row = 0;
                    win.focused_window_mut().cursor_col = 0;
                    buf.delete_line(win.focused_window_mut());
                }
            }
            let elapsed = start.elapsed().as_micros() as u64;
            let ops = if elapsed > 0 {
                (size as f64 / (elapsed as f64 / 1_000_000.0)) as u64
            } else {
                0
            };
            (elapsed, ops)
        }
        "syntax_parse" => {
            // Generate synthetic Rust source and parse it.
            let mut source = String::new();
            for i in 0..size {
                source.push_str(&format!("fn func_{}(x: i32) -> i32 {{ x + {} }}\n", i, i));
            }
            let start = std::time::Instant::now();
            let mut syntax_map = mae_core::syntax::SyntaxMap::new();
            syntax_map.set_language(0, mae_core::syntax::Language::Rust);
            let _ = syntax_map.spans_for(0, &source, 0);
            let elapsed = start.elapsed().as_micros() as u64;
            let ops = if elapsed > 0 {
                (size as f64 / (elapsed as f64 / 1_000_000.0)) as u64
            } else {
                0
            };
            (elapsed, ops)
        }
        _ => return Err(format!("Unknown benchmark type: {}", benchmark)),
    };

    let result = serde_json::json!({
        "benchmark": benchmark,
        "size": size,
        "duration_us": duration_us,
        "ops_per_sec": ops_per_sec,
    });
    Ok(serde_json::to_string_pretty(&result).unwrap())
}
