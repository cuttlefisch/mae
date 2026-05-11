//! Display policy — enum-based buffer placement rules.
//!
//! Emacs has 29 `display-buffer-*` functions, a regex alist, and a global
//! override. We have 4 actions and O(1) dispatch by `BufferKind`.
//!
//! See `concept:display-policy` in the KB for full rationale.

use crate::buffer::BufferKind;
use crate::window::SplitDirection;

/// How a buffer should be placed when it needs to become visible.
/// 4 actions (vs Emacs' 29 display-buffer-* functions).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DisplayAction {
    /// Replace focused window. Falls through to AvoidConversation if focused on conversation.
    ReplaceFocused,
    /// Route via switch_to_buffer_non_conversation (protects conversation pair).
    AvoidConversation,
    /// Reuse existing window of same BufferKind, or create a split.
    /// Emacs 28+ "side window" pattern — tool buffers reuse their dedicated window.
    ReuseOrSplit {
        direction: SplitDirection,
        ratio: f32,
    },
    /// Buffer exists only for programmatic access — never shown.
    Hidden,
}

/// Display policy: maps each `BufferKind` to a `DisplayAction`.
///
/// Uses match dispatch (not HashMap) because `BufferKind` doesn't derive Hash
/// and the variant set is small and stable.
#[derive(Debug, Clone, Default)]
pub struct DisplayPolicy {
    // Override slots — None means "use default".
    overrides: Vec<(BufferKind, DisplayAction)>,
}

impl DisplayPolicy {
    /// Look up the display action for a buffer kind.
    pub fn action_for(&self, kind: BufferKind) -> DisplayAction {
        // Check overrides first.
        for &(k, action) in &self.overrides {
            if k == kind {
                return action;
            }
        }
        Self::default_action(kind)
    }

    /// Set an override for a specific buffer kind.
    pub fn set_override(&mut self, kind: BufferKind, action: DisplayAction) {
        // Replace existing override or add new one.
        if let Some(entry) = self.overrides.iter_mut().find(|(k, _)| *k == kind) {
            entry.1 = action;
        } else {
            self.overrides.push((kind, action));
        }
    }

    /// Default rules — the baseline policy.
    fn default_action(kind: BufferKind) -> DisplayAction {
        match kind {
            // Normal files never invade conversation
            BufferKind::Text => DisplayAction::AvoidConversation,
            // AI diffs avoid conversation
            BufferKind::Diff => DisplayAction::AvoidConversation,
            // Reuse existing help window, or 50% vsplit
            BufferKind::Help => DisplayAction::ReuseOrSplit {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
            },
            // Bottom 30%, reuse if open
            BufferKind::Messages => DisplayAction::ReuseOrSplit {
                direction: SplitDirection::Horizontal,
                ratio: 0.3,
            },
            // Bottom 35%
            BufferKind::Shell => DisplayAction::ReuseOrSplit {
                direction: SplitDirection::Horizontal,
                ratio: 0.35,
            },
            // Bottom 40%
            BufferKind::Debug => DisplayAction::ReuseOrSplit {
                direction: SplitDirection::Horizontal,
                ratio: 0.4,
            },
            // Left 20% sidebar
            BufferKind::FileTree => DisplayAction::ReuseOrSplit {
                direction: SplitDirection::Vertical,
                ratio: 0.2,
            },
            // Full window (Magit style)
            BufferKind::GitStatus => DisplayAction::ReplaceFocused,
            // Splash takes full window
            BufferKind::Dashboard => DisplayAction::ReplaceFocused,
            // Scene graph
            BufferKind::Visual => DisplayAction::ReplaceFocused,
            // Read-only preview
            BufferKind::Preview => DisplayAction::ReplaceFocused,
            // Managed by open_conversation_buffer()
            BufferKind::Conversation => DisplayAction::Hidden,
            // Full window (like git-status)
            BufferKind::Agenda => DisplayAction::ReplaceFocused,
            // Full window — editable sandbox
            BufferKind::Demo => DisplayAction::ReplaceFocused,
        }
    }

