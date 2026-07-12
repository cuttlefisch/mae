//! Editor state save/restore (push/pop state stack) for temporary
//! operations like self-test, extracted from `Editor`'s main impl block.
//! Split out of `mod.rs` (ADR none needed) — pure code motion, same pattern
//! as `kb_ops.rs`.

use super::{ConversationPair, Editor, Mode};

/// Snapshot of editor state for save/restore (push/pop state stack).
/// Captures the buffer list, window layout, focus, and mode so tools
/// can restore the editor to a known state after temporary operations.
#[derive(Clone)]
pub struct EditorStateSnapshot {
    /// Buffer names that were open (ordered).
    pub buffer_names: Vec<String>,
    /// The focused window's buffer name.
    pub focused_buffer: String,
    /// Cloned window manager state: all windows + layout tree + focus.
    pub windows: std::collections::HashMap<crate::window::WindowId, crate::window::Window>,
    pub layout: crate::window::LayoutNode,
    pub focused_id: crate::window::WindowId,
    pub next_window_id: crate::window::WindowId,
    /// Editor mode at snapshot time.
    pub mode: Mode,
    /// Conversation pair (AI split layout) at snapshot time.
    pub conversation_pair: Option<ConversationPair>,
}

impl Editor {
    /// Save current editor state (buffer list, window layout, focus, mode)
    /// onto the state stack. Returns the stack depth after push.
    pub fn save_state(&mut self) -> usize {
        let buffer_names: Vec<String> = self.buffers.iter().map(|b| b.name.clone()).collect();
        let focused_buffer = self.active_buffer().name.clone();
        let (windows, layout, focused_id, next_id) = self.window_mgr.snapshot();
        self.state_stack.push(EditorStateSnapshot {
            buffer_names,
            focused_buffer,
            windows,
            layout,
            focused_id,
            next_window_id: next_id,
            mode: self.mode,
            conversation_pair: self.ai.conversation_pair.clone(),
        });
        self.state_stack.len()
    }

    /// Restore editor state from the state stack. Closes buffers that weren't
    /// in the snapshot, restores window layout and focus. Returns a summary
    /// of what was restored, or an error if the stack is empty.
    pub fn restore_state(&mut self) -> Result<String, String> {
        let snapshot = self
            .state_stack
            .pop()
            .ok_or_else(|| "State stack is empty — nothing to restore".to_string())?;

        // 1. Close buffers that weren't in the snapshot (reverse order to keep indices stable)
        let mut closed = Vec::new();
        let mut i = self.buffers.len();
        while i > 0 {
            i -= 1;
            if !snapshot.buffer_names.contains(&self.buffers[i].name) {
                closed.push(self.buffers[i].name.clone());
                self.buffers.remove(i);
                self.notify_buffer_removed(i);
            }
        }

        // 2. Remap window buffer_idx values: snapshot had indices into the old buffer list,
        //    but buffers may have shifted. Remap by name.
        let mut restored_windows = snapshot.windows;
        for win in restored_windows.values_mut() {
            // Find the buffer name this window was pointing to
            let old_name = snapshot
                .buffer_names
                .get(win.buffer_idx)
                .cloned()
                .unwrap_or_default();
            // Find new index for that buffer
            if let Some(new_idx) = self.buffers.iter().position(|b| b.name == old_name) {
                win.buffer_idx = new_idx;
            } else {
                // Buffer no longer exists — point to buffer 0
                win.buffer_idx = 0;
            }
        }

        // 3. Restore window manager
        self.window_mgr.restore(
            restored_windows,
            snapshot.layout,
            snapshot.focused_id,
            snapshot.next_window_id,
        );

        // 4. Restore mode
        self.mode = snapshot.mode;

        // 5. Restore conversation pair with remapped buffer indices.
        if let Some(mut pair) = snapshot.conversation_pair {
            let out_name = snapshot
                .buffer_names
                .get(pair.output_buffer_idx)
                .cloned()
                .unwrap_or_default();
            let in_name = snapshot
                .buffer_names
                .get(pair.input_buffer_idx)
                .cloned()
                .unwrap_or_default();
            let out_ok = self.buffers.iter().position(|b| b.name == out_name);
            let in_ok = self.buffers.iter().position(|b| b.name == in_name);
            if let (Some(out_idx), Some(in_idx)) = (out_ok, in_ok) {
                pair.output_buffer_idx = out_idx;
                pair.input_buffer_idx = in_idx;
                self.ai.conversation_pair = Some(pair);
            } else {
                self.ai.conversation_pair = None;
            }
        } else {
            self.ai.conversation_pair = None;
        }

        // 6. Focus the originally focused buffer
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.name == snapshot.focused_buffer)
        {
            self.window_mgr.focused_window_mut().buffer_idx = idx;
        }

        let summary = if closed.is_empty() {
            "State restored (no buffers closed)".to_string()
        } else {
            format!(
                "State restored, closed {} buffer(s): {}",
                closed.len(),
                closed.join(", ")
            )
        };
        Ok(summary)
    }

    /// Clean up self-test state after cancellation or completion.
    /// Returns true if cleanup was performed.
    pub fn cleanup_self_test(&mut self) -> bool {
        if !self.self_test_active {
            return false;
        }
        self.self_test_active = false;
        if let Some(ref dir) = self.test_sandbox_dir.take() {
            if dir.exists() && dir.starts_with(std::env::temp_dir()) {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        let _ = self.restore_state();
        true
    }
}
