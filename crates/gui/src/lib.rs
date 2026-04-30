//! GUI rendering backend for MAE.
//!
//! Uses winit for window management + OS-level input, and skia-safe for
//! GPU-accelerated 2D rendering. This gives MAE direct key access (no host
//! terminal intercepting keybindings) and rich rendering capabilities:
//! variable font heights, inline images, PDF preview, font decorations.
//!
//! # Architecture
//!
//! - `GuiRenderer` implements `mae_renderer::Renderer` — drop-in replacement
//!   for the terminal backend, selected with `--gui`.
//! - `canvas` — Skia surface management and frame composition.
//! - `text` — Styled cell types for per-character rendering.
//! - `input` — winit KeyEvent/MouseEvent → mae_core::InputEvent translation.
//! - `theme` — ThemeStyle → Skia Color4f/Paint conversion.
//! - `cursor` — Mode-aware cursor rendering.
//! - `gutter` — Line numbers, breakpoint/diagnostic markers.
//! - `buffer_render` — Text buffer rendering with syntax, selection, search.
//! - `status_render` — Status bar and command line.
//! - `popup_render` — File picker, browser, command palette, completion, which-key.
//! - `splash_render` — Splash screen with ASCII art.
//! - `conversation_render` — AI conversation buffer rendering.
//! - `messages_render` — *Messages* log buffer rendering.
//! - `shell_render` — Terminal emulator buffer rendering.
//! - `debug_render` — DAP debug panel rendering.

// GUI renderers pass editor state through multiple rendering layers —
// many-argument functions are the natural pattern (same as terminal renderer).
#![allow(clippy::too_many_arguments)]

mod buffer_render;
mod canvas;
mod conversation_render;
mod cursor;
mod debug_render;
mod file_tree_render;
mod gutter;
mod input;
mod layout;
mod messages_render;
mod popup_render;
mod scrollbar;
mod shell_render;
mod splash_render;
mod status_render;
pub mod text;
pub mod theme;

use std::collections::HashMap;
use std::io;
use std::rc::Rc;

use mae_core::{BufferKind, Editor, HighlightSpan, SyntaxSpanMap};
use mae_renderer::Renderer;
use mae_shell::ShellTerminal;
use tracing::{debug, info, trace_span};
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

pub use input::{winit_event_to_input, winit_key_to_keypress, winit_mouse_button};

/// GUI renderer implementing the `Renderer` trait.
///
/// Manages a winit window with a Skia surface for GPU-accelerated rendering.
/// The window is created on first `render()` call (lazy initialization to
/// match the terminal backend's pattern).
pub struct GuiRenderer {
    window: Option<Rc<Window>>,
    canvas: Option<canvas::SkiaCanvas>,
    cols: u16,
    rows: u16,
    cell_width: f32,
    cell_height: f32,
    /// Timestamp of the last frame start, for FPS overlay.
    last_frame_start: Option<std::time::Instant>,
    /// Configured font family (None = use default fallback chain).
    font_family: Option<String>,
    /// Configured icon font family (None = use default fallback chain).
    icon_font_family: Option<String>,
    /// Configured font size (None = 14.0).
    font_size: Option<f32>,
    /// Cached FrameLayout from the last render of the focused window.
    /// Used by the mouse handler for pixel-precise click positioning.
    last_focused_layout: Option<layout::FrameLayout>,
}

