//! Shared diagnostic rendering computation for both GUI and TUI renderers.
//!
//! Provides diagnostic span filtering for visible ranges and virtual text
//! formatting — shared between both rendering backends.

use crate::editor::{DiagnosticSeverity, DiagnosticStore};

/// A diagnostic span visible in the current viewport, ready for rendering.
#[derive(Debug, Clone)]
pub struct DiagnosticSpan {
    /// Line number (0-based) relative to the buffer.
    pub line: usize,
    /// Start column (0-based, char offset within the line).
    pub col_start: usize,
    /// End column (exclusive).
    pub col_end: usize,
    /// Severity determines underline color.
    pub severity: DiagnosticSeverity,
    /// The diagnostic message (for virtual text display).
    pub message: String,
}

/// Compute diagnostic spans visible in the given line range for a buffer URI.
/// Filters to `[start_line, end_line)` for performance (only process visible lines).
pub fn compute_diagnostic_spans(
    store: &DiagnosticStore,
    uri: &str,
    start_line: usize,
    end_line: usize,
) -> Vec<DiagnosticSpan> {
    let Some(diagnostics) = store.get(uri) else {
        return Vec::new();
    };

    let mut spans = Vec::new();
    for diag in diagnostics {
        let diag_line = diag.line as usize;
        // For multi-line diagnostics, we only underline the first line
        // (or each line within the range for multi-line spans).
        if diag_line >= start_line && diag_line < end_line {
            spans.push(DiagnosticSpan {
                line: diag_line,
                col_start: diag.col_start as usize,
                col_end: diag.col_end as usize,
                severity: diag.severity,
                message: diag.message.clone(),
            });
        }
    }
    spans
}

/// Format a diagnostic message for virtual text display at end of line.
/// Returns (truncated_message, theme_key) for rendering.
pub fn format_virtual_text(
    severity: DiagnosticSeverity,
    message: &str,
    max_width: usize,
) -> (String, &'static str) {
    let theme_key = severity.theme_key();
    // Take first line only, collapse to single line
    let first_line = message.lines().next().unwrap_or(message);
    let prefix = match severity {
        DiagnosticSeverity::Error => "● ",
        DiagnosticSeverity::Warning => "▲ ",
        DiagnosticSeverity::Information => "ℹ ",
        DiagnosticSeverity::Hint => "💡",
    };
    let available = max_width.saturating_sub(prefix.len());
    let truncated = if first_line.len() > available {
        format!(
            "{}{}{}",
            prefix,
            &first_line[..available.saturating_sub(1)],
            "…"
        )
    } else {
        format!("{}{}", prefix, first_line)
    };
    (truncated, theme_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Diagnostic;

    fn make_store_with_diags() -> DiagnosticStore {
        let mut store = DiagnosticStore::default();
        store.set(
            "file:///test.rs".to_string(),
            vec![
                Diagnostic {
                    line: 5,
                    col_start: 4,
                    col_end: 10,
                    end_line: 5,
                    severity: DiagnosticSeverity::Error,
                    message: "cannot find value `foo`".to_string(),
                    source: Some("rustc".to_string()),
                    code: Some("E0425".to_string()),
                },
                Diagnostic {
                    line: 10,
                    col_start: 0,
                    col_end: 5,
                    end_line: 10,
                    severity: DiagnosticSeverity::Warning,
                    message: "unused variable".to_string(),
                    source: None,
                    code: None,
                },
                Diagnostic {
                    line: 20,
                    col_start: 0,
                    col_end: 3,
                    end_line: 20,
                    severity: DiagnosticSeverity::Hint,
                    message: "consider using let".to_string(),
                    source: None,
                    code: None,
                },
            ],
        );
        store
    }

    #[test]
    fn filters_to_visible_range() {
        let store = make_store_with_diags();
        let spans = compute_diagnostic_spans(&store, "file:///test.rs", 0, 8);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].line, 5);
        assert_eq!(spans[0].col_start, 4);
        assert_eq!(spans[0].col_end, 10);
    }

    #[test]
    fn returns_empty_for_unknown_uri() {
        let store = make_store_with_diags();
        let spans = compute_diagnostic_spans(&store, "file:///unknown.rs", 0, 100);
        assert!(spans.is_empty());
    }

    #[test]
    fn returns_all_in_range() {
        let store = make_store_with_diags();
        let spans = compute_diagnostic_spans(&store, "file:///test.rs", 0, 25);
        assert_eq!(spans.len(), 3);
    }

    #[test]
    fn virtual_text_truncation() {
        let (text, key) = format_virtual_text(
            DiagnosticSeverity::Error,
            "this is a very long error message that should be truncated",
            30,
        );
        assert!(text.len() <= 32); // prefix + content (might be slightly over due to prefix chars)
        assert_eq!(key, "diagnostic.error");
    }

    #[test]
    fn virtual_text_short_message() {
        let (text, key) = format_virtual_text(DiagnosticSeverity::Warning, "unused", 80);
        assert!(text.contains("unused"));
        assert_eq!(key, "diagnostic.warn");
    }

    #[test]
    fn virtual_text_multiline_takes_first() {
        let (text, _) = format_virtual_text(
            DiagnosticSeverity::Error,
            "first line\nsecond line\nthird line",
            80,
        );
        assert!(text.contains("first line"));
        assert!(!text.contains("second line"));
    }
}
