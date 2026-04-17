//! Status bar and command line rendering.

use mae_core::{Editor, Mode, VisualType};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::theme_convert::ts;

pub(crate) fn render_status_bar(frame: &mut Frame, area: Rect, editor: &Editor) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let recording_label: String;
    let mode_str = if editor.macro_recording {
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
    let mode_style = match editor.mode {
        Mode::Normal => ts(editor, "ui.statusline.mode.normal"),
        Mode::Insert => ts(editor, "ui.statusline.mode.insert"),
        Mode::Visual(_) => ts(editor, "ui.statusline.mode.normal"),
        Mode::Command => ts(editor, "ui.statusline.mode.command"),
        Mode::ConversationInput => ts(editor, "ui.statusline.mode.conversation"),
        Mode::ShellInsert => ts(editor, "ui.statusline.mode.insert"),
        Mode::Search | Mode::FilePicker | Mode::FileBrowser | Mode::CommandPalette => {
            ts(editor, "ui.statusline.mode.command")
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
    let position = format!(" {}:{} ", win.cursor_row + 1, win.cursor_col + 1);

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

    let remaining = (area.width as usize)
        .saturating_sub(mode_str.len())
        .saturating_sub(file_info.len())
        .saturating_sub(ai_info.len())
        .saturating_sub(position.len());

    let status_line = Line::from(vec![
        Span::styled(mode_str, mode_style),
        Span::styled(file_info, sl_style),
        Span::styled(" ".repeat(remaining), sl_style),
        Span::styled(ai_info, sl_style),
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