impl GuiRenderer {
    /// Create a new GUI renderer. The window is not created until the event
    /// loop drives initialization via `ApplicationHandler::resumed()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            window: None,
            canvas: None,
            cols: 120,
            rows: 40,
            cell_width: 0.0,
            cell_height: 0.0,
            last_frame_start: None,
            font_family: None,
            icon_font_family: None,
            font_size: None,
            last_focused_layout: None,
        }
    }

    /// Set the font family and size before window initialization.
    pub fn set_font_config(
        &mut self,
        family: Option<String>,
        icon_family: Option<String>,
        size: Option<f32>,
    ) {
        self.font_family = family;
        self.icon_font_family = icon_family;
        self.font_size = size;
    }

    /// Initialize the window and Skia canvas. Called from the event loop.
    pub fn init_window(&mut self, event_loop: &ActiveEventLoop) -> io::Result<()> {
        let attrs = Window::default_attributes()
            .with_title("MAE — Modern AI Editor")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));

        let window = event_loop
            .create_window(attrs)
            .map_err(|e| io::Error::other(e.to_string()))?;

        let window = Rc::new(window);
        let size = window.inner_size();
        let canvas = canvas::SkiaCanvas::new(
            size.width,
            size.height,
            window.clone(),
            self.font_family.as_deref(),
            self.icon_font_family.as_deref(),
            self.font_size,
        )?;

        // Compute cell dimensions from the default monospace font.
        let (cw, ch) = canvas.cell_size();
        self.cell_width = cw;
        self.cell_height = ch;
        self.cols = (size.width as f32 / cw) as u16;
        // Use floor to ensure rows * cell_height <= window height.
        // This prevents the bottom text row from overlapping the window border.
        self.rows = (size.height as f32 / ch).floor() as u16;

        info!(
            cols = self.cols,
            rows = self.rows,
            cell_w = cw,
            cell_h = ch,
            "GUI window initialized"
        );

        self.window = Some(window);
        self.canvas = Some(canvas);
        Ok(())
    }

    /// Update column/row counts after a resize.
    pub fn handle_resize(&mut self, width: u32, height: u32) {
        if let Some(canvas) = &mut self.canvas {
            canvas.resize(width, height);
            let (cw, ch) = canvas.cell_size();
            self.cell_width = cw;
            self.cell_height = ch;
            self.cols = (width as f32 / cw) as u16;
            self.rows = (height as f32 / ch).floor() as u16;
        }
    }

    /// Request a redraw of the window.
    pub fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Returns a reference to the window, if initialized.
    pub fn window(&self) -> Option<&Window> {
        self.window.as_deref()
    }

    /// Returns (cell_width, cell_height) in pixels. Used for mouse coordinate
    /// translation (pixel position → cell coordinates).
    pub fn cell_dimensions(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    /// Current font size. Returns the configured size or the default (14.0).
    pub fn current_font_size(&self) -> f32 {
        self.font_size.unwrap_or(14.0)
    }

    /// Access the cached FrameLayout from the last render.
    /// Used by the mouse handler for pixel-precise click positioning
    /// on scaled/folded lines.
    pub fn last_focused_layout(&self) -> Option<&layout::FrameLayout> {
        self.last_focused_layout.as_ref()
    }

    /// Apply a new font size at runtime — recreates font objects, recalculates
    /// cell metrics and column/row counts. This is the lisp-machine contract:
    /// `(set-option! "font-size" "20")` must take effect immediately.
    pub fn apply_font_size(&mut self, size: f32) {
        self.font_size = Some(size);
        if let Some(canvas) = &mut self.canvas {
            canvas.update_font_size(size);
            let (cw, ch) = canvas.cell_size();
            self.cell_width = cw;
            self.cell_height = ch;
            if let Some(window) = &self.window {
                let ws = window.inner_size();
                self.cols = (ws.width as f32 / cw) as u16;
                self.rows = (ws.height as f32 / ch).floor() as u16;
            }
        }
    }
}

