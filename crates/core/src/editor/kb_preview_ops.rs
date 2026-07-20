//! KB-link hover preview (KB-graph-view architecture plan, Part D).
//!
//! Modeled directly on the LSP hover popup (`lsp_ops.rs`'s
//! `apply_hover_result`/`dismiss_hover_popup`/`hover_scroll_down`/
//! `hover_scroll_up`), which already proves out every piece needed. The one
//! structural difference: `HoverPopup` lives as a single top-level field on
//! `Editor.lsp`, while `KbPreviewPopup` lives per-buffer on `KbView`
//! (`crate::kb_view::KbView::kb_preview_popup`) — scoped to KB-view-mode
//! buffers only, since each non-builtin KB node gets its own buffer (see
//! `Editor::ensure_kb_buffer_idx`).
//!
//! `fetch_kb_preview_content` is deliberately a small, swappable function
//! (clear boundary, no inlining into the popup-population call sites) so a
//! future AI-summarization primitive can be substituted for it later
//! without touching the popup/rendering machinery — not built here, just
//! the seam kept clean.

use crate::buffer::BufferKind;
use crate::kb_view::KbPreviewPopup;

use super::Editor;

impl Editor {
    /// Fetch cheap preview content for a KB node: `(title, body)`, body
    /// noise-stripped (`help_ops::strip_kb_body_noise` — drops property
    /// drawers / leading `#+`-keyword lines, the same pre-pass
    /// `render_kb_node_for_query` uses) and truncated to
    /// `kb_preview_max_lines` raw lines. Deliberately does NOT go through
    /// `kb_populate_buffer`/`render_kb_node_for_query` (expensive, mutates a
    /// live buffer's rope) — direct struct access via `kb_for_node` instead.
    /// Returns `None` if the node doesn't exist in the local or any
    /// federated KB.
    pub(crate) fn fetch_kb_preview_content(&self, id: &str) -> Option<(String, String)> {
        let node = self.kb_for_node(id).and_then(|kb| kb.get(id))?;
        let title = node.title.clone();
        let filtered = super::help_ops::strip_kb_body_noise(&node.body);
        let max_lines = self.kb_preview_max_lines;
        let body: String = filtered
            .lines()
            .take(max_lines)
            .collect::<Vec<_>>()
            .join("\n");
        Some((title, body))
    }

    /// The active (focused window's) buffer's KB preview popup, if any.
    /// Mirrors `Editor::kb_view()`'s active-buffer scoping.
    pub fn kb_preview_popup(&self) -> Option<&KbPreviewPopup> {
        self.kb_view().and_then(|v| v.kb_preview_popup.as_ref())
    }

    /// Show the KB preview popup for the KB link at the cursor in the
    /// active buffer, if the active buffer is KB-kind and the cursor is
    /// over a link. Shared by the idle-hover trigger
    /// (`Editor::maybe_show_kb_preview_popup`) and the manual `kb-preview`
    /// command/keybinding — both call the SAME `Editor` method.
    ///
    /// `force`: when `false` (idle trigger), a no-op if a popup for the
    /// exact same `(buffer, row, col)` is already showing, so repeated idle
    /// ticks with a motionless cursor don't re-fetch KB content or force a
    /// redraw every tick. When `true` (manual trigger), always re-fetch and
    /// reset scroll, so re-pressing the key always gives a fresh popup.
    ///
    /// Returns `true` if the popup was (re)populated.
    pub(crate) fn kb_preview_show_at_cursor(&mut self, force: bool) -> bool {
        let buf_idx = self.active_buffer_idx();
        if self.buffers.get(buf_idx).map(|b| b.kind) != Some(BufferKind::Kb) {
            return false;
        }
        let (row, col) = {
            let win = self.window_mgr.focused_window();
            (win.cursor_row, win.cursor_col)
        };
        if !force {
            // Re-arm BEFORE the link lookup below — a suppression at a
            // DIFFERENT position no longer applies now that the cursor has
            // moved, even if the cursor's new position isn't on a link at
            // all (the common case: most idle ticks land off-link). Doing
            // this first means the marker still clears on every such tick
            // instead of only on ticks that happen to land on some link.
            // See `KbView::kb_preview_suppressed_at`'s doc comment for why
            // this is the cheapest hook (already reads live cursor
            // position every idle tick, no separate "cursor moved" event
            // needed).
            if let Some(view) = self.buffers[buf_idx].kb_view_mut() {
                if view
                    .kb_preview_suppressed_at
                    .is_some_and(|p| p != (row, col))
                {
                    view.kb_preview_suppressed_at = None;
                }
            }
        }
        let Some(link) = self.kb_link_at(row, col) else {
            return false;
        };
        if !force {
            let suppressed = self.buffers[buf_idx]
                .kb_view()
                .and_then(|v| v.kb_preview_suppressed_at)
                == Some((row, col));
            if suppressed {
                return false;
            }
            let already_showing = self.buffers[buf_idx]
                .kb_view()
                .and_then(|v| v.kb_preview_popup.as_ref())
                .is_some_and(|p| {
                    p.buffer_idx == buf_idx && p.anchor_row == row && p.anchor_col == col
                });
            if already_showing {
                return false;
            }
        }
        let Some((title, body)) = self.fetch_kb_preview_content(&link.target) else {
            return false;
        };
        let contents = format!("{}\n\n{}", title, body);
        if let Some(view) = self.buffers[buf_idx].kb_view_mut() {
            view.kb_preview_popup = Some(KbPreviewPopup {
                contents,
                buffer_idx: buf_idx,
                anchor_row: row,
                anchor_col: col,
                scroll_offset: 0,
            });
            if force {
                view.kb_preview_suppressed_at = None;
            }
        }
        true
    }

