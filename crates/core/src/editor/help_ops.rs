//! Help-buffer operations — commands that manipulate the *Help* buffer
//! and its underlying KB navigation state.
//!
//! The dispatch layer calls these as part of `dispatch_builtin`; the AI
//! agent calls the KB directly via its `kb_*` tools (no need for these
//! view-layer helpers).

use crate::buffer::BufferKind;
use crate::help_view::HelpLinkSpan;

use super::Editor;

fn node_kind_label(kind: mae_kb::NodeKind) -> &'static str {
    match kind {
        mae_kb::NodeKind::Index => "index",
        mae_kb::NodeKind::Command => "command",
        mae_kb::NodeKind::Concept => "concept",
        mae_kb::NodeKind::Key => "key",
        mae_kb::NodeKind::Note => "note",
    }
}

/// Render a KB node into plain text and extract link byte ranges.
/// Returns `(rendered_text, link_spans)`.
fn render_help_node(kb: &mae_kb::KnowledgeBase, node_id: &str) -> (String, Vec<HelpLinkSpan>) {
    let mut out = String::new();
    let mut links: Vec<HelpLinkSpan> = Vec::new();

    let Some(node) = kb.get(node_id) else {
        out.push_str(&format!("(no such KB node: {})\n", node_id));
        return (out, links);
    };

    // Header
    out.push_str(&node.title);
    out.push('\n');
    out.push_str(&format!("{} · {}\n", node_kind_label(node.kind), node.id));
    if !node.tags.is_empty() {
        out.push_str(&format!("tags: {}\n", node.tags.join(", ")));
    }
    out.push('\n');

    // Body — parse [[target|display]] link markers
    for body_line in node.body.lines() {
        render_body_line(body_line, &mut out, &mut links);
        out.push('\n');
    }

    // Neighborhood
    let outgoing = kb.links_from(node_id);
    let incoming = kb.links_to(node_id);

    if !outgoing.is_empty() || !incoming.is_empty() {
        out.push('\n');
        out.push_str("── Neighborhood ──\n");
    }
    if !outgoing.is_empty() {
        out.push_str("Outgoing:\n");
        for target in &outgoing {
            let (title_text, _missing) = match kb.get(target) {
                Some(n) => (n.title.clone(), false),
                None => ("(missing)".to_string(), true),
            };
            out.push_str("  → ");
            let link_start = out.len();
            out.push_str(target);
            let link_end = out.len();
            links.push(HelpLinkSpan {
                byte_start: link_start,
                byte_end: link_end,
                target: target.clone(),
            });
            out.push_str(&format!("  {}\n", title_text));
        }
    }
    if !incoming.is_empty() {
        out.push_str(&format!("Backlinks ({}):\n", incoming.len()));
        for src in &incoming {
            let (title_text, _missing) = match kb.get(src) {
                Some(n) => (n.title.clone(), false),
                None => ("(missing)".to_string(), true),
            };
            out.push_str("  ← ");
            let link_start = out.len();
            out.push_str(src);
            let link_end = out.len();
            links.push(HelpLinkSpan {
                byte_start: link_start,
                byte_end: link_end,
                target: src.clone(),
            });
            out.push_str(&format!("  {}\n", title_text));
        }
    }

    out.push('\n');
    out.push_str("Tab/S-Tab: focus link · Enter: follow · C-o/C-i: back/forward · q: close\n");

    (out, links)
}

/// Render a single body line, stripping `[[target|display]]` markers and
/// recording link spans.
fn render_body_line(line: &str, out: &mut String, links: &mut Vec<HelpLinkSpan>) {
    let bytes = line.as_bytes();
    let mut cursor = 0usize;
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(end_rel) = line[i + 2..].find("]]") {
                let inner = &line[i + 2..i + 2 + end_rel];
                let (target, display) = match inner.find('|') {
                    Some(bar) => (inner[..bar].trim(), inner[bar + 1..].trim()),
                    None => {
                        let t = inner.trim();
                        (t, t)
                    }
                };
                if !target.is_empty() {
                    // Emit text before the link
                    out.push_str(&line[cursor..i]);
                    // Emit link display text
                    let link_start = out.len();
                    out.push_str(display);
                    let link_end = out.len();
                    links.push(HelpLinkSpan {
                        byte_start: link_start,
                        byte_end: link_end,
                        target: target.to_string(),
                    });
                    cursor = i + 2 + end_rel + 2;
                    i = cursor;
                    continue;
                }
            }
        }
        i += 1;
    }
    // Remainder
    out.push_str(&line[cursor..]);
}

