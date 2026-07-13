//! mae-renderer: Display rendering — Renderer trait + terminal backend.
//!
//! @stability: stable
//! @since: 0.1.0

use std::io::{self, Stdout};

use crossterm::{
    cursor::SetCursorStyle,
    event::{DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture},
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
mod graph_view_render;
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
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableFocusChange
        )?;
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
            DisableFocusChange,
            DisableMouseCapture,
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

    // Pre-compute markup spans for visible org/markdown buffers (cache by generation).
    // Large files (>5K lines) use viewport-local computation.
    {
        let visible: Vec<(usize, usize)> = editor
            .window_mgr
            .iter_windows()
            .map(|w| (w.buffer_idx, w.scroll_offset))
            .collect();
        let area_height = area.height as usize;
        for &(bi, scroll) in &visible {
            if bi >= editor.buffers.len() {
                continue;
            }
            let flavor = editor.effective_markup_flavor(bi);
            if flavor == mae_core::MarkupFlavor::None {
                continue;
            }
            let gen = editor.buffers[bi].generation;
            let line_count = editor.buffers[bi].rope().len_lines();
            let is_large = line_count > editor.large_file_lines;
            let (vp_start, vp_end) = if is_large {
                let vh = area_height;
                (
                    scroll.saturating_sub(vh * 2),
                    (scroll + vh * 3).min(line_count),
                )
            } else {
                (0, line_count)
            };
            let needs_update = editor
                .markup_cache
                .get(&bi)
                .is_none_or(|c| !c.covers(gen, flavor, vp_start, vp_end));
            if needs_update {
                if is_large {
                    let rope = editor.buffers[bi].rope().clone();
                    let (byte_offset, spans) =
                        mae_core::compute_markup_spans_for_range(&rope, flavor, vp_start, vp_end);
                    editor.markup_cache.insert(
                        bi,
                        mae_core::MarkupCache {
                            generation: gen,
                            flavor,
                            line_start: vp_start,
                            line_end: vp_end,
                            byte_offset,
                            spans,
                        },
                    );
                } else {
                    let source: String = editor.buffers[bi].rope().chars().collect();
                    let spans = mae_core::compute_markup_spans(&source, flavor);
                    editor.markup_cache.insert(
                        bi,
                        mae_core::MarkupCache {
                            generation: gen,
                            flavor,
                            line_start: 0,
                            line_end: line_count,
                            byte_offset: 0,
                            spans,
                        },
                    );
                }
            }
        }
    }

    let editor: &Editor = editor;

    // Overlay PRIORITY is shared with the GUI via `render_common::overlay::active_overlay`
    // (single source of truth) so the two backends can't diverge — the bug where the TUI
    // drew the blocking mini-dialog only under the command palette while the GUI drew it
    // on top (B-22a). A blocking modal is highest priority, matching the input side (B-22b).
    use mae_core::render_common::overlay::{active_overlay, ActiveOverlay};
    let overlay = active_overlay(editor);
    if overlay == ActiveOverlay::MiniDialog {
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

        render_window_area(frame, chunks[0], editor, &syntax_spans, shells);
        status_render::render_status_bar(frame, chunks[1], editor);
        status_render::render_command_line(frame, chunks[2], editor);
        // render_command_palette draws the mini-dialog (it checks mini_dialog first).
        popup_render::render_command_palette(frame, area, editor);
    } else if overlay == ActiveOverlay::FilePicker {
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
    } else if overlay == ActiveOverlay::FileBrowser {
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
    } else if overlay == ActiveOverlay::CommandPalette {
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
    } else if overlay == ActiveOverlay::WhichKey {
        let (entries, title_override) = if editor.buffer_keys_popup {
            let kind = editor.active_buffer().kind;
            use mae_core::buffer_mode::BufferMode;
            let title = kind.mode_name().to_string();
            (editor.buffer_keys_entries(), Some(title))
        } else {
            (editor.which_key_entries_for_current_keymap(), None)
        };

        let separator = editor
            .get_option("which-key-separator")
            .map(|(v, _)| v)
            .unwrap_or_else(|| " ".to_string());
        let max_desc: usize = editor
            .get_option("which-key-max-desc-length")
            .and_then(|(v, _)| v.parse().ok())
            .unwrap_or(40);
        let sep_width = mae_core::text_utils::display_width(&separator);
        let (_col_w, num_cols) = mae_core::text_utils::which_key_column_layout(
            &entries,
            area.width as usize - 2,
            sep_width,
            max_desc,
        );
        let entry_rows = entries.len().div_ceil(num_cols);
        let max_pct: usize = editor
            .get_option("which-key-max-height-pct")
            .and_then(|(v, _)| v.parse().ok())
            .unwrap_or(mae_core::text_utils::WK_MAX_HEIGHT_PCT_DEFAULT)
            .clamp(
                mae_core::text_utils::WK_MAX_HEIGHT_PCT_MIN,
                mae_core::text_utils::WK_MAX_HEIGHT_PCT_MAX,
            );
        let max_h = (area.height as usize) * max_pct / 100;
        let popup_height = ((entry_rows + 2) as u16)
            .min(max_h as u16)
            .max(mae_core::text_utils::WK_MIN_HEIGHT as u16);

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
    } else if overlay == ActiveOverlay::Splash {
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
        if !editor.lsp.completion_items.is_empty() {
            popup_render::render_completion_popup(frame, chunks[0], editor);
        }
        if editor.lsp.hover_popup.is_some() {
            popup_render::render_hover_popup(frame, chunks[0], editor);
        }
        if editor.kb_preview_popup().is_some() {
            popup_render::render_kb_preview_popup(frame, chunks[0], editor);
        }
        if editor.lsp.code_action_menu.is_some() {
            popup_render::render_code_action_popup(frame, chunks[0], editor);
        }
        if editor.lsp.signature_help.is_some() {
            popup_render::render_signature_help_popup(frame, chunks[0], editor);
        }
        if editor.lsp.peek_state.is_some() {
            popup_render::render_peek_definition_popup(frame, chunks[0], editor);
        }
        if editor.lsp.symbol_outline.is_some() {
            popup_render::render_symbol_outline_popup(frame, chunks[0], editor);
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
                mae_core::BufferKind::Graph => {
                    // TUI has no Skia canvas to draw `GraphView.scene`'s
                    // positions with, so it reuses the existing KB
                    // "** Neighborhood" textual machinery for the graph's
                    // center node instead (`render_graph_view_as_text`,
                    // GUI-primary/TUI-degraded — same precedent as other
                    // buffer kinds).
                    graph_view_render::render_graph_view_window(
                        frame,
                        ratatui_rect,
                        buf,
                        is_focused,
                        editor,
                    );
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
                    // Standard text pipeline: shared span selection for Conversation,
                    // Help, GitStatus, *AI-Diff*; syntax spans for Text/Preview/Dashboard.
                    // Text buffers with a markup flavor get inline markup spans merged.
                    let owned_spans: Option<Vec<mae_core::HighlightSpan>>;
                    let spans = if let Some(shared) =
                        mae_core::render_common::spans::highlight_spans_for_buffer(buf)
                    {
                        owned_spans = Some(shared);
                        owned_spans.as_deref()
                    } else {
                        let flavor = editor.effective_markup_flavor(win.buffer_idx);
                        if flavor != mae_core::MarkupFlavor::None {
                            let mut enriched = syntax_spans
                                .get(&win.buffer_idx)
                                .map(|v| v.as_ref().clone())
                                .unwrap_or_default();
                            let gen = buf.generation;
                            let cached = editor.markup_cache.get(&win.buffer_idx);
                            if let Some(c) =
                                cached.filter(|c| c.generation == gen && c.flavor == flavor)
                            {
                                enriched.extend_from_slice(&c.spans);
                            } else {
                                let source: String = buf.rope().chars().collect();
                                enriched.extend(mae_core::compute_markup_spans(&source, flavor));
                            }
                            enriched.sort_by_key(|s| s.byte_start);
                            owned_spans = Some(enriched);
                            owned_spans.as_deref()
                        } else {
                            owned_spans = None;
                            let _ = &owned_spans;
                            syntax_spans.get(&win.buffer_idx).map(|v| v.as_slice())
                        }
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
                    // Overlay remote collaborative cursors/selections.
                    if buf.collab_doc_id.is_some() {
                        use ratatui::widgets::{Block, Borders};
                        let inner = Block::default().borders(Borders::ALL).inner(ratatui_rect);
                        let gutter_w =
                            mae_core::render_common::gutter::gutter_width(buf.rope().len_lines());
                        buffer_render::render_remote_cursors(
                            frame, inner, editor, win, buf, gutter_w,
                        );
                    }
                }
            }
        }
    }

    // Breadcrumb bar: overlay on top of the focused window.
    if editor.show_breadcrumbs && editor.lsp.breadcrumbs.is_some() {
        if let Some(focused_rect) = rects
            .iter()
            .find(|(id, _)| *id == focused_id)
            .map(|(_, r)| r)
        {
            let bar_rect = Rect::new(focused_rect.x, focused_rect.y, focused_rect.width, 1);
            render_breadcrumb_bar(frame, bar_rect, editor);
        }
    }
}

fn render_breadcrumb_bar(frame: &mut Frame, area: Rect, editor: &Editor) {
    let crumbs = match &editor.lsp.breadcrumbs {
        Some(c) if !c.is_empty() => c,
        _ => return,
    };
    let text = crumbs.join(" > ");
    let display: String = text.chars().take(area.width as usize).collect();
    let style = Style::default().fg(Color::DarkGray).bg(Color::Black);
    let bar = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(Paragraph::new(display).style(style), bar);
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
