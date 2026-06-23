//! `*Notifications*` buffer data model (ADR-024) — the magit-style attention
//! buffer. Mirrors the git-status assembly (`git_status.rs`): a flat `Vec` of
//! semantic lines + a per-category fold map, built from the `NotificationCenter`.

use std::collections::HashMap;

/// Semantic line type for rendering + cursor dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifLineKind {
    /// Top "Notifications (N outstanding)" header.
    Header,
    /// A foldable category heading (the notification `source`).
    CategoryHeader(String),
    /// An outstanding notification's title row.
    Item { notif_id: u64 },
    /// An at-point action row under an item.
    ActionRow { notif_id: u64, action_idx: usize },
    /// A recently-resolved notification (dimmed).
    ResolvedItem { notif_id: u64 },
    /// Blank separator.
    Blank,
}

/// A line in the `*Notifications*` buffer mapped to its source notification/action.
#[derive(Debug, Clone)]
pub struct NotifLine {
    pub text: String,
    pub kind: NotifLineKind,
    pub category: Option<String>,
}

impl NotifLine {
    pub fn blank() -> Self {
        NotifLine {
            text: String::new(),
            kind: NotifLineKind::Blank,
            category: None,
        }
    }

    /// The notification id this line acts on, if any.
    pub fn notif_id(&self) -> Option<u64> {
        match &self.kind {
            NotifLineKind::Item { notif_id }
            | NotifLineKind::ActionRow { notif_id, .. }
            | NotifLineKind::ResolvedItem { notif_id } => Some(*notif_id),
            _ => None,
        }
    }

    /// The action index this line invokes, if it is an action row.
    pub fn action_idx(&self) -> Option<usize> {
        match &self.kind {
            NotifLineKind::ActionRow { action_idx, .. } => Some(*action_idx),
            _ => None,
        }
    }
}

/// Type-safe fold key — the `*Notifications*` buffer folds by category (source).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CollapseKey {
    Category(String),
}

/// Structured state for the `*Notifications*` buffer.
#[derive(Debug, Clone, Default)]
pub struct NotifView {
    pub lines: Vec<NotifLine>,
    pub collapsed: HashMap<CollapseKey, bool>,
}

impl NotifView {
    pub fn new() -> Self {
        NotifView::default()
    }

    pub fn line_at(&self, row: usize) -> Option<&NotifLine> {
        self.lines.get(row)
    }

    /// Toggle collapse state for a key (default expanded).
    pub fn toggle(&mut self, key: CollapseKey) {
        let collapsed = self.collapsed.entry(key).or_insert(false);
        *collapsed = !*collapsed;
    }

    pub fn is_collapsed(&self, key: &CollapseKey) -> bool {
        self.collapsed.get(key).copied().unwrap_or(false)
    }

    /// The fold key for a line, if it is a (foldable) category header.
    pub fn collapse_key_for_line(line: &NotifLine) -> Option<CollapseKey> {
        match &line.kind {
            NotifLineKind::CategoryHeader(c) => Some(CollapseKey::Category(c.clone())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_accessors_map_to_notif_and_action() {
        let item = NotifLine {
            text: "x".into(),
            kind: NotifLineKind::Item { notif_id: 7 },
            category: Some("collab".into()),
        };
        assert_eq!(item.notif_id(), Some(7));
        assert_eq!(item.action_idx(), None);

        let action = NotifLine {
            text: "  [a] Accept".into(),
            kind: NotifLineKind::ActionRow {
                notif_id: 7,
                action_idx: 2,
            },
            category: Some("collab".into()),
        };
        assert_eq!(action.notif_id(), Some(7));
        assert_eq!(action.action_idx(), Some(2));
        assert_eq!(NotifLine::blank().notif_id(), None);
    }

    #[test]
    fn category_header_is_the_fold_key() {
        let hdr = NotifLine {
            text: "collab".into(),
            kind: NotifLineKind::CategoryHeader("collab".into()),
            category: Some("collab".into()),
        };
        assert_eq!(
            NotifView::collapse_key_for_line(&hdr),
            Some(CollapseKey::Category("collab".into()))
        );
        let mut v = NotifView::new();
        let k = CollapseKey::Category("collab".into());
        assert!(!v.is_collapsed(&k));
        v.toggle(k.clone());
        assert!(v.is_collapsed(&k));
    }
}