impl Editor {
    /// Open the *Help* buffer on the given KB node, creating it if it
    /// doesn't exist. Falls back to the `index` node if the requested id
    /// isn't found.
    pub fn open_help_at(&mut self, node_id: &str) {
        let target = if self.kb.contains(node_id) {
            node_id.to_string()
        } else {
            self.set_status(format!("No help node: {}  — showing index", node_id));
            "index".to_string()
        };
        let prev_idx = self.active_buffer_idx();
        let idx = self.ensure_help_buffer_idx(&target);
        if idx != prev_idx {
            self.alternate_buffer_idx = Some(prev_idx);
        }
        self.help_populate_buffer(idx);
        self.window_mgr.focused_window_mut().buffer_idx = idx;
        self.window_mgr.focused_window_mut().cursor_row = 0;
        self.window_mgr.focused_window_mut().cursor_col = 0;
    }

    /// Render the current KB node into the help buffer's rope and store
    /// link spans. Called on every navigation (open, follow link, back/forward).
    pub fn help_populate_buffer(&mut self, buf_idx: usize) {
        let node_id = match self.buffers[buf_idx].help_view.as_ref() {
            Some(v) => v.current.clone(),
            None => return,
        };
        let (text, link_spans) = render_help_node(&self.kb, &node_id);
        // Temporarily allow writing to the read-only buffer.
        self.buffers[buf_idx].read_only = false;
        self.buffers[buf_idx].replace_contents(&text);
        self.buffers[buf_idx].read_only = true;
        if let Some(view) = self.buffers[buf_idx].help_view.as_mut() {
            view.rendered_links = link_spans;
        }
    }

    /// Navigable link targets from the rendered help buffer, in document
    /// order. Backed by `HelpView.rendered_links` (populated by
    /// `help_populate_buffer`). This replaces the old KB-neighbor lookup.
    pub fn help_navigable_links(&self) -> Vec<String> {
        match self.help_view() {
            Some(view) => view
                .rendered_links
                .iter()
                .map(|l| l.target.clone())
                .collect(),
            None => Vec::new(),
        }
    }

    /// Follow the currently-focused link in the *Help* buffer.
    pub fn help_follow_link(&mut self) {
        let (target, buf_idx) = {
            let Some(view) = self.help_view() else {
                self.set_status("Not in a help buffer");
                return;
            };
            let Some(idx) = view.focused_link else {
                self.set_status("No link focused (Tab to move between links)");
                return;
            };
            let Some(link) = view.rendered_links.get(idx) else {
                return;
            };
            let target = link.target.clone();
            if !self.kb.contains(&target) {
                self.set_status(format!("Link target '{}' not found", target));
                return;
            }
            let buf_idx = self
                .buffers
                .iter()
                .position(|b| b.kind == BufferKind::Help)
                .unwrap();
            (target, buf_idx)
        };
        if let Some(view) = self.help_view_mut() {
            view.navigate_to(target);
        }
        self.help_populate_buffer(buf_idx);
        self.window_mgr.focused_window_mut().cursor_row = 0;
        self.window_mgr.focused_window_mut().cursor_col = 0;
    }

    pub fn help_back(&mut self) {
        let went_back = if let Some(view) = self.help_view_mut() {
            view.go_back()
        } else {
            false
        };
        if went_back {
            if let Some(buf_idx) = self.buffers.iter().position(|b| b.kind == BufferKind::Help) {
                self.help_populate_buffer(buf_idx);
                self.window_mgr.focused_window_mut().cursor_row = 0;
                self.window_mgr.focused_window_mut().cursor_col = 0;
            }
        } else {
            self.set_status("No previous help page");
        }
    }

