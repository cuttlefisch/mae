use std::io::{self, Stdout};

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use mae_core::{Editor, HighlightSpan};
use mae_shell::ShellTerminal;
use ratatui::prelude::*;
use std::collections::HashMap;

mod buffer_render;
mod conversation_render;
mod cursor;
mod help_render;
mod messages_render;
mod popup_render;
mod shell_render;
mod splash_render;
mod status_render;
mod theme_convert;
mod which_key_render;

// Re-export gutter_width for external use (e.g. cursor module).
pub use buffer_render::gutter_width;

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

    pub fn render(
        &mut self,
        editor: &mut Editor,
        shells: &HashMap<usize, ShellTerminal>,
    ) -> io::Result<()> {
        self.terminal.draw(|frame| {
            render_frame(frame, editor, shells);
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
// Frame layout (orchestrator)
// ---------------------------------------------------------------------------

/// Pure rendering function: Editor state in, frame out.
/// No side effects, no global state. Emacs lesson: this is the anti-xdisp.c.
fn render_frame(frame: &mut Frame, editor: &mut Editor, shells: &HashMap<usize, ShellTerminal>) {
    let area = frame.area();

    // Pre-compute syntax-highlight spans for every visible text buffer.
    let syntax_spans = compute_visible_syntax_spans(editor);
    let editor: &Editor = editor;

    if editor.file_picker.is_some() {
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

        render_window_area(frame, chunks[0], editor, &syntax_spans, shells);
        status_render::render_status_bar(frame, chunks[1], editor);
        status_render::render_command_line(frame, chunks[2], editor);
        popup_render::render_file_picker(frame, area, editor);
    } else if editor.file_browser.is_some() {
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

        render_window_area(frame, chunks[0], editor, &syntax_spans, shells);
        status_render::render_status_bar(frame, chunks[1], editor);
        status_render::render_command_line(frame, chunks[2], editor);
        popup_render::render_file_browser(frame, area, editor);
    } else if editor.command_palette.is_some() {
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

        render_window_area(frame, chunks[0], editor, &syntax_spans, shells);
        status_render::render_status_bar(frame, chunks[1], editor);
        status_render::render_command_line(frame, chunks[2], editor);
        popup_render::render_command_palette(frame, area, editor);
    } else if !editor.which_key_prefix.is_empty() {
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

        render_window_area(frame, chunks[0], editor, &syntax_spans, shells);
        which_key_render::render_which_key_popup(frame, chunks[1], editor, &entries);
    } else if splash_render::should_show_splash(editor) {
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

        splash_render::render_splash(frame, chunks[0], editor);
        status_render::render_status_bar(frame, chunks[1], editor);
        status_render::render_command_line(frame, chunks[2], editor);
    } else {
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

        render_window_area(frame, chunks[0], editor, &syntax_spans, shells);
        status_render::render_status_bar(frame, chunks[1], editor);
        status_render::render_command_line(frame, chunks[2], editor);
        // Shell buffers set their own cursor in render_shell_grid.
        if editor.mode != mae_core::Mode::ShellInsert {
            cursor::set_cursor(frame, editor, chunks[0], chunks[2]);
        }
        if !editor.completion_items.is_empty() {
            popup_render::render_completion_popup(frame, chunks[0], editor);
        }
    }
}

/// Compute tree-sitter highlight spans for every text buffer visible in the
/// current window layout.
fn compute_visible_syntax_spans(editor: &mut Editor) -> HashMap<usize, Vec<HighlightSpan>> {
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
// Window area dispatch
// ---------------------------------------------------------------------------

fn render_window_area(
    frame: &mut Frame,
    area: Rect,
    editor: &Editor,
    syntax_spans: &HashMap<usize, Vec<HighlightSpan>>,
    shells: &HashMap<usize, ShellTerminal>,
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
                    conversation_render::render_conversation_window(
                        frame,
                        ratatui_rect,
                        buf,
                        win,
                        is_focused,
                        editor,
                    );
                }
                mae_core::BufferKind::Messages => {
                    messages_render::render_messages_window(
                        frame,
                        ratatui_rect,
                        win,
                        is_focused,
                        editor,
                    );
                }
                mae_core::BufferKind::Help => {
                    // Help buffers go through the normal render path.
                    // Convert HelpLinkSpans → HighlightSpans for link styling.
                    let help_spans: Vec<HighlightSpan> = buf
                        .help_view
                        .as_ref()
                        .map(|view| {
                            view.rendered_links
                                .iter()
                                .enumerate()
                                .map(|(i, link)| {
                                    let is_focused_link = view.focused_link == Some(i);
                                    HighlightSpan {
                                        byte_start: link.byte_start,
                                        byte_end: link.byte_end,
                                        theme_key: if is_focused_link {
                                            "ui.selection"
                                        } else {
                                            "markup.link"
                                        },
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    buffer_render::render_window(
                        frame,
                        ratatui_rect,
                        buf,
                        win,
                        is_focused,
                        editor,
                        Some(&help_spans),
                    );
                }
                mae_core::BufferKind::Shell => {
                    if let Some(shell) = shells.get(&win.buffer_idx) {
                        shell_render::render_shell_window(
                            frame,
                            ratatui_rect,
                            buf,
                            win,
                            is_focused,
                            editor,
                            shell,
                        );
                    }
                }
                _ => {
                    let spans = syntax_spans.get(&win.buffer_idx).map(|v| v.as_slice());
                    buffer_render::render_window(
                        frame,
                        ratatui_rect,
                        buf,
                        win,
                        is_focused,
                        editor,
                        spans,
                    );
                }
            }
        }
    }
}