    /// `(kb-preview-show ID)` / `kb_preview_show` MCP tool entry point:
    /// show the preview popup for an ARBITRARY node id, anchored at the
    /// current cursor position. Unlike `kb_preview_show_at_cursor`, the
    /// cursor does not need to be sitting on a link to `id` — this is the
    /// direct, id-addressed primitive the AI peer (or a future
    /// AI-summarization caller) uses. Scoped to KB-view-mode buffers like
    /// the rest of this feature; sets a status message and no-ops outside
    /// one or if `id` doesn't resolve.
    pub fn kb_preview_show(&mut self, id: &str) {
        let buf_idx = self.active_buffer_idx();
        if self.buffers.get(buf_idx).map(|b| b.kind) != Some(BufferKind::Kb) {
            self.set_status("kb-preview-show: not in a KB buffer");
            return;
        }
        let Some((title, body)) = self.fetch_kb_preview_content(id) else {
            self.set_status(format!("kb-preview-show: no such KB node: {}", id));
            return;
        };
        let (row, col) = {
            let win = self.window_mgr.focused_window();
            (win.cursor_row, win.cursor_col)
        };
        let contents = format!("{}\n\n{}", title, body);
        if let Some(view) = self.buffers[buf_idx].kb_view_mut() {
            view.kb_preview_popup = Some(KbPreviewPopup {
                contents,
                buffer_idx: buf_idx,
                anchor_row: row,
                anchor_col: col,
                scroll_offset: 0,
            });
            // Manual, id-addressed invocation always wins — clear any
            // suppression the cursor happens to be sitting on.
            view.kb_preview_suppressed_at = None;
        }
        self.set_status("[KB] K to scroll, any key to dismiss");
    }

    /// Dismiss the KB preview popup on the active buffer, if any — and
    /// suppress idle-triggered re-show at that exact position until the
    /// cursor moves elsewhere. This is the SINGLE funnel for every dismiss
    /// path (the explicit `dismiss-kb-preview-popup` command AND the
    /// generic auto-dismiss guard in `dispatch_builtin_inner` that fires on
    /// any other command while a popup is showing — Escape is just one
    /// command that hits that guard), so fixing suppression here covers
    /// all of them. See `KbView::kb_preview_suppressed_at`'s doc comment.
    pub fn kb_preview_dismiss(&mut self) {
        if let Some(view) = self.kb_view_mut() {
            if let Some(popup) = view.kb_preview_popup.take() {
                view.kb_preview_suppressed_at = Some((popup.anchor_row, popup.anchor_col));
            }
        }
    }

    /// Scroll the KB preview popup down.
    pub fn kb_preview_scroll_down(&mut self) {
        if let Some(view) = self.kb_view_mut() {
            if let Some(popup) = &mut view.kb_preview_popup {
                popup.scroll_offset += 1;
            }
        }
    }

