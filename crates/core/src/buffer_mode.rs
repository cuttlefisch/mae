//! The contract every buffer mode must satisfy.
//!
//! Mirrors Emacs's `define-derived-mode`: identity, keymap, rendering hints,
//! content lifecycle. Implemented on `BufferKind`.

use crate::buffer::BufferKind;

pub trait BufferMode {
    /// Display name for the status bar (Emacs mode-name).
    fn mode_name(&self) -> &str;

    /// Name of the keymap to use when this buffer is focused in Normal mode.
    /// Returns None to use the default "normal" keymap.
    fn keymap_name(&self) -> Option<&'static str> {
        None
    }

    /// Whether this buffer is read-only by default (Emacs special-mode base).
    fn read_only(&self) -> bool {
        false
    }

    /// Whether word-wrap should default to on (prose buffers).
    fn default_word_wrap(&self) -> bool {
        false
    }

    /// Whether this buffer kind should show a line-number gutter.
    fn has_gutter(&self) -> bool {
        true
    }

    /// Optional hint text for the command area when this buffer is first entered.
    /// Returns None for buffers that don't need discoverability hints.
    /// Emacs equivalent: mode-specific `message` on mode entry.
    fn status_hint(&self) -> Option<&'static str> {
        None
    }

    /// Override the status-bar mode theme key for this buffer kind.
    /// Returns `None` to fall through to the standard mode-based theme key.
    fn mode_theme_key(&self) -> Option<&'static str> {
        None
    }

    /// Which insert mode to enter for this buffer kind.
    fn insert_mode(&self) -> crate::Mode {
        crate::Mode::Insert
    }

    /// Override markup flavor for this buffer kind. Returns `None` to fall
    /// through to the language-derived flavor.
    fn markup_flavor(&self) -> Option<crate::syntax::MarkupFlavor> {
        None
    }
}

impl BufferMode for BufferKind {
    fn mode_name(&self) -> &str {
        match self {
            Self::Text => "Text",
            Self::Conversation => "Conversation",
            Self::Help => "Help",
            Self::Messages => "Messages",
            Self::Debug => "Debug",
            Self::GitStatus => "Git Status",
            Self::FileTree => "File Tree",
            Self::Shell => "Shell",
            Self::Dashboard => "Dashboard",
            Self::Preview => "Preview",
            Self::Visual => "Visual",
            Self::Diff => "Diff",
        }
    }

