use std::io::{self, Stdout};

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use mae_core::{
    grapheme, DiagnosticSeverity, Editor, Key, Mode, NamedColor, ThemeColor, ThemeStyle,
    VisualType, Window,
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};

/// Terminal renderer using ratatui/crossterm.
///
/// Design: no global state, no static variables. The render function takes
/// an immutable reference to Editor and produces a frame. This is the opposite
/// of Emacs's xdisp.c (38,605 lines, 118 static vars, 30 gotos).
///
/// Emacs lesson: build the full frame each cycle (immediate mode). No sparse
/// glyph matrix diffing. ratatui handles terminal diffing internally.
pub struct TerminalRenderer {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalRenderer {
    pub fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(TerminalRenderer { terminal })
    }

    pub fn render(&mut self, editor: &Editor) -> io::Result<()> {
        self.terminal.draw(|frame| {
            render_frame(frame, editor);
        })?;
        Ok(())
    }

    pub fn terminal_size(&self) -> io::Result<(u16, u16)> {
        let size = self.terminal.size()?;
        Ok((size.width, size.height))
    }

    pub fn viewport_height(&self) -> io::Result<usize> {
        let size = self.terminal.size()?;
        // Subtract 2 for status bar and command/message line
        Ok((size.height as usize).saturating_sub(2))
    }