    /// Scroll the KB preview popup up.
    pub fn kb_preview_scroll_up(&mut self) {
        if let Some(view) = self.kb_view_mut() {
            if let Some(popup) = &mut view.kb_preview_popup {
                popup.scroll_offset = popup.scroll_offset.saturating_sub(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    fn editor_with_link() -> (Editor, String) {
        let mut e = Editor::new();
        e.kb.primary.insert(mae_kb::Node::new(
            "user:preview-target",
            "Preview Target",
            mae_kb::NodeKind::Note,
            "Target body text.",
        ));
        e.kb.primary.insert(mae_kb::Node::new(
            "user:preview-source",
            "Preview Source",
            mae_kb::NodeKind::Note,
            "See [[user:preview-target]] for details.",
        ));
        e.open_help_at("user:preview-source");
        (e, "user:preview-target".to_string())
    }

    #[test]
    fn kb_link_at_hits_and_misses() {
        let (e, target) = editor_with_link();
        let link = e
            .kb_view()
            .unwrap()
            .rendered_links
            .iter()
            .find(|l| l.target == target)
            .cloned()
            .expect("target link rendered");
        let rope = e.buffers[e.active_buffer_idx()].rope();
        let row = rope.byte_to_line(link.byte_start);
        let col = link.byte_start - rope.line_to_byte(row);

        // Hit: exactly on the link.
        let found = e.kb_link_at(row, col);
        assert_eq!(found.map(|l| l.target), Some(target.clone()));

        // Miss: far past the end of the buffer.
        let last_row = rope.len_lines().saturating_sub(1);
        assert!(
            e.kb_link_at(last_row + 1000, 0).is_none()
                || e.kb_link_at(last_row + 1000, 0).unwrap().target != target,
            "position far outside content must not spuriously hit the target link"
        );

        // Miss: row 0 col 0 (header line, not a link) in a fresh non-KB buffer.
        let plain = Editor::new();
        assert!(
            plain.kb_link_at(0, 0).is_none(),
            "non-KB buffer must never report a link"
        );
    }

    #[test]
    fn fetch_kb_preview_content_returns_title_and_stripped_body() {
        let (e, target) = editor_with_link();
        let (title, body) = e
            .fetch_kb_preview_content(&target)
            .expect("target node must resolve");
        assert_eq!(title, "Preview Target");
        assert_eq!(body, "Target body text.");
    }

    #[test]
    fn fetch_kb_preview_content_missing_node_is_none() {
        let (e, _) = editor_with_link();
        assert!(e.fetch_kb_preview_content("user:does-not-exist").is_none());
    }

    #[test]
    fn fetch_kb_preview_content_strips_property_drawer_and_truncates() {
        let mut e = Editor::new();
        let long_body = (0..30)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let body = format!(":PROPERTIES:\n:ID: abc\n:END:\n{}", long_body);
        e.kb.primary.insert(mae_kb::Node::new(
            "user:noisy",
            "Noisy",
            mae_kb::NodeKind::Note,
            body,
        ));
        e.kb_preview_max_lines = 5;
        let (title, body) = e.fetch_kb_preview_content("user:noisy").unwrap();
        assert_eq!(title, "Noisy");
        assert!(
            !body.contains(":PROPERTIES:"),
            "property drawer must be stripped"
        );
        assert_eq!(
            body.lines().count(),
            5,
            "body must be truncated to kb_preview_max_lines"
        );
        assert_eq!(body.lines().next(), Some("line 0"));
    }

    #[test]
    fn maybe_show_kb_preview_popup_requires_option_kind_and_link() {
        let (mut e, target) = editor_with_link();
        let link = e
            .kb_view()
            .unwrap()
            .rendered_links
            .iter()
            .find(|l| l.target == target)
            .cloned()
            .unwrap();
        let buf_idx = e.active_buffer_idx();
        let rope = e.buffers[buf_idx].rope();
        let row = rope.byte_to_line(link.byte_start);
        let col = link.byte_start - rope.line_to_byte(row);
        {
            let win = e.window_mgr.focused_window_mut();
            win.cursor_row = row;
            win.cursor_col = col;
        }

        // Option disabled: idle hook must never populate the popup.
        e.kb_preview_on_hover = false;
        assert!(!e.on_idle_tick(10_000));
        assert!(e.kb_preview_popup().is_none());

        // Option enabled, cursor on a link, KB buffer: must populate.
        e.kb_preview_on_hover = true;
        assert!(e.on_idle_tick(10_000));
        let popup = e.kb_preview_popup().expect("popup must be shown");
        assert!(popup.contents.contains("Preview Target"));
        assert_eq!(popup.anchor_row, row);
        assert_eq!(popup.anchor_col, col);

        // Second idle tick, cursor unchanged: must be a no-op (idempotent),
        // not force another redraw.
        assert!(!e.on_idle_tick(10_000));
    }

    #[test]
    fn maybe_show_kb_preview_popup_false_when_cursor_not_on_link() {
        let (mut e, _target) = editor_with_link();
        e.kb_preview_on_hover = true;
        {
            let win = e.window_mgr.focused_window_mut();
            win.cursor_row = 0;
            win.cursor_col = 0;
        }
        // Row/col 0 is the node's own heading line, not a link.
        assert!(!e.on_idle_tick(10_000));
        assert!(e.kb_preview_popup().is_none());
    }

    #[test]
    fn maybe_show_kb_preview_popup_false_outside_kb_buffer() {
        let mut e = Editor::new();
        e.kb_preview_on_hover = true;
        assert!(e.buffers[e.active_buffer_idx()].kind != BufferKind::Kb);
        assert!(!e.on_idle_tick(10_000));
        assert!(e.kb_preview_popup().is_none());
    }

    #[test]
    fn kb_preview_show_by_id_anchors_at_cursor_regardless_of_link() {
        let (mut e, target) = editor_with_link();
        {
            let win = e.window_mgr.focused_window_mut();
            win.cursor_row = 0;
            win.cursor_col = 0;
        }
        e.kb_preview_show(&target);
        let popup = e.kb_preview_popup().expect("popup must be shown");
        assert!(popup.contents.contains("Preview Target"));
        assert_eq!(popup.anchor_row, 0);
        assert_eq!(popup.anchor_col, 0);
    }

    #[test]
    fn kb_preview_show_missing_node_sets_status_and_no_popup() {
        let (mut e, _target) = editor_with_link();
        e.kb_preview_show("user:does-not-exist");
        assert!(e.kb_preview_popup().is_none());
        assert!(e.status_msg.contains("no such KB node"));
    }

    #[test]
    fn kb_preview_dismiss_and_scroll() {
        let (mut e, target) = editor_with_link();
        e.kb_preview_show(&target);
        assert!(e.kb_preview_popup().is_some());

        e.kb_preview_scroll_down();
        assert_eq!(e.kb_preview_popup().unwrap().scroll_offset, 1);
        e.kb_preview_scroll_down();
        assert_eq!(e.kb_preview_popup().unwrap().scroll_offset, 2);
        e.kb_preview_scroll_up();
        assert_eq!(e.kb_preview_popup().unwrap().scroll_offset, 1);
        // Saturates at 0, never negative.
        e.kb_preview_scroll_up();
        e.kb_preview_scroll_up();
        assert_eq!(e.kb_preview_popup().unwrap().scroll_offset, 0);

        e.kb_preview_dismiss();
        assert!(e.kb_preview_popup().is_none());
    }

    #[test]
    fn dispatch_kb_preview_command_shows_and_scrolls() {
        let (mut e, _target) = editor_with_link();
        let target_link = e.kb_view().unwrap().rendered_links[0].clone();
        let rope = e.buffers[e.active_buffer_idx()].rope();
        let row = rope.byte_to_line(target_link.byte_start);
        let col = target_link.byte_start - rope.line_to_byte(row);
        {
            let win = e.window_mgr.focused_window_mut();
            win.cursor_row = row;
            win.cursor_col = col;
        }

        assert!(e.dispatch_builtin("kb-preview"));
        assert!(e.kb_preview_popup().is_some());
        assert_eq!(e.kb_preview_popup().unwrap().scroll_offset, 0);

        // Pressing again while shown scrolls instead of re-fetching.
        assert!(e.dispatch_builtin("kb-preview"));
        assert_eq!(e.kb_preview_popup().unwrap().scroll_offset, 1);

        assert!(e.dispatch_builtin("dismiss-kb-preview-popup"));
        assert!(e.kb_preview_popup().is_none());
    }

    #[test]
    fn any_other_command_auto_dismisses_kb_preview_popup() {
        let (mut e, target) = editor_with_link();
        e.kb_preview_show(&target);
        assert!(e.kb_preview_popup().is_some());

        // Any unrelated command must dismiss it, mirroring the hover popup.
        e.dispatch_builtin("move-right");
        assert!(e.kb_preview_popup().is_none());
    }

    /// Cursor onto the rendered link's position — shared setup for the
    /// suppression tests below (mirrors
    /// `maybe_show_kb_preview_popup_requires_option_kind_and_link`'s pattern).
    fn place_cursor_on_link(e: &mut Editor, target: &str) -> (usize, usize) {
        let link = e
            .kb_view()
            .unwrap()
            .rendered_links
            .iter()
            .find(|l| l.target == target)
            .cloned()
            .unwrap();
        let buf_idx = e.active_buffer_idx();
        let rope = e.buffers[buf_idx].rope();
        let row = rope.byte_to_line(link.byte_start);
        let col = link.byte_start - rope.line_to_byte(row);
        let win = e.window_mgr.focused_window_mut();
        win.cursor_row = row;
        win.cursor_col = col;
        (row, col)
    }

    #[test]
    fn kb_preview_dismiss_suppresses_reshow_at_same_position() {
        let (mut e, target) = editor_with_link();
        e.kb_preview_on_hover = true;
        let (row, col) = place_cursor_on_link(&mut e, &target);

        assert!(e.on_idle_tick(10_000));
        assert!(e.kb_preview_popup().is_some());

        e.kb_preview_dismiss();
        assert!(e.kb_preview_popup().is_none());
        assert_eq!(
            e.kb_view().unwrap().kb_preview_suppressed_at,
            Some((row, col))
        );

        // Re-firing the idle path at the SAME position must stay hidden —
        // this is the reported bug: Escape dismissed it, but it reappeared
        // almost immediately because the idle tick had no memory of the
        // dismissal.
        assert!(!e.on_idle_tick(10_000));
        assert!(e.kb_preview_popup().is_none());
    }

    #[test]
    fn kb_preview_suppression_clears_on_cursor_move_and_can_reshow() {
        let (mut e, target) = editor_with_link();
        e.kb_preview_on_hover = true;
        let (row, col) = place_cursor_on_link(&mut e, &target);
        assert!(e.on_idle_tick(10_000));
        e.kb_preview_dismiss();
        assert_eq!(
            e.kb_view().unwrap().kb_preview_suppressed_at,
            Some((row, col))
        );

        // Move the cursor off the link entirely (row 0 col 0 — the header
        // line, never a link) — idle tick is a no-op (no link under
        // cursor) but must still clear the suppression marker.
        {
            let win = e.window_mgr.focused_window_mut();
            win.cursor_row = 0;
            win.cursor_col = 0;
        }
        assert!(!e.on_idle_tick(10_000));
        assert_eq!(e.kb_view().unwrap().kb_preview_suppressed_at, None);

        // Move back onto the link — must show again now that it's re-armed.
        {
            let win = e.window_mgr.focused_window_mut();
            win.cursor_row = row;
            win.cursor_col = col;
        }
        assert!(e.on_idle_tick(10_000));
        assert!(e.kb_preview_popup().is_some());
    }

    #[test]
    fn kb_preview_force_bypasses_and_clears_suppression() {
        let (mut e, target) = editor_with_link();
        e.kb_preview_on_hover = true;
        let (row, col) = place_cursor_on_link(&mut e, &target);
        assert!(e.on_idle_tick(10_000));
        e.kb_preview_dismiss();
        assert_eq!(
            e.kb_view().unwrap().kb_preview_suppressed_at,
            Some((row, col))
        );

        // A deliberate manual invocation at the exact same position always
        // wins over suppression.
        assert!(e.kb_preview_show_at_cursor(true));
        assert!(e.kb_preview_popup().is_some());
        assert_eq!(e.kb_view().unwrap().kb_preview_suppressed_at, None);
    }

    #[test]
    fn kb_preview_show_by_id_clears_suppression() {
        let (mut e, target) = editor_with_link();
        e.kb_preview_on_hover = true;
        let (row, col) = place_cursor_on_link(&mut e, &target);
        assert!(e.on_idle_tick(10_000));
        e.kb_preview_dismiss();
        assert_eq!(
            e.kb_view().unwrap().kb_preview_suppressed_at,
            Some((row, col))
        );

        e.kb_preview_show(&target);
        assert!(e.kb_preview_popup().is_some());
        assert_eq!(e.kb_view().unwrap().kb_preview_suppressed_at, None);
    }

    #[test]
    fn kb_preview_dismiss_with_no_popup_shown_is_a_true_no_op() {
        let (mut e, _target) = editor_with_link();
        assert!(e.kb_preview_popup().is_none());
        e.kb_preview_dismiss();
        assert_eq!(e.kb_view().unwrap().kb_preview_suppressed_at, None);
    }
}