    pub fn help_forward(&mut self) {
        let went_fwd = if let Some(view) = self.help_view_mut() {
            view.go_forward()
        } else {
            false
        };
        if went_fwd {
            if let Some(buf_idx) = self.buffers.iter().position(|b| b.kind == BufferKind::Help) {
                self.help_populate_buffer(buf_idx);
                self.window_mgr.focused_window_mut().cursor_row = 0;
                self.window_mgr.focused_window_mut().cursor_col = 0;
            }
        } else {
            self.set_status("No forward help page");
        }
    }

    pub fn help_next_link(&mut self) {
        let cursor_byte = self.help_cursor_byte_offset();
        if let Some(view) = self.help_view_mut() {
            view.focus_next_link(cursor_byte);
        }
        self.help_move_cursor_to_focused_link();
    }

    pub fn help_prev_link(&mut self) {
        let cursor_byte = self.help_cursor_byte_offset();
        if let Some(view) = self.help_view_mut() {
            view.focus_prev_link(cursor_byte);
        }
        self.help_move_cursor_to_focused_link();
    }

    /// Move the cursor to the start of the currently focused link so the
    /// viewport scrolls to show it and the user sees where they landed.
    fn help_move_cursor_to_focused_link(&mut self) {
        let byte_start = match self.help_view() {
            Some(view) => match view.focused_link {
                Some(idx) => match view.rendered_links.get(idx) {
                    Some(link) => link.byte_start,
                    None => return,
                },
                None => return,
            },
            None => return,
        };
        let idx = self.active_buffer_idx();
        let rope = self.buffers[idx].rope().clone();
        let row = rope.byte_to_line(byte_start);
        let line_byte_start = rope.line_to_byte(row);
        let col = byte_start - line_byte_start;
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = row;
        win.cursor_col = col;
    }

    /// Compute the byte offset in the rope corresponding to the cursor position.
    fn help_cursor_byte_offset(&self) -> usize {
        let idx = self.active_buffer_idx();
        let buf = &self.buffers[idx];
        let win = self.window_mgr.focused_window();
        let rope = buf.rope();
        let row = win.cursor_row.min(rope.len_lines().saturating_sub(1));
        let line_start = rope.line_to_byte(row);
        let line_len = rope.line(row).len_bytes();
        let col_bytes = win.cursor_col.min(line_len);
        line_start + col_bytes
    }

