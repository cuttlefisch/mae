//! Gutter rendering: line numbers, breakpoint markers, diagnostic markers.

use mae_core::{DiagnosticSeverity, Editor};
use std::collections::{HashMap, HashSet};

use crate::canvas::SkiaCanvas;
use crate::theme;

/// Compute gutter width (same logic as terminal renderer).
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

/// Render the gutter for one visible line at a pixel Y position.
/// `line_height` is the pixel height of this line (for cursorline bg).
/// `scale` is the font scale (for scaled line numbers on headings).
pub fn render_gutter_line_at_y(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    pixel_y: f32,
    screen_col_offset: usize,
    line_idx: usize,
    gutter_w: usize,
    cursor_row: usize,
    is_cursor_line: bool,
    line_height: f32,
    scale: f32,
    breakpoint_lines: &HashSet<u32>,
    stopped_line: Option<u32>,
    line_severities: &HashMap<u32, DiagnosticSeverity>,
) {
    let gutter_fg = theme::ts_fg(editor, "ui.gutter");
    let cursorline_bg = if is_cursor_line {
        theme::ts_bg(editor, "ui.cursorline")
    } else {
        None
    };

    // Background for cursorline gutter (pixel-precise height).
    if let Some(bg) = cursorline_bg {
        canvas.draw_rect_at_y(pixel_y, screen_col_offset, gutter_w, line_height, bg);
    }

    // Line number (scaled to match heading text).
    let line_num = if !editor.show_line_numbers {
        " ".to_string()
    } else if editor.relative_line_numbers && line_idx != cursor_row {
        let offset = line_idx.abs_diff(cursor_row);
        format!("{:>width$}", offset, width = gutter_w - 1)
    } else {
        format!("{:>width$}", line_idx + 1, width = gutter_w - 1)
    };
    canvas.draw_text_at_y(pixel_y, screen_col_offset, &line_num, gutter_fg, scale);

    // Marker column (last char of gutter).
    let line_idx_u32 = line_idx as u32;
    let marker = resolve_gutter_marker(
        stopped_line == Some(line_idx_u32),
        breakpoint_lines.contains(&line_idx_u32),
        line_severities.get(&line_idx_u32).copied(),
    );
    if let Some((ch, key)) = marker.glyph_and_theme_key() {
        let marker_fg = theme::ts_fg(editor, key);
        canvas.draw_char_at_y(
            pixel_y,
            screen_col_offset + gutter_w - 1,
            ch,
            marker_fg,
            false,
            false,
            scale,
        );
    }
}

/// Render the gutter for one visible line (cell-based, for non-buffer contexts).
#[allow(dead_code)]
pub fn render_gutter_line(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    screen_row: usize,
    screen_col_offset: usize,
    line_idx: usize,
    gutter_w: usize,
    cursor_row: usize,
    is_cursor_line: bool,
    breakpoint_lines: &HashSet<u32>,
    stopped_line: Option<u32>,
    line_severities: &HashMap<u32, DiagnosticSeverity>,
) {
    let gutter_fg = theme::ts_fg(editor, "ui.gutter");
    let cursorline_bg = if is_cursor_line {
        theme::ts_bg(editor, "ui.cursorline")
    } else {
        None
    };

    // Background for cursorline gutter.
    if let Some(bg) = cursorline_bg {
        canvas.draw_rect_fill(screen_row, screen_col_offset, gutter_w, 1, bg);
    }

    // Line number.
    let line_num = if !editor.show_line_numbers {
        " ".to_string()
    } else if editor.relative_line_numbers && line_idx != cursor_row {
        let offset = line_idx.abs_diff(cursor_row);
        format!("{:>width$}", offset, width = gutter_w - 1)
    } else {
        format!("{:>width$}", line_idx + 1, width = gutter_w - 1)
    };
    canvas.draw_text_at(screen_row, screen_col_offset, &line_num, gutter_fg);

    // Marker column (last char of gutter).
    let line_idx_u32 = line_idx as u32;
    let marker = resolve_gutter_marker(
        stopped_line == Some(line_idx_u32),
        breakpoint_lines.contains(&line_idx_u32),
        line_severities.get(&line_idx_u32).copied(),
    );
    if let Some((ch, key)) = marker.glyph_and_theme_key() {
        let marker_fg = theme::ts_fg(editor, key);
        canvas.draw_text_at(
            screen_row,
            screen_col_offset + gutter_w - 1,
            &ch.to_string(),
            marker_fg,
        );
    }
}

/// Collect per-line diagnostic severities for a buffer.
pub fn collect_line_severities(
    buf: &mae_core::Buffer,
    editor: &Editor,
) -> HashMap<u32, DiagnosticSeverity> {
    let mut map: HashMap<u32, DiagnosticSeverity> = HashMap::new();
    if let Some(path) = buf.file_path() {
        let uri = mae_core::path_to_uri(path);
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
pub fn collect_breakpoints(buf: &mae_core::Buffer, editor: &Editor) -> (HashSet<u32>, Option<u32>) {
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
}
