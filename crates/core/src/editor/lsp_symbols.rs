//! LSP symbol outline, breadcrumbs, and peek references.

use crate::lsp_intent::{language_id_from_path, path_to_uri};

use super::Editor;

impl Editor {
    /// Request document symbols for the symbol outline popup.
    pub fn lsp_request_symbol_outline(&mut self) {
        let idx = self.active_buffer_idx();
        let buf = &self.buffers[idx];
        let Some(path) = buf.file_path() else {
            self.set_status("[LSP] symbol outline: buffer has no file path");
            return;
        };
        let Some(lang_id) = language_id_from_path(path) else {
            self.set_status("[LSP] symbol outline: unsupported language");
            return;
        };
        let uri = path_to_uri(path);
        self.lsp
            .pending_requests
            .push(crate::LspIntent::DocumentSymbols {
                uri,
                language_id: lang_id,
            });
        self.lsp.symbol_outline_pending = true;
        self.set_status("[LSP] loading symbol outline\u{2026}");
    }

    /// Apply document symbol results to populate the symbol outline popup.
    pub fn apply_symbol_outline_result(&mut self, symbols: &[crate::editor::SymbolOutlineEntry]) {
        self.lsp.symbol_outline_pending = false;
        // Cache symbols for breadcrumbs.
        self.lsp.cached_doc_symbols = symbols.to_vec();
        self.lsp.cached_doc_symbols_buf = Some(self.active_buffer_idx());
        if symbols.is_empty() {
            self.set_status("[LSP] no symbols in document");
            return;
        }
        let len = symbols.len();
        let all_indices: Vec<usize> = (0..len).collect();
        self.lsp.symbol_outline = Some(super::SymbolOutlineState {
            entries: symbols.to_vec(),
            selected: 0,
            filter: String::new(),
            filtered_indices: all_indices,
        });
        self.set_status(format!(
            "[LSP] {} symbols — j/k navigate, Enter jump, Esc dismiss",
            len
        ));
    }

    /// Apply document symbol results only for breadcrumbs (no popup).
    pub fn apply_breadcrumb_symbols(&mut self, symbols: &[crate::editor::SymbolOutlineEntry]) {
        self.lsp.breadcrumb_symbols_pending = false;
        self.lsp.cached_doc_symbols = symbols.to_vec();
        self.lsp.cached_doc_symbols_buf = Some(self.active_buffer_idx());
        self.update_breadcrumbs();
    }

    /// Navigate the symbol outline popup down.
    pub fn symbol_outline_next(&mut self) {
        if let Some(ref mut state) = self.lsp.symbol_outline {
            if !state.filtered_indices.is_empty() {
                state.selected = (state.selected + 1) % state.filtered_indices.len();
            }
        }
    }

    /// Navigate the symbol outline popup up.
    pub fn symbol_outline_prev(&mut self) {
        if let Some(ref mut state) = self.lsp.symbol_outline {
            if !state.filtered_indices.is_empty() {
                state.selected = state
                    .selected
                    .checked_sub(1)
                    .unwrap_or(state.filtered_indices.len().saturating_sub(1));
            }
        }
    }

    /// Select the current symbol outline entry — jump to its line and dismiss.
    pub fn symbol_outline_select(&mut self) {
        let line = {
            let state = match self.lsp.symbol_outline.as_ref() {
                Some(s) => s,
                None => return,
            };
            let idx = match state.filtered_indices.get(state.selected) {
                Some(&i) => i,
                None => return,
            };
            state.entries[idx].line
        };
        self.lsp.symbol_outline = None;
        let buf_idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = line;
        win.cursor_col = 0;
        win.clamp_cursor(&self.buffers[buf_idx]);
        let vh = self.viewport_height;
        win.scroll_center(vh);
    }

    /// Dismiss the symbol outline popup.
    pub fn symbol_outline_dismiss(&mut self) {
        self.lsp.symbol_outline = None;
    }

