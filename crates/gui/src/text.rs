//! Styled cell types for per-character rendering.
//!
//! Each visible character is a `StyledCell` with individual fg/bg/attributes.
//! A line of cells is a `StyledLine`. These types bridge the gap between
//! the editor's buffer content + syntax spans and the Skia drawing calls.

use skia_safe::Color4f;

/// A single styled character cell.
#[derive(Debug, Clone)]
pub struct StyledCell {
    pub ch: char,
    pub fg: Color4f,
    pub bg: Option<Color4f>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl StyledCell {
    pub fn new(ch: char, fg: Color4f) -> Self {
        Self {
            ch,
            fg,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }
    }

    pub fn with_bg(mut self, bg: Color4f) -> Self {
        self.bg = Some(bg);
        self
    }

    pub fn with_bold(mut self, bold: bool) -> Self {
        self.bold = bold;
        self
    }

    pub fn with_italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }

    pub fn with_underline(mut self, underline: bool) -> Self {
        self.underline = underline;
        self
    }
}

/// A line of individually-styled cells.
pub type StyledLine = Vec<StyledCell>;

/// Build a `StyledLine` from text with uniform style.
pub fn uniform_line(text: &str, fg: Color4f, bg: Option<Color4f>) -> StyledLine {
    text.chars()
        .map(|ch| {
            let mut cell = StyledCell::new(ch, fg);
            cell.bg = bg;
            cell
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styled_cell_construction() {
        let cell = StyledCell::new('A', Color4f::new(1.0, 0.0, 0.0, 1.0));
        assert_eq!(cell.ch, 'A');
        assert!(cell.bg.is_none());
        assert!(!cell.bold);
    }

    #[test]
    fn styled_cell_with_bg() {
        let bg = Color4f::new(0.0, 0.0, 1.0, 1.0);
        let cell = StyledCell::new('B', Color4f::new(1.0, 1.0, 1.0, 1.0)).with_bg(bg);
        assert!(cell.bg.is_some());
    }

    #[test]
    fn styled_cell_builder_chain() {
        let cell = StyledCell::new('C', Color4f::new(1.0, 1.0, 1.0, 1.0))
            .with_bold(true)
            .with_italic(true)
            .with_underline(true);
        assert!(cell.bold);
        assert!(cell.italic);
        assert!(cell.underline);
    }

    #[test]
    fn uniform_line_length() {
        let line = uniform_line("hello", Color4f::new(1.0, 1.0, 1.0, 1.0), None);
        assert_eq!(line.len(), 5);
        assert_eq!(line[0].ch, 'h');
        assert_eq!(line[4].ch, 'o');
    }

    #[test]
    fn uniform_line_with_bg() {
        let bg = Color4f::new(0.1, 0.1, 0.1, 1.0);
        let line = uniform_line("ab", Color4f::new(1.0, 1.0, 1.0, 1.0), Some(bg));
        assert!(line[0].bg.is_some());
    }
}
