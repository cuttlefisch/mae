//! Scheme -> Editor state application: `apply_to_editor` drains
//! `SharedState`'s pending-mutation queues into the live `Editor` (called
//! after eval). This file holds the dispatcher plus the first half of the
//! per-concern methods (keymap/command registration through CRDT/sync);
//! `state_sync_apply2.rs` holds the rest (key removals through splash
//! arts).
//!
//! Split out of `runtime.rs` (CLAUDE.md architecture debt reduction pass)
//! -- pure code motion, no behavior change. `apply_to_editor` drains its
//! queues in a specific, DOCUMENTED order (e.g. keymaps must be created
//! before bindings are applied -- see the comment on
//! `apply_keymap_and_context`). The methods below are called from
//! `apply_to_editor` in EXACTLY the original order; do not reorder them
//! without re-verifying the ordering comments inline.

use mae_core::parse_key_seq_spaced;
use mae_core::Editor;

use tracing::{debug, info, warn};

use super::{SchemeRuntime, SharedState};

impl SchemeRuntime {
    pub fn apply_to_editor(&mut self, editor: &mut Editor) {
        let mut state = self.shared.lock();

        Self::apply_keymap_and_context(&mut state, editor);
        let binding_count = Self::apply_keymap_bindings(&mut state, editor);
        let cmd_count = Self::apply_command_defs(&mut state, editor);
        Self::apply_autoload_dynopts_unregisters(&mut state, editor);
        Self::apply_hooks(&mut state, editor);
        Self::apply_display_policy(&mut state, editor);
        Self::apply_kb_mutations(&mut state, editor);
        Self::apply_options_status_theme(&mut state, editor);
        Self::apply_live_editing(&mut state, editor);
        Self::apply_round2_buffer_editing(&mut state, editor);
        Self::apply_advice_and_undo(&mut state, editor);
        Self::apply_crdt_sync(&mut state, editor);
        Self::apply_key_removals_and_group_names(&mut state, editor);
        let (commands, ex_commands) = Self::take_pending_commands(&mut state);
        Self::apply_messages_shell_recent_agenda(&mut state, editor);
        Self::apply_visual_buffer_ops(&mut state, editor);
        Self::apply_local_options(&mut state, editor);
        Self::apply_ai_tools(&mut state, editor);
        Self::apply_splash_arts(&mut state, editor);

        // Drop the lock before dispatching commands (which may call
        // back into Scheme via user-defined commands).
        drop(state);

        for name in commands {
            editor.dispatch_builtin(&name);
        }

        for cmd in ex_commands {
            editor.execute_command(&cmd);
        }

        if binding_count > 0 || cmd_count > 0 {
            info!(
                keybindings = binding_count,
                commands = cmd_count,
                "scheme config applied to editor"
            );
        }

        // Note: We do NOT call inject_editor_state here — the caller
        // is responsible for calling it before eval if needed.

        // Update cached scheme stats for MCP introspection
        self.update_editor_scheme_stats(editor);
    }

    /// Create new Scheme-defined keymaps (must come before bindings so
    /// `define-key` can target them), then apply context routing (buffer
    /// kind / language -> context keymap).
    fn apply_keymap_and_context(state: &mut SharedState, editor: &mut Editor) {
        // Create new keymaps (must come before bindings so define-key can target them)
        for (name, parent) in state.keymap_defs.drain(..) {
            if !editor.keymaps.contains_key(&name) {
                debug!(keymap = %name, parent = %parent, "creating scheme keymap");
                editor
                    .keymaps
                    .insert(name.clone(), mae_core::Keymap::with_parent(&name, &parent));
            }
        }

        // Apply context routing (buffer kind / language -> context keymap).
        for (sel_type, sel_value, keymap) in state.context_bindings.drain(..) {
            if let Err(e) = editor
                .keymap_registry
                .apply_binding(&sel_type, &sel_value, &keymap)
            {
                warn!(
                    selector_type = %sel_type,
                    selector_value = %sel_value,
                    keymap = %keymap,
                    "ignoring bind-context-keymap: {e}"
                );
            }
        }
    }

