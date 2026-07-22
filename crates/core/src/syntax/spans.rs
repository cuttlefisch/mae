//! Per-frame syntax span computation and caching.

use std::collections::HashMap;

use super::HighlightSpan;

/// Shared type alias for the per-frame syntax span map.
/// Uses `Arc` to avoid cloning all highlight spans every frame.
pub type SyntaxSpanMap = HashMap<usize, std::sync::Arc<Vec<HighlightSpan>>>;

/// Result of resolving one visible buffer's cached spans for the current
/// frame — shared by [`compute_visible_syntax_spans`] and
/// [`cached_visible_syntax_spans`] so both backends make the exact same
/// fresh/stale/missing decision (#355 — the GUI fast path used to skip this
/// entirely and serve stale spans unconditionally, causing misaligned
/// "highlight sliding" whenever a buffer's `generation` was bumped without a
/// `redraw_level` escalation, e.g. a programmatic edit that forgot to call
/// `mark_full_redraw`).
enum SpanResolution {
    /// Usable now — either fresh, or a large file's accepted stale-serve
    /// (already queued into `syntax_reparse_pending` if it needed one).
    Ready(std::sync::Arc<Vec<HighlightSpan>>),
    /// Stale and small enough to reparse synchronously right now — caller
    /// must do so (immediately, or not at all if it wants to stay cheap).
    NeedsSyncReparse,
    /// No cached spans exist at all yet (first-ever computation).
    NoCacheAtAll,
}

/// Resolve `idx`'s cached spans for generation `gen`, mutating
/// `syntax_reparse_pending`/`perf_stats` exactly as the two call sites
/// already did inline before this was extracted. Does not itself perform a
/// synchronous reparse — see [`SpanResolution`].
fn resolve_span_cache(editor: &mut crate::editor::Editor, idx: usize, gen: u64) -> SpanResolution {
    let Some(buf) = editor.buffers.get(idx) else {
        return SpanResolution::NoCacheAtAll;
    };
    match editor.syntax.cached_spans_arc(idx, gen) {
        Some((arc, true)) => {
            // Fresh cache — cheap Arc clone (no data copy).
            // For large files, also check if scrolling moved outside cached viewport.
            if buf.rope().len_lines() > editor.large_file_lines {
                let scroll = editor
                    .window_mgr
                    .iter_windows()
                    .find(|w| w.buffer_idx == idx)
                    .map(|w| w.scroll_offset)
                    .unwrap_or(0);
                let vh = editor.viewport_height.max(50);
                let vp_start = scroll.saturating_sub(vh * 2);
                let vp_end = (scroll + vh * 3).min(buf.rope().len_lines());
                if !editor.syntax.viewport_covers(idx, vp_start, vp_end) {
                    editor.syntax_reparse_pending.insert(idx);
                    editor.perf_stats.cache_miss_count += 1;
                    editor.perf_stats.syntax_cache_misses += 1;
                } else {
                    editor.perf_stats.syntax_cache_hits += 1;
                }
            } else {
                editor.perf_stats.syntax_cache_hits += 1;
            }
            SpanResolution::Ready(arc)
        }
        Some((arc, false)) => {
            // Stale cache. For non-large files, reparse immediately to avoid
            // rendering with shifted byte offsets (causes highlight sliding).
            let line_count = buf.rope().len_lines();
            editor.perf_stats.cache_miss_count += 1;
            editor.perf_stats.syntax_cache_misses += 1;
            if line_count <= editor.large_file_lines {
                SpanResolution::NeedsSyncReparse
            } else {
                // Large file: use stale spans, queue deferred reparse.
                editor.syntax_reparse_pending.insert(idx);
                SpanResolution::Ready(arc)
            }
        }
        None => SpanResolution::NoCacheAtAll,
    }
}

