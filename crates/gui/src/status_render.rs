//! Status bar and command line rendering for the GUI backend.
//!
//! Segments are priority-ordered: when total width exceeds available columns,
//! lowest-priority segments are dropped first.

use mae_core::{Editor, LspServerStatus, Mode, SearchDirection, VisualType};
use skia_safe::Color4f;
use unicode_width::UnicodeWidthStr;

use crate::canvas::SkiaCanvas;
use crate::theme;

/// A status bar segment with a priority (higher = harder to drop).
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

/// Truncate a path to `.../<filename>` if it exceeds `max_w` columns.
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

/// Truncate a branch name with ellipsis if it exceeds `max_w`.
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

/// Build the mode label string.
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
            mae_core::Mode::GitStatus => " GIT STATUS ",
        }
        .to_string()
    }
}

/// Render the full status bar at the given screen row.
pub fn render_status_bar(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    row: usize,
    cols: usize,
    frame_ms: Option<u64>,
) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let mode_str = mode_label(editor);

    // Mode label colors.
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
            mae_core::Mode::GitStatus => "ui.statusline.mode.command",
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

    canvas.draw_rect_fill(row, 0, cols, 1, sl_bg);

    // Mode label.
    let mode_len = UnicodeWidthStr::width(mode_str.as_str());
    canvas.draw_rect_fill(row, 0, mode_len, 1, mode_bg);
    canvas.draw_text_at(row, 0, &mode_str, mode_fg);

    // Available space after mode label.
    let avail = cols.saturating_sub(mode_len);
    if avail == 0 {
        return;
    }

    // Build segments with priorities (1 = highest = last to drop, 8 = first to drop).
    let mut segments: Vec<Segment> = Vec::new();

    // Priority 1: cursor position (always visible).
    let position = format!(" {}:{} ", win.cursor_row + 1, win.cursor_col + 1);
    segments.push(Segment::new(position, 1));

    // Priority 2: filename + modified flag.
    let modified = if buf.modified { " [+]" } else { "" };
    let file_info = format!(" {}{}", buf.name, modified);
    segments.push(Segment::new(file_info, 2));

    // Priority 3: diagnostics summary (hide if all zero).
    let (e, w, _i, _h) = editor.diagnostics.severity_counts();
    if e > 0 || w > 0 {
        segments.push(Segment::new(format!(" E:{} W:{}", e, w), 3));
    }

    // Priority 4: AI info.
    let ai_info = format_ai_info(editor);
    if !ai_info.is_empty() {
        segments.push(Segment::new(ai_info, 4));
    }

    // Priority 4 (between AI info and debug): LSP status.
    let lsp_status = format_lsp_status(editor);
    if !lsp_status.is_empty() {
        segments.push(Segment::new(lsp_status, 4));
    }

    // Priority 5: visual selection count (only in visual mode).
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
    } else if editor.show_fps {
        if let Some(ms) = frame_ms {
            segments.push(Segment::new(format!(" {}ms", ms), 6));
        }
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

    // Priority 8: git branch + project name (first to drop).
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
    // Sort by priority descending so we can pop lowest priority first.
    segments.sort_by_key(|s| s.priority);

    let total_width = |segs: &[Segment]| -> usize { segs.iter().map(|s| s.width()).sum() };

    while total_width(&segments) > avail && !segments.is_empty() {
        segments.pop(); // Removes highest priority number = lowest importance.
    }

    // If the filename segment is still present but too long, truncate it.
    if let Some(fname_seg) = segments.iter_mut().find(|s| s.priority == 2) {
        if fname_seg.width() > avail / 2 {
            let max_w = avail / 2;
            let modified_suffix = if buf.modified { " [+]" } else { "" };
            let truncated =
                truncate_path(&buf.name, max_w.saturating_sub(modified_suffix.len() + 1));
            fname_seg.text = format!(" {}{}", truncated, modified_suffix);
        }
    }

    // Layout: left-aligned segments = filename (2) + git/project (8), rest right-aligned.
    // Separate into left and right groups.
    let mut left_parts: Vec<&Segment> = Vec::new();
    let mut right_parts: Vec<&Segment> = Vec::new();
    for seg in &segments {
        match seg.priority {
            2 | 8 => left_parts.push(seg),
            _ => right_parts.push(seg),
        }
    }
    // Sort left by priority (filename first, then git/project).
    left_parts.sort_by_key(|s| s.priority);
    // Sort right by priority (position first, then diagnostics, etc.).
    right_parts.sort_by_key(|s| s.priority);

    let left_text: String = left_parts.iter().map(|s| s.text.as_str()).collect();
    let right_text: String = right_parts.iter().map(|s| s.text.as_str()).collect();

    canvas.draw_text_at(row, mode_len, &left_text, sl_fg);

    let right_w = UnicodeWidthStr::width(right_text.as_str());
    let right_col = (mode_len + avail).saturating_sub(right_w);
    canvas.draw_text_at(row, right_col, &right_text, sl_fg);
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

    #[test]
    fn cache_hit_rate_zero() {
        assert_eq!(format_cache_hit_rate(0, 1000), "");
    }

    #[test]
    fn cache_hit_rate_some() {
        assert_eq!(format_cache_hit_rate(850, 1000), " C:85%");
    }

    #[test]
    fn context_usage_normal() {
        assert_eq!(format_context_usage(72000, 100000), " [72%]");
    }

    #[test]
    fn context_usage_zero_window() {
        assert_eq!(format_context_usage(5000, 0), "");
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
        assert!(result.len() <= 16); // 15 + 1 for ellipsis char
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
    fn lsp_status_failed() {
        let mut editor = Editor::new();
        editor
            .lsp_servers
            .insert("rust".to_string(), LspServerStatus::Failed);
        assert_eq!(format_lsp_status(&editor), " LSP:✗");
    }
}
