//! Scheme -> Editor state application, continued: key removals/group
//! names, queued command extraction, messages/shell/recent-files/agenda,
//! visual buffer ops, buffer-local options, AI tools, and splash arts.
//! See `state_sync_apply.rs` for the dispatcher (`apply_to_editor`) and
//! the rest of the sections.
//!
//! Split out of `runtime.rs` (CLAUDE.md architecture debt reduction pass)
//! -- pure code motion, no behavior change. These methods are `pub(super)`
//! solely so `state_sync_apply.rs`'s dispatcher can call them in the
//! exact original order; they are not part of any public API.

use mae_core::parse_key_seq_spaced;
use mae_core::Editor;

use tracing::{debug, info, warn};

use super::{SchemeRuntime, SharedState, VisualOp};

impl SchemeRuntime {
    /// `(undefine-key! MAP KEY)` and `(set-group-name MAP PREFIX LABEL)`.
    /// @ai-caution: [scheme-api] set-group-name must drain alongside
    /// keymap_bindings.
    pub(super) fn apply_key_removals_and_group_names(state: &mut SharedState, editor: &mut Editor) {
        // (undefine-key! MAP KEY)
        for (map_name, key_str) in state.pending_key_removals.drain(..) {
            if let Some(keymap) = editor.keymaps.get_mut(&map_name) {
                let seq = parse_key_seq_spaced(&key_str);
                if !seq.is_empty() {
                    keymap.unbind(&seq);
                }
            }
        }

        // (set-group-name MAP PREFIX LABEL)
        // @ai-caution: [scheme-api] set-group-name must drain in apply_to_editor alongside keymap_bindings.
        for (map_name, prefix_str, label) in state.pending_group_names.drain(..) {
            if let Some(keymap) = editor.keymaps.get_mut(&map_name) {
                let seq = parse_key_seq_spaced(&prefix_str);
                if !seq.is_empty() {
                    keymap.set_group_name(seq, &label);
                    debug!(keymap = %map_name, prefix = %prefix_str, label = %label,
                           "applying scheme group name");
                }
            }
        }
    }

    /// Extract queued `(run-command NAME)` / `(execute-ex CMD)` calls. We
    /// drain them here (still under the lock) but the CALLER dispatches
    /// them AFTER dropping the lock, since dispatch may re-enter shared
    /// state.
    pub(super) fn take_pending_commands(state: &mut SharedState) -> (Vec<String>, Vec<String>) {
        // (run-command NAME) — dispatch each queued command.
        // We drain them outside the lock since dispatch_builtin
        // may re-enter shared state.
        let commands: Vec<String> = std::mem::take(&mut state.pending_commands);

        // (execute-ex CMD) — dispatch through ex-command parser (supports args).
        let ex_commands: Vec<String> = std::mem::take(&mut state.pending_ex_commands);

        (commands, ex_commands)
    }

    /// `(message TEXT)` log lines, shell-send-input queue, recent
    /// files/projects, and agenda file management.
    pub(super) fn apply_messages_shell_recent_agenda(state: &mut SharedState, editor: &mut Editor) {
        // (message TEXT) — append to message log
        for msg in state.pending_messages.drain(..) {
            info!("[scheme] {}", msg);
        }

        // (shell-send-input BUF-IDX TEXT) — queue shell terminal input.
        for (buf_idx, text) in state.pending_shell_inputs.drain(..) {
            editor.shell.inputs.push((buf_idx, text));
        }

        // Recent files and projects
        for path in state.pending_recent_files.drain(..) {
            editor.recent_files.push(std::path::PathBuf::from(path));
        }
        for path in state.pending_recent_projects.drain(..) {
            editor.recent_projects.push(std::path::PathBuf::from(path));
        }

        // Agenda file management
        for path in state.pending_agenda_adds.drain(..) {
            editor.agenda_add_path(&path);
        }
        for path in state.pending_agenda_removes.drain(..) {
            editor.agenda_remove_path(&path);
        }
        if state.pending_agenda_list {
            state.pending_agenda_list = false;
            editor.agenda_list_paths();
        }
    }