    fn keymap_name(&self) -> Option<&'static str> {
        match self {
            Self::GitStatus => Some("git-status"),
            Self::FileTree => Some("file-tree"),
            Self::Help => Some("help"),
            Self::Debug => Some("debug"),
            _ => None,
        }
    }

    fn has_gutter(&self) -> bool {
        !matches!(
            self,
            Self::Conversation | Self::Messages | Self::Visual | Self::Dashboard | Self::Diff
        )
    }

    fn status_hint(&self) -> Option<&'static str> {
        match self {
            Self::GitStatus => Some("Press ? for key help, SPC m for mode menu"),
            Self::Debug => Some("Press ? for key help"),
            Self::FileTree => Some("Press ? for key help"),
            _ => None,
        }
    }

    fn mode_theme_key(&self) -> Option<&'static str> {
        match self {
            Self::GitStatus | Self::Debug => Some("ui.statusline.mode.command"),
            _ => None,
        }
    }

    fn insert_mode(&self) -> crate::Mode {
        match self {
            Self::Shell => crate::Mode::ShellInsert,
            _ => crate::Mode::Insert,
        }
    }

    fn markup_flavor(&self) -> Option<crate::syntax::MarkupFlavor> {
        match self {
            Self::Help | Self::Conversation => Some(crate::syntax::MarkupFlavor::Markdown),
            _ => None,
        }
    }

    fn read_only(&self) -> bool {
        matches!(
            self,
            Self::Help
                | Self::Messages
                | Self::Debug
                | Self::Dashboard
                | Self::GitStatus
                | Self::FileTree
                | Self::Shell
                | Self::Diff
        )
    }

    fn default_word_wrap(&self) -> bool {
        matches!(self, Self::Conversation | Self::Help | Self::Messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_mode_read_only() {
        assert!(!BufferKind::Text.read_only());
        assert!(!BufferKind::Conversation.read_only());
        assert!(BufferKind::Help.read_only());
        assert!(BufferKind::Messages.read_only());
        assert!(BufferKind::Debug.read_only());
        assert!(BufferKind::Dashboard.read_only());
        assert!(BufferKind::GitStatus.read_only());
        assert!(BufferKind::FileTree.read_only());
        assert!(BufferKind::Shell.read_only());
        assert!(!BufferKind::Preview.read_only());
        assert!(!BufferKind::Visual.read_only());
    }

    #[test]
    fn buffer_mode_keymap() {
        assert_eq!(BufferKind::GitStatus.keymap_name(), Some("git-status"));
        assert_eq!(BufferKind::FileTree.keymap_name(), Some("file-tree"));
        assert_eq!(BufferKind::Help.keymap_name(), Some("help"));
        assert_eq!(BufferKind::Debug.keymap_name(), Some("debug"));
        assert_eq!(BufferKind::Text.keymap_name(), None);
        assert_eq!(BufferKind::Conversation.keymap_name(), None);
    }

    #[test]
    fn buffer_mode_word_wrap() {
        assert!(BufferKind::Conversation.default_word_wrap());
        assert!(BufferKind::Help.default_word_wrap());
        assert!(BufferKind::Messages.default_word_wrap());
        assert!(!BufferKind::Text.default_word_wrap());
        assert!(!BufferKind::Shell.default_word_wrap());
    }

    #[test]
    fn buffer_mode_has_gutter() {
        assert!(BufferKind::Text.has_gutter());
        assert!(BufferKind::Help.has_gutter());
        assert!(BufferKind::GitStatus.has_gutter());
        assert!(!BufferKind::Conversation.has_gutter());
        assert!(!BufferKind::Messages.has_gutter());
        assert!(!BufferKind::Dashboard.has_gutter());
        assert!(!BufferKind::Visual.has_gutter());
    }

    #[test]
    fn buffer_mode_status_hint() {
        assert_eq!(
            BufferKind::GitStatus.status_hint(),
            Some("Press ? for key help, SPC m for mode menu")
        );
        assert_eq!(
            BufferKind::Debug.status_hint(),
            Some("Press ? for key help")
        );
        assert_eq!(
            BufferKind::FileTree.status_hint(),
            Some("Press ? for key help")
        );
        assert_eq!(BufferKind::Text.status_hint(), None);
        assert_eq!(BufferKind::Conversation.status_hint(), None);
    }

    #[test]
    fn buffer_mode_theme_key() {
        assert_eq!(
            BufferKind::GitStatus.mode_theme_key(),
            Some("ui.statusline.mode.command")
        );
        assert_eq!(
            BufferKind::Debug.mode_theme_key(),
            Some("ui.statusline.mode.command")
        );
        assert_eq!(BufferKind::Text.mode_theme_key(), None);
        assert_eq!(BufferKind::Conversation.mode_theme_key(), None);
        assert_eq!(BufferKind::FileTree.mode_theme_key(), None);
    }

    #[test]
    fn buffer_mode_insert_mode() {
        assert_eq!(BufferKind::Shell.insert_mode(), crate::Mode::ShellInsert);
        assert_eq!(BufferKind::Text.insert_mode(), crate::Mode::Insert);
        assert_eq!(BufferKind::Conversation.insert_mode(), crate::Mode::Insert);
    }

    #[test]
    fn buffer_mode_name() {
        assert_eq!(BufferKind::Text.mode_name(), "Text");
        assert_eq!(BufferKind::Conversation.mode_name(), "Conversation");
        assert_eq!(BufferKind::GitStatus.mode_name(), "Git Status");
    }

    #[test]
    fn buffer_mode_markup_flavor() {
        use crate::syntax::MarkupFlavor;
        assert_eq!(
            BufferKind::Help.markup_flavor(),
            Some(MarkupFlavor::Markdown)
        );
        assert_eq!(
            BufferKind::Conversation.markup_flavor(),
            Some(MarkupFlavor::Markdown)
        );
        assert_eq!(BufferKind::Text.markup_flavor(), None);
        assert_eq!(BufferKind::Shell.markup_flavor(), None);
        assert_eq!(BufferKind::GitStatus.markup_flavor(), None);
    }

    #[test]
    fn diff_buffer_kind_is_read_only() {
        assert!(BufferKind::Diff.read_only());
    }

    #[test]
    fn diff_buffer_kind_has_no_gutter() {
        assert!(!BufferKind::Diff.has_gutter());
    }

    #[test]
    fn diff_buffer_kind_mode_name() {
        assert_eq!(BufferKind::Diff.mode_name(), "Diff");
    }
}
