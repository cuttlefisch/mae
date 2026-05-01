//! Editor-side diagnostic store.
//!
//! Language servers push `textDocument/publishDiagnostics` notifications
//! for each file they analyze. We store them keyed by file URI so the
//! renderer can draw gutter markers, `]d`/`[d` can jump between them,
//! and the `*Diagnostics*` buffer can list them.
//!
//! The LSP contract: every publish replaces the prior set for that URI.
//! We never merge — a publish with an empty `diagnostics` array means
//! "all diagnostics for this file are resolved".

use std::collections::HashMap;

use crate::lsp_intent::path_to_uri;

use super::Editor;

/// Diagnostic severity, matching LSP but defined here so `mae-core` has no
/// dependency on `mae-lsp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl DiagnosticSeverity {
    /// Single-character gutter marker for this severity.
    pub fn gutter_char(&self) -> char {
        match self {
            DiagnosticSeverity::Error => 'E',
            DiagnosticSeverity::Warning => 'W',
            DiagnosticSeverity::Information => 'I',
            DiagnosticSeverity::Hint => 'H',
        }
    }

    /// Theme key for styling this severity. Matches the existing
    /// `diagnostic.*` convention in the bundled themes.
    pub fn theme_key(&self) -> &'static str {
        match self {
            DiagnosticSeverity::Error => "diagnostic.error",
            DiagnosticSeverity::Warning => "diagnostic.warn",
            DiagnosticSeverity::Information => "diagnostic.info",
            DiagnosticSeverity::Hint => "diagnostic.hint",
        }
    }
}

/// A single diagnostic at a range in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: u32,
    pub col_start: u32,
    pub col_end: u32,
    pub end_line: u32,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
}

impl Diagnostic {
    /// Render a single-line summary for the `*Diagnostics*` buffer or AI tool.
    /// Format: `<severity> <line>:<col> [<source>:<code>] <message>`
    pub fn format_summary(&self) -> String {
        let sev = match self.severity {
            DiagnosticSeverity::Error => "ERROR",
            DiagnosticSeverity::Warning => "WARN ",
            DiagnosticSeverity::Information => "INFO ",
            DiagnosticSeverity::Hint => "HINT ",
        };
        let tag = match (&self.source, &self.code) {
            (Some(s), Some(c)) => format!(" [{}:{}]", s, c),
            (Some(s), None) => format!(" [{}]", s),
            (None, Some(c)) => format!(" [{}]", c),
            (None, None) => String::new(),
        };
        // Collapse newlines so a single diagnostic stays on one line.
        let message = self.message.replace('\n', " ");
        format!(
            "{} {}:{}{} {}",
            sev,
            self.line + 1,
            self.col_start + 1,
            tag,
            message
        )
    }
}

/// Map from file URI → diagnostics for that file.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticStore {
    map: HashMap<String, Vec<Diagnostic>>,
}

impl DiagnosticStore {
    /// Replace the diagnostics for a single URI. Empty `diagnostics` clears
    /// the entry entirely so the file shows "clean" in the UI.
    /// Returns `true` if the stored diagnostics actually changed.
    pub fn set(&mut self, uri: String, diagnostics: Vec<Diagnostic>) -> bool {
        if diagnostics.is_empty() {
            return self.map.remove(&uri).is_some();
        }
        // Check if identical to avoid unnecessary redraws.
        if let Some(existing) = self.map.get(&uri) {
            if existing.len() == diagnostics.len()
                && existing.iter().zip(&diagnostics).all(|(a, b)| {
                    a.line == b.line && a.message == b.message && a.severity == b.severity
                })
            {
                return false;
            }
        }
        self.map.insert(uri, diagnostics);
        true
    }

    pub fn get(&self, uri: &str) -> Option<&Vec<Diagnostic>> {
        self.map.get(uri)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Vec<Diagnostic>)> {
        self.map.iter()
    }

    /// Total diagnostic count across all files.
    pub fn total(&self) -> usize {
        self.map.values().map(|v| v.len()).sum()
    }

    /// Count per severity across all files: (errors, warnings, infos, hints).
    pub fn severity_counts(&self) -> (usize, usize, usize, usize) {
        let mut e = 0;
        let mut w = 0;
        let mut i = 0;
        let mut h = 0;
        for diags in self.map.values() {
            for d in diags {
                match d.severity {
                    DiagnosticSeverity::Error => e += 1,
                    DiagnosticSeverity::Warning => w += 1,
                    DiagnosticSeverity::Information => i += 1,
                    DiagnosticSeverity::Hint => h += 1,
                }
            }
        }
        (e, w, i, h)
    }

