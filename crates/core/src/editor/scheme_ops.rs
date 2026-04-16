//! Scheme REPL integration: eval-line, eval-region, eval-buffer, and
//! the `*Scheme*` output buffer.
//!
//! The core crate cannot depend on `mae-scheme` (which depends on
//! `mae-core`), so the actual evaluation happens in the binary
//! (`key_handling.rs`) after these methods capture the text and push it
//! onto [`Editor::pending_scheme_eval`]. This is the same intent-queue
//! pattern used for LSP and DAP requests.

use crate::Mode;

use super::Editor;

impl Editor {
    /// `eval-line` — capture the current line as Scheme code and
    /// queue it for evaluation.
    pub fn eval_current_line(&mut self) {
        let idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let text = self.buffers[idx].line_text(row).to_string();
        let text = text.trim().to_string();
        if text.is_empty() {
            self.set_status("eval-line: empty line");
            return;
        }
        self.pending_scheme_eval.push(text);
    }

    /// `eval-region` — capture the visual selection as Scheme code.
    /// Only valid in Visual mode; returns to Normal after capture.
    pub fn eval_visual_region(&mut self) {
        if !matches!(self.mode, Mode::Visual(_)) {
            self.set_status("eval-region: no visual selection");
            return;
        }
        let (start, end) = self.visual_selection_range();
        if start >= end {
            self.mode = Mode::Normal;
            self.set_status("eval-region: empty selection");
            return;
        }
        let idx = self.active_buffer_idx();
        let text = self.buffers[idx].text_range(start, end);
        let text = text.trim().to_string();
        self.mode = Mode::Normal;
        if text.is_empty() {
            self.set_status("eval-region: empty selection");
            return;
        }
        self.pending_scheme_eval.push(text);
    }

    /// `eval-buffer` — capture the entire current buffer as Scheme
    /// code.
    pub fn eval_current_buffer(&mut self) {
        let text = self.active_buffer().text();
        let text = text.trim().to_string();
        if text.is_empty() {
            self.set_status("eval-buffer: empty buffer");
            return;
        }
        self.pending_scheme_eval.push(text);
    }

    /// Open or switch to the `*Scheme*` REPL output buffer.
    pub fn open_scheme_repl(&mut self) {
        let existing = self.buffers.iter().position(|b| b.name == "*Scheme*");
        let idx = if let Some(i) = existing {
            i
        } else {
            let mut buf = crate::buffer::Buffer::new();
            buf.name = "*Scheme*".into();
            buf.replace_contents(
                ";; MAE Scheme REPL — evaluate from any buffer with SPC e\n\
                 ;; or type expressions here and use :eval <code>\n\n",
            );
            self.buffers.push(buf);
            self.buffers.len() - 1
        };
        self.switch_to_buffer(idx);
    }

    /// Append REPL output to the `*Scheme*` buffer, creating it if
    /// needed. Called by the binary after eval completes.
    pub fn append_to_scheme_repl(&mut self, output: &str) {
        let existing = self.buffers.iter().position(|b| b.name == "*Scheme*");
        let idx = if let Some(i) = existing {
            i
        } else {
            let mut buf = crate::buffer::Buffer::new();
            buf.name = "*Scheme*".into();
            buf.replace_contents(
                ";; MAE Scheme REPL — evaluate from any buffer with SPC e\n\
                 ;; or type expressions here and use :eval <code>\n\n",
            );
            self.buffers.push(buf);
            self.buffers.len() - 1
        };
        let rope = self.buffers[idx].rope();
        let end = rope.len_chars();
        self.buffers[idx].insert_text_at(end, output);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    #[test]
    fn eval_current_line_captures_text() {
        let mut buf = Buffer::new();
        buf.replace_contents("(+ 1 2)\n(+ 3 4)\n");
        let mut ed = Editor::with_buffer(buf);
        // cursor on line 0
        ed.eval_current_line();
        assert_eq!(ed.pending_scheme_eval.len(), 1);
        assert_eq!(ed.pending_scheme_eval[0], "(+ 1 2)");
    }

    #[test]
    fn eval_current_line_empty_sets_status() {
        let mut ed = Editor::new();
        ed.eval_current_line();
        assert!(ed.status_msg.contains("empty"));
        assert!(ed.pending_scheme_eval.is_empty());
    }

    #[test]
    fn eval_current_buffer_captures_all_text() {
        let mut buf = Buffer::new();
        buf.replace_contents("(define x 42)\n(+ x 1)\n");
        let mut ed = Editor::with_buffer(buf);
        ed.eval_current_buffer();
        assert_eq!(ed.pending_scheme_eval.len(), 1);
        assert!(ed.pending_scheme_eval[0].contains("(define x 42)"));
        assert!(ed.pending_scheme_eval[0].contains("(+ x 1)"));
    }

    #[test]
    fn open_scheme_repl_creates_buffer() {
        let mut ed = Editor::new();
        ed.open_scheme_repl();
        assert!(ed.buffers.iter().any(|b| b.name == "*Scheme*"));
        assert_eq!(ed.active_buffer().name, "*Scheme*");
    }

    #[test]
    fn open_scheme_repl_reuses_existing() {
        let mut ed = Editor::new();
        ed.open_scheme_repl();
        let count = ed.buffers.len();
        ed.switch_to_buffer(0);
        ed.open_scheme_repl();
        assert_eq!(ed.buffers.len(), count);
    }

    #[test]
    fn append_to_scheme_repl_adds_text() {
        let mut ed = Editor::new();
        ed.append_to_scheme_repl("> (+ 1 2)\n; => 3\n");
        let buf = ed.buffers.iter().find(|b| b.name == "*Scheme*").unwrap();
        assert!(buf.text().contains("; => 3"));
    }
}