    /// Apply keymap bindings queued via `define-key`. Returns the count for
    /// the summary log at the end of `apply_to_editor`.
    fn apply_keymap_bindings(state: &mut SharedState, editor: &mut Editor) -> usize {
        // Apply keymap bindings
        let binding_count = state.keymap_bindings.len();
        for (map_name, key_str, cmd_name) in state.keymap_bindings.drain(..) {
            if let Some(keymap) = editor.keymaps.get_mut(&map_name) {
                let seq = parse_key_seq_spaced(&key_str);
                if seq.is_empty() {
                    warn!(keymap = %map_name, key = %key_str, command = %cmd_name,
                          "scheme keybinding produced empty key sequence, skipping");
                } else {
                    let prev = keymap.bind(seq, &cmd_name);
                    if let Some(ref prev_cmd) = prev {
                        if prev_cmd != &cmd_name {
                            warn!(keymap = %map_name, key = %key_str, command = %cmd_name,
                                   previous = %prev_cmd, "keybinding conflict: overwriting");
                            editor.message_log.push(
                                mae_core::MessageLevel::Warn,
                                "keybinding",
                                format!(
                                    "Key conflict in '{}': {} was '{}', now '{}' (module load order)",
                                    map_name, key_str, prev_cmd, cmd_name
                                ),
                            );
                        }
                    } else {
                        debug!(keymap = %map_name, key = %key_str, command = %cmd_name,
                               "applying scheme keybinding");
                    }
                }
            } else {
                warn!(keymap = %map_name, key = %key_str, command = %cmd_name, "scheme keybinding targets unknown keymap");
            }
        }

        binding_count
    }

    /// Register Scheme-defined commands. Returns the count for the summary
    /// log at the end of `apply_to_editor`.
    fn apply_command_defs(state: &mut SharedState, editor: &mut Editor) -> usize {
        // Register Scheme-defined commands
        let cmd_count = state.command_defs.len();
        for (name, doc, scheme_fn) in state.command_defs.drain(..) {
            debug!(command = %name, scheme_fn = %scheme_fn, "registering scheme command");
            let overwrote = editor.commands.register_scheme(&name, &doc, &scheme_fn);
            if overwrote {
                editor.message_log.push(
                    mae_core::MessageLevel::Warn,
                    "command",
                    format!(
                        "Module overrides builtin command '{}' with Scheme function '{}'",
                        name, scheme_fn
                    ),
                );
            }
        }

        cmd_count
    }

    /// Register autoload commands + dynamic options, and process command/
    /// option unregistration (module unload).
    fn apply_autoload_dynopts_unregisters(state: &mut SharedState, editor: &mut Editor) {
        // Register autoload commands
        for (cmd_name, feature, doc) in state.pending_autoloads.drain(..) {
            debug!(command = %cmd_name, feature = %feature, "registering autoload command");
            editor.commands.register_autoload(&cmd_name, &doc, &feature);
        }

        // Register dynamic options from (define-option!)
        for (name, kind_str, default, doc) in state.pending_dynamic_options.drain(..) {
            let kind = match kind_str.as_str() {
                "bool" | "boolean" => mae_core::options::OptionKind::Bool,
                "int" | "integer" => mae_core::options::OptionKind::Int,
                "string" => mae_core::options::OptionKind::String,
                other => {
                    warn!(name = %name, kind = %other, "define-option! unknown kind, defaulting to string");
                    mae_core::options::OptionKind::String
                }
            };
            editor
                .option_registry
                .register_dynamic(name.clone(), vec![], doc, kind, default, None);
            debug!(option = %name, "registered dynamic option from module");
        }

        // Unregister commands (for module unload)
        for name in state.pending_command_unregisters.drain(..) {
            if editor.commands.unregister(&name) {
                debug!(command = %name, "unregistered command");
            }
        }

        // Unregister options (for module unload)
        for name in state.pending_option_unregisters.drain(..) {
            if editor.option_registry.unregister(&name) {
                debug!(option = %name, "unregistered option");
            }
        }
    }

    /// Apply hook add/remove registrations.
    fn apply_hooks(state: &mut SharedState, editor: &mut Editor) {
        // Apply hook registrations
        for (hook, fn_name) in state.pending_hook_adds.drain(..) {
            editor.hooks.add(&hook, &fn_name);
            debug!(hook = %hook, fn_name = %fn_name, "hook registered");
        }
        for (hook, fn_name) in state.pending_hook_removes.drain(..) {
            if editor.hooks.remove(&hook, &fn_name) {
                debug!(hook = %hook, fn_name = %fn_name, "hook removed");
            }
        }
    }

