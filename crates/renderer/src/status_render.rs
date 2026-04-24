//! Status bar and command line rendering.

use mae_core::{Editor, Mode, VisualType};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::theme_convert::ts;

pub(crate) fn render_status_bar(frame: &mut Frame, area: Rect, editor: &Editor) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let recording_label: String;
    let mode_str = if editor.input_lock != mae_core::InputLock::None {
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
            Mode::GitStatus => " GIT STATUS ",
        }
    };
    let mode_style = if editor.input_lock != mae_core::InputLock::None {
        let key = match editor.input_lock {
            mae_core::InputLock::AiBusy => "ui.statusline.mode.locked",
            mae_core::InputLock::McpBusy => "ui.statusline.mode.mcp",
            mae_core::InputLock::None => "ui.statusline.mode.normal",
        };
        ts(editor, key)
    } else {
        match editor.mode {
            Mode::Normal => ts(editor, "ui.statusline.mode.normal"),
            Mode::Insert => ts(editor, "ui.statusline.mode.insert"),
            Mode::Visual(_) => ts(editor, "ui.statusline.mode.normal"),
            Mode::Command => ts(editor, "ui.statusline.mode.command"),
            Mode::ConversationInput => ts(editor, "ui.statusline.mode.conversation"),
            Mode::ShellInsert => ts(editor, "ui.statusline.mode.insert"),
            Mode::GitStatus => ts(editor, "ui.statusline.mode.command"),
            Mode::Search | Mode::FilePicker | Mode::FileBrowser | Mode::CommandPalette => {
                ts(editor, "ui.statusline.mode.command")
            }
        }
    };

    let sl_style = if editor.bell_active() {
        // Visual bell: invert the status bar for one frame.
        let base = ts(editor, "ui.statusline");
        Style::default()
            .fg(base.bg.unwrap_or(Color::Black))
            .bg(base.fg.unwrap_or(Color::White))
    } else {
        ts(editor, "ui.statusline")
    };

    let modified = if buf.modified { " [+]" } else { "" };
    let file_info = format!(" {}{}", buf.name, modified);

    // Git branch
    let git_info = editor
        .git_branch
        .as_ref()
        .map(|b| format!("  {}", b))
        .unwrap_or_default();

    // Project root basename
    let project_info = editor
        .project
        .as_ref()
        .map(|p| format!(" [{}]", p.name))
        .unwrap_or_default();

    // Right section: file type, percentage, AI tier, position
    let buf_idx = win.buffer_idx;
    let file_type = editor
        .syntax
        .language_of(buf_idx)
        .map(|l| l.id())
        .unwrap_or("");
    let file_type_str = if file_type.is_empty() {
        String::new()
    } else {
        format!(" {} ", file_type)
    };

    let total_lines = buf.line_count();
    let pct = if total_lines <= 1 {
        "All".to_string()
    } else if win.cursor_row == 0 {
        "Top".to_string()
    } else if win.cursor_row + 1 >= total_lines {
        "Bot".to_string()
    } else {
        format!("{}%", (win.cursor_row + 1) * 100 / total_lines)
    };

    let tier_str = format!(" [AI:{}|{}]", editor.ai_mode, editor.ai_permission_tier);

    let position = format!(" {}:{} ", win.cursor_row + 1, win.cursor_col + 1);

    let debug_info: String = if editor.debug_mode {
        let rss_mb = editor.perf_stats.rss_bytes as f64 / (1024.0 * 1024.0);
        format!(
            " [DBG] {:.0}MB {:.1}% {:.0}fps ",
            rss_mb,
            editor.perf_stats.cpu_percent,
            editor.perf_stats.fps(),
        )
    } else {
        String::new()
    };

    let ai_info: String = if editor.ai_session_tokens_in == 0 && editor.ai_session_tokens_out == 0 {
        String::new()
    } else {
        let tokens = format!(
            "{}/{}",
            format_tokens(editor.ai_session_tokens_in),
            format_tokens(editor.ai_session_tokens_out),
        );
        if editor.ai_session_cost_usd > 0.0 {
            format!(" ${:.2} · {} ", editor.ai_session_cost_usd, tokens)
        } else {
            format!(" {} ", tokens)
        }
    };

    let left_text = format!("{}{}{}", file_info, git_info, project_info);
    let right_extra = format!("{}{}{}", file_type_str, pct, tier_str);

    let remaining = (area.width as usize)
        .saturating_sub(mode_str.len())
        .saturating_sub(left_text.len())
        .saturating_sub(debug_info.len())
        .saturating_sub(ai_info.len())
        .saturating_sub(right_extra.len())
        .saturating_sub(position.len());

    let status_line = Line::from(vec![
        Span::styled(mode_str, mode_style),
        Span::styled(left_text, sl_style),
        Span::styled(" ".repeat(remaining), sl_style),
        Span::styled(debug_info, sl_style),
        Span::styled(ai_info, sl_style),
        Span::styled(right_extra, sl_style),
        Span::styled(position, sl_style),
    ]);

    let paragraph = Paragraph::new(status_line);
    frame.render_widget(paragraph, area);
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

pub(crate) fn render_command_line(frame: &mut Frame, area: Rect, editor: &Editor) {
    let text = if editor.mode == Mode::Command {
        format!(":{}", editor.command_line)
    } else if editor.mode == Mode::Search {
        let prompt = if editor.search_state.direction == mae_core::SearchDirection::Forward {
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

    let style = ts(editor, "ui.commandline");
    let paragraph = Paragraph::new(Span::styled(text, style));
    frame.render_widget(paragraph, area);
}