    /// Visual buffer drawing operations (add-rect/line/circle/text, clear).
    pub(super) fn apply_visual_buffer_ops(state: &mut SharedState, editor: &mut Editor) {
        // Visual buffer operations
        let visual_ops = std::mem::take(&mut state.pending_visual_ops);
        if !visual_ops.is_empty() {
            let buf_idx = editor.active_buffer_idx();
            if editor.buffers[buf_idx].kind == mae_core::BufferKind::Visual {
                if let Some(vb) = editor.buffers[buf_idx].visual_mut() {
                    for op in visual_ops {
                        match op {
                            VisualOp::AddRect {
                                x,
                                y,
                                w,
                                h,
                                fill,
                                stroke,
                            } => {
                                vb.add(mae_core::visual_buffer::VisualElement::Rect {
                                    x,
                                    y,
                                    w,
                                    h,
                                    fill,
                                    stroke,
                                });
                            }
                            VisualOp::AddLine {
                                x1,
                                y1,
                                x2,
                                y2,
                                color,
                                thickness,
                            } => {
                                vb.add(mae_core::visual_buffer::VisualElement::Line {
                                    x1,
                                    y1,
                                    x2,
                                    y2,
                                    color,
                                    thickness,
                                });
                            }
                            VisualOp::AddCircle {
                                cx,
                                cy,
                                r,
                                fill,
                                stroke,
                            } => {
                                vb.add(mae_core::visual_buffer::VisualElement::Circle {
                                    cx,
                                    cy,
                                    r,
                                    fill,
                                    stroke,
                                });
                            }
                            VisualOp::AddText {
                                x,
                                y,
                                text,
                                font_size,
                                color,
                            } => {
                                vb.add(mae_core::visual_buffer::VisualElement::Text {
                                    x,
                                    y,
                                    text,
                                    font_size,
                                    color,
                                });
                            }
                            VisualOp::Clear => vb.clear(),
                        }
                    }
                }
            }
        }
    }

    /// Buffer-local options: `(set-local-option! KEY VALUE)`.
    pub(super) fn apply_local_options(state: &mut SharedState, editor: &mut Editor) {
        // Buffer-local options: (set-local-option! KEY VALUE)
        for (key, value) in state.pending_local_options.drain(..) {
            match editor.set_local_option(&key, &value) {
                Ok(_) => {}
                Err(e) => {
                    warn!(key = key.as_str(), "set-local-option! error: {}", e);
                    editor.set_status(e);
                }
            }
        }
    }

    /// Scheme-registered AI tools (merge late-registered params, then
    /// upsert into `editor.ai.scheme_tools`).
    pub(super) fn apply_ai_tools(state: &mut SharedState, editor: &mut Editor) {
        // Scheme-registered AI tools
        let mut ai_tools: Vec<mae_core::SchemeToolDef> =
            std::mem::take(&mut state.pending_ai_tools);
        for tool in &mut ai_tools {
            // Merge any late-registered params (ai-tool-param! called after register-ai-tool!)
            if let Some(extra) = state.pending_ai_tool_params.remove(&tool.name) {
                tool.params.extend(extra);
            }
            if let Some(extra) = state.pending_ai_tool_required.remove(&tool.name) {
                tool.required.extend(extra);
            }
        }
        for tool in ai_tools {
            debug!(name = %tool.name, handler = %tool.handler_fn, "registering Scheme AI tool");
            // Upsert: replace if already registered by name
            if let Some(existing) = editor
                .ai
                .scheme_tools
                .iter_mut()
                .find(|t| t.name == tool.name)
            {
                *existing = tool;
            } else {
                editor.ai.scheme_tools.push(tool);
            }
        }
    }

    /// Custom splash arts (upsert by name).
    pub(super) fn apply_splash_arts(state: &mut SharedState, editor: &mut Editor) {
        // Custom splash arts
        for (name, art, image_path) in state.pending_splash_arts.drain(..) {
            use mae_core::render_common::splash::CustomSplashArt;
            let entry = CustomSplashArt {
                name: name.clone(),
                art,
                accent_lines: Vec::new(),
                image_path,
            };
            // Upsert by name
            if let Some(existing) = editor
                .custom_splash_arts
                .iter_mut()
                .find(|a| a.name == name)
            {
                *existing = entry;
            } else {
                editor.custom_splash_arts.push(entry);
            }
        }
    }
}