/// Compute tree-sitter highlight spans for every text buffer visible in the
/// current window layout. Uses stale spans during typing (never blocks render)
/// and queues buffers for deferred reparse into `editor.syntax_reparse_pending`.
///
/// Synchronous parse only happens on first file open (no cached spans at all).
pub fn compute_visible_syntax_spans(editor: &mut crate::editor::Editor) -> SyntaxSpanMap {
    let mut out: SyntaxSpanMap = HashMap::new();
    let mut need_first_parse: Vec<(usize, u64)> = Vec::new();
    let visible_idxs: Vec<usize> = editor
        .window_mgr
        .iter_windows()
        .map(|w| w.buffer_idx)
        .collect();
    for idx in visible_idxs {
        if out.contains_key(&idx) || need_first_parse.iter().any(|(i, _)| *i == idx) {
            continue;
        }
        let Some(buf) = editor.buffers.get(idx) else {
            continue;
        };
        if !matches!(buf.kind, crate::buffer::BufferKind::Text) {
            continue;
        }
        if editor.syntax.language_of(idx).is_none() {
            continue;
        }
        let gen = editor.buffers[idx].generation;
        match resolve_span_cache(editor, idx, gen) {
            SpanResolution::Ready(arc) => {
                out.insert(idx, arc);
            }
            SpanResolution::NeedsSyncReparse | SpanResolution::NoCacheAtAll => {
                need_first_parse.push((idx, gen));
            }
        }
    }

    // Synchronous first-parse only for buffers with no cached spans at all.
    let large_file_lines = editor.large_file_lines;
    for (idx, gen) in need_first_parse {
        let line_count = editor.buffers[idx].rope().len_lines();
        if line_count > large_file_lines {
            let scroll = editor
                .window_mgr
                .iter_windows()
                .find(|w| w.buffer_idx == idx)
                .map(|w| w.scroll_offset)
                .unwrap_or(0);
            let vh = editor.viewport_height.max(50);
            let vp_start = scroll.saturating_sub(vh * 2);
            let vp_end = (scroll + vh * 3).min(line_count);
            let rope = editor.buffers[idx].rope().clone();
            if let Some(arc) = editor
                .syntax
                .spans_for_viewport_arc(idx, &rope, gen, vp_start, vp_end)
            {
                out.insert(idx, arc);
            }
        } else {
            let source: String = editor.buffers[idx].rope().chars().collect();
            if let Some(arc) = editor.syntax.spans_for_arc(idx, &source, gen) {
                out.insert(idx, arc);
            }
        }
    }

    // Recompute display regions for visible text buffers whose generation changed.
    // Collect indices first to avoid borrow conflicts.
    let display_region_bufs: Vec<usize> = editor
        .window_mgr
        .iter_windows()
        .map(|w| w.buffer_idx)
        .filter(|&idx| {
            let buf = &editor.buffers[idx];
            buf.kind == crate::buffer::BufferKind::Text && buf.display_regions_gen != buf.generation
        })
        .collect();
    for idx in display_region_bufs {
        // Skip display regions entirely for degraded (large) files —
        // link concealment and inline images add no value when features are shed.
        if editor.should_degrade_features(idx) {
            let gen = editor.buffers[idx].generation;
            editor.buffers[idx].display_regions.clear();
            editor.buffers[idx].display_regions_gen = gen;
            continue;
        }
        // Bypass debounce when display_regions_gen == u64::MAX (explicit force signal
        // from toggle-inline-images / toggle-image-at-point).
        let force = editor.buffers[idx].display_regions_gen == u64::MAX;
        if !force {
            // Debounce: defer recomputation until configured ms after the last edit.
            // Stale display regions are approximately correct and self-correct.
            let now = std::time::Instant::now();
            let dirty_since = *editor.buffers[idx]
                .display_regions_dirty_since
                .get_or_insert(now);
            if now.duration_since(dirty_since)
                < std::time::Duration::from_millis(editor.display_region_debounce_ms)
            {
                continue; // use stale regions, recompute later
            }
        }
        editor.buffers[idx].display_regions_dirty_since = None;
        let link_descriptive = editor.link_descriptive_for(idx);
        let inline_images = editor.inline_images_for(idx);
        editor.buffers[idx].recompute_display_regions(link_descriptive, inline_images);
    }

    // Set display_reveal_cursor per-frame for the focused window's buffer.
    // This implements org-appear: when cursor is inside a display region,
    // that region is suppressed so raw text is visible for editing.
    let focused_idx = editor.window_mgr.focused_window().buffer_idx;
    if !editor.buffers[focused_idx].display_regions.is_empty() {
        let win = editor.window_mgr.focused_window();
        let buf = &editor.buffers[focused_idx];
        let char_offset = buf.char_offset_at(win.cursor_row, win.cursor_col);
        let byte_offset = buf.rope().char_to_byte(char_offset);
        editor.buffers[focused_idx].display_reveal_cursor = Some(byte_offset);
    } else {
        editor.buffers[focused_idx].display_reveal_cursor = None;
    }

    out
}

