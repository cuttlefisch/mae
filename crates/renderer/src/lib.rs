use std::io::{self, Stdout};

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use mae_core::{
    grapheme, DiagnosticSeverity, Editor, HighlightSpan, Key, Mode, NamedColor, ThemeColor,
    ThemeStyle, VisualType, Window,
};
use std::collections::HashMap;
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

    pub fn render(&mut self, editor: &mut Editor) -> io::Result<()> {
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
fn render_frame(frame: &mut Frame, editor: &mut Editor) {
    let area = frame.area();

    // Pre-compute syntax-highlight spans for every visible text buffer.
    // Done up front so the rest of the render pipeline can borrow editor
    // immutably.
    let syntax_spans = compute_visible_syntax_spans(editor);
    let editor: &Editor = editor;

    if editor.file_picker.is_some() {
        // File picker overlay on top of normal layout
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

        render_window_area(frame, chunks[0], editor, &syntax_spans);
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

        render_window_area(frame, chunks[0], editor, &syntax_spans);
        render_which_key_popup(frame, chunks[1], editor, &entries);
    } else {
        // Normal layout: [window area | status bar | command line]
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

        render_window_area(frame, chunks[0], editor, &syntax_spans);
        render_status_bar(frame, chunks[1], editor);
        render_command_line(frame, chunks[2], editor);
        set_cursor(frame, editor, chunks[0], chunks[2]);
        // Completion popup rendered on top after cursor is set.
        if !editor.completion_items.is_empty() {
            render_completion_popup(frame, chunks[0], editor);
        }
    }
}

/// Compute tree-sitter highlight spans for every text buffer visible in the
/// current window layout. Other buffer kinds (Conversation, Messages) skip
/// syntax highlighting. Each buffer is parsed at most once per frame; the
/// `SyntaxMap` cache hands back `Vec<HighlightSpan>` directly on subsequent
/// renders until an edit invalidates it.
fn compute_visible_syntax_spans(editor: &mut Editor) -> HashMap<usize, Vec<HighlightSpan>> {
    // Collect (buf_idx, source_string) for each visible, text-kind buffer,
    // deduped. We snapshot the source to release the immutable borrow on
    // editor before calling `syntax.spans_for` (which needs &mut).
    let mut targets: Vec<(usize, String)> = Vec::new();
    for win in editor.window_mgr.iter_windows() {
        let idx = win.buffer_idx;
        if targets.iter().any(|(i, _)| *i == idx) {
            continue;
        }
        let Some(buf) = editor.buffers.get(idx) else {
            continue;
        };
        if !matches!(buf.kind, mae_core::BufferKind::Text) {
            continue;
        }
        if editor.syntax.language_of(idx).is_none() {
            continue;
        }
        let source: String = buf.rope().chars().collect();
        targets.push((idx, source));
    }

    let mut out = HashMap::new();
    for (idx, src) in targets {
        if let Some(spans) = editor.syntax.spans_for(idx, &src) {
            out.insert(idx, spans.to_vec());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Window area
// ---------------------------------------------------------------------------

fn render_window_area(
    frame: &mut Frame,
    area: Rect,
    editor: &Editor,
    syntax_spans: &HashMap<usize, Vec<HighlightSpan>>,
) {
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
                    let spans = syntax_spans.get(&win.buffer_idx).map(|v| v.as_slice());
                    render_window(frame, ratatui_rect, buf, win, is_focused, editor, spans);
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
            // command_cursor is a byte offset; count chars for display width.
            let cursor_col = editor.command_line[..editor.command_cursor.min(editor.command_line.len())]
                .chars()
                .count() as u16;
            frame.set_cursor_position(Position::new(
                cmd_area.x + 1 + cursor_col,
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
    syntax_spans: Option<&[HighlightSpan]>,
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

    render_buffer(frame, inner, buf, win, editor, syntax_spans);
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
    syntax_spans: Option<&[HighlightSpan]>,
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
    let has_syntax = syntax_spans.map(|s| !s.is_empty()).unwrap_or(false);
    let needs_spans = highlight_search || highlight_selection || has_syntax;

    // Per-line worst-severity diagnostic for gutter markers. We only need
    // the highest severity per line (Error > Warning > Information > Hint).
    let line_severities: HashMap<u32, DiagnosticSeverity> = {
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
    };

    // Breakpoint lines + stopped line for the current buffer's source.
    // DAP reports lines 1-indexed; the renderer's `line_idx` is 0-indexed,
    // so we store 0-indexed values here to match the rendering loop.
    let (breakpoint_lines, stopped_line): (std::collections::HashSet<u32>, Option<u32>) = {
        let mut bps: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut stopped: Option<u32> = None;
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
    };
    let stopped_line_style = ts(editor, "debug.current_line");

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

            // Gutter layout: "{line_num_padded}{marker_or_space}"
            // total width = gutter_w. Marker priority: stopped-line > breakpoint
            // > diagnostic — DAP state is more ephemeral and thus more salient
            // than LSP diagnostics; the user needs to see it first.
            let line_num = format!("{:>width$}", line_idx + 1, width = gutter_w - 1);
            let line_idx_u32 = line_idx as u32;
            let marker = resolve_gutter_marker(
                stopped_line == Some(line_idx_u32),
                breakpoint_lines.contains(&line_idx_u32),
                line_severities.get(&line_idx_u32).copied(),
            );
            let (marker_char, marker_style) = match marker.glyph_and_theme_key() {
                Some((ch, key)) => (ch, ts(editor, key)),
                None => (' ', gutter_style),
            };
            // Whole-line background cue for the stopped line. Falls through
            // per-char overrides (selection, search) naturally because
            // `line_text_style` is only the *base* style.
            let line_text_style = if stopped_line == Some(line_idx_u32) {
                stopped_line_style
            } else {
                text_style
            };

            if needs_spans {
                let line_char_start = buf.rope().line_to_char(line_idx);
                let full_chars: Vec<char> = full_display.chars().collect();
                let full_count = full_chars.len();
                let line_char_end = line_char_start + full_count;

                // Build a per-char style array over the full line. Initialize
                // with `line_text_style` so a stopped-line background survives
                // under syntax highlights (which typically only set fg).
                let mut styles: Vec<Style> = vec![line_text_style; full_count];

                // Apply tree-sitter syntax highlights first (lowest priority —
                // everything else overwrites these). Spans are byte-based;
                // convert each intersecting span to the current line's char
                // coordinate space using the rope's byte_to_char mapping.
                //
                // We `patch` rather than replace so the stopped-line bg shows
                // through syntax fg overrides.
                if let Some(spans) = syntax_spans {
                    let line_byte_start = buf.rope().char_to_byte(line_char_start);
                    let line_byte_end = buf.rope().char_to_byte(line_char_end);
                    for span in spans {
                        if span.byte_end <= line_byte_start
                            || span.byte_start >= line_byte_end
                        {
                            continue;
                        }
                        let sb = span.byte_start.max(line_byte_start);
                        let eb = span.byte_end.min(line_byte_end);
                        let sc = buf
                            .rope()
                            .byte_to_char(sb)
                            .saturating_sub(line_char_start);
                        let ec = buf
                            .rope()
                            .byte_to_char(eb)
                            .saturating_sub(line_char_start)
                            .min(full_count);
                        let style = ts(editor, span.theme_key);
                        for s in styles[sc..ec].iter_mut() {
                            *s = s.patch(style);
                        }
                    }
                }

                // Apply selection highlight (overrides syntax)
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
                    Span::styled(display, line_text_style),
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

    // Recording indicator takes priority over the normal mode label.
    let recording_label: String;
    let mode_str = if editor.macro_recording {
        recording_label = format!(
            " REC @{} ",
            editor.macro_register.unwrap_or('?')
        );
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
        }
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
// LSP completion popup
// ---------------------------------------------------------------------------

/// Render a small completion popup just below the cursor position.
/// Shows up to 10 items; the selected item is highlighted.
fn render_completion_popup(frame: &mut Frame, editor_area: Rect, editor: &Editor) {
    let items = &editor.completion_items;
    if items.is_empty() {
        return;
    }

    // Find cursor screen position (row, col) within editor_area.
    let win = editor.window_mgr.focused_window();
    let scroll_row = win.scroll_offset;
    let cursor_screen_row = win.cursor_row.saturating_sub(scroll_row) as u16;
    let cursor_screen_col = win.cursor_col as u16;

    // Popup dimensions: up to 10 items, width = longest label + detail + padding.
    const MAX_ITEMS: usize = 10;
    let visible_count = items.len().min(MAX_ITEMS) as u16;
    let popup_width = items
        .iter()
        .take(MAX_ITEMS)
        .map(|i| {
            let detail_len = i.detail.as_deref().map(|d| d.len() + 2).unwrap_or(0);
            i.label.len() + detail_len + 4 // sigil + spaces + padding
        })
        .max()
        .unwrap_or(20)
        .min(50) as u16;
    let popup_height = visible_count + 2; // border top + bottom

    // Position popup below cursor; flip above if too close to bottom edge.
    let popup_top = if cursor_screen_row + 1 + popup_height < editor_area.height {
        editor_area.y + cursor_screen_row + 1
    } else {
        editor_area.y + cursor_screen_row.saturating_sub(popup_height)
    };
    let popup_left = (editor_area.x + cursor_screen_col).min(
        editor_area.x + editor_area.width.saturating_sub(popup_width),
    );

    let popup_area = Rect {
        x: popup_left,
        y: popup_top,
        width: popup_width,
        height: popup_height,
    };

    // Style helpers.
    let border_style = ts(editor, "ui.window.border");
    let normal_style = ts(editor, "ui.popup.text");
    let selected_style = ts(editor, "ui.popup.key"); // reuse highlighted key style

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(normal_style);
    frame.render_widget(block, popup_area);

    let inner = Rect {
        x: popup_area.x + 1,
        y: popup_area.y + 1,
        width: popup_area.width.saturating_sub(2),
        height: popup_area.height.saturating_sub(2),
    };

    let lines: Vec<Line> = items
        .iter()
        .take(MAX_ITEMS)
        .enumerate()
        .map(|(i, item)| {
            let style = if i == editor.completion_selected {
                selected_style
            } else {
                normal_style
            };
            let sigil = item.kind_sigil;
            let label = &item.label;
            let detail_part = item
                .detail
                .as_deref()
                .map(|d| {
                    let truncated: String = d.chars().take(20).collect();
                    format!("  {}", truncated)
                })
                .unwrap_or_default();
            let text = format!("{} {}{}", sigil, label, detail_part);
            // Truncate to inner width
            let max_chars = inner.width as usize;
            let display: String = text.chars().take(max_chars).collect();
            Line::styled(display, style)
        })
        .collect();

    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

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

/// A gutter marker for a single line. Variants are ordered by display
/// priority — a Stopped line hides a Breakpoint marker, which in turn
/// hides a Diagnostic marker. Only one glyph fits in the gutter column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GutterMarker {
    None,
    Diagnostic(DiagnosticSeverity),
    Breakpoint,
    Stopped,
}

impl GutterMarker {
    /// Returns the glyph + theme key to render this marker with, or `None`
    /// when the gutter column should stay blank.
    fn glyph_and_theme_key(self) -> Option<(char, &'static str)> {
        match self {
            GutterMarker::None => None,
            GutterMarker::Diagnostic(sev) => Some((sev.gutter_char(), sev.theme_key())),
            // Filled circle is the ubiquitous breakpoint glyph (Helix, VSCode).
            GutterMarker::Breakpoint => Some(('●', "debug.breakpoint")),
            // Right-pointing triangle cues execution arrow.
            GutterMarker::Stopped => Some(('▶', "debug.current_line")),
        }
    }
}

/// Pick the gutter marker for a line given all possible contributors.
/// Priority: Stopped > Breakpoint > Diagnostic > None.
fn resolve_gutter_marker(
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

pub fn gutter_width(line_count: usize) -> usize {
    let digits = if line_count == 0 {
        1
    } else {
        (line_count as f64).log10().floor() as usize + 1
    };
    digits.max(2) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Gutter marker priority ----------------------------------------

    #[test]
    fn marker_priority_stopped_beats_breakpoint_and_diagnostic() {
        let m = resolve_gutter_marker(true, true, Some(DiagnosticSeverity::Error));
        assert_eq!(m, GutterMarker::Stopped);
    }

    #[test]
    fn marker_priority_breakpoint_beats_diagnostic() {
        let m = resolve_gutter_marker(false, true, Some(DiagnosticSeverity::Error));
        assert_eq!(m, GutterMarker::Breakpoint);
    }

    #[test]
    fn marker_priority_diagnostic_when_no_debug_state() {
        let m = resolve_gutter_marker(false, false, Some(DiagnosticSeverity::Warning));
        assert_eq!(m, GutterMarker::Diagnostic(DiagnosticSeverity::Warning));
    }

    #[test]
    fn marker_none_when_nothing_present() {
        let m = resolve_gutter_marker(false, false, None);
        assert_eq!(m, GutterMarker::None);
    }

    // --- Marker glyph rendering ----------------------------------------

    #[test]
    fn stopped_glyph_uses_current_line_theme() {
        let (ch, key) = GutterMarker::Stopped.glyph_and_theme_key().unwrap();
        assert_eq!(ch, '▶');
        assert_eq!(key, "debug.current_line");
    }

    #[test]
    fn breakpoint_glyph_uses_debug_breakpoint_theme() {
        let (ch, key) = GutterMarker::Breakpoint.glyph_and_theme_key().unwrap();
        assert_eq!(ch, '●');
        assert_eq!(key, "debug.breakpoint");
    }

    #[test]
    fn diagnostic_glyph_matches_severity() {
        let cases = [
            DiagnosticSeverity::Error,
            DiagnosticSeverity::Warning,
            DiagnosticSeverity::Information,
            DiagnosticSeverity::Hint,
        ];
        for sev in cases {
            let (ch, key) = GutterMarker::Diagnostic(sev).glyph_and_theme_key().unwrap();
            assert_eq!(ch, sev.gutter_char());
            assert_eq!(key, sev.theme_key());
        }
    }

    #[test]
    fn none_marker_has_no_glyph() {
        assert!(GutterMarker::None.glyph_and_theme_key().is_none());
    }

    // --- gutter_width ---------------------------------------------------

    #[test]
    fn gutter_width_minimum_is_three() {
        // 1 digit + 1 marker col, but min-padded to 2 digits + 1 = 3.
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
}