impl Default for GuiRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer for GuiRenderer {
    fn render(
        &mut self,
        editor: &mut Editor,
        shells: &HashMap<usize, ShellTerminal>,
    ) -> io::Result<()> {
        let _span = trace_span!("gui_render").entered();
        let frame_start = std::time::Instant::now();

        let Some(canvas) = &mut self.canvas else {
            return Ok(());
        };

        let cols = self.cols as usize;
        let rows = self.rows as usize;

        // Compute frame ms from previous frame for FPS overlay.
        let frame_ms = self
            .last_frame_start
            .map(|prev| prev.elapsed().as_millis() as u64);

        // Begin frame.
        canvas.begin_frame(&editor.theme);

        // Clip rendering to allocated row height — prevents descender overflow.
        let clip_height = self.rows as f32 * self.cell_height;
        canvas.set_clip_height(clip_height);

        // Pre-compute syntax-highlight spans for every visible text buffer.
        // Uses stale spans during typing; deferred reparse happens in the event loop.
        let syntax_spans = mae_core::syntax::compute_visible_syntax_spans(editor);

        let editor: &Editor = editor;

        // Layout: window area = rows-2, status bar = 1, command line = 1.
        let status_row = rows.saturating_sub(2);
        let cmd_row = rows.saturating_sub(1);
        let window_height = rows.saturating_sub(2);

        // Track focused layout across render branches for mouse click caching.
        let mut focused_frame_layout: Option<layout::FrameLayout> = None;

        // Check for fullscreen overlays first.
        if editor.file_picker.is_some() {
            debug!("render: file_picker overlay");
            render_window_area(
                canvas,
                editor,
                &syntax_spans,
                shells,
                0,
                0,
                cols,
                window_height,
            );
            status_render::render_status_bar(canvas, editor, status_row, cols, frame_ms);
            status_render::render_command_line(canvas, editor, cmd_row, cols);
            popup_render::render_file_picker(canvas, editor, cols, rows);
        } else if editor.file_browser.is_some() {
            debug!("render: file_browser overlay");
            render_window_area(
                canvas,
                editor,
                &syntax_spans,
                shells,
                0,
                0,
                cols,
                window_height,
            );
            status_render::render_status_bar(canvas, editor, status_row, cols, frame_ms);
            status_render::render_command_line(canvas, editor, cmd_row, cols);
            popup_render::render_file_browser(canvas, editor, cols, rows);
        } else if editor.command_palette.is_some() {
            debug!("render: command_palette overlay");
            render_window_area(
                canvas,
                editor,
                &syntax_spans,
                shells,
                0,
                0,
                cols,
                window_height,
            );
            status_render::render_status_bar(canvas, editor, status_row, cols, frame_ms);
            status_render::render_command_line(canvas, editor, cmd_row, cols);
            popup_render::render_command_palette(canvas, editor, cols, rows);
        } else if !editor.which_key_prefix.is_empty() {
            debug!("render: which_key popup");
            let entries = if let Some(km) = editor.keymaps.get("normal") {
                km.which_key_entries(&editor.which_key_prefix, &editor.commands)
            } else {
                vec![]
            };

            let entry_cols = (cols / 25).max(1);
            let entry_rows = entries.len().div_ceil(entry_cols);
            let popup_height = (entry_rows + 2).min(rows / 2).max(3);

            let win_height = rows.saturating_sub(popup_height);
            render_window_area(
                canvas,
                editor,
                &syntax_spans,
                shells,
                0,
                0,
                cols,
                win_height,
            );
            popup_render::render_which_key_popup(
                canvas,
                editor,
                win_height,
                popup_height,
                cols,
                &entries,
            );
        } else if splash_render::should_show_splash(editor) {
            debug!("render: splash screen");
            splash_render::render_splash(canvas, editor, 0, 0, cols, window_height);
            status_render::render_status_bar(canvas, editor, status_row, cols, frame_ms);
            status_render::render_command_line(canvas, editor, cmd_row, cols);
        } else {
            debug!("render: normal window area");
            focused_frame_layout = render_window_area(
                canvas,
                editor,
                &syntax_spans,
                shells,
                0,
                0,
                cols,
                window_height,
            );
            status_render::render_status_bar(canvas, editor, status_row, cols, frame_ms);
            status_render::render_command_line(canvas, editor, cmd_row, cols);

            // Cursor (not for shell buffers — they render their own).
            if editor.mode != mae_core::Mode::ShellInsert {
                render_gui_cursor(
                    canvas,
                    editor,
                    cols,
                    window_height,
                    status_row,
                    cmd_row,
                    &syntax_spans,
                    focused_frame_layout.as_ref(),
                );
            }

            // Completion popup.
            if !editor.completion_items.is_empty() {
                popup_render::render_completion_popup(
                    canvas,
                    editor,
                    0,
                    0,
                    cols,
                    window_height,
                    focused_frame_layout.as_ref(),
                );
            }
        }

        // Cache focused layout for mouse click positioning.
        self.last_focused_layout = focused_frame_layout;

        canvas.end_frame();

        // Log slow frames for debugging.
        let frame_elapsed = frame_start.elapsed();
        if frame_elapsed.as_millis() > 16 {
            debug!(
                "slow frame: {:.1}ms (budget 16.7ms)",
                frame_elapsed.as_secs_f64() * 1000.0
            );
        }

        self.last_frame_start = Some(frame_start);
        Ok(())
    }

