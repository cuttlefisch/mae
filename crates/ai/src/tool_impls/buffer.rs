use mae_core::Editor;

use super::resolve_buffer_idx;

pub fn execute_buffer_read(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let buf_idx = resolve_buffer_idx(editor, args)?;
    let buf = &editor.buffers[buf_idx];
    let total_lines = buf.line_count();
    let start = args
        .get("start_line")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).saturating_sub(1)) // 1-indexed → 0-indexed
        .unwrap_or(0);
    let end = args
        .get("end_line")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(total_lines);

    let mut output = String::new();
    for i in start..end.min(total_lines) {
        let line = buf.rope().line(i);
        output.push_str(&format!("{:>4} | {}", i + 1, line));
    }
    Ok(output)
}

pub fn execute_buffer_write(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    if editor.ai.mode == "plan" {
        return Err(
            "buffer_write is disabled in plan mode. Use create_plan to draft changes instead."
                .into(),
        );
    }

    let start_line = args
        .get("start_line")
        .and_then(|v| v.as_u64())
        .ok_or("Missing 'start_line' argument")? as usize;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'content' argument")?;

    let buf_idx = resolve_buffer_idx(editor, args)?;

    // ADR-086: `insert_text_at`/`delete_range` silently no-op on a
    // read-only buffer (`Buffer::read_only`), so without this check the
    // call below would do nothing and this function would still report a
    // success describing a write that never happened. The requested
    // postcondition ("this buffer now has this content") cannot hold for a
    // read-only buffer, so refuse before attempting rather than after.
    if editor.buffers[buf_idx].read_only {
        return Err(format!(
            "Buffer '{}' is read-only; buffer_write refused.",
            editor.buffers[buf_idx].name
        ));
    }

    let buf = &mut editor.buffers[buf_idx];
    let total_lines = buf.line_count();

    // Convert 1-indexed to 0-indexed
    let start_idx = (start_line.saturating_sub(1)).min(total_lines);

    let end_line = args.get("end_line").and_then(|v| v.as_u64());

    if let Some(end) = end_line {
        // Replace range: delete lines [start, end], then insert content
        let end_idx = (end as usize).min(total_lines);
        let char_start = buf.rope().line_to_char(start_idx);
        let char_end = if end_idx >= total_lines {
            buf.rope().len_chars()
        } else {
            buf.rope().line_to_char(end_idx)
        };
        buf.delete_range(char_start, char_end);
        if !content.is_empty() {
            buf.insert_text_at(char_start, content);
        }
        // #355: direct rope mutation bumps `generation` but never escalates
        // `redraw_level` on its own -- without this, a subsequent pure
        // scroll/cursor-move frame can serve stale, misaligned syntax spans.
        editor.mark_full_redraw();
        editor.recompute_search_matches();
        editor.clamp_all_cursors();
        Ok(format!(
            "Replaced lines {}-{} ({} chars written)",
            start_line,
            end,
            content.len()
        ))
    } else {
        // Insert before start_line
        let char_pos = if start_idx >= total_lines {
            buf.rope().len_chars()
        } else {
            buf.rope().line_to_char(start_idx)
        };
        buf.insert_text_at(char_pos, content);
        editor.mark_full_redraw();
        editor.recompute_search_matches();
        editor.clamp_all_cursors();
        Ok(format!(
            "Inserted at line {} ({} chars)",
            start_line,
            content.len()
        ))
    }
}

pub fn execute_cursor_info(editor: &Editor) -> Result<String, String> {
    let target_win_id = super::resolve_active_window_id(editor);
    let (buf_idx, row, col, scroll_offset) = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.id == target_win_id)
        .map(|w| (w.buffer_idx, w.cursor_row, w.cursor_col, w.scroll_offset))
        .unwrap_or_else(|| {
            // Fallback: use ai_target_buffer_idx or active buffer.
            let idx = editor
                .ai
                .target_buffer_idx
                .unwrap_or_else(|| editor.active_buffer_idx());
            let win_data = editor
                .window_mgr
                .iter_windows()
                .find(|w| w.buffer_idx == idx)
                .map(|w| (w.cursor_row, w.cursor_col, w.scroll_offset))
                .unwrap_or((0, 0, 0));
            (idx, win_data.0, win_data.1, win_data.2)
        });
    let buf = &editor.buffers[buf_idx];

    let info = serde_json::json!({
        "buffer_name": buf.name,
        "cursor_row": row + 1,
        "cursor_col": col + 1,
        "line_count": buf.line_count(),
        "modified": buf.modified,
        "mode": format!("{:?}", editor.mode),
        "scroll_offset": scroll_offset,
        "viewport_height": editor.viewport_height,
    });
    Ok(info.to_string())
}

