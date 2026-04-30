use std::io::{self, Stdout};

use crossterm::{
    cursor::SetCursorStyle,
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use mae_core::{Editor, SyntaxSpanMap};
use mae_shell::ShellTerminal;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::collections::HashMap;

mod buffer_render;
mod conversation_render;
mod cursor;
mod debug_render;
mod file_tree_render;
mod help_render;
mod messages_render;
mod popup_render;
mod shell_render;
pub mod splash_render;
mod status_render;
mod theme_convert;
mod which_key_render;

// Re-export gutter_width for external use.
pub use mae_core::render_common::gutter::gutter_width;

/// Backend-agnostic rendering interface.
///
/// Emacs lesson: `xdisp.c` is 38,605 lines because the display engine is
/// tightly coupled to X11/GTK/NS backends. This trait ensures MAE's rendering
/// backends (terminal, GUI) are interchangeable without touching the core.
///
/// The terminal backend (ratatui/crossterm) is the default. The GUI backend
/// (winit/skia) is selected with `--gui` and provides direct OS-level key
/// access, variable-height lines, inline images, and PDF preview.
pub trait Renderer {
    /// Render the current editor state to the display.
    fn render(
        &mut self,
        editor: &mut Editor,
        shells: &HashMap<usize, ShellTerminal>,
    ) -> io::Result<()>;

    /// Return the display size as (width, height).
    /// Terminal: (columns, rows). GUI: (columns, rows) based on font metrics.
    fn size(&self) -> io::Result<(u16, u16)>;

    /// Return the number of visible text lines (excluding chrome like
    /// status bar, command line, etc.).
    fn viewport_height(&self) -> io::Result<usize>;

    /// Tear down the rendering backend (restore terminal state, close window, etc.).
    fn cleanup(&mut self) -> io::Result<()>;
}

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
}

impl Renderer for TerminalRenderer {
    fn render(
        &mut self,
        editor: &mut Editor,
        shells: &HashMap<usize, ShellTerminal>,
    ) -> io::Result<()> {
        let _span = tracing::trace_span!("tui_render").entered();
        // Set terminal cursor style based on mode (bar for insert-like, block otherwise).
        let cursor_style = match editor.mode {
            mae_core::Mode::Insert | mae_core::Mode::ConversationInput => SetCursorStyle::SteadyBar,
            _ => SetCursorStyle::SteadyBlock,
        };
        execute!(self.terminal.backend_mut(), cursor_style)?;
        self.terminal.draw(|frame| {
            render_frame(frame, editor, shells);
        })?;
        Ok(())
    }

    fn size(&self) -> io::Result<(u16, u16)> {
        let size = self.terminal.size()?;
        Ok((size.width, size.height))
    }

    fn viewport_height(&self) -> io::Result<usize> {
        let size = self.terminal.size()?;
        // Subtract 2 for status bar and command/message line
        Ok((size.height as usize).saturating_sub(2))
    }

    fn cleanup(&mut self) -> io::Result<()> {
        disable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            SetCursorStyle::DefaultUserShape,
            LeaveAlternateScreen
        )?;
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
    // Uses stale spans during typing; deferred reparse happens in the event loop.
    let syntax_spans = mae_core::syntax::compute_visible_syntax_spans(editor);
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
    } else if !editor.which_key_prefix.is_empty() || editor.buffer_keys_popup {
        let (entries, title_override) = if editor.buffer_keys_popup {
            let kind = editor.active_buffer().kind;
            use mae_core::buffer_mode::BufferMode;
            let title = kind.mode_name().to_string();
            (editor.buffer_keys_entries(), Some(title))
        } else {
            (editor.which_key_entries_for_current_keymap(), None)
        };

        let cols = (area.width as usize / 25).max(1);
        let rows = entries.len().div_ceil(cols);
        let popup_height = (rows as u16 + 2).min(area.height / 2).max(3);

        let chunks =
            Layout::vertical([Constraint::Min(1), Constraint::Length(popup_height)]).split(area);

        render_window_area(frame, chunks[0], editor, &syntax_spans, shells);
        which_key_render::render_which_key_popup(
            frame,
            chunks[1],
            editor,
            &entries,
            title_override.as_deref(),
        );
    } else if mae_core::render_common::splash::should_show_splash(editor) {
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

        splash_render::render_splash_if_needed(frame, chunks[0], editor);
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

// compute_visible_syntax_spans is now in mae_core::syntax (shared by all renderers).

// ---------------------------------------------------------------------------
// Window area dispatch
// ---------------------------------------------------------------------------

fn render_window_area(
    frame: &mut Frame,
    area: Rect,
    editor: &Editor,
    syntax_spans: &SyntaxSpanMap,
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
                    // Route through standard render pipeline with inline markdown.
                    let conv_spans = if let Some(conv) = buf.conversation() {
                        conv.highlight_spans_with_markup(buf.rope())
                    } else {
                        Vec::new()
                    };
                    buffer_render::render_window(
                        frame,
                        ratatui_rect,
                        buf,
                        win,
                        is_focused,
                        editor,
                        Some(&conv_spans),
                    );
                }
                mae_core::BufferKind::Messages => {
                    messages_render::render_messages_window(
                        frame,
                        ratatui_rect,
                        buf,
                        win,
                        is_focused,
                        editor,
                    );
                }
                mae_core::BufferKind::Help => {
                    let help_spans = mae_core::render_common::help::compute_help_spans(buf);

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
                mae_core::BufferKind::GitStatus => {
                    let git_spans =
                        mae_core::render_common::git_status::compute_git_status_spans(buf);
                    buffer_render::render_window(
                        frame,
                        ratatui_rect,
                        buf,
                        win,
                        is_focused,
                        editor,
                        Some(&git_spans),
                    );
                }
                mae_core::BufferKind::Debug => {
                    debug_render::render_debug_window(
                        frame,
                        ratatui_rect,
                        buf,
                        win,
                        is_focused,
                        editor,
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
                mae_core::BufferKind::Visual => {
                    if let Some(vb) = buf.visual() {
                        render_visual_buffer(frame, ratatui_rect, vb);
                    }
                }
                mae_core::BufferKind::FileTree => {
                    file_tree_render::render_file_tree_window(
                        frame,
                        ratatui_rect,
                        buf,
                        win,
                        is_focused,
                        editor,
                    );
                }
                _ => {
                    // Diff buffers get line-level diff highlighting.
                    let diff_spans_storage;
                    let spans = if buf.name == "*AI-Diff*" {
                        diff_spans_storage = mae_core::diff::diff_highlight_spans(buf.rope());
                        Some(diff_spans_storage.as_slice())
                    } else {
                        syntax_spans.get(&win.buffer_idx).map(|v| v.as_slice())
                    };
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

fn render_visual_buffer(frame: &mut Frame, area: Rect, vb: &mae_core::visual_buffer::VisualBuffer) {
    let count = vb.elements.len();
    let text = format!("[Visual Buffer: {} elements]", count);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Visual Debugger ")
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center),
        Rect::new(inner.x, inner.y + inner.height / 2, inner.width, 1),
    );
}
