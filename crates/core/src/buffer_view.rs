//! Mode-specific state for a buffer.
//!
//! Replaces 6 scattered `Option<T>` fields on `Buffer` with a single tagged enum.
//! Each variant carries the view struct that only that buffer kind uses.

use crate::agenda_view::AgendaView;
use crate::conversation::Conversation;
use crate::debug_view::DebugView;
use crate::file_tree::FileTree;
use crate::git_status::GitStatusView;
use crate::kb_sharing::KbSharingView;
use crate::kb_view::KbView;
use crate::notifications_view::NotifView;
use crate::visual_buffer::VisualBuffer;

#[derive(Debug)]
pub enum BufferView {
    /// No special view — plain text editing buffer.
    None,
    /// AI conversation state.
    Conversation(Box<Conversation>),
    /// KB buffer navigation state.
    Kb(Box<KbView>),
    /// DAP debug panel state.
    Debug(Box<DebugView>),
    /// Git status porcelain state.
    GitStatus(Box<GitStatusView>),
    /// Visual scene-graph state.
    Visual(Box<VisualBuffer>),
    /// File tree sidebar state.
    FileTree(Box<FileTree>),
    /// Agenda view state.
    Agenda(Box<AgendaView>),
    /// `*Notifications*` attention-buffer state (ADR-024).
    Notifications(Box<NotifView>),
    /// `*KB Sharing*` management-buffer state.
    KbSharing(Box<KbSharingView>),
}

impl BufferView {
    pub fn conversation(&self) -> Option<&Conversation> {
        match self {
            BufferView::Conversation(c) => Some(c),
            _ => None,
        }
    }

    pub fn conversation_mut(&mut self) -> Option<&mut Conversation> {
        match self {
            BufferView::Conversation(c) => Some(c),
            _ => None,
        }
    }

    pub fn kb_view(&self) -> Option<&KbView> {
        match self {
            BufferView::Kb(h) => Some(h),
            _ => None,
        }
    }

    pub fn kb_view_mut(&mut self) -> Option<&mut KbView> {
        match self {
            BufferView::Kb(h) => Some(h),
            _ => None,
        }
    }

    pub fn debug_view(&self) -> Option<&DebugView> {
        match self {
            BufferView::Debug(d) => Some(d),
            _ => None,
        }
    }

    pub fn debug_view_mut(&mut self) -> Option<&mut DebugView> {
        match self {
            BufferView::Debug(d) => Some(d),
            _ => None,
        }
    }

    pub fn git_status(&self) -> Option<&GitStatusView> {
        match self {
            BufferView::GitStatus(g) => Some(g),
            _ => None,
        }
    }

    pub fn git_status_mut(&mut self) -> Option<&mut GitStatusView> {
        match self {
            BufferView::GitStatus(g) => Some(g),
            _ => None,
        }
    }

    pub fn visual(&self) -> Option<&VisualBuffer> {
        match self {
            BufferView::Visual(v) => Some(v),
            _ => None,
        }
    }

    pub fn visual_mut(&mut self) -> Option<&mut VisualBuffer> {
        match self {
            BufferView::Visual(v) => Some(v),
            _ => None,
        }
    }

    pub fn file_tree(&self) -> Option<&FileTree> {
        match self {
            BufferView::FileTree(f) => Some(f),
            _ => None,
        }
    }

    pub fn file_tree_mut(&mut self) -> Option<&mut FileTree> {
        match self {
            BufferView::FileTree(f) => Some(f),
            _ => None,
        }
    }

    pub fn agenda_view(&self) -> Option<&AgendaView> {
        match self {
            BufferView::Agenda(a) => Some(a),
            _ => None,
        }
    }

    pub fn agenda_view_mut(&mut self) -> Option<&mut AgendaView> {
        match self {
            BufferView::Agenda(a) => Some(a),
            _ => None,
        }
    }

    pub fn notif_view(&self) -> Option<&NotifView> {
        match self {
            BufferView::Notifications(n) => Some(n),
            _ => None,
        }
    }

    pub fn notif_view_mut(&mut self) -> Option<&mut NotifView> {
        match self {
            BufferView::Notifications(n) => Some(n),
            _ => None,
        }
    }

    pub fn kb_sharing_view(&self) -> Option<&KbSharingView> {
        match self {
            BufferView::KbSharing(v) => Some(v),
            _ => None,
        }
    }

    pub fn kb_sharing_view_mut(&mut self) -> Option<&mut KbSharingView> {
        match self {
            BufferView::KbSharing(v) => Some(v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_view_accessors() {
        let conv = BufferView::Conversation(Box::default());
        assert!(conv.conversation().is_some());
        assert!(conv.kb_view().is_none());
        assert!(conv.debug_view().is_none());
        assert!(conv.git_status().is_none());
        assert!(conv.visual().is_none());
        assert!(conv.file_tree().is_none());

        let help = BufferView::Kb(Box::new(KbView::new("index".to_string())));
        assert!(help.kb_view().is_some());
        assert!(help.conversation().is_none());

        let none = BufferView::None;
        assert!(none.conversation().is_none());
        assert!(none.kb_view().is_none());
    }
}
