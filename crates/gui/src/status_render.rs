//! Status bar and command line rendering for the GUI backend.

use mae_core::{Editor, Mode, SearchDirection, VisualType};
use skia_safe::Color4f;

use crate::canvas::SkiaCanvas;
use crate::theme;

/// Render the full status bar at the given screen row.
///
/// `frame_ms` is the time between the previous two frames (for FPS overlay
/// when `editor.show_fps` is enabled).
pub fn render_status_bar(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    row: usize,
    cols: usize,
    frame_ms: Option<u64>,
) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let recording_label: String;
    let mode_str = if editor.input_lock != mae_core::InputLock::None {
        // Override mode label when input is locked by AI/MCP operations.
        match editor.input_lock {
            mae_core::InputLock::AiBusy => {
                if editor.ai_streaming {
                    " AI... "
                } else {
                    " AI BUSY "
                }
            }
            mae_core::InputLock::McpBusy => " MCP... ",
            mae_core::InputLock::None => unreachable!(),
        }
    } else if editor.macro_recording {
        recording_label = format!(" REC @{} ", editor.macro_register.unwrap_or('?'));
        recording_label.as_str()
    } else {
        match editor.mode {
            Mode::Normal => " NORMAL ",
            Mode::Insert => " INSERT ",
            Mode::Visual(VisualType::Char) => " VISUAL ",
            Mode::Visual(VisualType::Line) => " V-LINE ",
            Mode::Command => " COMMAND ",
            Mode::ConversationInput => " AI INPUT ",
            Mode::Search => " SEARCH ",
            Mode::FilePicker => " FIND FILE ",
            Mode::FileBrowser => " BROWSE ",
            Mode::CommandPalette => " COMMAND PALETTE ",
            Mode::ShellInsert => " TERMINAL ",
        }
    };

    // Mode label colors — use locked theme when input is locked.
    let mode_style = if editor.input_lock != mae_core::InputLock::None {
        let key = match editor.input_lock {
            mae_core::InputLock::AiBusy => "ui.statusline.mode.locked",
            mae_core::InputLock::McpBusy => "ui.statusline.mode.mcp",
            mae_core::InputLock::None => "ui.statusline.mode.normal",
        };
        editor.theme.style(key)
    } else {
        editor.theme.style(match editor.mode {
            Mode::Normal => "ui.statusline.mode.normal",
            Mode::Insert => "ui.statusline.mode.insert",
            Mode::Visual(_) => "ui.statusline.mode.normal",
            Mode::Command => "ui.statusline.mode.command",
            Mode::ConversationInput => "ui.statusline.mode.conversation",
            Mode::ShellInsert => "ui.statusline.mode.insert",
            Mode::Search | Mode::FilePicker | Mode::FileBrowser | Mode::CommandPalette => {
                "ui.statusline.mode.command"
            }
        })
    };
    let mode_fg = theme::color_or(mode_style.fg, theme::DEFAULT_FG);
    let mode_bg = theme::color_or(mode_style.bg, theme::STATUS_BG);

    // Status bar background.
    let sl_style = editor.theme.style("ui.statusline");
    let (sl_fg, sl_bg) = if editor.bell_active() {
        // Visual bell: invert.
        (
            theme::color_or(sl_style.bg, Color4f::new(0.0, 0.0, 0.0, 1.0)),
            theme::color_or(sl_style.fg, Color4f::new(1.0, 1.0, 1.0, 1.0)),
        )
    } else {
        (
            theme::color_or(sl_style.fg, theme::DEFAULT_FG),
            theme::color_or(sl_style.bg, theme::STATUS_BG),
        )
    };

    // Fill full status bar background.
    canvas.draw_rect_fill(row, 0, cols, 1, sl_bg);

    // Mode label with its own bg.
    let mode_len = mode_str.len();
    canvas.draw_rect_fill(row, 0, mode_len, 1, mode_bg);
    canvas.draw_text_at(row, 0, mode_str, mode_fg);

    // File info.
    let modified = if buf.modified { " [+]" } else { "" };
    let file_info = format!(" {}{}", buf.name, modified);
    canvas.draw_text_at(row, mode_len, &file_info, sl_fg);

    // Right-aligned: AI info + cursor position.
    let position = format!(" {}:{} ", win.cursor_row + 1, win.cursor_col + 1);

    let ai_info = if editor.ai_session_tokens_in == 0 && editor.ai_session_tokens_out == 0 {
        String::new()
    } else {
        let tokens = format!(
            "{}/{}",
            format_tokens(editor.ai_session_tokens_in),
            format_tokens(editor.ai_session_tokens_out),
        );
        if editor.ai_session_cost_usd > 0.0 {
            format!(" ${:.2} {} ", editor.ai_session_cost_usd, tokens)
        } else {
            format!(" {} ", tokens)
        }
    };

    let fps_info = if editor.show_fps {
        match frame_ms {
            Some(ms) => format!(" {}ms ", ms),
            None => String::new(),
        }
    } else {
        String::new()
    };

    let right_text = format!("{}{}{}", fps_info, ai_info, position);
    let right_col = cols.saturating_sub(right_text.len());
    canvas.draw_text_at(row, right_col, &right_text, sl_fg);
}

fn format_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// Render the command/message line at the given screen row.
pub fn render_command_line(canvas: &mut SkiaCanvas, editor: &Editor, row: usize, cols: usize) {
    let text = if editor.mode == Mode::Command {
        format!(":{}", editor.command_line)
    } else if editor.mode == Mode::Search {
        let prompt = if editor.search_state.direction == SearchDirection::Forward {
            "/"
        } else {
            "?"
        };
        format!("{}{}", prompt, editor.search_input)
    } else if let Some(count) = editor.count_prefix {
        format!("{}", count)
    } else {
        editor.status_msg.clone()
    };

    let fg = theme::ts_fg(editor, "ui.commandline");
    // Clear the row first.
    let bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);
    canvas.draw_rect_fill(row, 0, cols, 1, bg);
    canvas.draw_text_at(row, 0, &text, fg);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tokens_small() {
        assert_eq!(format_tokens(500), "500");
    }

    #[test]
    fn format_tokens_thousands() {
        assert_eq!(format_tokens(1500), "1.5k");
    }

    #[test]
    fn format_tokens_millions() {
        assert_eq!(format_tokens(1_500_000), "1.5M");
    }
}
