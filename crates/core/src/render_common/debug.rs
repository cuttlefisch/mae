//! Shared debug panel rendering logic.
//!
//! Pure data computation used by both GUI and TUI debug renderers.

use crate::debug::DebugTarget;
use crate::DebugLineItem;
use crate::Editor;

/// Build the debug window title string from the current debug state.
pub fn debug_title(editor: &Editor) -> String {
    match &editor.debug_state {
        Some(state) => match &state.target {
            DebugTarget::Dap {
                adapter_name,
                program,
            } => {
                let short = program.rsplit('/').next().unwrap_or(program);
                format!(" *Debug* [{}: {}] ", adapter_name, short)
            }
            DebugTarget::SelfDebug => " *Debug* [self] ".to_string(),
        },
        None => " *Debug* ".to_string(),
    }
}

/// Semantic style category for a debug line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugLineStyle {
    Default,
    SectionHeader,
    ActiveThread,
    InactiveThread,
    ActiveFrame,
    InactiveFrame,
    Variable,
    Output,
}

/// Determine the semantic style for a debug line item.
///
/// `active_thread_id` comes from `editor.debug_state`.
/// `selected_frame_id` comes from the buffer's `DebugView`.
pub fn debug_line_style(
    item: Option<&DebugLineItem>,
    active_thread_id: Option<i64>,
    selected_frame_id: Option<i64>,
) -> DebugLineStyle {
    match item {
        Some(DebugLineItem::SectionHeader(_)) => DebugLineStyle::SectionHeader,
        Some(DebugLineItem::Thread(tid)) => {
            if active_thread_id == Some(*tid) {
                DebugLineStyle::ActiveThread
            } else {
                DebugLineStyle::InactiveThread
            }
        }
        Some(DebugLineItem::Frame(fid)) => {
            if selected_frame_id == Some(*fid) {
                DebugLineStyle::ActiveFrame
            } else {
                DebugLineStyle::InactiveFrame
            }
        }
        Some(DebugLineItem::Variable { .. }) => DebugLineStyle::Variable,
        Some(DebugLineItem::OutputLine(_)) => DebugLineStyle::Output,
        Some(DebugLineItem::Blank) | None => DebugLineStyle::Default,
    }
}

/// Map a `DebugLineStyle` to the theme key for its foreground color.
pub fn debug_style_theme_key(style: DebugLineStyle) -> &'static str {
    match style {
        DebugLineStyle::Default
        | DebugLineStyle::InactiveThread
        | DebugLineStyle::InactiveFrame => "ui.text",
        DebugLineStyle::SectionHeader
        | DebugLineStyle::ActiveThread
        | DebugLineStyle::ActiveFrame => "markup.heading",
        DebugLineStyle::Variable => "variable",
        DebugLineStyle::Output => "comment",
    }
}

/// Compute scroll offset to keep `cursor_idx` visible within `visible_height` lines.
pub fn debug_scroll_offset(cursor_idx: usize, visible_height: usize) -> usize {
    if cursor_idx >= visible_height {
        cursor_idx - visible_height + 1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_title_no_state() {
        let editor = Editor::new();
        assert_eq!(debug_title(&editor), " *Debug* ");
    }

    #[test]
    fn section_header_style() {
        let item = DebugLineItem::SectionHeader("Threads".to_string());
        assert_eq!(
            debug_line_style(Some(&item), None, None),
            DebugLineStyle::SectionHeader
        );
        assert_eq!(
            debug_style_theme_key(DebugLineStyle::SectionHeader),
            "markup.heading"
        );
    }

    #[test]
    fn variable_style() {
        let item = DebugLineItem::Variable {
            scope: "Locals".to_string(),
            name: "x".to_string(),
            depth: 0,
            variables_reference: 42,
        };
        assert_eq!(
            debug_line_style(Some(&item), None, None),
            DebugLineStyle::Variable
        );
        assert_eq!(debug_style_theme_key(DebugLineStyle::Variable), "variable");
    }

    #[test]
    fn scroll_offset_calculation() {
        assert_eq!(debug_scroll_offset(5, 20), 0);
        assert_eq!(debug_scroll_offset(25, 20), 6);
    }

    #[test]
    fn output_line_style() {
        let item = DebugLineItem::OutputLine(0);
        assert_eq!(
            debug_line_style(Some(&item), None, None),
            DebugLineStyle::Output
        );
    }

    #[test]
    fn active_thread_style() {
        let item = DebugLineItem::Thread(5);
        assert_eq!(
            debug_line_style(Some(&item), Some(5), None),
            DebugLineStyle::ActiveThread
        );
        assert_eq!(
            debug_line_style(Some(&item), Some(3), None),
            DebugLineStyle::InactiveThread
        );
    }

    #[test]
    fn active_frame_style() {
        let item = DebugLineItem::Frame(10);
        assert_eq!(
            debug_line_style(Some(&item), None, Some(10)),
            DebugLineStyle::ActiveFrame
        );
        assert_eq!(
            debug_line_style(Some(&item), None, Some(7)),
            DebugLineStyle::InactiveFrame
        );
    }
}
