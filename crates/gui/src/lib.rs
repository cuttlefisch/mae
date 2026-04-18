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
//! - `text` — Text layout with mixed fonts and variable heights.
//! - `input` — winit KeyEvent/MouseEvent → mae_core::InputEvent translation.
//! - `image` — Inline image rendering (PNG/JPG/SVG). (Phase 8 M3)
//! - `pdf` — PDF page rendering via pdfium. (Phase 8 M4)
//! - `theme` — ThemeStyle → Skia Paint/Font conversion.
//!
//! # Neovide precedent
//!
//! Neovide (Rust + Skia GUI for Neovim) proves this stack works for exactly
//! our use case. Skia is battle-tested (Chrome, Android, Flutter) and provides
//! the richest 2D rendering API available.

mod canvas;
mod input;
mod text;
mod theme;

use std::collections::HashMap;
use std::io;
use std::rc::Rc;

use mae_core::Editor;
use mae_renderer::Renderer;
use mae_shell::ShellTerminal;
use tracing::info;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

pub use input::{winit_event_to_input, winit_key_to_keypress};

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
        }
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
        let canvas = canvas::SkiaCanvas::new(size.width, size.height, window.clone())?;

        // Compute cell dimensions from the default monospace font.
        let (cw, ch) = canvas.cell_size();
        self.cell_width = cw;
        self.cell_height = ch;
        self.cols = (size.width as f32 / cw) as u16;
        self.rows = (size.height as f32 / ch) as u16;

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
            self.rows = (height as f32 / ch) as u16;
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
        _shells: &HashMap<usize, ShellTerminal>,
    ) -> io::Result<()> {
        let Some(canvas) = &mut self.canvas else {
            return Ok(());
        };

        let theme = &editor.theme;
        canvas.begin_frame(theme);

        // Render visible buffer content as monospace text.
        let buf = &editor.buffers[editor.active_buffer_idx()];
        let win = editor.window_mgr.focused_window();
        let scroll_row = win.scroll_offset;
        let visible_lines = self.rows.saturating_sub(2) as usize; // status + command line

        for line_idx in 0..visible_lines {
            let buf_line = scroll_row + line_idx;
            if buf_line >= buf.line_count() {
                break;
            }
            let line_text = buf.line_text(buf_line);
            canvas.draw_text_line(line_idx, &line_text, theme);
        }

        // Status bar.
        let status = format!(" {} | {:?} ", buf.name, editor.mode,);
        canvas.draw_status_line(visible_lines, &status, theme);

        canvas.end_frame();
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
}
