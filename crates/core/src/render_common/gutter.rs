//! Shared gutter logic: line numbers, breakpoint markers, diagnostic markers.
//!
//! Backend-specific code handles the actual drawing; this module computes
//! gutter width, marker priority, and per-line diagnostic/breakpoint state.

use crate::{Buffer, DiagnosticSeverity, Editor};
use std::collections::{HashMap, HashSet};

/// Compute gutter width: enough digits for the line count, minimum 2, plus 1 for markers.
pub fn gutter_width(line_count: usize) -> usize {
    let digits = if line_count == 0 {
        1
    } else {
        (line_count as f64).log10().floor() as usize + 1
    };
    digits.max(2) + 1 // +1 for marker column
}

/// Gutter marker priority: Stopped > Breakpoint > Diagnostic > None.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GutterMarker {
    None,
    Diagnostic(DiagnosticSeverity),
    Breakpoint,
    Stopped,
}

impl GutterMarker {
    pub fn glyph_and_theme_key(self) -> Option<(char, &'static str)> {
        match self {
            GutterMarker::None => None,
            GutterMarker::Diagnostic(sev) => Some((sev.gutter_char(), sev.theme_key())),
            GutterMarker::Breakpoint => Some(('●', "debug.breakpoint")),
            GutterMarker::Stopped => Some(('▶', "debug.current_line")),
        }
    }
}

pub fn resolve_gutter_marker(
    is_stopped: bool,
    has_breakpoint: bool,
    diag_severity: Option<DiagnosticSeverity>,
) -> GutterMarker {
    if is_stopped {
        GutterMarker::Stopped
    } else if has_breakpoint {
        GutterMarker::Breakpoint
    } else if let Some(sev) = diag_severity {
        GutterMarker::Diagnostic(sev)
    } else {
        GutterMarker::None
    }
}

/// Format a line number string for the gutter.
pub fn format_line_number(
    line_idx: usize,
    cursor_row: usize,
    gutter_w: usize,
    show_line_numbers: bool,
    relative_line_numbers: bool,
) -> String {
    if !show_line_numbers {
        " ".to_string()
    } else if relative_line_numbers && line_idx != cursor_row {
        let offset = line_idx.abs_diff(cursor_row);
        format!("{:>width$}", offset, width = gutter_w - 1)
    } else {
        format!("{:>width$}", line_idx + 1, width = gutter_w - 1)
    }
}

/// Collect per-line diagnostic severities for a buffer.
pub fn collect_line_severities(buf: &Buffer, editor: &Editor) -> HashMap<u32, DiagnosticSeverity> {
    let mut map: HashMap<u32, DiagnosticSeverity> = HashMap::new();
    if let Some(path) = buf.file_path() {
        let uri = crate::path_to_uri(path);
        if let Some(diags) = editor.diagnostics.get(&uri) {
            for d in diags {
                let cur = map.get(&d.line).copied();
                if severity_higher(cur, Some(d.severity)) {
                    map.insert(d.line, d.severity);
                }
            }
        }
    }
    map
}

/// Collect breakpoint + stopped line for a buffer.
pub fn collect_breakpoints(buf: &Buffer, editor: &Editor) -> (HashSet<u32>, Option<u32>) {
    let mut bps = HashSet::new();
    let mut stopped = None;
    if let (Some(path), Some(state)) = (buf.file_path(), editor.debug_state.as_ref()) {
        let path_str = path.to_string_lossy();
        if let Some(list) = state.breakpoints.get(path_str.as_ref()) {
            for bp in list {
                if bp.line >= 1 {
                    bps.insert((bp.line - 1) as u32);
                }
            }
        }
        if let Some((src, line)) = &state.stopped_location {
            if src.as_str() == path_str.as_ref() && *line >= 1 {
                stopped = Some((*line - 1) as u32);
            }
        }
    }
    (bps, stopped)
}

/// Check if a line has been modified since the last save.
/// Returns `true` if the line index is in the buffer's changed_lines set.
pub fn is_line_changed(buf: &Buffer, line_idx: usize) -> bool {
    buf.changed_lines.contains(&line_idx)
}

fn severity_higher(cur: Option<DiagnosticSeverity>, new: Option<DiagnosticSeverity>) -> bool {
    fn rank(s: Option<DiagnosticSeverity>) -> u8 {
        match s {
            Some(DiagnosticSeverity::Error) => 4,
            Some(DiagnosticSeverity::Warning) => 3,
            Some(DiagnosticSeverity::Information) => 2,
            Some(DiagnosticSeverity::Hint) => 1,
            None => 0,
        }
    }
    rank(new) > rank(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gutter_width_minimum_is_three() {
        assert_eq!(gutter_width(0), 3);
        assert_eq!(gutter_width(1), 3);
        assert_eq!(gutter_width(99), 3);
    }

    #[test]
    fn gutter_width_scales_with_digits() {
        assert_eq!(gutter_width(100), 4);
        assert_eq!(gutter_width(999), 4);
        assert_eq!(gutter_width(1000), 5);
    }

    #[test]
    fn marker_priority_stopped_wins() {
        let m = resolve_gutter_marker(true, true, Some(DiagnosticSeverity::Error));
        assert_eq!(m, GutterMarker::Stopped);
    }

    #[test]
    fn marker_priority_breakpoint_beats_diagnostic() {
        let m = resolve_gutter_marker(false, true, Some(DiagnosticSeverity::Error));
        assert_eq!(m, GutterMarker::Breakpoint);
    }

    #[test]
    fn marker_priority_diagnostic_when_no_debug() {
        let m = resolve_gutter_marker(false, false, Some(DiagnosticSeverity::Warning));
        assert_eq!(m, GutterMarker::Diagnostic(DiagnosticSeverity::Warning));
    }

    #[test]
    fn marker_none_when_nothing() {
        let m = resolve_gutter_marker(false, false, None);
        assert_eq!(m, GutterMarker::None);
    }

    #[test]
    fn stopped_glyph() {
        let (ch, key) = GutterMarker::Stopped.glyph_and_theme_key().unwrap();
        assert_eq!(ch, '▶');
        assert_eq!(key, "debug.current_line");
    }

    #[test]
    fn breakpoint_glyph() {
        let (ch, key) = GutterMarker::Breakpoint.glyph_and_theme_key().unwrap();
        assert_eq!(ch, '●');
        assert_eq!(key, "debug.breakpoint");
    }

    #[test]
    fn none_marker_no_glyph() {
        assert!(GutterMarker::None.glyph_and_theme_key().is_none());
    }

    #[test]
    fn format_line_number_absolute() {
        assert_eq!(format_line_number(0, 0, 3, true, false), " 1");
        assert_eq!(format_line_number(9, 0, 3, true, false), "10");
    }

    #[test]
    fn format_line_number_relative() {
        assert_eq!(format_line_number(5, 5, 3, true, true), " 6");
        assert_eq!(format_line_number(3, 5, 3, true, true), " 2");
    }

    #[test]
    fn format_line_number_hidden() {
        assert_eq!(format_line_number(0, 0, 3, false, false), " ");
    }
}
