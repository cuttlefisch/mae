use crate::{Mode, VisualType};

use super::super::Editor;

impl Editor {
    /// Dispatch visual mode commands. Returns `Some(true)` if handled.
    pub(super) fn dispatch_visual(&mut self, name: &str) -> Option<bool> {
        match name {
            "enter-visual-char" => match self.mode {
                Mode::Visual(VisualType::Char) => self.set_mode(Mode::Normal),
                Mode::Visual(_) => self.set_mode(Mode::Visual(VisualType::Char)),
                _ => self.enter_visual_mode(VisualType::Char),
            },
            "enter-visual-line" => match self.mode {
                Mode::Visual(VisualType::Line) => self.set_mode(Mode::Normal),
                Mode::Visual(_) => self.set_mode(Mode::Visual(VisualType::Line)),
                _ => self.enter_visual_mode(VisualType::Line),
            },
            "enter-visual-block" => match self.mode {
                Mode::Visual(VisualType::Block) => self.set_mode(Mode::Normal),
                Mode::Visual(_) => self.set_mode(Mode::Visual(VisualType::Block)),
                _ => self.enter_visual_mode(VisualType::Block),
            },
            "visual-delete" => {
                self.save_visual_state();
                if self.mode == Mode::Visual(VisualType::Block) {
                    self.block_visual_delete();
                } else {
                    self.visual_delete();
                }
            }
            "visual-yank" => {
                self.save_visual_state();
                if self.mode == Mode::Visual(VisualType::Block) {
                    self.block_visual_yank();
                } else {
                    self.visual_yank();
                }
            }
            "visual-change" => {
                self.save_visual_state();
                if self.mode == Mode::Visual(VisualType::Block) {
                    self.block_visual_change();
                } else {
                    self.visual_change();
                }
            }
            "block-visual-insert" => {
                if self.mode == Mode::Visual(VisualType::Block) {
                    let (min_row, max_row, min_col, _max_col) = self.block_selection_rect();
                    self.save_visual_state();
                    self.pending_block_insert = Some((min_row, max_row, min_col));
                    self.search_state.highlight_active = false;
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = min_row;
                    win.cursor_col = min_col;
                    let idx = self.active_buffer_idx();
                    self.insert_start_offset =
                        Some(self.buffers[idx].char_offset_at(min_row, min_col));
                    self.insert_initiated_by = Some("block-visual-insert".to_string());
                    self.buffers[idx].begin_undo_group();
                    self.set_mode(Mode::Insert);
                }
            }
            "block-visual-append" => {
                if self.mode == Mode::Visual(VisualType::Block) {
                    let (min_row, max_row, _min_col, max_col) = self.block_selection_rect();
                    self.save_visual_state();
                    let append_col = max_col + 1;
                    self.pending_block_insert = Some((min_row, max_row, append_col));
                    self.search_state.highlight_active = false;
                    let idx = self.active_buffer_idx();
                    let line_len = self.buffers[idx]
                        .line_text(min_row)
                        .trim_end_matches('\n')
                        .chars()
                        .count();
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = min_row;
                    win.cursor_col = append_col.min(line_len);
                    self.insert_start_offset =
                        Some(self.buffers[idx].char_offset_at(min_row, win.cursor_col));
                    self.insert_initiated_by = Some("block-visual-append".to_string());
                    self.buffers[idx].begin_undo_group();
                    self.set_mode(Mode::Insert);
                }
            }
            "visual-indent" => self.visual_indent(),
            "visual-dedent" => self.visual_dedent(),
            "visual-join" => self.visual_join(),
            "visual-paste" => self.visual_paste(),
            "visual-swap-ends" => self.visual_swap_ends(),
            "visual-uppercase" => self.visual_uppercase(),
            "visual-lowercase" => self.visual_lowercase(),
            _ => return None,
        }
        self.mark_full_redraw();
        Some(true)
    }
}