    /// Apply display-rule overrides and replaceable-kind changes.
    fn apply_display_policy(state: &mut SharedState, editor: &mut Editor) {
        // Apply display-rule overrides from (set-display-rule!)
        for (kind_str, action_str) in state.pending_display_rules.drain(..) {
            use mae_core::display_policy::{parse_action, parse_buffer_kind};
            match (parse_buffer_kind(&kind_str), parse_action(&action_str)) {
                (Some(kind), Some(action)) => {
                    editor.display_policy.set_override(kind, action);
                    debug!(kind = %kind_str, action = %action_str, "display rule override applied");
                }
                _ => {
                    warn!(kind = %kind_str, action = %action_str, "invalid set-display-rule! args");
                    editor.set_status(format!(
                        "Invalid display rule: kind='{}', action='{}'",
                        kind_str, action_str
                    ));
                }
            }
        }

        // Apply replaceable-kind changes from (set-buffer-kind-replaceable!)
        for (kind_str, enable) in state.pending_replaceable_kinds.drain(..) {
            use mae_core::display_policy::parse_buffer_kind;
            match parse_buffer_kind(&kind_str) {
                Some(kind) => {
                    if enable {
                        if !editor.replaceable_kinds.contains(&kind) {
                            editor.replaceable_kinds.push(kind);
                        }
                    } else {
                        editor.replaceable_kinds.retain(|k| *k != kind);
                    }
                    debug!(kind = %kind_str, enable = %enable, "replaceable kind updated");
                }
                None => {
                    warn!(kind = %kind_str, "invalid set-buffer-kind-replaceable! arg");
                    editor.set_status(format!("Unknown buffer kind: '{}'", kind_str));
                }
            }
        }
    }

    /// Apply KB nodes, KB collaboration lifecycle actions, and typed link
    /// mutations (add/remove link, add/remove meta member).
    fn apply_kb_mutations(state: &mut SharedState, editor: &mut Editor) {
        // Apply KB nodes registered from Scheme via (define-kb-node! ID TITLE BODY)
        for (id, title, body) in state.pending_kb_nodes.drain(..) {
            let node = mae_core::KbNode::new(id.clone(), title, mae_core::KbNodeKind::Note, body)
                .with_tags(["scheme"]);
            editor.kb.primary.insert(node);
            debug!(id = %id, "kb node registered from scheme");
        }

        // Apply KB collaboration lifecycle actions from `(kb-share)` etc. — lowered
        // to the SAME CollabIntent the commands + MCP tools use.
        for action in state.pending_kb_collab_actions.drain(..) {
            editor.queue_kb_collab_action(action);
        }

        // Apply native KB graph-view intents from `(kb-graph-view-open)` etc.
        // (Part C Phase 1) — each variant maps 1:1 onto the same
        // `Editor::kb_graph_view_*` method the human keybindings + MCP tools
        // call, per CLAUDE.md principle #3 (AI/human parity).
        for intent in state.pending_graph_view_intents.drain(..) {
            match intent {
                mae_core::GraphViewIntent::Open { center, depth } => {
                    editor.kb_graph_view_open(center, depth);
                }
                mae_core::GraphViewIntent::Close => editor.kb_graph_view_close(),
                mae_core::GraphViewIntent::Refresh => editor.kb_graph_view_refresh_if_open(),
                mae_core::GraphViewIntent::SetDepth(depth) => {
                    editor.kb_graph_view_set_depth(depth);
                }
                mae_core::GraphViewIntent::Navigate(dir) => editor.kb_graph_view_navigate(dir),
                mae_core::GraphViewIntent::SelectCurrent => editor.kb_graph_view_select_current(),
            }
        }

        // Apply KB-link hover preview intents from `(kb-preview-show)` /
        // `(kb-preview-dismiss)` (Part D) — same 1:1 mapping onto
        // `Editor::kb_preview_*` as the graph-view intents above.
        for intent in state.pending_kb_preview_intents.drain(..) {
            match intent {
                mae_core::KbPreviewIntent::Show(id) => editor.kb_preview_show(&id),
                mae_core::KbPreviewIntent::Dismiss => editor.kb_preview_dismiss(),
            }
        }

        // Apply typed link additions from (kb-add-link! SRC DST REL_TYPE)
        if let Some(ref store) = editor.kb.store {
            for (src, dst, rel_type) in state.pending_kb_links.drain(..) {
                if let Err(e) = store.add_typed_link(&src, &dst, &rel_type, 1.0) {
                    warn!(src = %src, dst = %dst, rel = %rel_type, "kb-add-link! error: {}", e);
                } else {
                    debug!(src = %src, dst = %dst, rel = %rel_type, "typed link added from scheme");
                }
            }
            for (src, dst) in state.pending_kb_link_removals.drain(..) {
                if let Err(e) = store.remove_link(&src, &dst) {
                    warn!(src = %src, dst = %dst, "kb-remove-link! error: {}", e);
                } else {
                    debug!(src = %src, dst = %dst, "link removed from scheme");
                }
            }
            for (meta_id, member_id, role) in state.pending_kb_meta_adds.drain(..) {
                if let Err(e) = store.add_meta_member(&meta_id, &member_id, 0, &role) {
                    warn!(meta = %meta_id, member = %member_id, "kb-add-meta-member! error: {}", e);
                } else {
                    debug!(meta = %meta_id, member = %member_id, role = %role, "meta member added from scheme");
                }
            }
            for (meta_id, member_id) in state.pending_kb_meta_removes.drain(..) {
                if let Err(e) = store.remove_meta_member(&meta_id, &member_id) {
                    warn!(meta = %meta_id, member = %member_id, "kb-remove-meta-member! error: {}", e);
                } else {
                    debug!(meta = %meta_id, member = %member_id, "meta member removed from scheme");
                }
            }
        } else {
            // No store — just drain to avoid accumulating
            state.pending_kb_links.clear();
            state.pending_kb_link_removals.clear();
            state.pending_kb_meta_adds.clear();
            state.pending_kb_meta_removes.clear();
        }
    }

