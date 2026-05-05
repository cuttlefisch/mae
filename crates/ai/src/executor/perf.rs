//! Performance tools: `perf_stats`, `perf_benchmark`, and `perf_profile`.

use std::collections::HashMap;

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
    editor: &mut Editor,
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
        "scroll_stress" => {
            // Scroll stress test: dispatch scroll-down-line N times on current buffer.
            // Records per-iteration timing from perf_stats.last_command_us.
            let mut times = Vec::with_capacity(size);
            for _ in 0..size {
                let start = std::time::Instant::now();
                editor.dispatch_builtin("scroll-down-line");
                let us = start.elapsed().as_micros() as u64;
                times.push(us);
            }
            times.sort();
            let total: u64 = times.iter().sum();
            let mean = total / size.max(1) as u64;
            let min = *times.first().unwrap_or(&0);
            let max = *times.last().unwrap_or(&0);
            let p50 = times.get(size / 2).copied().unwrap_or(0);
            let p95 = times.get(size * 95 / 100).copied().unwrap_or(0);

            // Find the 5 slowest iterations with their positions.
            let mut indexed: Vec<(usize, u64)> = times.iter().copied().enumerate().collect();
            indexed.sort_by(|a, b| b.1.cmp(&a.1));
            let slowest: Vec<serde_json::Value> = indexed
                .iter()
                .take(5)
                .map(|(i, us)| {
                    serde_json::json!({
                        "iter": i,
                        "us": us,
                    })
                })
                .collect();

            let result = serde_json::json!({
                "benchmark": "scroll_stress",
                "iterations": size,
                "min_us": min,
                "max_us": max,
                "p50_us": p50,
                "p95_us": p95,
                "mean_us": mean,
                "slowest_iterations": slowest,
            });
            return Ok(serde_json::to_string_pretty(&result).unwrap());
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

pub(crate) fn execute_perf_profile(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("report");

    match action {
        "start" => {
            editor.event_recorder.start_recording();
            Ok(r#"{"status": "recording_started"}"#.to_string())
        }
        "stop" => {
            editor.event_recorder.stop_recording();
            Ok(r#"{"status": "recording_stopped"}"#.to_string())
        }
        "report" => {
            let frames = editor.event_recorder.frames();
            let events = editor.event_recorder.events();
            let total_frames = frames.len();
            let total_events = events.len();

            if total_frames == 0 {
                return Ok(serde_json::json!({
                    "error": "No frames recorded. Call with action='start', perform actions, then action='report'."
                })
                .to_string());
            }

            // Compute duration from first to last frame.
            let duration_us = frames
                .back()
                .map(|f| f.offset_us)
                .unwrap_or(0)
                .saturating_sub(frames.front().map(|f| f.offset_us).unwrap_or(0));
            let duration_ms = duration_us / 1000;

            // Frame time statistics.
            let mut frame_times: Vec<u64> = frames.iter().map(|f| f.frame_time_us).collect();
            frame_times.sort();
            let min_us = *frame_times.first().unwrap_or(&0);
            let max_us = *frame_times.last().unwrap_or(&0);
            let p50_us = frame_times.get(total_frames / 2).copied().unwrap_or(0);
            let p95_us = frame_times
                .get(total_frames * 95 / 100)
                .copied()
                .unwrap_or(0);

            // Redraw level distribution.
            let mut redraw_dist: HashMap<String, u64> = HashMap::new();
            for f in frames.iter() {
                *redraw_dist.entry(f.redraw_level.clone()).or_insert(0) += 1;
            }

            // Cache hit rates.
            let syntax_hits = frames.iter().filter(|f| f.syntax_cache_hit).count();
            let vr_hits = frames.iter().filter(|f| f.visual_rows_cache_hit).count();
            let syntax_hit_rate = syntax_hits as f64 / total_frames as f64;
            let vr_hit_rate = vr_hits as f64 / total_frames as f64;

            // Slow frames (>16.7ms = below 60fps).
            let mut slow_frames: Vec<serde_json::Value> = frames
                .iter()
                .filter(|f| f.frame_time_us > 16_667)
                .take(10)
                .map(|f| {
                    serde_json::json!({
                        "offset_ms": f.offset_us / 1000,
                        "frame_time_us": f.frame_time_us,
                        "total_render_us": f.total_render_us,
                        "redraw_level": f.redraw_level,
                        "render_syntax_us": f.render_syntax_us,
                        "render_layout_us": f.render_layout_us,
                        "render_draw_us": f.render_draw_us,
                    })
                })
                .collect();
            slow_frames.truncate(10);

            // Auto-diagnosis heuristics.
            let mut diagnosis: Vec<String> = Vec::new();
            let full_count = redraw_dist.get("Full").copied().unwrap_or(0);
            let full_pct = full_count as f64 / total_frames as f64 * 100.0;
            if full_pct > 80.0 {
                diagnosis.push(format!(
                    "{:.0}% of frames used Full redraw — scroll commands should use Scroll level",
                    full_pct
                ));
            }
            if syntax_hit_rate < 0.1 && total_frames > 10 {
                diagnosis.push(format!(
                    "syntax cache hit rate is {:.0}% — every frame recomputes syntax spans",
                    syntax_hit_rate * 100.0
                ));
            }
            if p95_us > 16_667 {
                diagnosis.push(format!(
                    "p95 frame time is {}μs (>{}) — below 60fps target",
                    p95_us, 16_667
                ));
            }
            if max_us > 100_000 {
                diagnosis.push(format!(
                    "worst frame took {}ms — perceptible stall",
                    max_us / 1000
                ));
            }
            // Check if render time exceeds frame budget.
            let render_over_budget = frames.iter().filter(|f| f.total_render_us > 16_667).count();
            if render_over_budget > 0 {
                diagnosis.push(format!(
                    "{} frames had render time exceeding 16ms frame budget",
                    render_over_budget
                ));
            }

            let result = serde_json::json!({
                "duration_ms": duration_ms,
                "total_frames": total_frames,
                "total_input_events": total_events,
                "frame_stats": {
                    "min_us": min_us,
                    "max_us": max_us,
                    "p50_us": p50_us,
                    "p95_us": p95_us,
                },
                "slow_frames": slow_frames,
                "redraw_level_distribution": redraw_dist,
                "cache_stats": {
                    "syntax_hit_rate": (syntax_hit_rate * 100.0).round() / 100.0,
                    "visual_rows_hit_rate": (vr_hit_rate * 100.0).round() / 100.0,
                },
                "diagnosis": diagnosis,
            });
            Ok(serde_json::to_string_pretty(&result).unwrap())
        }
        _ => Err(format!(
            "Unknown perf_profile action: '{}'. Use 'start', 'stop', or 'report'.",
            action
        )),
    }
}
