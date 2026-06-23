//! `*KB Sharing*` management buffer — open / refresh / build.
//!
//! Mirrors the `*Notifications*` buffer ops (`notify_ops.rs`): a magit-style
//! interactive buffer built from the KB-sharing snapshot (P0), with at-point
//! actions dispatched in `dispatch/kb_sharing.rs`. The buffer reflects the same
//! single-source-of-truth snapshot the MCP tool + Scheme primitive expose (#8).

use crate::kb_sharing::{build_view, CollapseKey, KbSharingView};

const KB_SHARING_BUFFER: &str = "*KB Sharing*";

impl super::Editor {
    /// Open (or replace) the `*KB Sharing*` management buffer and focus it.
    pub fn open_kb_sharing(&mut self) {
        let (view, text) = self.build_kb_sharing_view(&Default::default());
        let idx = if let Some(i) = self.find_buffer_by_name(KB_SHARING_BUFFER) {
            self.buffers[i] = crate::buffer::Buffer::new();
            self.buffers[i].name = KB_SHARING_BUFFER.to_string();
            self.buffers[i].kind = crate::buffer::BufferKind::KbSharing;
            i
        } else {
            let mut buf = crate::buffer::Buffer::new();
            buf.name = KB_SHARING_BUFFER.to_string();
            buf.kind = crate::buffer::BufferKind::KbSharing;
            self.buffers.push(buf);
            self.buffers.len() - 1
        };
        self.buffers[idx].view = crate::buffer_view::BufferView::KbSharing(Box::new(view));
        self.buffers[idx].insert_text_at(0, &text);
        self.buffers[idx].read_only = true;
        self.buffers[idx].modified = false;
        let prev = self.active_buffer_idx();
        self.vi.alternate_buffer_idx = Some(prev);
        self.display_buffer(idx);
        self.set_mode(crate::Mode::Normal);
    }

    /// Rebuild the `*KB Sharing*` buffer in place if it is open — called on collab
    /// events (share/join/leave + live `kbc:` membership broadcasts) so the view
    /// tracks remote promote/demote/approve without a manual refresh. Preserves
    /// fold state and clamps any showing window's cursor.
    pub fn refresh_kb_sharing_buffer(&mut self) {
        let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == crate::buffer::BufferKind::KbSharing)
        else {
            return;
        };
        let prev_collapsed = self.buffers[idx]
            .kb_sharing_view()
            .map(|v| v.collapsed.clone())
            .unwrap_or_default();
        let (view, text) = self.build_kb_sharing_view(&prev_collapsed);
        self.buffers[idx].read_only = false;
        let end = self.buffers[idx].rope().len_chars();
        self.buffers[idx].delete_range(0, end);
        self.buffers[idx].insert_text_at(0, &text);
        self.buffers[idx].read_only = true;
        self.buffers[idx].modified = false;
        self.buffers[idx].view = crate::buffer_view::BufferView::KbSharing(Box::new(view));
        let last = self.buffers[idx].display_line_count().saturating_sub(1);
        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx == idx && win.cursor_row > last {
                win.cursor_row = last;
            }
        }
        self.mark_full_redraw();
    }

    fn build_kb_sharing_view(
        &self,
        collapsed: &std::collections::HashMap<CollapseKey, bool>,
    ) -> (KbSharingView, String) {
        build_view(&self.kb_sharing_snapshot(), collapsed)
    }
}