    /// Apply editor options (via the OptionRegistry), status message, and
    /// theme change requests.
    fn apply_options_status_theme(state: &mut SharedState, editor: &mut Editor) {
        // Apply editor options via the OptionRegistry (single source of truth)
        for (key, value) in state.pending_options.drain(..) {
            match editor.set_option(&key, &value) {
                Ok(_) => {}
                Err(e) => {
                    warn!(key = key.as_str(), "set-option! error: {}", e);
                    editor.set_status(e);
                }
            }
        }

        // Apply status message
        if let Some(msg) = state.status_message.take() {
            editor.set_status(msg);
        }

        // Apply theme change
        if let Some(theme_name) = state.theme_request.take() {
            info!(theme = %theme_name, "applying scheme theme request");
            editor.set_theme_by_name(&theme_name);
        }
    }

    /// Live editing primitives: buffer-insert, cursor-goto/goto-char,
    /// open-file.
    fn apply_live_editing(state: &mut SharedState, editor: &mut Editor) {
        // --- Live editing primitives ---

        // (buffer-insert TEXT)
        if let Some(text) = state.pending_insert.take() {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window();
            let offset = editor.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
            editor.buffers[idx].insert_text_at(offset, &text);
            // Advance cursor past inserted text.
            let end = offset + text.chars().count();
            let rope = editor.buffers[idx].rope();
            let new_row = rope.char_to_line(end.min(rope.len_chars()));
            let line_start = rope.line_to_char(new_row);
            let win = editor.window_mgr.focused_window_mut();
            win.cursor_row = new_row;
            win.cursor_col = end.saturating_sub(line_start);
            editor.fire_hook("after-insert");
        }

        // (cursor-goto ROW COL) or (goto-char OFFSET)
        if let Some((row, col)) = state.pending_cursor.take() {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            if row == usize::MAX {
                // goto-char mode: col holds the char offset
                let offset = col.min(editor.buffers[idx].rope().len_chars());
                let rope = editor.buffers[idx].rope();
                let new_row = rope.char_to_line(offset);
                let line_start = rope.line_to_char(new_row);
                win.cursor_row = new_row;
                win.cursor_col = offset.saturating_sub(line_start);
            } else {
                win.cursor_row = row;
                win.cursor_col = col;
            }
            win.clamp_cursor(&editor.buffers[idx]);
        }

        // (open-file PATH)
        if let Some(path) = state.pending_open_file.take() {
            editor.open_file(&path);
        }
    }

