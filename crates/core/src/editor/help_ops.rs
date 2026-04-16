//! Help-buffer operations — commands that manipulate the *Help* buffer
//! and its underlying KB navigation state.
//!
//! The dispatch layer calls these as part of `dispatch_builtin`; the AI
//! agent calls the KB directly via its `kb_*` tools (no need for these
//! view-layer helpers).

use crate::buffer::BufferKind;

use super::Editor;

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
        self.window_mgr.focused_window_mut().buffer_idx = idx;
        self.window_mgr.focused_window_mut().cursor_row = 0;
        self.window_mgr.focused_window_mut().cursor_col = 0;
    }

    /// Combined navigable link list for the current help node:
    /// `outgoing ++ incoming`. This is the single source of truth for
    /// Tab-focus ordering shared by `help_next_link`, `help_prev_link`,
    /// `help_follow_link`, and the renderer's focus highlight. Thin
    /// wrapper over `KnowledgeBase::neighbors` — the dedup contract is
    /// owned by the KB so the AI's `kb_graph` tool sees the same shape.
    pub fn help_navigable_links(&self) -> Vec<String> {
        match self.help_view() {
            Some(view) => self.kb.neighbors(&view.current),
            None => Vec::new(),
        }
    }

    /// Follow the currently-focused link in the *Help* buffer.
    pub fn help_follow_link(&mut self) {
        let Some(view) = self.help_view() else {
            self.set_status("Not in a help buffer");
            return;
        };
        let Some(idx) = view.focused_link else {
            self.set_status("No link focused (Tab to move between links)");
            return;
        };
        let links = self.help_navigable_links();
        let Some(target) = links.get(idx).cloned() else {
            return;
        };
        if !self.kb.contains(&target) {
            self.set_status(format!("Link target '{}' not found", target));
            return;
        }
        if let Some(view) = self.help_view_mut() {
            view.navigate_to(target);
        }
    }

    pub fn help_back(&mut self) {
        if let Some(view) = self.help_view_mut() {
            if !view.go_back() {
                self.set_status("No previous help page");
            }
        }
    }

    pub fn help_forward(&mut self) {
        if let Some(view) = self.help_view_mut() {
            if !view.go_forward() {
                self.set_status("No forward help page");
            }
        }
    }

    pub fn help_next_link(&mut self) {
        let link_count = self.help_navigable_links().len();
        if let Some(view) = self.help_view_mut() {
            view.focus_next_link(link_count);
        }
    }

    pub fn help_prev_link(&mut self) {
        let link_count = self.help_navigable_links().len();
        if let Some(view) = self.help_view_mut() {
            view.focus_prev_link(link_count);
        }
    }

    /// Close the *Help* buffer if one exists, switching to the alternate
    /// buffer (or scratch).
    pub fn help_close(&mut self) {
        let help_idx = self.buffers.iter().position(|b| b.kind == BufferKind::Help);
        let Some(help_idx) = help_idx else {
            return;
        };
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
        // `index` has many incoming links (every cmd:* node's "See also"
        // template cites it). Outgoing links should come first, followed
        // by any backlinks not already in the outgoing set.
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
        // First N entries match outgoing order.
        assert_eq!(&nav[..outgoing.len()], outgoing.as_slice());
        // Every entry is unique.
        let unique: std::collections::HashSet<&String> = nav.iter().collect();
        assert_eq!(unique.len(), nav.len(), "navigable links must be unique");
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
        let outgoing_count = e.kb.links_from("concept:buffer").len();
        // Only meaningful if there's at least one backlink past the outgoing set.
        if nav.len() > outgoing_count {
            let backlink_idx = outgoing_count; // first backlink
            if let Some(view) = e.help_view_mut() {
                view.focused_link = Some(backlink_idx);
            }
            let expected = nav[backlink_idx].clone();
            e.help_follow_link();
            assert_eq!(e.help_view().unwrap().current, expected);
        }
    }
}