pub fn execute_file_read(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let raw_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;
    let path = mae_core::file_picker::expand_tilde(raw_path);

    // Story C (R10): refuse AT THE EFFECT for a detached KB's stale archive.
    //
    // See `stale_archive` for why this refuses rather than returns stale
    // content, and why the wording is shaped the way it is.
    if let Some(msg) = super::stale_archive::refuse_read(editor, &path, "file_read") {
        return Err(msg);
    }

    let content = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "File read error: {} (path: {}). Hint: use absolute paths — call audit_configuration for correct config paths.",
            e, path
        )
    })?;
    let mut output = String::new();
    for (i, line) in content.lines().enumerate() {
        output.push_str(&format!("{:>4} | {}\n", i + 1, line));
    }
    Ok(output)
}

pub fn execute_list_buffers(editor: &Editor) -> Result<String, String> {
    let buffers: Vec<serde_json::Value> = editor
        .buffers
        .iter()
        .enumerate()
        .map(|(i, buf)| {
            // Find window(s) showing this buffer for targeting info.
            let window_ids: Vec<u32> = editor
                .window_mgr
                .iter_windows()
                .filter(|w| w.buffer_idx == i)
                .map(|w| w.id)
                .collect();
            let mut obj = serde_json::json!({
                "index": i,
                "name": buf.name,
                "modified": buf.modified,
                "active": i == editor.active_buffer_idx(),
                "line_count": buf.line_count(),
            });
            if !window_ids.is_empty() {
                obj["window_ids"] = serde_json::json!(window_ids);
            }
            obj
        })
        .collect();
    serde_json::to_string_pretty(&buffers).map_err(|e| e.to_string())
}

#[cfg(test)]
mod buffer_write_tests {
    use super::*;
    use mae_core::redraw::RedrawLevel;

    /// #355: `buffer_write` mutates the rope directly (bumping `generation`)
    /// but previously never escalated `redraw_level` -- leaving a stale
    /// syntax-span cache in place until the next keystroke. Regression guard
    /// for both mutation branches (replace-range and insert-before-line).
    #[test]
    fn execute_buffer_write_replace_range_escalates_redraw_level() {
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "line one\nline two\nline three\n");
        editor.redraw_level = RedrawLevel::None;

        execute_buffer_write(
            &mut editor,
            &serde_json::json!({"start_line": 2, "end_line": 3, "content": "replaced\n"}),
        )
        .unwrap();