    /// Round 2 buffer editing primitives: delete-range, replace-range,
    /// create-buffer, kill-buffer-by-name.
    fn apply_round2_buffer_editing(state: &mut SharedState, editor: &mut Editor) {
        // --- Round 2: buffer editing primitives ---

        // (buffer-delete-range START END)
        if let Some((start, end)) = state.pending_delete_range.take() {
            let idx = editor.active_buffer_idx();
            let len = editor.buffers[idx].rope().len_chars();
            let start = start.min(len);
            let end = end.min(len);
            if start < end {
                editor.buffers[idx].delete_range(start, end);
                editor.fire_hook("after-delete");
            }
        }

        // (buffer-replace-range START END TEXT)
        if let Some((start, end, text)) = state.pending_replace_range.take() {
            let idx = editor.active_buffer_idx();
            let len = editor.buffers[idx].rope().len_chars();
            let start = start.min(len);
            let end = end.min(len);
            if start <= end {
                if start < end {
                    editor.buffers[idx].delete_range(start, end);
                }
                editor.buffers[idx].insert_text_at(start, &text);
            }
        }

        // (create-buffer NAME)
        if let Some(name) = state.pending_create_buffer.take() {
            let mut buf = mae_core::Buffer::new();
            buf.name = name;
            editor.buffers.push(buf);
            let new_idx = editor.buffers.len() - 1;
            editor.display_buffer(new_idx);
        }

        // (kill-buffer-by-name NAME)
        if let Some(name) = state.pending_kill_buffer.take() {
            if let Some(idx) = editor.buffers.iter().position(|b| b.name == name) {
                if editor.buffers.len() > 1 {
                    editor.buffers.remove(idx);
                    editor.notify_buffer_removed(idx);
                    for w in editor.window_mgr.iter_windows_mut() {
                        if w.buffer_idx == idx {
                            w.buffer_idx = 0;
                        } else if w.buffer_idx > idx {
                            w.buffer_idx -= 1;
                        }
                    }
                }
            }
        }
    }

    /// Advice registrations/removals, then buffer-undo / buffer-redo /
    /// buffer-undo-boundary.
    fn apply_advice_and_undo(state: &mut SharedState, editor: &mut Editor) {
        // Apply advice registrations
        for (command, kind_str, fn_name) in state.pending_advice_adds.drain(..) {
            let kind = match kind_str.as_str() {
                ":before" | "before" => mae_core::hooks::AdviceKind::Before,
                ":after" | "after" => mae_core::hooks::AdviceKind::After,
                other => {
                    warn!(kind = %other, "advice-add! unknown kind, defaulting to :before");
                    mae_core::hooks::AdviceKind::Before
                }
            };
            editor.hooks.add_advice(&command, kind, &fn_name);
            debug!(command = %command, kind = %kind_str, fn_name = %fn_name, "advice registered");
        }

        // Apply advice removals
        for (command, fn_name) in state.pending_advice_removes.drain(..) {
            editor.hooks.remove_advice(&command, &fn_name);
            debug!(command = %command, fn_name = %fn_name, "advice removed");
        }

        // (buffer-undo)
        if state.pending_undo {
            state.pending_undo = false;
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].undo(win);
        }