    /// Format all rules as a human-readable report.
    pub fn format_report(&self) -> String {
        let mut lines = Vec::new();
        lines.push("Display Policy Rules".to_string());
        lines.push("====================".to_string());
        lines.push(String::new());
        lines.push(format!(
            "{:<15} {:<15} {}",
            "BufferKind", "Action", "Details"
        ));
        lines.push(format!(
            "{:<15} {:<15} {}",
            "----------", "------", "-------"
        ));

        let kinds = [
            BufferKind::Text,
            BufferKind::Diff,
            BufferKind::Help,
            BufferKind::Messages,
            BufferKind::Shell,
            BufferKind::Debug,
            BufferKind::FileTree,
            BufferKind::GitStatus,
            BufferKind::Dashboard,
            BufferKind::Visual,
            BufferKind::Preview,
            BufferKind::Conversation,
            BufferKind::Agenda,
            BufferKind::Demo,
        ];

        for kind in &kinds {
            let action = self.action_for(*kind);
            let (action_name, details) = format_action(&action);
            let kind_name = format!("{:?}", kind);
            let overridden = self.overrides.iter().any(|(k, _)| k == kind);
            let suffix = if overridden { " *" } else { "" };
            lines.push(format!(
                "{:<15} {:<15} {}{}",
                kind_name, action_name, details, suffix
            ));
        }

        lines.push(String::new());
        lines.push("* = user override (from init.scm set-display-rule!)".to_string());
        lines.join("\n")
    }
}

/// Format a DisplayAction for display.
pub fn format_action(action: &DisplayAction) -> (&'static str, String) {
    match action {
        DisplayAction::ReplaceFocused => ("ReplaceFocused", String::new()),
        DisplayAction::AvoidConversation => ("AvoidConv", String::new()),
        DisplayAction::ReuseOrSplit { direction, ratio } => {
            let dir = match direction {
                SplitDirection::Vertical => "vertical",
                SplitDirection::Horizontal => "horizontal",
            };
            ("ReuseOrSplit", format!("{}:{:.0}%", dir, ratio * 100.0))
        }
        DisplayAction::Hidden => ("Hidden", String::new()),
    }
}

/// Parse a display action from a string representation.
/// Formats: "replace-focused", "avoid-conversation", "hidden",
///          "reuse-or-split:vertical:0.5", "reuse-or-split:horizontal:0.3"
pub fn parse_action(s: &str) -> Option<DisplayAction> {
    match s {
        "replace-focused" => Some(DisplayAction::ReplaceFocused),
        "avoid-conversation" => Some(DisplayAction::AvoidConversation),
        "hidden" => Some(DisplayAction::Hidden),
        _ if s.starts_with("reuse-or-split:") => {
            let parts: Vec<&str> = s.split(':').collect();
            if parts.len() != 3 {
                return None;
            }
            let direction = match parts[1] {
                "vertical" => SplitDirection::Vertical,
                "horizontal" => SplitDirection::Horizontal,
                _ => return None,
            };
            let ratio: f32 = parts[2].parse().ok()?;
            if !(0.0..=1.0).contains(&ratio) {
                return None;
            }
            Some(DisplayAction::ReuseOrSplit { direction, ratio })
        }
        _ => None,
    }
}

/// Format a display action as a parseable string.
pub fn action_to_string(action: &DisplayAction) -> String {
    match action {
        DisplayAction::ReplaceFocused => "replace-focused".to_string(),
        DisplayAction::AvoidConversation => "avoid-conversation".to_string(),
        DisplayAction::Hidden => "hidden".to_string(),
        DisplayAction::ReuseOrSplit { direction, ratio } => {
            let dir = match direction {
                SplitDirection::Vertical => "vertical",
                SplitDirection::Horizontal => "horizontal",
            };
            format!("reuse-or-split:{}:{}", dir, ratio)
        }
    }
}

