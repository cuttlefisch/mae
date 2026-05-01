//! Per-frame syntax span computation and caching.

use std::collections::HashMap;

use super::HighlightSpan;

/// Shared type alias for the per-frame syntax span map.
/// Uses `Arc` to avoid cloning all highlight spans every frame.
pub type SyntaxSpanMap = HashMap<usize, std::sync::Arc<Vec<HighlightSpan>>>;

/// Compute tree-sitter highlight spans for every text buffer visible in the
/// current window layout. Uses stale spans during typing (never blocks render)
/// and queues buffers for deferred reparse into `editor.syntax_reparse_pending`.
///
/// Synchronous parse only happens on first file open (no cached spans at all).
pub fn compute_visible_syntax_spans(editor: &mut crate::editor::Editor) -> SyntaxSpanMap {
    let mut out: SyntaxSpanMap = HashMap::new();
    let mut need_first_parse: Vec<(usize, u64)> = Vec::new();
    for win in editor.window_mgr.iter_windows() {
        let idx = win.buffer_idx;
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
        let gen = buf.generation;
        match editor.syntax.cached_spans_arc(idx, gen) {
            Some((arc, true)) => {
                // Fresh cache — cheap Arc clone (no data copy).
                out.insert(idx, arc);
            }
            Some((arc, false)) => {
                // Stale cache — use stale spans for this frame, queue reparse.
                out.insert(idx, arc);
                editor.syntax_reparse_pending.insert(idx);
            }
            None => {
                need_first_parse.push((idx, gen));
            }
        }
    }

    // Synchronous first-parse only for buffers with no cached spans at all.
    for (idx, gen) in need_first_parse {
        let source: String = editor.buffers[idx].rope().chars().collect();
        if let Some(arc) = editor.syntax.spans_for_arc(idx, &source, gen) {
            out.insert(idx, arc);
        }
    }

    // Recompute display regions for visible text buffers whose generation changed.
    for win in editor.window_mgr.iter_windows() {
        let idx = win.buffer_idx;
        let buf = &editor.buffers[idx];
        if buf.kind != crate::buffer::BufferKind::Text {
            continue;
        }
        if buf.display_regions_gen == buf.generation {
            continue;
        }
        let link_descriptive = editor.link_descriptive_for(idx);
        editor.buffers[idx].recompute_display_regions(link_descriptive);
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

/// Return cached syntax spans without triggering any reparses.
/// Used by the CursorOnly fast path — reuses whatever was computed last frame.
pub fn cached_visible_syntax_spans(editor: &mut crate::editor::Editor) -> SyntaxSpanMap {
    let mut out: SyntaxSpanMap = HashMap::new();
    for win in editor.window_mgr.iter_windows() {
        let idx = win.buffer_idx;
        if out.contains_key(&idx) {
            continue;
        }
        let Some(buf) = editor.buffers.get(idx) else {
            continue;
        };
        if !matches!(buf.kind, crate::buffer::BufferKind::Text) {
            continue;
        }
        if let Some((arc, _)) = editor.syntax.cached_spans_arc(idx, buf.generation) {
            out.insert(idx, arc);
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
pub fn drain_pending_reparses(editor: &mut crate::editor::Editor) {
    let pending: Vec<usize> = editor.syntax_reparse_pending.drain().collect();
    for idx in pending {
        let Some(buf) = editor.buffers.get(idx) else {
            continue;
        };
        let gen = buf.generation;
        let source: String = buf.rope().chars().collect();
        editor.syntax.spans_for(idx, &source, gen);
    }
}
