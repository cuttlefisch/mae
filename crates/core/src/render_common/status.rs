//! Shared status bar logic: segment building, truncation, formatting.
//!
//! Both GUI and TUI backends call [`build_status_segments`] to get a list of
//! prioritized segments, then [`layout_status_segments`] to split them into
//! left/right text.  The backend only needs to draw the resulting strings.

use crate::{Buffer, BufferKind, Editor, InputLock, LspServerStatus, Mode, VisualType, Window};
use unicode_width::UnicodeWidthStr;

/// A status bar segment with a priority (1 = highest = last to drop).
pub struct Segment {
    pub text: String,
    pub priority: u8,
    /// Optional theme key for custom styling (e.g. colored AI mode badge).
    pub style_hint: Option<&'static str>,
}

impl Segment {
    pub fn new(text: String, priority: u8) -> Self {
        Self {
            text,
            priority,
            style_hint: None,
        }
    }

    pub fn with_style(text: String, priority: u8, style_hint: &'static str) -> Self {
        Self {
            text,
            priority,
            style_hint: Some(style_hint),
        }
    }

    pub fn width(&self) -> usize {
        UnicodeWidthStr::width(self.text.as_str())
    }
}

/// Truncate a path to `.../<filename>` if it exceeds `max_w` columns.
pub fn truncate_path(path: &str, max_w: usize) -> String {
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
pub fn truncate_branch(branch: &str, max_w: usize) -> String {
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
pub fn mode_label(editor: &Editor) -> String {
    if editor.input_lock != InputLock::None {
        match editor.input_lock {
            InputLock::AiBusy => {
                if editor.ai_streaming {
                    " AI... ".to_string()
                } else {
                    " AI BUSY ".to_string()
                }
            }
            InputLock::McpBusy => " MCP... ".to_string(),
            InputLock::None => unreachable!(),
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

/// Return the theme key for the current mode's status bar style.
pub fn mode_theme_key(editor: &Editor) -> &'static str {
    if editor.input_lock != InputLock::None {
        match editor.input_lock {
            InputLock::AiBusy => "ui.statusline.mode.locked",
            InputLock::McpBusy => "ui.statusline.mode.mcp",
            InputLock::None => "ui.statusline.mode.normal",
        }
    } else {
        match editor.mode {
            Mode::Normal => "ui.statusline.mode.normal",
            Mode::Insert => "ui.statusline.mode.insert",
            Mode::Visual(_) => "ui.statusline.mode.visual",
            Mode::Command => "ui.statusline.mode.command",
            Mode::ConversationInput => "ui.statusline.mode.conversation",
            Mode::ShellInsert => "ui.statusline.mode.insert",
            Mode::GitStatus => "ui.statusline.mode.command",
            Mode::Search | Mode::FilePicker | Mode::FileBrowser | Mode::CommandPalette => {
                "ui.statusline.mode.command"
            }
        }
    }
}

/// Build prioritized status bar segments from editor state.
///
/// `frame_ms` is optional frame timing for the FPS display (GUI-only).
pub fn build_status_segments(editor: &Editor, frame_ms: Option<u64>) -> Vec<Segment> {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let mut segments: Vec<Segment> = Vec::new();

    // Priority 1: cursor position (always visible).
    let position = format!(" {}:{} ", win.cursor_row + 1, win.cursor_col + 1);
    segments.push(Segment::new(position, 1));

    // Priority 2: filename + modified flag + narrowed indicator.
    let modified = if buf.modified { " [+]" } else { "" };
    let narrowed = if buf.narrowed_range.is_some() {
        " [Narrowed]"
    } else {
        ""
    };
    let file_info = format!(" {}{}{}", buf.name, modified, narrowed);
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

    // Priority 4: LSP status.
    let lsp_status = format_lsp_status(editor);
    if !lsp_status.is_empty() {
        segments.push(Segment::new(lsp_status, 4));
    }

    // Priority 5: visual selection count (only in visual mode).
    if matches!(editor.mode, Mode::Visual(_)) {
        let (lines, chars) = editor.visual_selection_size();
        segments.push(Segment::new(format!(" {}L {}C", lines, chars), 5));
    }

    // Priority 6: debug info / FPS.
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

    // Priority 6.5: nyan mode progress bar.
    if editor.nyan_mode {
        let nyan = build_nyan_segment(buf, win);
        segments.push(Segment::new(nyan, 6));
    }

    // Priority 7a: colored AI mode badge (only when AI session is active).
    if editor.conversation_pair.is_some()
        || editor.ai_session_tokens_in > 0
        || editor.ai_session_tokens_out > 0
    {
        let ai_mode_style = match editor.ai_mode.as_str() {
            "standard" => "ui.statusline.ai.standard",
            "auto-accept" => "ui.statusline.ai.auto",
            "plan" => "ui.statusline.ai.plan",
            _ => "ui.statusline.ai.standard",
        };
        segments.push(Segment::with_style(
            format!(" {} ", editor.ai_mode.to_uppercase()),
            7,
            ai_mode_style,
        ));
    }

    // Priority 7b: file type + scroll % + AI tier.
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
    let pct = compute_scroll_pct(buf, win);
    let tier_str = format!(" [{}]", editor.ai_permission_tier);
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

    segments
}

/// A span within the status bar text that should be rendered with a custom style.
pub struct StyledSpan {
    /// Byte offset within `right_text`.
    pub byte_offset: usize,
    /// Byte length of the span.
    pub byte_len: usize,
    /// Theme key for this span's style.
    pub style_key: &'static str,
}

/// Result of laying out status bar segments.
pub struct StatusLayout {
    pub left_text: String,
    pub right_text: String,
    /// Spans within `right_text` that need custom styling (e.g. colored AI mode).
    pub right_styled_spans: Vec<StyledSpan>,
}

/// Elide, truncate, and split segments into left/right text for the status bar.
///
/// `avail` is the number of columns available after the mode label.
pub fn layout_status_segments(
    segments: &mut Vec<Segment>,
    avail: usize,
    buf_name: &str,
    buf_modified: bool,
) -> StatusLayout {
    // Elide lowest-priority segments until they fit.
    segments.sort_by_key(|s| s.priority);
    let total_width = |segs: &[Segment]| -> usize { segs.iter().map(|s| s.width()).sum() };
    while total_width(segments) > avail && !segments.is_empty() {
        segments.pop();
    }

    // Truncate filename if still too long.
    if let Some(fname_seg) = segments.iter_mut().find(|s| s.priority == 2) {
        if fname_seg.width() > avail / 2 {
            let max_w = avail / 2;
            let modified_suffix = if buf_modified { " [+]" } else { "" };
            let truncated =
                truncate_path(buf_name, max_w.saturating_sub(modified_suffix.len() + 1));
            fname_seg.text = format!(" {}{}", truncated, modified_suffix);
        }
    }

    // Separate left/right: filename (2) + git/project (8) left, rest right.
    let mut left_parts: Vec<&Segment> = Vec::new();
    let mut right_parts: Vec<&Segment> = Vec::new();
    for seg in segments.iter() {
        match seg.priority {
            2 | 8 => left_parts.push(seg),
            _ => right_parts.push(seg),
        }
    }
    left_parts.sort_by_key(|s| s.priority);
    right_parts.sort_by_key(|s| s.priority);

    let left_text: String = left_parts.iter().map(|s| s.text.as_str()).collect();
    let mut right_text = String::new();
    let mut right_styled_spans = Vec::new();
    for seg in &right_parts {
        if let Some(key) = seg.style_hint {
            let offset = right_text.len();
            right_text.push_str(&seg.text);
            right_styled_spans.push(StyledSpan {
                byte_offset: offset,
                byte_len: seg.text.len(),
                style_key: key,
            });
        } else {
            right_text.push_str(&seg.text);
        }
    }

    StatusLayout {
        left_text,
        right_text,
        right_styled_spans,
    }
}

/// Build the command/message line text.
pub fn command_line_text(editor: &Editor) -> String {
    if editor.mode == Mode::Command {
        format!(":{}", editor.command_line)
    } else if editor.mode == Mode::Search {
        let prompt = if editor.search_state.direction == crate::SearchDirection::Forward {
            "/"
        } else {
            "?"
        };
        format!("{}{}", prompt, editor.search_input)
    } else if let Some(count) = editor.count_prefix {
        format!("{}", count)
    } else {
        editor.status_msg.clone()
    }
}

/// Build a nyan cat progress indicator: filled bar + cat at scroll position.
/// Width: 20 chars. Format: `[=========>          ]` style with rainbow fill + cat emoji.
fn build_nyan_segment(buf: &Buffer, win: &Window) -> String {
    let total = buf.line_count();
    let ratio = if total <= 1 {
        0.0
    } else {
        (win.cursor_row as f32 / (total - 1) as f32).clamp(0.0, 1.0)
    };
    let bar_width = 18; // Inside the brackets
    let cat_pos = (ratio * bar_width as f32).round() as usize;
    let cat_pos = cat_pos.min(bar_width);

    // Rainbow bar fills left of cat, empty fills right.
    let rainbow_chars = ['='; 1];
    let mut bar = String::with_capacity(bar_width + 4);
    bar.push('[');
    for i in 0..bar_width {
        if i == cat_pos {
            bar.push_str("~>");
            if i + 1 < bar_width {
                // Skip next position (cat is 2 chars)
                continue;
            }
        } else if i < cat_pos {
            bar.push(rainbow_chars[0]);
        } else {
            bar.push(' ');
        }
    }
    bar.push(']');
    format!(" {}", bar)
}

fn compute_scroll_pct(buf: &Buffer, win: &Window) -> String {
    if buf.kind == BufferKind::Conversation {
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

pub fn format_ai_info(editor: &Editor) -> String {
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

pub fn format_lsp_status(editor: &Editor) -> String {
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

pub fn format_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

pub fn format_cache_hit_rate(cache_read: u64, total_in: u64) -> String {
    if cache_read == 0 || total_in == 0 {
        return String::new();
    }
    let pct = (cache_read as f64 / total_in as f64 * 100.0).min(100.0);
    format!(" C:{:.0}%", pct)
}

pub fn format_context_usage(used: u64, window: u64) -> String {
    if window == 0 {
        return String::new();
    }
    let pct = (used as f64 / window as f64 * 100.0).min(100.0);
    format!(" [{:.0}%]", pct)
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
        let mut segs = vec![
            Segment::new("AAAA".to_string(), 1),
            Segment::new("BB".to_string(), 5),
            Segment::new("CCCCCC".to_string(), 8),
        ];
        let layout = layout_status_segments(&mut segs, 7, "test", false);
        // After elision, the 8-priority segment should be gone
        assert!(!layout.left_text.contains("CCCCCC"));
    }

    #[test]
    fn mode_label_normal() {
        let editor = Editor::new();
        assert_eq!(mode_label(&editor), " NORMAL ");
    }

    #[test]
    fn visual_mode_has_distinct_theme_key() {
        let mut editor = Editor::new();
        editor.mode = Mode::Visual(VisualType::Char);
        assert_eq!(mode_theme_key(&editor), "ui.statusline.mode.visual");
    }

    #[test]
    fn ai_mode_badge_has_style_hint() {
        let mut editor = Editor::new();
        // Badge only appears when AI session is active.
        editor.ai_session_tokens_in = 100;
        let segments = build_status_segments(&editor, None);
        let ai_seg = segments.iter().find(|s| s.text.contains("STANDARD"));
        assert!(ai_seg.is_some());
        assert_eq!(
            ai_seg.unwrap().style_hint,
            Some("ui.statusline.ai.standard")
        );
    }

    #[test]
    fn ai_mode_badge_hidden_without_session() {
        let editor = Editor::new();
        let segments = build_status_segments(&editor, None);
        let ai_seg = segments.iter().find(|s| s.text.contains("STANDARD"));
        assert!(
            ai_seg.is_none(),
            "AI badge should not show on splash/no session"
        );
    }

    #[test]
    fn styled_spans_in_layout() {
        let mut segs = vec![
            Segment::new("pos".to_string(), 1),
            Segment::with_style(" MODE ".to_string(), 4, "test.style"),
        ];
        let layout = layout_status_segments(&mut segs, 40, "test", false);
        assert_eq!(layout.right_styled_spans.len(), 1);
        assert_eq!(layout.right_styled_spans[0].style_key, "test.style");
        let span = &layout.right_styled_spans[0];
        assert_eq!(
            &layout.right_text[span.byte_offset..span.byte_offset + span.byte_len],
            " MODE "
        );
    }
}
