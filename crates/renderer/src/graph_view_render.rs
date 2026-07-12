//! TUI renderer for `BufferKind::Graph` — the native KB graph view's
//! textual fallback (Part C Phase 1). The TUI has no Skia canvas to draw
//! `GraphView.scene`'s node/edge positions with, so it reuses
//! `Editor::render_graph_view_as_text`'s existing KB "** Neighborhood"
//! machinery instead — GUI-primary/TUI-degraded, the same precedent other
//! buffer kinds already follow.

use mae_core::Editor;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::theme_convert::ts;

/// Render a KB graph view window as wrapped plain text.
pub(crate) fn render_graph_view_window(
    frame: &mut Frame,
    area: Rect,
    _buf: &mae_core::Buffer,
    focused: bool,
    editor: &Editor,
) {
    let border_style = if focused {
        ts(editor, "ui.statusline")
    } else {
        ts(editor, "ui.statusline.inactive")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(" KB Graph ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let text = editor.render_graph_view_as_text();
    frame.render_widget(
        Paragraph::new(text)
            .style(ts(editor, "ui.text"))
            .wrap(Wrap { trim: false }),
        inner,
    );
}
