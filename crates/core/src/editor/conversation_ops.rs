//! Conversation buffer and KB-view helpers extracted from `Editor`'s main
//! impl block: locating/creating the `*AI*` conversation buffer, syncing its
//! rope, and navigating KB-view buffers. Split out of `mod.rs` (ADR none
//! needed) — pure code motion, same pattern as `kb_ops.rs`.

use crate::buffer::Buffer;

use super::Editor;

impl Editor {
    /// First conversation attached to any buffer, if any.
    pub fn conversation(&self) -> Option<&crate::conversation::Conversation> {
        self.buffers.iter().find_map(|b| b.conversation())
    }

    /// Mutable view of the first conversation attached to any buffer.
    pub fn conversation_mut(&mut self) -> Option<&mut crate::conversation::Conversation> {
        self.buffers.iter_mut().find_map(|b| b.conversation_mut())
    }

    /// Sync the rope of the first conversation buffer.
    /// Only escalates to `Full` redraw when the rope content actually changed,
    /// avoiding unnecessary syntax recomputation on no-op AI events.
    pub fn sync_conversation_buffer_rope(&mut self) {
        if let Some(buf) = self
            .buffers
            .iter_mut()
            .find(|b| b.kind == crate::buffer::BufferKind::Conversation)
        {
            if buf.sync_conversation_rope() {
                self.mark_full_redraw();
            }
        }
    }

    /// Index of the conversation buffer, creating `*AI*` if none exists.
    /// Used by both interactive open and programmatic load to keep the
    /// "find or create by kind" logic in one place.
    pub(crate) fn ensure_conversation_buffer_idx(&mut self) -> usize {
        if let Some(i) = self
            .buffers
            .iter()
            .position(|b| b.kind == crate::buffer::BufferKind::Conversation)
        {
            return i;
        }
        self.buffers.push(Buffer::new_conversation("*AI*"));
        self.buffers.len() - 1
    }

    /// Find or create the appropriate KB buffer and navigate it to `node_id`.
    /// Returns the buffer index. Does NOT switch focus — callers decide.
    ///
    /// Builtin (`cmd:`/`concept:`/...) nodes keep the single shared `*Help*`
    /// buffer, reused/mutated in place — a deliberate Emacs-style shared docs
    /// browser: browsing built-in help across windows is meant to feel like
    /// one help window.
    ///
    /// Non-builtin nodes (your own KB notes) instead get one buffer PER node
    /// id, matching how `:find-file` already treats real files: reuse a `*KB*`
    /// buffer only if it's already showing this exact node, otherwise create a
    /// new, distinct one. Before this, ALL non-builtin nodes shared one `*KB*`
    /// buffer keyed by name alone — navigating to a new node in one window
    /// silently mutated whatever `*KB*` buffer any OTHER window happened to be
    /// showing too, since there was only ever one such buffer in existence.
    pub fn ensure_kb_buffer_idx(&mut self, node_id: &str) -> usize {
        use crate::buffer::buffer_names;
        use crate::editor::help_ops::is_builtin_node;

        if is_builtin_node(node_id) {
            if let Some(idx) = self.buffers.iter().position(|b| {
                b.kind == crate::buffer::BufferKind::Kb && b.name == buffer_names::HELP
            }) {
                if let Some(view) = self.buffers[idx].kb_view_mut() {
                    view.navigate_to(node_id.to_string());
                }
                return idx;
            }
            let mut buf = Buffer::new_kb(node_id);
            buf.name = buffer_names::HELP.to_string();
            self.buffers.push(buf);
            return self.buffers.len() - 1;
        }

        // Reuse a *KB: <title>* buffer only if it's already showing this
        // exact node — same semantics as opening an already-open file twice.
        // Match by node id (not by name), since the name is now derived
        // from the node's title and isn't itself a stable identity.
        if let Some(idx) = self.buffers.iter().position(|b| {
            b.kind == crate::buffer::BufferKind::Kb
                && b.kb_view().is_some_and(|v| v.current == node_id)
        }) {
            return idx;
        }
        let mut buf = Buffer::new_kb(node_id);
        buf.name = self.kb_buffer_display_name(node_id);
        self.buffers.push(buf);
        self.buffers.len() - 1
    }