        // (buffer-redo)
        if state.pending_redo {
            state.pending_redo = false;
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].redo(win);
        }

        // (buffer-undo-boundary)
        if state.pending_undo_boundary {
            state.pending_undo_boundary = false;
            let idx = editor.active_buffer_idx();
            editor.buffers[idx].sync_undo_boundary();
        }
    }

    /// CRDT/sync operations: enable/disable-sync, load-sync-state, apply
    /// sync updates, encode-state-vector, compute-diff, reconcile-to,
    /// switch-to-buffer.
    fn apply_crdt_sync(state: &mut SharedState, editor: &mut Editor) {
        // --- CRDT/sync operations ---

        // (buffer-enable-sync CLIENT-ID)
        if let Some(client_id) = state.pending_enable_sync.take() {
            let idx = editor.active_buffer_idx();
            editor.buffers[idx].enable_sync(client_id);
            debug!(client_id = client_id, "sync enabled on active buffer");
        }

        // (buffer-disable-sync)
        if state.pending_disable_sync {
            state.pending_disable_sync = false;
            let idx = editor.active_buffer_idx();
            editor.buffers[idx].disable_sync();
            debug!("sync disabled on active buffer");
        }

        // (buffer-load-sync-state STATE-BYTES CLIENT-ID)
        if let Some((state_bytes, client_id)) = state.pending_load_sync_state.take() {
            let idx = editor.active_buffer_idx();
            match editor.buffers[idx].load_sync_state(&state_bytes, client_id) {
                Ok(()) => debug!(client_id = client_id, "sync state loaded on active buffer"),
                Err(e) => warn!(error = %e, "failed to load sync state"),
            }
        }

        // (buffer-drain-updates) — now handled by capture_pending_sync_updates(),
        // which must run before drain_and_broadcast in the test runner.

        // (buffer-apply-update BUFFER-NAME UPDATE-BYTES)
        let sync_applies: Vec<(String, Vec<u8>)> = std::mem::take(&mut state.pending_sync_applies);
        for (buf_name, update_bytes) in sync_applies {
            if let Some(idx) = editor.buffers.iter().position(|b| b.name == buf_name) {
                match editor.buffers[idx].apply_sync_update(&update_bytes) {
                    Ok(()) => debug!(buffer = %buf_name, "sync update applied"),
                    Err(e) => warn!(buffer = %buf_name, error = %e, "failed to apply sync update"),
                }
            } else {
                warn!(buffer = %buf_name, "buffer not found for sync update");
            }
        }

        // (buffer-encode-state-vector) — encode active buffer's state vector.
        if state.pending_encode_state_vector {
            state.pending_encode_state_vector = false;
            let idx = editor.active_buffer_idx();
            if let Some(ref sync) = editor.buffers[idx].sync_doc {
                use base64::Engine as _;
                let sv = sync.state_vector();
                state.encoded_state_vector =
                    Some(base64::engine::general_purpose::STANDARD.encode(&sv));
            } else {
                state.encoded_state_vector = None;
            }
        }

        // (buffer-compute-diff SV-BASE64) — compute diff from remote state vector.
        if let Some(sv_b64) = state.pending_compute_diff.take() {
            use base64::Engine as _;
            use mae_sync::yrs::updates::decoder::Decode;
            use mae_sync::yrs::{ReadTxn, Transact};
            let idx = editor.active_buffer_idx();
            if let Some(ref sync) = editor.buffers[idx].sync_doc {
                match base64::engine::general_purpose::STANDARD.decode(&sv_b64) {
                    Ok(sv_bytes) => {
                        let txn = sync.doc().transact();
                        match mae_sync::yrs::StateVector::decode_v1(&sv_bytes) {
                            Ok(sv) => {
                                let diff = txn.encode_state_as_update_v1(&sv);
                                state.computed_diff =
                                    Some(base64::engine::general_purpose::STANDARD.encode(&diff));
                            }
                            Err(e) => {
                                warn!(error = %e, "failed to decode state vector");
                                state.computed_diff = None;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to base64-decode state vector");
                        state.computed_diff = None;
                    }
                }
            } else {
                state.computed_diff = None;
            }
        }

        // (buffer-reconcile-to TEXT) — reconcile sync doc to target text.
        if let Some(target) = state.pending_reconcile_to.take() {
            use base64::Engine as _;
            let idx = editor.active_buffer_idx();
            let has_sync = editor.buffers[idx].sync_doc.is_some();
            if has_sync {
                let update = editor.buffers[idx]
                    .sync_doc
                    .as_mut()
                    .unwrap()
                    .reconcile_to(&target);
                if update.is_empty() {
                    state.reconcile_result = Some(String::new());
                } else {
                    state.reconcile_result =
                        Some(base64::engine::general_purpose::STANDARD.encode(&update));
                }
                // Rebuild the buffer rope from the sync doc.
                editor.buffers[idx].rebuild_rope_from_sync();
            } else {
                state.reconcile_result = None;
            }
        }

        // (switch-to-buffer IDX)
        if let Some(idx) = state.pending_switch_buffer.take() {
            if idx < editor.buffers.len() {
                let prev = editor.active_buffer_idx();
                editor.vi.alternate_buffer_idx = Some(prev);
                editor.display_buffer(idx);
            }
        }
    }
}