    /// Close the *Help* buffer if one exists, switching to the alternate
    /// buffer (or scratch). Saves the view state for `help-reopen`.
    pub fn help_close(&mut self) {
        let help_idx = self.buffers.iter().position(|b| b.kind == BufferKind::Help);
        let Some(help_idx) = help_idx else {
            return;
        };
        // Save state for reopen.
        self.last_help_state = self.buffers[help_idx].help_view.clone();
        // Pick a sensible destination: alternate if set (and not the
        // help buffer itself), otherwise the first non-help buffer.
        let dest_idx = self
            .alternate_buffer_idx
            .filter(|&i| i != help_idx && i < self.buffers.len())
            .or_else(|| self.buffers.iter().position(|b| b.kind != BufferKind::Help))
            .unwrap_or(0);
        // Retarget any window focused on help before we remove it.
        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx == help_idx {
                win.buffer_idx = dest_idx;
                win.cursor_row = 0;
                win.cursor_col = 0;
            }
        }
        self.buffers.remove(help_idx);
        self.syntax.shift_after_remove(help_idx);
        // Fix indices that were above the removed buffer.
        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx > help_idx {
                win.buffer_idx -= 1;
            }
        }
        if let Some(idx) = self.alternate_buffer_idx {
            self.alternate_buffer_idx = match idx {
                i if i == help_idx => None,
                i if i > help_idx => Some(i - 1),
                i => Some(i),
            };
        }
    }

    /// Reopen the last-closed help buffer at exactly the node and
    /// navigation state where the user left off.
    pub fn help_reopen(&mut self) {
        let Some(saved) = self.last_help_state.take() else {
            self.set_status("No previous help session");
            return;
        };
        let node_id = saved.current.clone();
        let prev_idx = self.active_buffer_idx();
        let idx = self.ensure_help_buffer_idx(&node_id);
        // Restore full navigation state (back/forward stacks, focused link).
        self.buffers[idx].help_view = Some(saved);
        self.help_populate_buffer(idx);
        if idx != prev_idx {
            self.alternate_buffer_idx = Some(prev_idx);
        }
        self.window_mgr.focused_window_mut().buffer_idx = idx;
        self.window_mgr.focused_window_mut().cursor_row = 0;
        self.window_mgr.focused_window_mut().cursor_col = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_help_at_creates_buffer() {
        let mut e = Editor::new();
        e.open_help_at("index");
        assert_eq!(e.active_buffer().kind, BufferKind::Help);
        assert_eq!(e.help_view().unwrap().current, "index");
    }

    #[test]
    fn open_help_at_missing_falls_back_to_index() {
        let mut e = Editor::new();
        e.open_help_at("nonexistent:thing");
        assert_eq!(e.help_view().unwrap().current, "index");
        assert!(e.status_msg.contains("No help node"));
    }

    #[test]
    fn open_help_reuses_existing_buffer() {
        let mut e = Editor::new();
        e.open_help_at("index");
        e.open_help_at("concept:buffer");
        let helps = e
            .buffers
            .iter()
            .filter(|b| b.kind == BufferKind::Help)
            .count();
        assert_eq!(helps, 1);
        assert_eq!(e.help_view().unwrap().current, "concept:buffer");
        // back_stack should show the previous node.
        assert_eq!(e.help_view().unwrap().back_stack, vec!["index"]);
    }

    #[test]
    fn help_follow_link_navigates() {
        let mut e = Editor::new();
        e.open_help_at("index");
        e.help_next_link(); // focus first link
        let focused_target = {
            let links = e.help_navigable_links();
            let v = e.help_view().unwrap();
            links[v.focused_link.unwrap()].clone()
        };
        e.help_follow_link();
        assert_eq!(e.help_view().unwrap().current, focused_target);
    }

    #[test]
    fn help_back_and_forward() {
        let mut e = Editor::new();
        e.open_help_at("index");
        e.open_help_at("concept:buffer");
        e.help_back();
        assert_eq!(e.help_view().unwrap().current, "index");
        e.help_forward();
        assert_eq!(e.help_view().unwrap().current, "concept:buffer");
    }

    #[test]
    fn help_close_removes_buffer() {
        let mut e = Editor::new();
        e.open_help_at("index");
        assert_eq!(e.buffers.len(), 2);
        e.help_close();
        assert!(e.buffers.iter().all(|b| b.kind != BufferKind::Help));
        assert_eq!(e.active_buffer_idx(), 0);
    }

    #[test]
    fn help_next_prev_link_wraps() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let count = e.help_navigable_links().len();
        assert!(count > 0);
        e.help_next_link();
        assert_eq!(e.help_view().unwrap().focused_link, Some(0));
        e.help_prev_link();
        assert_eq!(e.help_view().unwrap().focused_link, Some(count - 1));
    }

    #[test]
    fn help_navigable_links_includes_backlinks() {
        let e = {
            let mut e = Editor::new();
            e.open_help_at("index");
            e
        };
        let outgoing = e.kb.links_from("index");
        let incoming = e.kb.links_to("index");
        assert!(!outgoing.is_empty(), "index must have outgoing links");
        assert!(!incoming.is_empty(), "index must have incoming links");

        let nav = e.help_navigable_links();
        // Every outgoing neighbor appears somewhere in nav links.
        for target in &outgoing {
            assert!(
                nav.contains(target),
                "missing outgoing link {} in nav list",
                target
            );
        }
        // Every backlink is reachable through the combined list.
        for src in &incoming {
            assert!(nav.contains(src), "missing backlink {} in nav list", src);
        }
    }

    #[test]
    fn help_follow_link_works_for_backlink_focus() {
        let mut e = Editor::new();
        e.open_help_at("concept:buffer");
        let nav = e.help_navigable_links();
        if nav.len() > 1 {
            let last_idx = nav.len() - 1;
            if let Some(view) = e.help_view_mut() {
                view.focused_link = Some(last_idx);
            }
            let expected = nav[last_idx].clone();
            e.help_follow_link();
            assert_eq!(e.help_view().unwrap().current, expected);
        }
    }

    // --- WU5: rope-backed help buffer tests ---

    #[test]
    fn help_buffer_is_read_only() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let idx = e.active_buffer_idx();
        assert!(e.buffers[idx].read_only);
        let before = e.buffers[idx].rope().len_chars();
        let mut win = crate::window::Window::new(999, idx);
        e.buffers[idx].insert_char(&mut win, 'x');
        assert_eq!(
            e.buffers[idx].rope().len_chars(),
            before,
            "read-only buffer should reject insert_char"
        );
    }

    #[test]
    fn help_buffer_has_rope_content() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let idx = e.active_buffer_idx();
        let text: String = e.buffers[idx].rope().chars().collect();
        assert!(text.contains("index"), "rope should contain the node id");
        assert!(
            text.len() > 50,
            "rendered help should have substantial content"
        );
    }

    #[test]
    fn help_buffer_link_spans_have_valid_byte_ranges() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let view = e.help_view().unwrap();
        let text: String = e.buffers[e.active_buffer_idx()].rope().chars().collect();
        assert!(!view.rendered_links.is_empty(), "index should have links");
        for link in &view.rendered_links {
            assert!(link.byte_start < link.byte_end);
            assert!(link.byte_end <= text.len());
            let display = &text[link.byte_start..link.byte_end];
            assert!(!display.is_empty(), "link display text should not be empty");
        }
    }

    #[test]
    fn render_body_line_strips_brackets() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_body_line("see [[concept:buffer]] for details", &mut out, &mut links);
        assert_eq!(out, "see concept:buffer for details");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "concept:buffer");
        assert_eq!(
            &out[links[0].byte_start..links[0].byte_end],
            "concept:buffer"
        );
    }

    #[test]
    fn render_body_line_display_override() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_body_line("goto [[concept:buffer|the buffer]]", &mut out, &mut links);
        assert_eq!(out, "goto the buffer");
        assert_eq!(links[0].target, "concept:buffer");
        assert_eq!(&out[links[0].byte_start..links[0].byte_end], "the buffer");
    }

    #[test]
    fn render_body_line_empty_target_is_plain() {
        let mut out = String::new();
        let mut links = Vec::new();
        render_body_line("[[]] stays", &mut out, &mut links);
        assert_eq!(out, "[[]] stays");
        assert!(links.is_empty());
    }

    #[test]
    fn help_populate_after_navigation() {
        let mut e = Editor::new();
        e.open_help_at("index");
        let text_before: String = e.buffers[e.active_buffer_idx()].rope().chars().collect();
        e.open_help_at("concept:buffer");
        let text_after: String = e.buffers[e.active_buffer_idx()].rope().chars().collect();
        assert_ne!(
            text_before, text_after,
            "rope should change after navigating to different node"
        );
        assert!(text_after.contains("concept:buffer"));
    }

    // --- WU6: reopen last help buffer ---

    #[test]
    fn help_close_saves_state_for_reopen() {
        let mut e = Editor::new();
        e.open_help_at("index");
        e.open_help_at("concept:buffer");
        e.help_close();
        assert!(e.last_help_state.is_some());
        assert_eq!(
            e.last_help_state.as_ref().unwrap().current,
            "concept:buffer"
        );
        assert_eq!(
            e.last_help_state.as_ref().unwrap().back_stack,
            vec!["index"]
        );
    }

    #[test]
    fn help_reopen_restores_state() {
        let mut e = Editor::new();
        e.open_help_at("index");
        e.open_help_at("concept:buffer");
        e.help_close();
        e.help_reopen();
        assert_eq!(e.help_view().unwrap().current, "concept:buffer");
        assert_eq!(e.help_view().unwrap().back_stack, vec!["index"]);
        assert_eq!(e.active_buffer().kind, BufferKind::Help);
    }

    #[test]
    fn help_reopen_no_previous() {
        let mut e = Editor::new();
        e.help_reopen();
        assert!(e.status_msg.contains("No previous help session"));
    }
}
