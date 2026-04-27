//! Status bar and command line rendering (terminal backend).
//!
//! Segments are priority-ordered: when total width exceeds available columns,
//! lowest-priority segments are dropped first.

use mae_core::{Editor, LspServerStatus, Mode, VisualType};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::theme_convert::ts;

/// A status bar segment with a priority (higher number = first to drop).
struct Segment {
    text: String,
    priority: u8,
}

impl Segment {
    fn new(text: String, priority: u8) -> Self {
        Self { text, priority }
    }

    fn width(&self) -> usize {
        UnicodeWidthStr::width(self.text.as_str())
    }
}

fn truncate_path(path: &str, max_w: usize) -> String {
    if UnicodeWidthStr::width(path) <= max_w || max_w < 6 {
        return path.to_string();
    }
    if let Some(name) = path.rsplit('/').next() {
        let truncated = format!(".../{}", name);
        if UnicodeWidthStr::width(truncated.as_str()) <= max_w {
            return truncated;
        }
    }
    let mut s = path.to_string();
    while UnicodeWidthStr::width(s.as_str()) > max_w && s.len() > 1 {
        s.pop();
    }
    s.push('…');
    s
}

fn truncate_branch(branch: &str, max_w: usize) -> String {
    if UnicodeWidthStr::width(branch) <= max_w {
        return branch.to_string();
    }
    let mut s = String::new();
    for ch in branch.chars() {
        if UnicodeWidthStr::width(s.as_str()) + 2 > max_w {
            break;
        }
        s.push(ch);
    }
    s.push('…');
    s
}

fn mode_label(editor: &Editor) -> String {
    if editor.input_lock != mae_core::InputLock::None {
        match editor.input_lock {
            mae_core::InputLock::AiBusy => {
                if editor.ai_streaming {
                    " AI... ".to_string()
                } else {
                    " AI BUSY ".to_string()
                }
            }
            mae_core::InputLock::McpBusy => " MCP... ".to_string(),
            mae_core::InputLock::None => unreachable!(),
        }
    } else if editor.macro_recording {
        format!(" REC @{} ", editor.macro_register.unwrap_or('?'))
    } else {
        match editor.mode {
            Mode::Normal => " NORMAL ",
            Mode::Insert => " INSERT ",
            Mode::Visual(VisualType::Char) => " VISUAL ",
            Mode::Visual(VisualType::Line) => " V-LINE ",
            Mode::Visual(VisualType::Block) => " V-BLOCK ",
            Mode::Command => " COMMAND ",
            Mode::ConversationInput => " AI INPUT ",
            Mode::Search => " SEARCH ",
            Mode::FilePicker => " FIND FILE ",
            Mode::FileBrowser => " BROWSE ",
            Mode::CommandPalette => " COMMAND PALETTE ",
            Mode::ShellInsert => " TERMINAL ",
            Mode::GitStatus => " GIT STATUS ",
        }
        .to_string()
    }
}