    pub fn cleanup(&mut self) -> io::Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Theme → ratatui conversion
// ---------------------------------------------------------------------------

fn to_ratatui_color(tc: ThemeColor) -> Color {
    match tc {
        ThemeColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
        ThemeColor::Named(n) => match n {
            NamedColor::Black => Color::Black,
            NamedColor::Red => Color::Red,
            NamedColor::Green => Color::Green,
            NamedColor::Yellow => Color::Yellow,
            NamedColor::Blue => Color::Blue,
            NamedColor::Magenta => Color::Magenta,
            NamedColor::Cyan => Color::Cyan,
            NamedColor::White => Color::White,
            NamedColor::DarkGray => Color::DarkGray,
            NamedColor::LightRed => Color::LightRed,
            NamedColor::LightGreen => Color::LightGreen,
            NamedColor::LightYellow => Color::LightYellow,
            NamedColor::LightBlue => Color::LightBlue,
            NamedColor::LightMagenta => Color::LightMagenta,
            NamedColor::LightCyan => Color::LightCyan,
            NamedColor::Gray => Color::Gray,
        },
    }
}

fn to_ratatui_style(ts: &ThemeStyle) -> Style {
    let mut style = Style::default();
    if let Some(fg) = ts.fg {
        style = style.fg(to_ratatui_color(fg));
    }
    if let Some(bg) = ts.bg {
        style = style.bg(to_ratatui_color(bg));
    }
    if ts.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if ts.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if ts.dim {
        style = style.add_modifier(Modifier::DIM);
    }
    if ts.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

/// Shorthand: look up a theme key and convert to ratatui Style.
fn ts(editor: &Editor, key: &str) -> Style {
    to_ratatui_style(&editor.theme.style(key))
}

// ---------------------------------------------------------------------------
// Frame layout
// ---------------------------------------------------------------------------

/// Pure rendering function: Editor state in, frame out.
/// No side effects, no global state. Emacs lesson: this is the anti-xdisp.c.
fn render_frame(frame: &mut Frame, editor: &Editor) {
    let area = frame.area();

    if editor.file_picker.is_some() {
        // File picker overlay on top of normal layout
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

        render_window_area(frame, chunks[0], editor);
        render_status_bar(frame, chunks[1], editor);
        render_command_line(frame, chunks[2], editor);
        render_file_picker(frame, area, editor);
    } else if !editor.which_key_prefix.is_empty() {
        // Which-key popup mode: [window area | which-key panel]
        let entries = if let Some(km) = editor.keymaps.get("normal") {
            km.which_key_entries(&editor.which_key_prefix, &editor.commands)
        } else {
            vec![]
        };

        let cols = (area.width as usize / 25).max(1);
        let rows = entries.len().div_ceil(cols);
        let popup_height = (rows as u16 + 2).min(area.height / 2).max(3);

        let chunks =
            Layout::vertical([Constraint::Min(1), Constraint::Length(popup_height)]).split(area);

        render_window_area(frame, chunks[0], editor);
        render_which_key_popup(frame, chunks[1], editor, &entries);
    } else {
        // Normal layout: [window area | status bar | command line]
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

        render_window_area(frame, chunks[0], editor);
        render_status_bar(frame, chunks[1], editor);
        render_command_line(frame, chunks[2], editor);
        set_cursor(frame, editor, chunks[0], chunks[2]);
    }
}

// ---------------------------------------------------------------------------
// Window area
// ---------------------------------------------------------------------------

fn render_window_area(frame: &mut Frame, area: Rect, editor: &Editor) {
    let window_area = mae_core::WinRect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height,
    };
    let rects = editor.window_mgr.layout_rects(window_area);
    let focused_id = editor.window_mgr.focused_id();

    for (win_id, win_rect) in &rects {
        let ratatui_rect = Rect::new(win_rect.x, win_rect.y, win_rect.width, win_rect.height);
        if let Some(win) = editor.window_mgr.window(*win_id) {
            let buf = &editor.buffers[win.buffer_idx];
            let is_focused = *win_id == focused_id;
            match buf.kind {
                mae_core::BufferKind::Conversation => {
                    render_conversation_window(frame, ratatui_rect, buf, win, is_focused, editor);
                }
                mae_core::BufferKind::Messages => {
                    render_messages_window(frame, ratatui_rect, win, is_focused, editor);
                }
                _ => {
                    render_window(frame, ratatui_rect, buf, win, is_focused, editor);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------

fn set_cursor(frame: &mut Frame, editor: &Editor, window_area: Rect, cmd_area: Rect) {
    let focused_win = editor.window_mgr.focused_window();
    let focused_buf = &editor.buffers[focused_win.buffer_idx];

    let wa = mae_core::WinRect {
        x: window_area.x,
        y: window_area.y,
        width: window_area.width,
        height: window_area.height,
    };
    let rects = editor.window_mgr.layout_rects(wa);
    let focused_id = editor.window_mgr.focused_id();

    if let Some((_, win_rect)) = rects.iter().find(|(id, _)| *id == focused_id) {
        let rr = Rect::new(win_rect.x, win_rect.y, win_rect.width, win_rect.height);
        let inner = inner_rect(rr);
        let gutter_w = gutter_width(focused_buf.line_count());

        if editor.mode == Mode::Command {
            frame.set_cursor_position(Position::new(
                cmd_area.x + 1 + editor.command_line.len() as u16,
                cmd_area.y,
            ));
        } else if editor.mode == Mode::Search {
            frame.set_cursor_position(Position::new(
                cmd_area.x + 1 + editor.search_input.len() as u16,
                cmd_area.y,
            ));
        } else if editor.mode == Mode::ConversationInput {
            if let Some(ref conv) = focused_buf.conversation {
                let input_x = inner.x + 2 + conv.input_line.len() as u16;
                let input_y = inner.y + inner.height.saturating_sub(1);
                frame.set_cursor_position(Position::new(input_x, input_y));
            }
        } else {
            let screen_row = focused_win
                .cursor_row
                .saturating_sub(focused_win.scroll_offset) as u16;
            let line_text = if focused_win.cursor_row < focused_buf.line_count() {
                let line = focused_buf.rope().line(focused_win.cursor_row);
                let s: String = line.chars().collect();
                s.trim_end_matches('\n').to_string()
            } else {
                String::new()
            };
            let display_col =
                grapheme::display_width_up_to_grapheme(&line_text, focused_win.cursor_col);
            let scroll_col =
                grapheme::display_width_up_to_grapheme(&line_text, focused_win.col_offset);
            let screen_col = gutter_w as u16 + (display_col.saturating_sub(scroll_col)) as u16;
            if screen_row < inner.height {
                frame
                    .set_cursor_position(Position::new(inner.x + screen_col, inner.y + screen_row));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Text buffer window
// ---------------------------------------------------------------------------

fn render_window(
    frame: &mut Frame,
    area: Rect,
    buf: &mae_core::Buffer,
    win: &Window,
    focused: bool,
    editor: &Editor,
) {
    let border_style = if focused {
        ts(editor, "ui.window.border.active")
    } else {
        ts(editor, "ui.window.border")
    };

    let modified = if buf.modified { " [+]" } else { "" };
    let title = format!(" {}{} ", buf.name, modified);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    render_buffer(frame, inner, buf, win, editor);
}

fn inner_rect(area: Rect) -> Rect {
    Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

fn render_buffer(
    frame: &mut Frame,
    area: Rect,
    buf: &mae_core::Buffer,
    win: &Window,
    editor: &Editor,
) {
    let viewport_height = area.height as usize;
    let gutter_w = gutter_width(buf.line_count());
    let gutter_style = ts(editor, "ui.gutter");
    let text_style = ts(editor, "ui.text");
    let search_style = ts(editor, "ui.search.match");
    let selection_style = ts(editor, "ui.selection");
    let highlight_search =
        editor.search_state.highlight_active && !editor.search_state.matches.is_empty();
    let highlight_selection = matches!(editor.mode, Mode::Visual(_));
    let (sel_start, sel_end) = if highlight_selection {
        editor.visual_selection_range()
    } else {
        (0, 0)
    };
    let needs_spans = highlight_search || highlight_selection;

    // Per-line worst-severity diagnostic for gutter markers. We only need
    // the highest severity per line (Error > Warning > Information > Hint).
    let line_severities: std::collections::HashMap<u32, DiagnosticSeverity> = {
        let mut map: std::collections::HashMap<u32, DiagnosticSeverity> =
            std::collections::HashMap::new();
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
    };

    let mut lines = Vec::with_capacity(viewport_height);

    let col_offset = win.col_offset;

    for i in 0..viewport_height {
        let line_idx = win.scroll_offset + i;
        if line_idx < buf.line_count() {
            let line_text = buf.rope().line(line_idx);
            let full_display: String = line_text
                .chars()
                .filter(|c| *c != '\n' && *c != '\r')
                .collect();

            // Gutter layout: "{line_num_padded}{severity_marker_or_space}"
            // total width = gutter_w.
            let line_num = format!("{:>width$}", line_idx + 1, width = gutter_w - 1);
            let (marker_char, marker_style) = match line_severities.get(&(line_idx as u32)) {
                Some(sev) => (sev.gutter_char(), ts(editor, sev.theme_key())),
                None => (' ', gutter_style),
            };

            if needs_spans {
                let line_char_start = buf.rope().line_to_char(line_idx);
                let full_chars: Vec<char> = full_display.chars().collect();
                let full_count = full_chars.len();
                let line_char_end = line_char_start + full_count;

                // Build a per-char style array over the full line
                let mut styles: Vec<Style> = vec![text_style; full_count];

                // Apply selection highlight (lower priority)
                if highlight_selection && sel_start < line_char_end && sel_end > line_char_start {
                    let s = sel_start.saturating_sub(line_char_start);
                    let e = (sel_end - line_char_start).min(full_count);
                    for style in styles[s..e].iter_mut() {
                        *style = selection_style;
                    }
                }

                // Apply search highlights (higher priority — overwrites selection)
                if highlight_search {
                    for m in &editor.search_state.matches {
                        if m.end <= line_char_start || m.start >= line_char_end {
                            continue;
                        }
                        let ms = m.start.saturating_sub(line_char_start);
                        let me = (m.end - line_char_start).min(full_count);
                        for style in styles[ms..me].iter_mut() {
                            *style = search_style;
                        }
                    }
                }

                // Apply horizontal scroll: slice chars and styles from col_offset
                let visible_start = col_offset.min(full_count);
                let display_chars = &full_chars[visible_start..];
                let visible_styles = &styles[visible_start..];

                // Coalesce consecutive chars with same style into spans
                let mut spans = vec![
                    Span::styled(line_num, gutter_style),
                    Span::styled(marker_char.to_string(), marker_style),
                ];
                if !display_chars.is_empty() {
                    let mut run_start = 0;
                    let mut run_style = visible_styles[0];
                    for j in 1..display_chars.len() {
                        if visible_styles[j] != run_style {
                            let s: String = display_chars[run_start..j].iter().collect();
                            spans.push(Span::styled(s, run_style));
                            run_start = j;
                            run_style = visible_styles[j];
                        }
                    }
                    let s: String = display_chars[run_start..].iter().collect();
                    spans.push(Span::styled(s, run_style));
                }

                lines.push(Line::from(spans));
            } else {
                // Apply horizontal scroll to simple (no highlight) lines
                let display: String = full_display.chars().skip(col_offset).collect();
                lines.push(Line::from(vec![
                    Span::styled(line_num, gutter_style),
                    Span::styled(marker_char.to_string(), marker_style),
                    Span::styled(display, text_style),
                ]));
            }
        } else {
            let padding = " ".repeat(gutter_w.saturating_sub(1));
            lines.push(Line::from(vec![Span::styled(
                format!("{}~", padding),
                gutter_style,
            )]));
        }
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn render_status_bar(frame: &mut Frame, area: Rect, editor: &Editor) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let mode_str = match editor.mode {
        Mode::Normal => " NORMAL ",
        Mode::Insert => " INSERT ",
        Mode::Visual(VisualType::Char) => " VISUAL ",
        Mode::Visual(VisualType::Line) => " V-LINE ",
        Mode::Command => " COMMAND ",
        Mode::ConversationInput => " AI INPUT ",
        Mode::Search => " SEARCH ",
        Mode::FilePicker => " FIND FILE ",
    };
    let mode_style = match editor.mode {
        Mode::Normal => ts(editor, "ui.statusline.mode.normal"),
        Mode::Insert => ts(editor, "ui.statusline.mode.insert"),
        Mode::Visual(_) => ts(editor, "ui.statusline.mode.normal"),
        Mode::Command => ts(editor, "ui.statusline.mode.command"),
        Mode::ConversationInput => ts(editor, "ui.statusline.mode.conversation"),
        Mode::Search | Mode::FilePicker => ts(editor, "ui.statusline.mode.command"),
    };

    let sl_style = ts(editor, "ui.statusline");

    let modified = if buf.modified { " [+]" } else { "" };
    let file_info = format!(" {}{}", buf.name, modified);
    let position = format!(" {}:{} ", win.cursor_row + 1, win.cursor_col + 1);

    let remaining = (area.width as usize)
        .saturating_sub(mode_str.len())
        .saturating_sub(file_info.len())
        .saturating_sub(position.len());

    let status_line = Line::from(vec![
        Span::styled(mode_str, mode_style),
        Span::styled(file_info, sl_style),
        Span::styled(" ".repeat(remaining), sl_style),
        Span::styled(position, sl_style),
    ]);

    let paragraph = Paragraph::new(status_line);
    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Command line
// ---------------------------------------------------------------------------

fn render_command_line(frame: &mut Frame, area: Rect, editor: &Editor) {
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
        // Show the count prefix being typed (e.g. "5" while user is typing 5j)
        format!("{}", count)
    } else {
        editor.status_msg.clone()
    };

    let style = ts(editor, "ui.commandline");
    let paragraph = Paragraph::new(Span::styled(text, style));
    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Which-key popup
// ---------------------------------------------------------------------------

fn format_keypress(kp: &mae_core::KeyPress) -> String {
    let mut s = String::new();
    if kp.ctrl {
        s.push_str("C-");
    }
    if kp.alt {
        s.push_str("M-");
    }
    match &kp.key {
        Key::Char(' ') => s.push_str("SPC"),
        Key::Char(c) => s.push(*c),
        Key::Escape => s.push_str("Esc"),
        Key::Enter => s.push_str("Enter"),
        Key::Tab => s.push_str("Tab"),
        Key::Backspace => s.push_str("BS"),
        Key::Up => s.push_str("Up"),
        Key::Down => s.push_str("Down"),
        Key::Left => s.push_str("Left"),
        Key::Right => s.push_str("Right"),
        Key::F(n) => {
            s.push_str(&format!("F{}", n));
        }
        _ => s.push('?'),
    }
    s
}

fn render_which_key_popup(
    frame: &mut Frame,
    area: Rect,
    editor: &Editor,
    entries: &[mae_core::WhichKeyEntry],
) {
    let breadcrumb: String = editor
        .which_key_prefix
        .iter()
        .map(format_keypress)
        .collect::<Vec<_>>()
        .join(" > ");

    let popup_border = ts(editor, "ui.window.border");
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(popup_border)
        .title(format!(" {} ", breadcrumb));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let group_style = ts(editor, "ui.popup.group");
    let key_style = ts(editor, "ui.popup.key");
    let text_style = ts(editor, "ui.popup.text");

    let col_width = 25_u16;
    let num_cols = (inner.width / col_width).max(1) as usize;

    let mut lines: Vec<Line> = Vec::new();
    let mut current_spans: Vec<Span> = Vec::new();
    let mut col = 0;

    for entry in entries {
        let key_str = format_keypress(&entry.key);
        let (ks, ls) = if entry.is_group {
            (group_style, group_style)
        } else {
            (key_style, text_style)
        };

        let max_label = (col_width as usize).saturating_sub(key_str.len() + 2);
        let label = if entry.label.len() > max_label {
            format!("{}..", &entry.label[..max_label.saturating_sub(2)])
        } else {
            entry.label.clone()
        };

        let entry_width = col_width as usize;
        let padding = entry_width.saturating_sub(key_str.len() + 1 + label.len());

        current_spans.push(Span::styled(key_str, ks));
        current_spans.push(Span::raw(" "));
        current_spans.push(Span::styled(label, ls));
        current_spans.push(Span::raw(" ".repeat(padding)));

        col += 1;
        if col >= num_cols {
            lines.push(Line::from(std::mem::take(&mut current_spans)));
            col = 0;
        }
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Conversation window
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// File picker popup
// ---------------------------------------------------------------------------

fn render_file_picker(frame: &mut Frame, area: Rect, editor: &Editor) {
    let picker = match &editor.file_picker {
        Some(p) => p,
        None => return,
    };

    // Centered popup: 70% width, 60% height (min 10 lines, min 40 cols)
    let popup_w = (area.width * 70 / 100).max(40).min(area.width);
    let popup_h = (area.height * 60 / 100).max(10).min(area.height);
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    // Clear the popup area first
    let clear = ratatui::widgets::Clear;
    frame.render_widget(clear, popup_area);

    let border_style = ts(editor, "ui.window.border.active");
    let match_count = picker.filtered.len();
    let total = picker.candidates.len();
    let title = format!(" Find File ({}/{}) ", match_count, total);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let text_style = ts(editor, "ui.text");
    let selection_style = ts(editor, "ui.selection");
    let prompt_style = ts(editor, "ui.popup.key");

    // First line: query input
    let query_line = Line::from(vec![
        Span::styled("> ", prompt_style),
        Span::styled(&picker.query, text_style),
    ]);

    let results_height = (inner.height - 1) as usize; // -1 for query line

    // Build result lines
    let mut lines = vec![query_line];

    let start = if picker.selected >= results_height {
        picker.selected - results_height + 1
    } else {
        0
    };

    for (display_idx, &filtered_idx) in picker
        .filtered
        .iter()
        .skip(start)
        .take(results_height)
        .enumerate()
    {
        let path = &picker.candidates[filtered_idx];
        let actual_idx = start + display_idx;
        let style = if actual_idx == picker.selected {
            selection_style
        } else {
            text_style
        };

        // Truncate long paths
        let max_w = inner.width as usize - 1;
        let display = if path.len() > max_w {
            format!("…{}", &path[path.len() - max_w + 1..])
        } else {
            path.clone()
        };

        lines.push(Line::from(Span::styled(display, style)));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);

    // Position cursor at end of query input
    frame.set_cursor_position(Position::new(
        inner.x + 2 + picker.query.len() as u16,
        inner.y,
    ));
}

fn render_conversation_window(
    frame: &mut Frame,
    area: Rect,
    buf: &mae_core::Buffer,
    _win: &Window,
    focused: bool,
    editor: &Editor,
) {
    let border_style = if focused {
        ts(editor, "ui.window.border.active")
    } else {
        ts(editor, "ui.window.border")
    };

    let title = format!(" {} ", buf.name);
    let streaming_indicator = if let Some(conv) = buf.conversation.as_ref() {
        if conv.streaming {
            if let Some(start) = conv.streaming_start {
                let elapsed = start.elapsed().as_secs();
                format!(" [waiting... {}s] ", elapsed)
            } else {
                " [waiting...] ".to_string()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(format!("{}{}", title, streaming_indicator));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(ref conv) = buf.conversation {
        let rendered = conv.rendered_lines();
        let viewport_height = inner.height as usize;

        let start = if rendered.len() > viewport_height {
            rendered.len() - viewport_height
        } else {
            0
        };

        let mut lines: Vec<Line> = Vec::new();
        for rl in rendered.iter().skip(start).take(viewport_height) {
            let style = match rl.style {
                mae_core::conversation::LineStyle::RoleMarker => {
                    if rl.text.contains("[You]") {
                        ts(editor, "conversation.user")
                    } else if rl.text.contains("[AI]") {
                        ts(editor, "conversation.assistant")
                    } else {
                        ts(editor, "conversation.system")
                    }
                }
                mae_core::conversation::LineStyle::UserText => ts(editor, "ui.text"),
                mae_core::conversation::LineStyle::AssistantText => {
                    ts(editor, "conversation.assistant")
                }
                mae_core::conversation::LineStyle::ToolCallHeader => {
                    ts(editor, "conversation.tool")
                }
                mae_core::conversation::LineStyle::ToolResultText => {
                    ts(editor, "conversation.tool.result")
                }
                mae_core::conversation::LineStyle::SystemText => ts(editor, "conversation.system"),
                mae_core::conversation::LineStyle::Separator => Style::default(),
                mae_core::conversation::LineStyle::InputPrompt => ts(editor, "conversation.input"),
            };
            lines.push(Line::from(Span::styled(rl.text.clone(), style)));
        }

        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, inner);
    }
}

// ---------------------------------------------------------------------------
// Messages window (live view of editor.message_log)
// ---------------------------------------------------------------------------

fn render_messages_window(
    frame: &mut Frame,
    area: Rect,
    win: &Window,
    focused: bool,
    editor: &Editor,
) {
    let border_style = if focused {
        ts(editor, "ui.window.border.active")
    } else {
        ts(editor, "ui.window.border")
    };

    let entry_count = editor.message_log.len();
    let title = format!(" *Messages* ({}) ", entry_count);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let entries = editor.message_log.entries();
    let viewport_height = inner.height as usize;

    // Auto-scroll to bottom, but respect manual scroll_offset
    let total = entries.len();
    let start = if win.scroll_offset > 0 {
        win.scroll_offset.min(total)
    } else {
        total.saturating_sub(viewport_height)
    };

    let target_style = ts(editor, "diagnostic.target");

    let mut lines: Vec<Line> = Vec::new();
    for entry in entries.iter().skip(start).take(viewport_height) {
        let level_style = match entry.level {
            mae_core::MessageLevel::Error => ts(editor, "diagnostic.error"),
            mae_core::MessageLevel::Warn => ts(editor, "diagnostic.warn"),
            mae_core::MessageLevel::Info => ts(editor, "diagnostic.info"),
            mae_core::MessageLevel::Debug => ts(editor, "diagnostic.debug"),
            mae_core::MessageLevel::Trace => ts(editor, "diagnostic.trace"),
        };

        let level_tag = match entry.level {
            mae_core::MessageLevel::Error => "ERROR",
            mae_core::MessageLevel::Warn => " WARN",
            mae_core::MessageLevel::Info => " INFO",
            mae_core::MessageLevel::Debug => "DEBUG",
            mae_core::MessageLevel::Trace => "TRACE",
        };

        lines.push(Line::from(vec![
            Span::styled(format!("[{}]", level_tag), level_style),
            Span::raw(" "),
            Span::styled(format!("[{}]", entry.target), target_style),
            Span::raw(" "),
            Span::styled(&entry.message, ts(editor, "ui.text")),
        ]));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no messages)",
            ts(editor, "ui.text"),
        )));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Is `new` a higher-priority diagnostic severity than `cur`?
/// Ordering: Error > Warning > Information > Hint > None.
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

pub fn gutter_width(line_count: usize) -> usize {
    let digits = if line_count == 0 {
        1
    } else {
        (line_count as f64).log10().floor() as usize + 1
    };
    digits.max(2) + 1
}