        assert!(
            editor.redraw_level >= RedrawLevel::Full,
            "expected redraw_level escalated to Full after a direct rope \
             mutation, got {:?}",
            editor.redraw_level
        );
    }

    #[test]
    fn execute_buffer_write_insert_before_line_escalates_redraw_level() {
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "line one\nline two\n");
        editor.redraw_level = RedrawLevel::None;

        execute_buffer_write(
            &mut editor,
            &serde_json::json!({"start_line": 1, "content": "inserted\n"}),
        )
        .unwrap();

        assert!(
            editor.redraw_level >= RedrawLevel::Full,
            "expected redraw_level escalated to Full after a direct rope \
             mutation, got {:?}",
            editor.redraw_level
        );
    }

    /// ADR-086: a read-only buffer's requested postcondition ("this buffer
    /// now has this content") can never hold -- `insert_text_at`/
    /// `delete_range` silently no-op on `read_only` buffers, so without the
    /// guard this returned `Ok("Replaced lines...")` describing a write
    /// that never happened. Per CLAUDE.md #14 this asserts the FAILING
    /// path, and verifies the buffer is byte-for-byte unchanged afterward
    /// -- not just that an `Err` string was returned.
    #[test]
    fn execute_buffer_write_on_read_only_buffer_is_refused_and_buffer_unchanged() {
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "original line one\noriginal line two\n");
        editor.buffers[0].read_only = true;
        let before = editor.buffers[0].text();

        let result = execute_buffer_write(
            &mut editor,
            &serde_json::json!({"start_line": 1, "end_line": 2, "content": "clobbered\n"}),
        );

        assert!(
            result.is_err(),
            "buffer_write on a read-only buffer must return Err, not a success \
             describing a write that didn't happen: {result:?}"
        );
        assert_eq!(
            editor.buffers[0].text(),
            before,
            "a refused write must leave the read-only buffer's content untouched"
        );
    }

    #[test]
    fn execute_buffer_write_on_read_only_buffer_names_the_buffer_in_the_error() {
        let mut editor = Editor::new();
        editor.buffers[0].name = "*readonly-target*".to_string();
        editor.buffers[0].read_only = true;

        let err = execute_buffer_write(
            &mut editor,
            &serde_json::json!({"start_line": 1, "content": "x"}),
        )
        .expect_err("must refuse writing a read-only buffer");
        assert!(
            err.contains("*readonly-target*"),
            "the refusal must name which buffer was refused, not a generic message: {err}"
        );
    }

    /// ADR-086 D2 guard: fixing the read-only refusal above must not
    /// over-correct into treating a normal, writable buffer's repeat write
    /// as an error. Two identical writes in a row against a writable buffer
    /// must both succeed.
    #[test]
    fn execute_buffer_write_second_identical_write_still_succeeds() {
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "line one\nline two\n");
        let args = serde_json::json!({"start_line": 1, "end_line": 2, "content": "same\n"});

        execute_buffer_write(&mut editor, &args).expect("first write must succeed");
        execute_buffer_write(&mut editor, &args)
            .expect("an identical second write to a writable buffer must still succeed");
    }
}

/// Story C: `file_read` must refuse a DETACHED KB's stale archive.
#[cfg(test)]
mod stale_archive_tests {
    use super::*;
    use tempfile::TempDir;

    use super::super::stale_archive::test_support::editor_with_detached_kb;

    /// **The failure this closes.** A detached KB's `.org` files are no longer
    /// read by any ingest, so their content may be arbitrarily old while looking
    /// authoritative. An agent that reads one answers confidently and WRONGLY,
    /// with nothing in the response to signal it — strictly worse than a refusal.
    #[test]
    fn reading_a_detached_kbs_stale_archive_is_refused() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.org");
        std::fs::write(&path, "STALE CONTENT nobody updates").unwrap();
        let editor = editor_with_detached_kb(dir.path());

        let err = execute_file_read(
            &editor,
            &serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect_err("a stale archive must not be read");

        // The message has to be actionable, not merely a denial: R10 measured
        // that a bare prohibition roughly TRIPLES the wrong-tool rate, and that
        // the shape which works states the consequence AND grants the tool
        // jurisdiction elsewhere.
        assert!(err.contains("kb_search"), "must redirect: {err}");
        assert!(
            err.contains("stale archive"),
            "must say WHY, not just no: {err}"
        );
        assert!(
            err.contains("source code"),
            "must grant file_read jurisdiction elsewhere: {err}"
        );
        assert!(
            !err.contains("STALE CONTENT"),
            "and must not leak the content it refused to serve"
        );
    }

    /// The paired positive, without which the test above passes on an
    /// implementation that refuses everything.
    #[test]
    fn an_ordinary_file_outside_any_detached_kb_still_reads() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "fn main() {}").unwrap();
        let editor = Editor::new(); // no detached instance at all

        let out = execute_file_read(
            &editor,
            &serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect("an ordinary file must still be readable");
        assert!(out.contains("fn main()"));
    }

    /// An ATTACHED KB's files are still live — the ingest reads them — so they
    /// must remain readable. Narrowed, never widened.
    #[test]
    fn an_attached_kbs_org_file_is_not_refused() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.org");
        std::fs::write(&path, "LIVE CONTENT").unwrap();
        // Same registration, but the default (attached) policy.
        let editor = super::super::stale_archive::test_support::editor_with_attached_kb(dir.path());

        let out = execute_file_read(
            &editor,
            &serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect("an attached KB's files are live and must stay readable");
        assert!(out.contains("LIVE CONTENT"));
    }
}