pub(crate) fn render_status_bar(frame: &mut Frame, area: Rect, editor: &Editor) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let mode_str = mode_label(editor);
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
        let base = ts(editor, "ui.statusline");
        Style::default()
            .fg(base.bg.unwrap_or(Color::Black))
            .bg(base.fg.unwrap_or(Color::White))
    } else {
        ts(editor, "ui.statusline")
    };

    let mode_len = UnicodeWidthStr::width(mode_str.as_str());
    let avail = (area.width as usize).saturating_sub(mode_len);

    // Build segments.
    let mut segments: Vec<Segment> = Vec::new();

    // Priority 1: cursor position.
    let position = format!(" {}:{} ", win.cursor_row + 1, win.cursor_col + 1);
    segments.push(Segment::new(position, 1));

    // Priority 2: filename + modified flag.
    let modified = if buf.modified { " [+]" } else { "" };
    let file_info = format!(" {}{}", buf.name, modified);
    segments.push(Segment::new(file_info, 2));

    // Priority 3: diagnostics summary.
    let (e, w, _i, _h) = editor.diagnostics.severity_counts();
    if e > 0 || w > 0 {
        segments.push(Segment::new(format!(" E:{} W:{}", e, w), 3));
    }

    // Priority 4: AI info.
    let ai_info = format_ai_info(editor);
    if !ai_info.is_empty() {
        segments.push(Segment::new(ai_info, 4));
    }

    // Priority 4: LSP status.
    let lsp_status = format_lsp_status(editor);
    if !lsp_status.is_empty() {
        segments.push(Segment::new(lsp_status, 4));
    }

    // Priority 5: visual selection count.
    if matches!(editor.mode, Mode::Visual(_)) {
        let (lines, chars) = editor.visual_selection_size();
        segments.push(Segment::new(format!(" {}L {}C", lines, chars), 5));
    }

    // Priority 6: debug info.
    if editor.debug_mode {
        let rss_mb = editor.perf_stats.rss_bytes as f64 / (1024.0 * 1024.0);
        segments.push(Segment::new(
            format!(
                " [DBG] {:.0}MB {:.1}% {:.0}fps",
                rss_mb,
                editor.perf_stats.cpu_percent,
                editor.perf_stats.fps(),
            ),
            6,
        ));
    }

    // Priority 7: file type + scroll % + AI tier.
    let buf_idx = win.buffer_idx;
    let file_type = editor
        .syntax
        .language_of(buf_idx)
        .map(|l| l.id())
        .unwrap_or("");
    let file_type_str = if file_type.is_empty() {
        String::new()
    } else {
        format!(" {}", file_type)
    };
    let pct = compute_scroll_pct(editor, buf, win);
    let tier_str = format!(" [AI:{}|{}]", editor.ai_mode, editor.ai_permission_tier);
    let combined_7 = format!("{} {}{}", file_type_str, pct, tier_str);
    if !combined_7.trim().is_empty() {
        segments.push(Segment::new(combined_7, 7));
    }

    // Priority 8: git branch + project name.
    let git_info = editor
        .git_branch
        .as_ref()
        .map(|b| format!("  {}", truncate_branch(b, 20)))
        .unwrap_or_default();
    let project_info = editor
        .project
        .as_ref()
        .map(|p| format!(" [{}]", p.name))
        .unwrap_or_default();
    let git_project = format!("{}{}", git_info, project_info);
    if !git_project.is_empty() {
        segments.push(Segment::new(git_project, 8));
    }

    // Elide lowest-priority segments until they fit.
    segments.sort_by_key(|s| s.priority);
    let total_width = |segs: &[Segment]| -> usize { segs.iter().map(|s| s.width()).sum() };
    while total_width(&segments) > avail && !segments.is_empty() {
        segments.pop(); // Removes highest priority number = lowest importance.
    }

    // Truncate filename if still too long.
    if let Some(fname_seg) = segments.iter_mut().find(|s| s.priority == 2) {
        if fname_seg.width() > avail / 2 {
            let max_w = avail / 2;
            let modified_suffix = if buf.modified { " [+]" } else { "" };
            let truncated =
                truncate_path(&buf.name, max_w.saturating_sub(modified_suffix.len() + 1));
            fname_seg.text = format!(" {}{}", truncated, modified_suffix);
        }
    }

    // Separate left/right.
    let mut left_parts: Vec<&Segment> = Vec::new();
    let mut right_parts: Vec<&Segment> = Vec::new();
    for seg in &segments {
        match seg.priority {
            2 | 8 => left_parts.push(seg),
            _ => right_parts.push(seg),
        }
    }
    left_parts.sort_by_key(|s| s.priority);
    right_parts.sort_by_key(|s| s.priority);

    let left_text: String = left_parts.iter().map(|s| s.text.as_str()).collect();
    let right_text: String = right_parts.iter().map(|s| s.text.as_str()).collect();

    let right_w = UnicodeWidthStr::width(right_text.as_str());
    let remaining = avail
        .saturating_sub(UnicodeWidthStr::width(left_text.as_str()))
        .saturating_sub(right_w);

    let status_line = Line::from(vec![
        Span::styled(&mode_str, mode_style),
        Span::styled(left_text, sl_style),
        Span::styled(" ".repeat(remaining), sl_style),
        Span::styled(right_text, sl_style),
    ]);

    let paragraph = Paragraph::new(status_line);
    frame.render_widget(paragraph, area);
}

fn compute_scroll_pct(_editor: &Editor, buf: &mae_core::Buffer, win: &mae_core::Window) -> String {
    if buf.kind == mae_core::BufferKind::Conversation {
        if let Some(ref conv) = buf.conversation {
            let total = conv.line_count();
            if total <= 1 {
                "All".to_string()
            } else if conv.scroll == 0 {
                "Bot".to_string()
            } else if conv.scroll >= total {
                "Top".to_string()
            } else {
                format!("{}%", (total.saturating_sub(conv.scroll)) * 100 / total)
            }
        } else {
            "All".to_string()
        }
    } else {
        let total_lines = buf.line_count();
        if total_lines <= 1 {
            "All".to_string()
        } else if win.cursor_row == 0 {
            "Top".to_string()
        } else if win.cursor_row + 1 >= total_lines {
            "Bot".to_string()
        } else {
            format!("{}%", (win.cursor_row + 1) * 100 / total_lines)
        }
    }
}

