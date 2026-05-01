//! Tiered redisplay cascade (Emacs `xdisp.c:19339` `try_cursor_movement` pattern).
//!
//! Each event sets a `RedrawLevel` on the editor. The renderer uses this to
//! skip expensive work (syntax recomputation, full canvas clear) when only
//! the cursor moved.

/// How much of the screen needs to be redrawn.
///
/// Levels are ordered: a higher level subsumes all lower levels.
/// `max()` of two levels gives the correct combined level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum RedrawLevel {
    /// Nothing changed — skip rendering entirely.
    #[default]
    None,
    /// Only the cursor position changed — reuse cached syntax spans.
    CursorOnly,
    /// Viewport scrolled — reuse spans but redraw all visible lines.
    Scroll,
    /// Some lines changed — redraw only the dirty range (future optimization).
    PartialLines,
    /// Full redraw needed (theme change, resize, mode switch, etc.).
    Full,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redraw_level_ordering() {
        assert!(RedrawLevel::None < RedrawLevel::CursorOnly);
        assert!(RedrawLevel::CursorOnly < RedrawLevel::Scroll);
        assert!(RedrawLevel::Scroll < RedrawLevel::PartialLines);
        assert!(RedrawLevel::PartialLines < RedrawLevel::Full);
    }

    #[test]
    fn redraw_level_default_is_none() {
        assert_eq!(RedrawLevel::default(), RedrawLevel::None);
    }

    #[test]
    fn redraw_level_max() {
        assert_eq!(
            RedrawLevel::CursorOnly.max(RedrawLevel::Scroll),
            RedrawLevel::Scroll
        );
        assert_eq!(
            RedrawLevel::Full.max(RedrawLevel::CursorOnly),
            RedrawLevel::Full
        );
    }
}