    /// Always create a brand-new, distinctly-named KB buffer for `node_id`,
    /// bypassing every reuse check `ensure_kb_buffer_idx` applies —
    /// including the shared `*Help*` buffer builtin nodes normally reuse.
    ///
    /// Used by the graph view's companion-window navigation when the
    /// buffer `ensure_kb_buffer_idx` would hand back is ALSO currently
    /// visible in some window OTHER than the one being navigated:
    /// repopulating it in place would silently change that unrelated
    /// window's content too, since both windows render the same buffer
    /// object — reported live ("if i have multiple kb buffers open... the
    /// kb node clicked has its content displayed on both buffers"). This is
    /// the builtin-node counterpart to the fix `ensure_kb_buffer_idx`'s own
    /// doc comment already describes for non-builtin nodes; builtin nodes
    /// still default to sharing `*Help*` in the common single-window case
    /// (avoiding ~1000-node buffer clutter from ordinary help browsing) —
    /// this is only reached when that sharing would actually leak.
    /// Named via `kb_buffer_display_name` (not the shared `*Help*` name),
    /// so a later, unrelated `ensure_kb_buffer_idx` call never mistakes
    /// this one-off buffer for the reusable shared one.
    pub(crate) fn fresh_kb_buffer_idx(&mut self, node_id: &str) -> usize {
        let mut buf = Buffer::new_kb(node_id);
        buf.name = self.kb_buffer_display_name(node_id);
        self.buffers.push(buf);
        self.buffers.len() - 1
    }

    /// A distinct, title-based buffer name for a KB node buffer — e.g.
    /// `*KB: ADR-0003*` — so multiple simultaneously-open KB buffers (one
    /// per node visited via the graph view's companion-window navigation,
    /// or plain multi-window KB browsing) are distinguishable in the buffer
    /// switcher (`SPC b b`), instead of every one showing the same generic
    /// `*KB*` name. Falls back to the raw node id if the title can't be
    /// looked up (e.g. a not-yet-synced federated node).
    fn kb_buffer_display_name(&self, node_id: &str) -> String {
        let title = self
            .kb_for_node(node_id)
            .and_then(|kb| kb.get(node_id))
            .map(|n| n.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| node_id.to_string());
        format!("*KB: {title}*")
    }

    /// Mutable view onto the ACTIVE (focused window's) buffer's KbView, if
    /// it's showing a KB buffer. Scoped to the active buffer rather than "the
    /// first Kb-kind buffer in the editor" so link-follow/history/TOC
    /// operations act on whatever KB content you're actually looking at, not
    /// an arbitrary one — load-bearing now that non-builtin nodes each get
    /// their own buffer (see `ensure_kb_buffer_idx`) rather than sharing one.
    pub fn kb_view_mut(&mut self) -> Option<&mut crate::kb_view::KbView> {
        let idx = self.active_buffer_idx();
        self.buffers.get_mut(idx).and_then(|b| b.kb_view_mut())
    }

    /// Immutable counterpart of [`Self::kb_view_mut`].
    pub fn kb_view(&self) -> Option<&crate::kb_view::KbView> {
        let idx = self.active_buffer_idx();
        self.buffers.get(idx).and_then(|b| b.kb_view())
    }

    /// Reset the AI session: request cancellation, clear state, and end streaming.
    pub fn reset_ai_session(&mut self) {
        self.ai.cancel_requested = true;
        self.ai.streaming = false;
        self.ai.current_round = 0;
        self.ai.transaction_start_idx = None;
        if let Some(conv) = self.conversation_mut() {
            conv.end_streaming();
            conv.push_system("[AI Session Reset]");
        }
        self.ai.input_lock = crate::InputLock::None;
    }
}