    /// Count diagnostics for a single file URI.
    pub fn count_for(&self, uri: &str) -> usize {
        self.map.get(uri).map(|v| v.len()).unwrap_or(0)
    }
}

impl Editor {
    /// Return the diagnostics for the active buffer's file, if any.
    pub fn active_buffer_diagnostics(&self) -> Option<&Vec<Diagnostic>> {
        let buf = self.active_buffer();
        let path = buf.file_path()?;
        let uri = path_to_uri(path);
        self.diagnostics.get(&uri)
    }

    /// Jump the cursor to the next diagnostic in the active buffer,
    /// wrapping to the first diagnostic when past the last one.
    /// Sets a status message describing the diagnostic (or "no diagnostics").
    pub fn jump_next_diagnostic(&mut self) {
        let Some(diags) = self.active_buffer_diagnostics().cloned() else {
            self.set_status("[LSP] no diagnostics for this buffer");
            return;
        };
        if diags.is_empty() {
            self.set_status("[LSP] no diagnostics for this buffer");
            return;
        }
        let win = self.window_mgr.focused_window();
        let cur_row = win.cursor_row as u32;
        let cur_col = win.cursor_col as u32;
        // Sort by (line, col) so "next" is well-defined.
        let mut sorted = diags;
        sorted.sort_by_key(|d| (d.line, d.col_start));
        let target = sorted
            .iter()
            .find(|d| d.line > cur_row || (d.line == cur_row && d.col_start > cur_col))
            .cloned()
            .or_else(|| sorted.first().cloned());
        if let Some(d) = target {
            self.jump_to_diagnostic(&d);
        }
    }

    /// Jump to the previous diagnostic, wrapping to the last one if above
    /// the first.
    pub fn jump_prev_diagnostic(&mut self) {
        let Some(diags) = self.active_buffer_diagnostics().cloned() else {
            self.set_status("[LSP] no diagnostics for this buffer");
            return;
        };
        if diags.is_empty() {
            self.set_status("[LSP] no diagnostics for this buffer");
            return;
        }
        let win = self.window_mgr.focused_window();
        let cur_row = win.cursor_row as u32;
        let cur_col = win.cursor_col as u32;
        let mut sorted = diags;
        sorted.sort_by_key(|d| (d.line, d.col_start));
        let target = sorted
            .iter()
            .rev()
            .find(|d| d.line < cur_row || (d.line == cur_row && d.col_start < cur_col))
            .cloned()
            .or_else(|| sorted.last().cloned());
        if let Some(d) = target {
            self.jump_to_diagnostic(&d);
        }
    }

    fn jump_to_diagnostic(&mut self, d: &Diagnostic) {
        let idx = self.active_buffer_idx();
        let line_count = self.buffers[idx].display_line_count();
        let target_row = (d.line as usize).min(line_count.saturating_sub(1));
        let target_col = d.col_start as usize;
        let vh = self.viewport_height;
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = target_row;
        win.cursor_col = target_col;
        win.clamp_cursor(&self.buffers[idx]);
        win.scroll_center(vh);
        self.set_status(d.format_summary());
    }

