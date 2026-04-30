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
    fn keymap_name(&self) -> Option<&str> {
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
        }
    }

    fn keymap_name(&self) -> Option<&str> {
        match self {
            Self::GitStatus => Some("git-status"),
            Self::FileTree => Some("file-tree"),
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
        assert_eq!(BufferKind::Text.keymap_name(), None);
        assert_eq!(BufferKind::Help.keymap_name(), None);
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
    fn buffer_mode_name() {
        assert_eq!(BufferKind::Text.mode_name(), "Text");
        assert_eq!(BufferKind::Conversation.mode_name(), "Conversation");
        assert_eq!(BufferKind::GitStatus.mode_name(), "Git Status");
    }
}