/// Return cached syntax spans, reusing whatever was computed last frame —
/// the GUI's fast path for `redraw_level <= Scroll` (pure scroll/cursor
/// movement, no edit). Despite the name, this is NOT "never reparse at any
/// cost": if the cache turns out to be stale (#355 — e.g. a programmatic
/// buffer edit that bumped `generation` without escalating `redraw_level`
/// past this fast path), it self-corrects via the same
/// reparse-or-queue decision [`compute_visible_syntax_spans`] uses, rather
/// than ever painting misaligned ("sliding") highlights. This only costs a
/// synchronous reparse in that rare case; the common case (truly fresh
/// cache) stays a cheap `Arc` clone as before.
pub fn cached_visible_syntax_spans(editor: &mut crate::editor::Editor) -> SyntaxSpanMap {
    let mut out: SyntaxSpanMap = HashMap::new();
    let visible_idxs: Vec<usize> = editor
        .window_mgr
        .iter_windows()
        .map(|w| w.buffer_idx)
        .collect();
    for idx in visible_idxs {
        if out.contains_key(&idx) {
            continue;
        }
        let Some(buf) = editor.buffers.get(idx) else {
            continue;
        };
        if !matches!(buf.kind, crate::buffer::BufferKind::Text) {
            continue;
        }
        let gen = editor.buffers[idx].generation;
        match resolve_span_cache(editor, idx, gen) {
            SpanResolution::Ready(arc) => {
                out.insert(idx, arc);
            }
            SpanResolution::NeedsSyncReparse => {
                // Rare safety net: stale despite being the "no reparse"
                // fast path. Only ever fires once per such mutation, since
                // typing afterward always escalates redraw_level past this
                // path (see compute_visible_syntax_spans).
                let source: String = editor.buffers[idx].rope().chars().collect();
                if let Some(arc) = editor.syntax.spans_for_arc(idx, &source, gen) {
                    out.insert(idx, arc);
                }
            }
            SpanResolution::NoCacheAtAll => {
                // No cached spans exist yet at all -- preserve this
                // function's "no reparse" contract for that case.
            }
        }
    }

    // Still update display_reveal_cursor for org-appear.
    let focused_idx = editor.window_mgr.focused_window().buffer_idx;
    if !editor.buffers[focused_idx].display_regions.is_empty() {
        let win = editor.window_mgr.focused_window();
        let buf = &editor.buffers[focused_idx];
        let char_offset = buf.char_offset_at(win.cursor_row, win.cursor_col);
        let byte_offset = buf.rope().char_to_byte(char_offset);
        editor.buffers[focused_idx].display_reveal_cursor = Some(byte_offset);
    } else {
        editor.buffers[focused_idx].display_reveal_cursor = None;
    }

    out
}