    /// Update the filter on the symbol outline popup.
    pub fn symbol_outline_filter_char(&mut self, ch: char) {
        if let Some(ref mut state) = self.lsp.symbol_outline {
            state.filter.push(ch);
            let filter_lower = state.filter.to_lowercase();
            state.filtered_indices = state
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.name.to_lowercase().contains(&filter_lower))
                .map(|(i, _)| i)
                .collect();
            state.selected = 0;
        }
    }

    /// Delete last char from symbol outline filter.
    pub fn symbol_outline_filter_backspace(&mut self) {
        if let Some(ref mut state) = self.lsp.symbol_outline {
            state.filter.pop();
            if state.filter.is_empty() {
                state.filtered_indices = (0..state.entries.len()).collect();
            } else {
                let filter_lower = state.filter.to_lowercase();
                state.filtered_indices = state
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.name.to_lowercase().contains(&filter_lower))
                    .map(|(i, _)| i)
                    .collect();
            }
            state.selected = 0;
        }
    }

    /// Request references for the symbol at cursor, for peek display.
    pub fn lsp_request_peek_references(&mut self) {
        self.lsp.peek_references_pending = true;
        self.lsp_request_references();
    }

    /// Navigate peek references forward.
    pub fn peek_references_next(&mut self) {
        if let Some(ref mut state) = self.lsp.peek_references {
            if !state.locations.is_empty() {
                state.current = (state.current + 1) % state.locations.len();
                self.update_peek_references_preview();
            }
        }
    }

    /// Navigate peek references backward.
    pub fn peek_references_prev(&mut self) {
        if let Some(ref mut state) = self.lsp.peek_references {
            if !state.locations.is_empty() {
                state.current = state
                    .current
                    .checked_sub(1)
                    .unwrap_or(state.locations.len().saturating_sub(1));
                self.update_peek_references_preview();
            }
        }
    }

    /// Update the peek state to show the current reference location.
    pub fn update_peek_references_preview(&mut self) {
        let (path, line, col, ctx, current, total) = {
            let state = match self.lsp.peek_references.as_ref() {
                Some(s) => s,
                None => return,
            };
            let loc = &state.locations[state.current];
            (
                loc.path.clone(),
                loc.line,
                loc.col,
                loc.context.clone(),
                state.current + 1,
                state.locations.len(),
            )
        };
        self.lsp.peek_state = Some(super::PeekState {
            file_path: path.clone(),
            line,
            col,
            context_lines: ctx,
            highlight_line: 0,
            scroll_offset: 0,
        });
        self.set_status(format!(
            "[LSP] reference [{}/{}] {}:{}",
            current,
            total,
            path.rsplit('/').next().unwrap_or(&path),
            line + 1
        ));
    }

    /// Request document symbols for breadcrumb computation (idle trigger).
    pub fn request_breadcrumb_symbols(&mut self) {
        if !self.show_breadcrumbs || self.lsp.breadcrumb_symbols_pending {
            return;
        }
        let idx = self.active_buffer_idx();
        // If we already have cached symbols for this buffer, just update breadcrumbs.
        if self.lsp.cached_doc_symbols_buf == Some(idx) && !self.lsp.cached_doc_symbols.is_empty() {
            self.update_breadcrumbs();
            return;
        }
        let buf = &self.buffers[idx];
        let Some(path) = buf.file_path() else { return };
        let Some(lang_id) = language_id_from_path(path) else {
            return;
        };
        let uri = path_to_uri(path);
        self.lsp
            .pending_requests
            .push(crate::LspIntent::DocumentSymbols {
                uri,
                language_id: lang_id,
            });
        self.lsp.breadcrumb_symbols_pending = true;
    }

    /// Compute breadcrumb path from cached document symbols and cursor position.
    pub fn update_breadcrumbs(&mut self) {
        if !self.show_breadcrumbs {
            self.lsp.breadcrumbs = None;
            return;
        }
        let idx = self.active_buffer_idx();
        let buf = &self.buffers[idx];
        let filename = buf
            .file_path()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "[buffer]".to_string());

        if self.lsp.cached_doc_symbols_buf != Some(idx) || self.lsp.cached_doc_symbols.is_empty() {
            self.lsp.breadcrumbs = Some(vec![filename]);
            return;
        }

        let cursor_line = self.window_mgr.focused_window().cursor_row;

        // Walk symbols to find ancestry path. Symbols are ordered by line with depth.
        // Build a stack of (depth, name) tracking the current nesting.
        let mut stack: Vec<(usize, String)> = Vec::new();
        for sym in &self.lsp.cached_doc_symbols {
            if sym.line > cursor_line {
                break;
            }
            // Pop deeper or same-level entries (cursor moved past them).
            while stack.last().map(|(d, _)| *d >= sym.depth).unwrap_or(false) {
                stack.pop();
            }
            stack.push((sym.depth, sym.name.clone()));
        }

        let mut crumbs = vec![filename];
        crumbs.extend(stack.into_iter().map(|(_, name)| name));
        self.lsp.breadcrumbs = Some(crumbs);
    }
}
