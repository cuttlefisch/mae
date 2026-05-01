use super::super::Editor;

impl Editor {
    /// Dispatch LSP and syntax commands. Returns `Some(true)` if handled.
    pub(super) fn dispatch_lsp(&mut self, name: &str) -> Option<bool> {
        match name {
            "lsp-goto-definition" => {
                self.record_jump();
                self.lsp_request_definition();
            }
            "lsp-find-references" => self.lsp_request_references(),
            "lsp-hover" => {
                self.lsp_request_hover();
                // Also show debug variable value if stopped.
                if let Some(state) = &self.debug_state {
                    if state.is_stopped() {
                        let buf = &self.buffers[self.active_buffer_idx()];
                        let win = self.window_mgr.focused_window();
                        let offset = buf.char_offset_at(win.cursor_row, win.cursor_col);
                        if let Some(pattern) = crate::search::word_at_offset(buf.rope(), offset) {
                            let word = pattern
                                .strip_prefix("\\b")
                                .and_then(|s| s.strip_suffix("\\b"))
                                .unwrap_or(&pattern);
                            if let Some((_scope, var)) = state.find_variable(word, None) {
                                let type_str = var
                                    .var_type
                                    .as_deref()
                                    .map(|t| format!(": {}", t))
                                    .unwrap_or_default();
                                let debug_info =
                                    format!("[Debug] {}{} = {}", var.name, type_str, var.value);
                                let existing = std::mem::take(&mut self.status_msg);
                                if existing.is_empty() {
                                    self.status_msg = debug_info;
                                } else {
                                    self.status_msg = format!("{} | {}", existing, debug_info);
                                }
                            }
                        }
                    }
                }
            }
            "dismiss-hover-popup" => self.dismiss_hover_popup(),
            "hover-scroll-down" => self.hover_scroll_down(),
            "hover-scroll-up" => self.hover_scroll_up(),
            "lsp-complete" => self.lsp_request_completion(),
            "lsp-accept-completion" => self.lsp_accept_completion(),
            "lsp-dismiss-completion" => self.lsp_dismiss_completion(),
            "lsp-complete-next" => self.lsp_complete_next(),
            "lsp-complete-prev" => self.lsp_complete_prev(),
            "lsp-next-diagnostic" => {
                self.record_jump();
                self.jump_next_diagnostic();
            }
            "lsp-prev-diagnostic" => {
                self.record_jump();
                self.jump_prev_diagnostic();
            }
            "lsp-show-diagnostics" => self.show_diagnostics_buffer(),
            "lsp-status" => self.show_lsp_status_buffer(),
            "lsp-code-action" => {
                self.lsp_request_code_action();
            }
            "lsp-code-action-next" => self.code_action_next(),
            "lsp-code-action-prev" => self.code_action_prev(),
            "lsp-code-action-select" => self.code_action_select(),
            "lsp-code-action-dismiss" => self.code_action_dismiss(),
            "toggle-lsp-diagnostics-inline" => {
                self.lsp_diagnostics_inline = !self.lsp_diagnostics_inline;
                let state = if self.lsp_diagnostics_inline {
                    "on"
                } else {
                    "off"
                };
                self.set_status(format!("Inline diagnostics: {}", state));
            }
            "lsp-rename" => {
                self.set_mode(crate::Mode::Command);
                self.command_line = "lsp-rename ".to_string();
                self.command_cursor = self.command_line.len();
                self.set_status("Enter new name for symbol");
            }
            "lsp-format" => {
                self.lsp_request_format();
            }
            "syntax-select-node" => {
                self.syntax_select_node();
            }
            "syntax-expand-selection" => {
                self.syntax_expand_selection();
            }
            "syntax-contract-selection" => {
                self.syntax_contract_selection();
            }
            _ => return None,
        }
        Some(true)
    }
}