    fn size(&self) -> io::Result<(u16, u16)> {
        Ok((self.cols, self.rows))
    }

    fn viewport_height(&self) -> io::Result<usize> {
        Ok((self.rows as usize).saturating_sub(2))
    }

    fn cleanup(&mut self) -> io::Result<()> {
        // winit window drops automatically; no terminal state to restore.
        self.canvas = None;
        self.window = None;
        Ok(())
    }
}

// compute_visible_syntax_spans is now in mae_core::syntax (shared by all renderers).

// ---------------------------------------------------------------------------
// Window area dispatch
// ---------------------------------------------------------------------------

fn render_window_area(
    canvas: &mut canvas::SkiaCanvas,
    editor: &Editor,
    syntax_spans: &SyntaxSpanMap,
    shells: &HashMap<usize, ShellTerminal>,
    area_row: usize,
    area_col: usize,
    area_width: usize,
    area_height: usize,
) -> Option<layout::FrameLayout> {
    // Pre-compute scaled glyph advances for heading scales.
    // Font engines grid-fit advances at each font size, so `cell_width * scale`
    // is incorrect. We measure once and pass into layout/render.
    let (cw, _ch) = canvas.cell_size();
    let advance_1_15 = canvas.scaled_cell_width(1.15);
    let advance_1_30 = canvas.scaled_cell_width(1.3);
    let advance_1_50 = canvas.scaled_cell_width(1.5);
    let glyph_advance_fn = |scale: f32| -> f32 {
        let key = (scale * 100.0).round() as u32;
        match key {
            115 => advance_1_15,
            130 => advance_1_30,
            150 => advance_1_50,
            _ => cw * scale, // fallback for unexpected scales
        }
    };

    let mut focused_layout: Option<layout::FrameLayout> = None;
    let window_area = mae_core::WinRect {
        x: area_col as u16,
        y: area_row as u16,
        width: area_width as u16,
        height: area_height as u16,
    };
    let rects = editor.window_mgr.layout_rects(window_area);
    let focused_id = editor.window_mgr.focused_id();

    for (win_id, win_rect) in &rects {
        let r_row = win_rect.y as usize;
        let r_col = win_rect.x as usize;
        let r_width = win_rect.width as usize;
        let r_height = win_rect.height as usize;

        if let Some(win) = editor.window_mgr.window(*win_id) {
            let buf = &editor.buffers[win.buffer_idx];
            let is_focused = *win_id == focused_id;

            match buf.kind {
                BufferKind::Conversation => {
                    // Generate highlight spans with inline markdown styling.
                    let conv_spans = if let Some(ref conv) = buf.conversation {
                        conv.highlight_spans_with_markup(buf.rope())
                    } else {
                        Vec::new()
                    };

                    // Border + streaming indicator.
                    let border_fg = if is_focused {
                        theme::ts_fg(editor, "ui.window.border.active")
                    } else {
                        theme::ts_fg(editor, "ui.window.border")
                    };
                    let streaming_indicator = if let Some(ref conv) = buf.conversation {
                        if conv.streaming {
                            if let Some(start) = conv.streaming_start {
                                format!(" [waiting... {}s] ", start.elapsed().as_secs())
                            } else {
                                " [waiting...] ".to_string()
                            }
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };
                    let title = format!(" {}{} ", buf.name, streaming_indicator);
                    draw_window_border(canvas, r_row, r_col, r_width, r_height, border_fg, &title);

                    let inner_row = r_row + 1;
                    let inner_col = r_col + 1;
                    let inner_width = r_width.saturating_sub(2);
                    let inner_height = r_height.saturating_sub(2);
                    let (_, cell_height) = canvas.cell_size();
                    let fl = layout::compute_layout(
                        editor,
                        buf,
                        win,
                        inner_row,
                        inner_col,
                        inner_width,
                        inner_height,
                        cell_height,
                        cw,
                        Some(&conv_spans),
                        Some(&glyph_advance_fn),
                    );
                    buffer_render::render_buffer_content(
                        canvas,
                        editor,
                        buf,
                        win,
                        is_focused,
                        &fl,
                        Some(&conv_spans),
                    );
                    scrollbar::render_scrollbar(canvas, editor, &fl);
                    if is_focused {
                        focused_layout = Some(fl);
                    }
                }
                BufferKind::Messages => {
                    messages_render::render_messages_window(
                        canvas, buf, win, is_focused, editor, r_row, r_col, r_width, r_height,
                    );
                }
                BufferKind::Help => {
                    // Help buffers: generate heading + inline markup + link spans.
                    let mut help_spans: Vec<HighlightSpan> = Vec::new();

                    // Heading spans from leading `*` or `#` chars in rope lines.
                    let rope = buf.rope();
                    for line_idx in 0..buf.line_count() {
                        let line = rope.line(line_idx);
                        let first_char = line.chars().next().unwrap_or(' ');
                        let (prefix_count, is_heading) = if first_char == '*' {
                            let c = line.chars().take_while(|&ch| ch == '*').count();
                            (c, c > 0 && line.len_chars() > c && line.char(c) == ' ')
                        } else if first_char == '#' {
                            let c = line.chars().take_while(|&ch| ch == '#').count();
                            (c, c > 0 && line.len_chars() > c && line.char(c) == ' ')
                        } else {
                            (0, false)
                        };
                        if is_heading && prefix_count > 0 {
                            let line_start = rope.line_to_char(line_idx);
                            let line_len = line.len_chars();
                            let text_len = if line_idx + 1 < buf.line_count() {
                                line_len.saturating_sub(1)
                            } else {
                                line_len
                            };
                            let byte_start = rope.char_to_byte(line_start);
                            let byte_end = rope.char_to_byte(line_start + text_len);
                            help_spans.push(HighlightSpan {
                                byte_start,
                                byte_end,
                                theme_key: "markup.heading",
                            });
                        }
                    }

                    // Inline markdown style spans (bold, code, italic).
                    let source_text: String = rope.chars().collect();
                    let inline_spans = mae_core::compute_markdown_style_spans(&source_text);
                    help_spans.extend(inline_spans);

                    // Link spans from help view.
                    if let Some(view) = buf.help_view.as_ref() {
                        for (i, link) in view.rendered_links.iter().enumerate() {
                            let is_focused_link = view.focused_link == Some(i);
                            help_spans.push(HighlightSpan {
                                byte_start: link.byte_start,
                                byte_end: link.byte_end,
                                theme_key: if is_focused_link {
                                    "ui.selection"
                                } else {
                                    "markup.link"
                                },
                            });
                        }
                    }
                    help_spans.sort_by_key(|s| s.byte_start);

                    // Render with border — CRITICAL: pass same help_spans to both
                    // compute_layout() and render_buffer_content() (span parity).
                    let border_fg = if is_focused {
                        theme::ts_fg(editor, "ui.window.border.active")
                    } else {
                        theme::ts_fg(editor, "ui.window.border")
                    };
                    let modified = if buf.modified { " [+]" } else { "" };
                    let title = format!(" {}{} ", buf.name, modified);
                    draw_window_border(canvas, r_row, r_col, r_width, r_height, border_fg, &title);

                    let inner_row = r_row + 1;
                    let inner_col = r_col + 1;
                    let inner_width = r_width.saturating_sub(2);
                    let inner_height = r_height.saturating_sub(2);
                    let (_, cell_height) = canvas.cell_size();
                    let fl = layout::compute_layout(
                        editor,
                        buf,
                        win,
                        inner_row,
                        inner_col,
                        inner_width,
                        inner_height,
                        cell_height,
                        cw,
                        Some(&help_spans),
                        Some(&glyph_advance_fn),
                    );
                    buffer_render::render_buffer_content(
                        canvas,
                        editor,
                        buf,
                        win,
                        is_focused,
                        &fl,
                        Some(&help_spans),
                    );
                    scrollbar::render_scrollbar(canvas, editor, &fl);
                    if is_focused {
                        focused_layout = Some(fl);
                    }
                }
                BufferKind::Debug => {
                    debug_render::render_debug_window(
                        canvas, buf, win, is_focused, editor, r_row, r_col, r_width, r_height,
                    );
                }
                BufferKind::Shell => {
                    if let Some(shell) = shells.get(&win.buffer_idx) {
                        shell_render::render_shell_window(
                            canvas, buf, win, is_focused, editor, shell, r_row, r_col, r_width,
                            r_height,
                        );
                    }
                }
                BufferKind::Visual => {
                    // Phase 1 Visual Debugger rendering
                    if let Some(ref vb) = buf.visual {
                        render_visual_buffer(canvas, vb, r_row, r_col, r_width, r_height);
                    }
                }
                BufferKind::FileTree => {
                    file_tree_render::render_file_tree_window(
                        canvas, buf, win, is_focused, editor, r_row, r_col, r_width, r_height,
                    );
                }
                _ => {
                    // Text (and Preview) buffers: border + syntax-highlighted content.
                    let border_fg = if is_focused {
                        theme::ts_fg(editor, "ui.window.border.active")
                    } else {
                        theme::ts_fg(editor, "ui.window.border")
                    };
                    let modified = if buf.modified { " [+]" } else { "" };
                    let title = format!(" {}{} ", buf.name, modified);
                    draw_window_border(canvas, r_row, r_col, r_width, r_height, border_fg, &title);

                    let inner_row = r_row + 1;
                    let inner_col = r_col + 1;
                    let inner_width = r_width.saturating_sub(2);
                    let inner_height = r_height.saturating_sub(2);
                    // Diff buffers get line-level diff highlighting.
                    let diff_spans_storage;
                    let spans = if buf.name == "*AI-Diff*" {
                        diff_spans_storage = mae_core::diff::diff_highlight_spans(buf.rope());
                        Some(diff_spans_storage.as_slice())
                    } else {
                        syntax_spans.get(&win.buffer_idx).map(|v| v.as_slice())
                    };
                    let (_, cell_height) = canvas.cell_size();
                    let fl = layout::compute_layout(
                        editor,
                        buf,
                        win,
                        inner_row,
                        inner_col,
                        inner_width,
                        inner_height,
                        cell_height,
                        cw,
                        spans,
                        Some(&glyph_advance_fn),
                    );
                    buffer_render::render_buffer_content(
                        canvas, editor, buf, win, is_focused, &fl, spans,
                    );
                    scrollbar::render_scrollbar(canvas, editor, &fl);
                    if is_focused {
                        focused_layout = Some(fl);
                    }
                }
            }
        }
    }

    // Window split borders (vertical lines between windows).
    if rects.len() > 1 {
        let border_fg = theme::ts_fg(editor, "ui.window.border");
        // Draw vertical separators where windows share an edge.
        for (_, win_rect) in &rects {
            let right_col = win_rect.x as usize + win_rect.width as usize;
            if right_col < area_col + area_width {
                canvas.draw_vline(
                    right_col,
                    win_rect.y as usize,
                    win_rect.y as usize + win_rect.height as usize,
                    border_fg,
                );
            }
        }
    }

    focused_layout
}

fn render_visual_buffer(
    canvas: &mut canvas::SkiaCanvas,
    vb: &mae_core::visual_buffer::VisualBuffer,
    r_row: usize,
    r_col: usize,
    r_width: usize,
    r_height: usize,
) {
    use mae_core::visual_buffer::VisualElement;
    use skia_safe::{Color4f, Paint, PaintStyle};

    // Draw background
    canvas.draw_rect_fill(
        r_row,
        r_col,
        r_width,
        r_height,
        Color4f::new(0.05, 0.05, 0.05, 1.0),
    );

    let (cw, ch) = canvas.cell_size();
    let x_off = r_col as f32 * cw;
    let y_off = r_row as f32 * ch;

    for element in &vb.elements {
        match element {
            VisualElement::Rect {
                x,
                y,
                w,
                h,
                fill,
                stroke,
            } => {
                let rect = skia_safe::Rect::from_xywh(x_off + x, y_off + y, *w, *h);
                if let Some(f) = fill {
                    if let Some(c) = theme::parse_hex_to_skia(f) {
                        let mut paint = Paint::new(c, None);
                        paint.set_style(PaintStyle::Fill);
                        canvas.canvas().draw_rect(rect, &paint);
                    }
                }
                if let Some(s) = stroke {
                    if let Some(c) = theme::parse_hex_to_skia(s) {
                        let mut paint = Paint::new(c, None);
                        paint.set_style(PaintStyle::Stroke);
                        paint.set_stroke_width(1.0);
                        canvas.canvas().draw_rect(rect, &paint);
                    }
                }
            }
            VisualElement::Line {
                x1,
                y1,
                x2,
                y2,
                color,
                thickness,
            } => {
                if let Some(c) = theme::parse_hex_to_skia(color) {
                    let mut paint = Paint::new(c, None);
                    paint.set_stroke_width(*thickness);
                    paint.set_style(PaintStyle::Stroke);
                    canvas.canvas().draw_line(
                        (x_off + x1, y_off + y1),
                        (x_off + x2, y_off + y2),
                        &paint,
                    );
                }
            }
            VisualElement::Circle {
                cx,
                cy,
                r,
                fill,
                stroke,
            } => {
                if let Some(f) = fill {
                    if let Some(c) = theme::parse_hex_to_skia(f) {
                        let mut paint = Paint::new(c, None);
                        paint.set_style(PaintStyle::Fill);
                        canvas
                            .canvas()
                            .draw_circle((x_off + cx, y_off + cy), *r, &paint);
                    }
                }
                if let Some(s) = stroke {
                    if let Some(c) = theme::parse_hex_to_skia(s) {
                        let mut paint = Paint::new(c, None);
                        paint.set_style(PaintStyle::Stroke);
                        paint.set_stroke_width(1.0);
                        canvas
                            .canvas()
                            .draw_circle((x_off + cx, y_off + cy), *r, &paint);
                    }
                }
            }
            VisualElement::Text {
                x,
                y,
                text,
                font_size: _,
                color,
            } => {
                if let Some(c) = theme::parse_hex_to_skia(color) {
                    let mut paint = Paint::new(c, None);
                    paint.set_anti_alias(true);
                    let font = skia_safe::Font::default(); // TODO: use real font
                    canvas
                        .canvas()
                        .draw_str(text, (x_off + x, y_off + y), &font, &paint);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GUI cursor rendering
// ---------------------------------------------------------------------------

fn render_gui_cursor(
    canvas: &mut canvas::SkiaCanvas,
    editor: &Editor,
    cols: usize,
    window_height: usize,
    _status_row: usize,
    cmd_row: usize,
    syntax_spans: &SyntaxSpanMap,
    frame_layout: Option<&layout::FrameLayout>,
) {
    let focused_win = editor.window_mgr.focused_window();
    let focused_buf = &editor.buffers[focused_win.buffer_idx];

    // Find the focused window's rect for offset calculation.
    let window_area = mae_core::WinRect {
        x: 0,
        y: 0,
        width: cols as u16,
        height: window_height as u16,
    };
    let rects = editor.window_mgr.layout_rects(window_area);
    let focused_id = editor.window_mgr.focused_id();

    if let Some((_, win_rect)) = rects.iter().find(|(id, _)| *id == focused_id) {
        let inner_row = win_rect.y as usize + 1;
        let inner_col = win_rect.x as usize + 1;
        let inner_width = (win_rect.width as usize).saturating_sub(2);
        let inner_height = (win_rect.height as usize).saturating_sub(2);

        // Conversation buffers render without a gutter — cursor gutter offset must be 0.
        let gutter_w = if focused_buf.kind == mae_core::BufferKind::Conversation {
            0
        } else if editor.show_line_numbers {
            gutter::gutter_width(focused_buf.display_line_count())
        } else {
            2
        };

        let inner = canvas::CellRect::new(inner_row, inner_col, inner_width, inner_height);

        let (_, ch) = canvas.cell_size();

        let (cw, _) = canvas.cell_size();
        if editor.mode == mae_core::Mode::Command {
            // Command line cursor — always cell-based (no scaling).
            let cursor_col = editor.command_line
                [..editor.command_cursor.min(editor.command_line.len())]
                .chars()
                .count();
            let pixel_y = cmd_row as f32 * ch;
            let pixel_x = (1 + cursor_col) as f32 * cw;
            cursor::render_cursor(canvas, editor, pixel_y, pixel_x, 1.0);
        } else if editor.mode == mae_core::Mode::Search {
            let col = 1 + editor.search_input.len();
            let pixel_y = cmd_row as f32 * ch;
            let pixel_x = col as f32 * cw;
            cursor::render_cursor(canvas, editor, pixel_y, pixel_x, 1.0);
        } else if let Some(pos) = cursor::compute_cursor_position(
            editor,
            frame_layout,
            inner,
            gutter_w,
            syntax_spans
                .get(&focused_win.buffer_idx)
                .map(|v| v.as_slice()),
        ) {
            let cursor_pixel_y = pos.pixel_y.unwrap_or((inner_row + pos.row) as f32 * ch);
            let cursor_pixel_x = if let Some(px) = pos.pixel_x {
                px
            } else {
                (inner_col + pos.col) as f32 * cw
            };
            cursor::render_cursor(canvas, editor, cursor_pixel_y, cursor_pixel_x, pos.scale);
        }
    }
}

// ---------------------------------------------------------------------------
// Shared border helper
// ---------------------------------------------------------------------------

/// Draw a box border with an optional title embedded in the top edge.
///
/// The title is rendered as part of the border string so that dashes don't
/// overlap the title glyphs (which causes a strikethrough effect in Skia).
pub(crate) fn draw_window_border(
    canvas: &mut canvas::SkiaCanvas,
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    color: skia_safe::Color4f,
    title: &str,
) {
    if width < 2 || height < 2 {
        return;
    }
    let inner_w = width.saturating_sub(2);
    let title_len = title.chars().count();
    let top = if !title.is_empty() && title_len < inner_w {
        let pad = inner_w - title_len;
        format!("┌{}{}┐", title, "─".repeat(pad))
    } else {
        format!("┌{}┐", "─".repeat(inner_w))
    };
    canvas.draw_text_at(row, col, &top, color);
    for r in 1..height.saturating_sub(1) {
        canvas.draw_text_at(row + r, col, "│", color);
        canvas.draw_text_at(row + r, col + width - 1, "│", color);
    }
    let bottom = format!("└{}┘", "─".repeat(inner_w));
    canvas.draw_text_at(row + height - 1, col, &bottom, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_renderer_default_size() {
        let renderer = GuiRenderer::new();
        let (cols, rows) = renderer.size().unwrap();
        assert_eq!(cols, 120);
        assert_eq!(rows, 40);
    }

    #[test]
    fn gui_renderer_viewport_height() {
        let renderer = GuiRenderer::new();
        assert_eq!(renderer.viewport_height().unwrap(), 38);
    }

    /// Verify that `rows * cell_height` never exceeds window pixel height.
    /// Regression test: the command line at `rows - 1` was clipped when the
    /// row calculation allowed a partial bottom row.
    #[test]
    fn command_line_row_fits_in_window() {
        for height in [600u32, 720, 768, 800, 900, 1080, 1440] {
            for ch_tenth in [140, 160, 185, 200, 225] {
                let ch = ch_tenth as f32 / 10.0;
                let rows = (height as f32 / ch).floor() as u16;
                assert!(
                    (rows as f32 * ch).ceil() <= height as f32,
                    "rows={} * ch={} = {} exceeds height={}",
                    rows,
                    ch,
                    (rows as f32 * ch).ceil(),
                    height
                );
            }
        }
    }

    /// Regression: specific heights that caused border overlap with the old
    /// ceil-based guard. floor() is correct for all fractional remainders.
    #[test]
    fn floor_row_calc_no_overlap() {
        // Cell height 18.5, window height 741 → 741/18.5 = 40.054...
        // floor = 40, 40*18.5 = 740 < 741 ✓
        // Old code: raw_rows = 40, ceil(40*18.5) = 740 <= 741 → 40, OK
        // But: 742/18.5 = 40.108, raw=40, ceil(740)=740 <= 742 → 40 ✓
        // Edge case: 740/18.5 = 40.0 exactly → floor=40, 40*18.5=740 ✓
        let ch = 18.5f32;
        for h in [740u32, 741, 742, 743, 750, 755, 757] {
            let rows = (h as f32 / ch).floor() as u16;
            let used = rows as f32 * ch;
            assert!(
                used <= h as f32,
                "h={} rows={} used={} overflows",
                h,
                rows,
                used
            );
        }
    }
}