/// Parse a BufferKind from its name string.
pub fn parse_buffer_kind(s: &str) -> Option<BufferKind> {
    match s.to_lowercase().as_str() {
        "text" => Some(BufferKind::Text),
        "conversation" => Some(BufferKind::Conversation),
        "preview" => Some(BufferKind::Preview),
        "messages" => Some(BufferKind::Messages),
        "help" => Some(BufferKind::Help),
        "shell" => Some(BufferKind::Shell),
        "debug" => Some(BufferKind::Debug),
        "dashboard" => Some(BufferKind::Dashboard),
        "gitstatus" | "git-status" => Some(BufferKind::GitStatus),
        "visual" => Some(BufferKind::Visual),
        "filetree" | "file-tree" => Some(BufferKind::FileTree),
        "diff" => Some(BufferKind::Diff),
        "agenda" => Some(BufferKind::Agenda),
        "demo" => Some(BufferKind::Demo),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rules_cover_all_kinds() {
        let policy = DisplayPolicy::default();
        let kinds = [
            BufferKind::Text,
            BufferKind::Conversation,
            BufferKind::Preview,
            BufferKind::Messages,
            BufferKind::Help,
            BufferKind::Shell,
            BufferKind::Debug,
            BufferKind::Dashboard,
            BufferKind::GitStatus,
            BufferKind::Visual,
            BufferKind::FileTree,
            BufferKind::Diff,
            BufferKind::Agenda,
            BufferKind::Demo,
        ];
        for kind in &kinds {
            let _ = policy.action_for(*kind);
        }
    }

    #[test]
    fn action_for_correct() {
        let policy = DisplayPolicy::default();
        assert!(matches!(
            policy.action_for(BufferKind::Help),
            DisplayAction::ReuseOrSplit { .. }
        ));
        assert_eq!(
            policy.action_for(BufferKind::Text),
            DisplayAction::AvoidConversation
        );
        assert_eq!(
            policy.action_for(BufferKind::Conversation),
            DisplayAction::Hidden
        );
    }

    #[test]
    fn override_replaces_default() {
        let mut policy = DisplayPolicy::default();
        policy.set_override(BufferKind::Help, DisplayAction::ReplaceFocused);
        assert_eq!(
            policy.action_for(BufferKind::Help),
            DisplayAction::ReplaceFocused
        );
        // Other kinds unchanged.
        assert_eq!(
            policy.action_for(BufferKind::Text),
            DisplayAction::AvoidConversation
        );
    }

    #[test]
    fn parse_roundtrip() {
        let actions = [
            DisplayAction::ReplaceFocused,
            DisplayAction::AvoidConversation,
            DisplayAction::Hidden,
            DisplayAction::ReuseOrSplit {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
            },
            DisplayAction::ReuseOrSplit {
                direction: SplitDirection::Horizontal,
                ratio: 0.3,
            },
        ];
        for action in &actions {
            let s = action_to_string(action);
            let parsed = parse_action(&s).unwrap_or_else(|| panic!("Failed to parse: {}", s));
            assert_eq!(*action, parsed);
        }
    }

    #[test]
    fn parse_buffer_kind_works() {
        assert_eq!(parse_buffer_kind("text"), Some(BufferKind::Text));
        assert_eq!(parse_buffer_kind("Help"), Some(BufferKind::Help));
        assert_eq!(parse_buffer_kind("git-status"), Some(BufferKind::GitStatus));
        assert_eq!(parse_buffer_kind("nonsense"), None);
    }

    #[test]
    fn format_report_contains_all_kinds() {
        let policy = DisplayPolicy::default();
        let report = policy.format_report();
        assert!(report.contains("Text"));
        assert!(report.contains("Help"));
        assert!(report.contains("Conversation"));
        assert!(report.contains("Hidden"));
    }
}