    /// Render every diagnostic (across all files) into a `*Diagnostics*`
    /// buffer, grouped by file URI. If the buffer already exists it is
    /// refreshed in place. Focus is moved to the buffer.
    ///
    /// Format (one diagnostic per line):
    ///   `<file>:<line>:<col>: <SEVERITY> [<source>:<code>] <message>`
    ///
    /// This is a snapshot; the user re-runs `:diagnostics` (or presses the
    /// bound key) to refresh.
    pub fn show_diagnostics_buffer(&mut self) {
        let total = self.diagnostics.total();
        let (e, w, i, h) = self.diagnostics.severity_counts();
        let mut body = String::new();
        body.push_str(&format!(
            "*Diagnostics*  {} total  ({}E {}W {}I {}H)\n\n",
            total, e, w, i, h
        ));

        if total == 0 {
            body.push_str("No diagnostics.\n");
        } else {
            // Sort files for stable display.
            let mut entries: Vec<(&String, &Vec<Diagnostic>)> = self.diagnostics.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (uri, diags) in entries {
                // Strip file:// prefix for readability; show URI otherwise.
                let display = uri.strip_prefix("file://").unwrap_or(uri);
                body.push_str(&format!("{}\n", display));
                let mut sorted = diags.clone();
                sorted.sort_by_key(|d| (d.line, d.col_start));
                for d in &sorted {
                    body.push_str(&format!(
                        "  {}:{}:{} {}\n",
                        display,
                        d.line + 1,
                        d.col_start + 1,
                        d.format_summary(),
                    ));
                }
                body.push('\n');
            }
        }

        // Reuse an existing *Diagnostics* buffer or create one.
        let existing = self.buffers.iter().position(|b| b.name == "*Diagnostics*");
        let idx = if let Some(i) = existing {
            self.buffers[i].replace_contents(&body);
            i
        } else {
            let mut buf = crate::buffer::Buffer::new();
            buf.replace_contents(&body);
            buf.name = "*Diagnostics*".into();
            self.buffers.push(buf);
            self.buffers.len() - 1
        };
        self.window_mgr.focused_window_mut().buffer_idx = idx;
        self.set_status(format!(
            "Diagnostics: {} total ({}E {}W {}I {}H)",
            total, e, w, i, h
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use std::path::PathBuf;

    fn diag(line: u32, col: u32, sev: DiagnosticSeverity, msg: &str) -> Diagnostic {
        Diagnostic {
            line,
            col_start: col,
            col_end: col + 1,
            end_line: line,
            severity: sev,
            message: msg.into(),
            source: None,
            code: None,
        }
    }

    fn editor_with_file(path: &str, text: &str) -> Editor {
        let mut buf = Buffer::new();
        buf.set_file_path(PathBuf::from(path));
        if !text.is_empty() {
            buf.insert_text_at(0, text);
        }
        Editor::with_buffer(buf)
    }

    #[test]
    fn store_set_and_get() {
        let mut store = DiagnosticStore::default();
        store.set(
            "file:///a.rs".into(),
            vec![diag(0, 0, DiagnosticSeverity::Error, "bad")],
        );
        assert_eq!(store.total(), 1);
        assert_eq!(store.count_for("file:///a.rs"), 1);
    }

    #[test]
    fn store_set_empty_clears() {
        let mut store = DiagnosticStore::default();
        store.set(
            "file:///a.rs".into(),
            vec![diag(0, 0, DiagnosticSeverity::Error, "bad")],
        );
        assert!(store.set("file:///a.rs".into(), vec![]));
        assert_eq!(store.total(), 0);
        assert!(store.get("file:///a.rs").is_none());
    }

    /// Regression: identical diagnostics should not trigger a redraw.
    /// LSP servers republish unchanged diagnostics frequently.
    #[test]
    fn store_set_returns_changed() {
        let mut store = DiagnosticStore::default();
        let d = vec![diag(0, 0, DiagnosticSeverity::Error, "bad")];
        // First set: changed
        assert!(store.set("file:///a.rs".into(), d.clone()));
        // Same diagnostics: not changed
        assert!(!store.set("file:///a.rs".into(), d.clone()));
        // Different message: changed
        let d2 = vec![diag(0, 0, DiagnosticSeverity::Error, "worse")];
        assert!(store.set("file:///a.rs".into(), d2));
        // Clear non-existent: not changed
        assert!(!store.set("file:///b.rs".into(), vec![]));
    }

    #[test]
    fn store_severity_counts() {
        let mut store = DiagnosticStore::default();
        store.set(
            "file:///a.rs".into(),
            vec![
                diag(0, 0, DiagnosticSeverity::Error, "e"),
                diag(1, 0, DiagnosticSeverity::Warning, "w"),
                diag(2, 0, DiagnosticSeverity::Warning, "w2"),
                diag(3, 0, DiagnosticSeverity::Hint, "h"),
            ],
        );
        assert_eq!(store.severity_counts(), (1, 2, 0, 1));
    }

    #[test]
    fn format_summary_one_line() {
        let d = Diagnostic {
            line: 2,
            col_start: 4,
            col_end: 10,
            end_line: 2,
            severity: DiagnosticSeverity::Error,
            message: "unresolved import\nnext line".into(),
            source: Some("rustc".into()),
            code: Some("E0432".into()),
        };
        let s = d.format_summary();
        assert!(!s.contains('\n'));
        assert!(s.contains("ERROR"));
        assert!(s.contains("3:5")); // 1-indexed
        assert!(s.contains("[rustc:E0432]"));
        assert!(s.contains("unresolved import"));
    }

    #[test]
    fn severity_gutter_chars() {
        assert_eq!(DiagnosticSeverity::Error.gutter_char(), 'E');
        assert_eq!(DiagnosticSeverity::Warning.gutter_char(), 'W');
        assert_eq!(DiagnosticSeverity::Information.gutter_char(), 'I');
        assert_eq!(DiagnosticSeverity::Hint.gutter_char(), 'H');
    }

    #[test]
    fn active_buffer_diagnostics_finds_match() {
        let mut ed = editor_with_file("/tmp/a.rs", "fn main() {}\n");
        ed.diagnostics.set(
            "file:///tmp/a.rs".into(),
            vec![diag(0, 0, DiagnosticSeverity::Error, "bad")],
        );
        assert_eq!(ed.active_buffer_diagnostics().unwrap().len(), 1);
    }

    #[test]
    fn active_buffer_diagnostics_returns_none_without_file() {
        let ed = Editor::new();
        assert!(ed.active_buffer_diagnostics().is_none());
    }

    #[test]
    fn jump_next_no_diagnostics_sets_status() {
        let mut ed = editor_with_file("/tmp/a.rs", "fn main() {}\n");
        ed.jump_next_diagnostic();
        assert!(ed.status_msg.contains("no diagnostics"));
    }

    #[test]
    fn jump_next_moves_forward() {
        let mut ed = editor_with_file("/tmp/a.rs", "line0\nline1\nline2\nline3\n");
        ed.diagnostics.set(
            "file:///tmp/a.rs".into(),
            vec![
                diag(1, 0, DiagnosticSeverity::Error, "d1"),
                diag(3, 2, DiagnosticSeverity::Warning, "d2"),
            ],
        );
        // Cursor starts at 0,0 — should jump to line 1.
        ed.jump_next_diagnostic();
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 1);
        // Jump again → line 3.
        ed.jump_next_diagnostic();
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 3);
        // One more → wraps back to first diagnostic.
        ed.jump_next_diagnostic();
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 1);
    }