fn format_ai_info(editor: &Editor) -> String {
    if editor.ai_session_tokens_in == 0 && editor.ai_session_tokens_out == 0 {
        return String::new();
    }
    let tokens = format!(
        "{}/{}",
        format_tokens(editor.ai_session_tokens_in),
        format_tokens(editor.ai_session_tokens_out),
    );
    let cache_str = format_cache_hit_rate(editor.ai_cache_read_tokens, editor.ai_session_tokens_in);
    let ctx_str = format_context_usage(editor.ai_context_used_tokens, editor.ai_context_window);
    if editor.ai_session_cost_usd > 0.0 {
        format!(
            " ${:.2} {}{}{}",
            editor.ai_session_cost_usd, tokens, cache_str, ctx_str
        )
    } else {
        format!(" {}{}{}", tokens, cache_str, ctx_str)
    }
}

fn format_lsp_status(editor: &Editor) -> String {
    if editor.lsp_servers.is_empty() {
        return String::new();
    }
    let any_connected = editor
        .lsp_servers
        .values()
        .any(|s| *s == LspServerStatus::Connected);
    let any_starting = editor
        .lsp_servers
        .values()
        .any(|s| *s == LspServerStatus::Starting);
    if any_connected {
        " LSP:✓".to_string()
    } else if any_starting {
        " LSP:…".to_string()
    } else {
        " LSP:✗".to_string()
    }
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

fn format_cache_hit_rate(cache_read: u64, total_in: u64) -> String {
    if cache_read == 0 || total_in == 0 {
        return String::new();
    }
    let pct = (cache_read as f64 / total_in as f64 * 100.0).min(100.0);
    format!(" C:{:.0}%", pct)
}

fn format_context_usage(used: u64, window: u64) -> String {
    if window == 0 {
        return String::new();
    }
    let pct = (used as f64 / window as f64 * 100.0).min(100.0);
    format!(" [{:.0}%]", pct)
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

    #[test]
    fn cache_hit_rate_zero() {
        assert_eq!(format_cache_hit_rate(0, 1000), "");
    }

    #[test]
    fn cache_hit_rate_some() {
        assert_eq!(format_cache_hit_rate(850, 1000), " C:85%");
    }

    #[test]
    fn cache_hit_rate_no_input() {
        assert_eq!(format_cache_hit_rate(100, 0), "");
    }

    #[test]
    fn context_usage_zero_window() {
        assert_eq!(format_context_usage(5000, 0), "");
    }

    #[test]
    fn context_usage_normal() {
        assert_eq!(format_context_usage(72000, 100000), " [72%]");
    }

    #[test]
    fn context_usage_full() {
        assert_eq!(format_context_usage(100000, 100000), " [100%]");
    }

    #[test]
    fn truncate_path_short() {
        assert_eq!(truncate_path("src/main.rs", 20), "src/main.rs");
    }

    #[test]
    fn truncate_path_long() {
        let long = "some/deeply/nested/directory/file.rs";
        let result = truncate_path(long, 15);
        assert!(result.contains("file.rs"));
        assert!(result.starts_with("..."));
    }

    #[test]
    fn truncate_branch_short() {
        assert_eq!(truncate_branch("main", 10), "main");
    }

    #[test]
    fn truncate_branch_long() {
        let result = truncate_branch("feature/very-long-branch-name", 15);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn lsp_status_empty() {
        let editor = Editor::new();
        assert_eq!(format_lsp_status(&editor), "");
    }

    #[test]
    fn lsp_status_connected() {
        let mut editor = Editor::new();
        editor
            .lsp_servers
            .insert("rust".to_string(), LspServerStatus::Connected);
        assert_eq!(format_lsp_status(&editor), " LSP:✓");
    }

    #[test]
    fn lsp_status_all_failed() {
        let mut editor = Editor::new();
        editor
            .lsp_servers
            .insert("rust".to_string(), LspServerStatus::Failed);
        assert_eq!(format_lsp_status(&editor), " LSP:✗");
    }

    #[test]
    fn segment_elision_drops_lowest_priority() {
        // Simulate: 3 segments, only room for 2
        let mut segs = vec![
            Segment::new("AAAA".to_string(), 1),   // 4 wide, keep
            Segment::new("BB".to_string(), 5),     // 2 wide, keep
            Segment::new("CCCCCC".to_string(), 8), // 6 wide, drop
        ];
        let avail = 7;
        segs.sort_by_key(|s| s.priority);
        let total_width = |s: &[Segment]| -> usize { s.iter().map(|s| s.width()).sum() };
        while total_width(&segs) > avail && !segs.is_empty() {
            segs.pop();
        }
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].priority, 1);
        assert_eq!(segs[1].priority, 5);
    }
}