/// Perform deferred syntax reparses for buffers in `syntax_reparse_pending`.
/// Called from event loops after a debounce period (~50ms after last edit).
/// Large files (> `large_file_lines`) use viewport-local O(viewport) reparse.
pub fn drain_pending_reparses(editor: &mut crate::editor::Editor) {
    let pending: Vec<usize> = editor.syntax_reparse_pending.drain().collect();
    let large_file_lines = editor.large_file_lines;
    for idx in pending {
        let Some(buf) = editor.buffers.get(idx) else {
            continue;
        };
        let gen = buf.generation;
        let line_count = buf.rope().len_lines();

        if line_count > large_file_lines {
            let scroll = editor
                .window_mgr
                .iter_windows()
                .find(|w| w.buffer_idx == idx)
                .map(|w| w.scroll_offset)
                .unwrap_or(0);
            let vh = editor.viewport_height.max(50);
            let vp_start = scroll.saturating_sub(vh * 2);
            let vp_end = (scroll + vh * 3).min(line_count);
            let rope = buf.rope().clone();
            editor
                .syntax
                .spans_for_viewport(idx, &rope, gen, vp_start, vp_end);
        } else {
            let source: String = buf.rope().chars().collect();
            editor.syntax.spans_for(idx, &source, gen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::Editor;

    fn rust_editor(text: &str) -> Editor {
        let mut buf = Buffer::new();
        buf.set_file_path(std::path::PathBuf::from("/tmp/test.rs"));
        buf.insert_text_at(0, text);
        let mut editor = Editor::with_buffer(buf);
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.window_mgr.focused_window_mut().cursor_col = 0;
        editor
    }

    /// #355 regression: `cached_visible_syntax_spans` must not serve spans
    /// computed against stale (pre-mutation) content. Simulates the bug's
    /// exact precondition — a direct rope mutation that bumps `generation`
    /// without any `mark_*` call (e.g. an AI tool edit) — then exercises the
    /// GUI's `redraw_level <= Scroll` fast path directly.
    #[test]
    fn cached_visible_syntax_spans_self_corrects_stale_cache() {
        let mut editor = rust_editor("let a = 1;\n");

        // Populate the initial cache via the normal (full) path.
        let first = compute_visible_syntax_spans(&mut editor);
        let spans = first.get(&0).expect("expected spans for buffer 0");
        let let_span = spans
            .iter()
            .find(|s| s.theme_key.contains("keyword"))
            .expect("expected a keyword span for `let`");
        assert_eq!(let_span.byte_start, 0, "`let` should start at byte 0");

        // Directly mutate the rope, bumping `generation`, without calling
        // any `mark_*` escalation -- exactly what a buggy AI-tool edit does.
        editor.buffers[0].insert_text_at(0, "// comment\n");
        assert_eq!(
            editor.buffers[0].rope().to_string(),
            "// comment\nlet a = 1;\n"
        );

        // The GUI fast path must self-correct rather than serve the stale
        // (byte-offset-misaligned) spans computed before the mutation.
        let second = cached_visible_syntax_spans(&mut editor);
        let spans = second
            .get(&0)
            .expect("expected self-corrected spans for buffer 0");
        let let_span = spans
            .iter()
            .find(|s| s.theme_key.contains("keyword"))
            .expect("expected a keyword span for `let` after self-correction");
        assert_eq!(
            let_span.byte_start,
            "// comment\n".len(),
            "`let` keyword span must reflect its shifted position in the new content, \
             not the stale pre-mutation offset"
        );
    }

    /// #355 adversarial case: large files must keep queuing a deferred
    /// reparse instead of synchronously reparsing on the fast path -- the
    /// self-correction added for the bug above must not regress the
    /// large-file performance contract the original design protected.
    #[test]
    fn cached_visible_syntax_spans_large_file_queues_instead_of_sync_reparse() {
        let mut lines = String::new();
        for i in 0..50 {
            lines.push_str(&format!("let x{i} = {i};\n"));
        }
        let mut editor = rust_editor(&lines);
        editor.large_file_lines = 10;

        // Populate the initial cache via the normal (full) path.
        let first = compute_visible_syntax_spans(&mut editor);
        assert!(
            first.contains_key(&0),
            "expected initial spans for buffer 0"
        );
        editor.syntax_reparse_pending.clear();

        // Mutate without any `mark_*` escalation, same precondition as above.
        editor.buffers[0].insert_text_at(0, "// comment\n");

        let second = cached_visible_syntax_spans(&mut editor);
        // Large-file stale cache is served as-is (queued for deferred
        // reparse), never synchronously reparsed on this fast path.
        assert!(
            second.contains_key(&0),
            "expected stale-but-served spans for the large file"
        );
        assert!(
            editor.syntax_reparse_pending.contains(&0),
            "expected buffer 0 to be queued for deferred reparse, not \
             synchronously reparsed on the cheap fast path"
        );
    }
}