    #[test]
    fn jump_prev_moves_backward() {
        let mut ed = editor_with_file("/tmp/a.rs", "line0\nline1\nline2\nline3\n");
        ed.diagnostics.set(
            "file:///tmp/a.rs".into(),
            vec![
                diag(1, 0, DiagnosticSeverity::Error, "d1"),
                diag(3, 2, DiagnosticSeverity::Warning, "d2"),
            ],
        );
        // Move cursor to end.
        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_row = 3;
            win.cursor_col = 4;
        }
        ed.jump_prev_diagnostic();
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 3);
        assert_eq!(ed.window_mgr.focused_window().cursor_col, 2);
        ed.jump_prev_diagnostic();
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 1);
        // Wraps to last.
        ed.jump_prev_diagnostic();
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 3);
    }

    #[test]
    fn show_diagnostics_buffer_empty() {
        let mut ed = Editor::new();
        ed.show_diagnostics_buffer();
        let buf = ed.active_buffer();
        assert_eq!(buf.name, "*Diagnostics*");
        let text = buf.text();
        assert!(text.contains("*Diagnostics*"));
        assert!(text.contains("No diagnostics"));
    }

    #[test]
    fn show_diagnostics_buffer_lists_entries() {
        let mut ed = editor_with_file("/tmp/a.rs", "fn main() {}\n");
        ed.diagnostics.set(
            "file:///tmp/a.rs".into(),
            vec![
                diag(0, 0, DiagnosticSeverity::Error, "bad"),
                diag(2, 3, DiagnosticSeverity::Warning, "meh"),
            ],
        );
        ed.diagnostics.set(
            "file:///tmp/b.rs".into(),
            vec![diag(5, 0, DiagnosticSeverity::Hint, "consider")],
        );
        ed.show_diagnostics_buffer();
        let buf = ed.active_buffer();
        assert_eq!(buf.name, "*Diagnostics*");
        let text = buf.text();
        assert!(text.contains("/tmp/a.rs"));
        assert!(text.contains("/tmp/b.rs"));
        assert!(text.contains("ERROR"));
        assert!(text.contains("WARN"));
        assert!(text.contains("HINT"));
        assert!(text.contains("bad"));
        assert!(text.contains("meh"));
        assert!(text.contains("consider"));
    }

    #[test]
    fn show_diagnostics_buffer_refreshes_existing() {
        let mut ed = Editor::new();
        ed.show_diagnostics_buffer();
        let first_len = ed.buffers.len();
        // Populate and refresh — must reuse the same buffer.
        ed.diagnostics.set(
            "file:///tmp/a.rs".into(),
            vec![diag(0, 0, DiagnosticSeverity::Error, "bad")],
        );
        ed.show_diagnostics_buffer();
        assert_eq!(ed.buffers.len(), first_len);
        assert!(ed.active_buffer().text().contains("bad"));
    }
}
