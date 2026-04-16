//! Conversation buffer: structured AI interaction history.
//!
//! This is NOT backed by a rope. The conversation entries are the single
//! source of truth. Rendering happens directly from structured data,
//! avoiding the sync problem of keeping a rope and entry list coherent.
//!
//! Emacs lesson: don't try to shoehorn structured data into a flat text
//! buffer. Conversation is inherently structured (roles, tool calls,
//! results). Render it directly.

use serde::{Deserialize, Serialize};

// Role of a conversation entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversationRole {
    User,
    Assistant,
    ToolCall { name: String },
    ToolResult { success: bool },
    System,
}

/// A single entry in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEntry {
    pub role: ConversationRole,
    pub content: String,
    pub collapsed: bool,
}

/// Style hint for rendered lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineStyle {
    RoleMarker,
    UserText,
    AssistantText,
    ToolCallHeader,
    ToolResultText,
    SystemText,
    Separator,
    InputPrompt,
}

/// A single rendered line ready for display.
#[derive(Debug, Clone)]
pub struct RenderedLine {
    pub text: String,
    pub style: LineStyle,
}

/// Conversation state for an AI interaction pane.
pub struct Conversation {
    pub entries: Vec<ConversationEntry>,
    pub input_line: String,
    pub streaming: bool,
    /// When streaming started, used to display elapsed time in the UI.
    pub streaming_start: Option<std::time::Instant>,
    version: u64,
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

impl Conversation {
    pub fn new() -> Self {
        Conversation {
            entries: Vec::new(),
            input_line: String::new(),
            streaming: false,
            streaming_start: None,
            version: 0,
        }
    }

    /// Maximum conversation entries to retain.
    const MAX_ENTRIES: usize = 5000;

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Trim oldest entries if the conversation exceeds the bound.
    fn trim_entries(&mut self) {
        if self.entries.len() > Self::MAX_ENTRIES {
            let excess = self.entries.len() - Self::MAX_ENTRIES;
            self.entries.drain(..excess);
        }
    }

    pub fn push_user(&mut self, text: impl Into<String>) {
        self.entries.push(ConversationEntry {
            role: ConversationRole::User,
            content: text.into(),
            collapsed: false,
        });
        self.version += 1;
        self.trim_entries();
    }

    pub fn push_assistant(&mut self, text: impl Into<String>) {
        self.entries.push(ConversationEntry {
            role: ConversationRole::Assistant,
            content: text.into(),
            collapsed: false,
        });
        self.version += 1;
        self.trim_entries();
    }

    pub fn push_tool_call(&mut self, name: impl Into<String>) {
        self.entries.push(ConversationEntry {
            role: ConversationRole::ToolCall { name: name.into() },
            content: String::new(),
            collapsed: true,
        });
        self.version += 1;
        self.trim_entries();
    }

    pub fn push_tool_result(&mut self, success: bool, output: impl Into<String>) {
        self.entries.push(ConversationEntry {
            role: ConversationRole::ToolResult { success },
            content: output.into(),
            collapsed: true,
        });
        self.version += 1;
        self.trim_entries();
    }

    pub fn push_system(&mut self, text: impl Into<String>) {
        self.entries.push(ConversationEntry {
            role: ConversationRole::System,
            content: text.into(),
            collapsed: false,
        });
        self.version += 1;
        self.trim_entries();
    }

    /// Append a streaming chunk to the last assistant entry.
    /// If the last entry isn't an assistant entry, creates one.
    pub fn append_streaming_chunk(&mut self, text: &str) {
        if let Some(last) = self.entries.last_mut() {
            if last.role == ConversationRole::Assistant {
                last.content.push_str(text);
                self.version += 1;
                return;
            }
        }
        // No assistant entry to append to — create one
        self.push_assistant(text);
    }

    /// Toggle collapsed state of an entry.
    pub fn toggle_collapsed(&mut self, index: usize) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.collapsed = !entry.collapsed;
            self.version += 1;
        }
    }

    /// Serialize the conversation's entries to pretty-printed JSON. Only
    /// entries are persisted; transient state (streaming, input buffer,
    /// version counter) is intentionally not serialized since it has no
    /// meaning across sessions.
    pub fn to_json(&self) -> Result<String, String> {
        #[derive(Serialize)]
        struct Wire<'a> {
            version: u32,
            entries: &'a [ConversationEntry],
        }
        serde_json::to_string_pretty(&Wire {
            version: 1,
            entries: &self.entries,
        })
        .map_err(|e| e.to_string())
    }

    /// Replace entries with those loaded from JSON. Rejects unknown
    /// schema versions so future format changes fail loudly.
    pub fn load_json(&mut self, json: &str) -> Result<(), String> {
        #[derive(Deserialize)]
        struct Wire {
            version: u32,
            entries: Vec<ConversationEntry>,
        }
        let wire: Wire = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if wire.version != 1 {
            return Err(format!(
                "Unsupported conversation format version: {}",
                wire.version
            ));
        }
        self.entries = wire.entries;
        self.trim_entries();
        self.version += 1;
        Ok(())
    }

    /// Render all entries + input line into display lines.
    pub fn rendered_lines(&self) -> Vec<RenderedLine> {
        let mut lines = Vec::new();

        for entry in &self.entries {
            match &entry.role {
                ConversationRole::User => {
                    lines.push(RenderedLine {
                        text: "[You]".into(),
                        style: LineStyle::RoleMarker,
                    });
                    for line in entry.content.lines() {
                        lines.push(RenderedLine {
                            text: line.to_string(),
                            style: LineStyle::UserText,
                        });
                    }
                    if entry.content.is_empty() {
                        lines.push(RenderedLine {
                            text: String::new(),
                            style: LineStyle::UserText,
                        });
                    }
                }
                ConversationRole::Assistant => {
                    lines.push(RenderedLine {
                        text: "[AI]".into(),
                        style: LineStyle::RoleMarker,
                    });
                    for line in entry.content.lines() {
                        lines.push(RenderedLine {
                            text: line.to_string(),
                            style: LineStyle::AssistantText,
                        });
                    }
                    if entry.content.is_empty() {
                        lines.push(RenderedLine {
                            text: String::new(),
                            style: LineStyle::AssistantText,
                        });
                    }
                }
                ConversationRole::ToolCall { name } => {
                    if entry.collapsed {
                        lines.push(RenderedLine {
                            text: format!("▸ [Tool: {}]", name),
                            style: LineStyle::ToolCallHeader,
                        });
                    } else {
                        lines.push(RenderedLine {
                            text: format!("▾ [Tool: {}]", name),
                            style: LineStyle::ToolCallHeader,
                        });
                        for line in entry.content.lines() {
                            lines.push(RenderedLine {
                                text: format!("  {}", line),
                                style: LineStyle::ToolResultText,
                            });
                        }
                    }
                }
                ConversationRole::ToolResult { success } => {
                    let marker = if *success { "✓" } else { "✗" };
                    if entry.collapsed {
                        lines.push(RenderedLine {
                            text: format!("  [{}]", marker),
                            style: LineStyle::ToolResultText,
                        });
                    } else {
                        lines.push(RenderedLine {
                            text: format!("  [{}]", marker),
                            style: LineStyle::ToolResultText,
                        });
                        for line in entry.content.lines() {
                            lines.push(RenderedLine {
                                text: format!("  {}", line),
                                style: LineStyle::ToolResultText,
                            });
                        }
                    }
                }
                ConversationRole::System => {
                    lines.push(RenderedLine {
                        text: "[System]".into(),
                        style: LineStyle::RoleMarker,
                    });
                    for line in entry.content.lines() {
                        lines.push(RenderedLine {
                            text: line.to_string(),
                            style: LineStyle::SystemText,
                        });
                    }
                }
            }

            // Separator between entries
            lines.push(RenderedLine {
                text: String::new(),
                style: LineStyle::Separator,
            });
        }

        // Input prompt
        lines.push(RenderedLine {
            text: format!("> {}", self.input_line),
            style: LineStyle::InputPrompt,
        });

        lines
    }

    /// Total rendered line count (for scroll calculations).
    pub fn line_count(&self) -> usize {
        self.rendered_lines().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_conversation_is_empty() {
        let conv = Conversation::new();
        assert!(conv.entries.is_empty());
        assert!(conv.input_line.is_empty());
        assert!(!conv.streaming);
        assert_eq!(conv.version(), 0);
    }

    #[test]
    fn push_entries_ordering() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        conv.push_assistant("hi there");
        conv.push_tool_call("buffer_read");
        conv.push_tool_result(true, "content here");

        assert_eq!(conv.entries.len(), 4);
        assert_eq!(conv.entries[0].role, ConversationRole::User);
        assert_eq!(conv.entries[1].role, ConversationRole::Assistant);
        assert!(matches!(
            conv.entries[2].role,
            ConversationRole::ToolCall { .. }
        ));
        assert!(matches!(
            conv.entries[3].role,
            ConversationRole::ToolResult { .. }
        ));
    }

    #[test]
    fn streaming_append() {
        let mut conv = Conversation::new();
        conv.push_assistant("Hello");
        conv.append_streaming_chunk(", world");
        conv.append_streaming_chunk("!");

        assert_eq!(conv.entries.len(), 1);
        assert_eq!(conv.entries[0].content, "Hello, world!");
    }

    #[test]
    fn streaming_append_creates_entry_if_needed() {
        let mut conv = Conversation::new();
        conv.push_user("ask something");
        conv.append_streaming_chunk("response");

        assert_eq!(conv.entries.len(), 2);
        assert_eq!(conv.entries[1].role, ConversationRole::Assistant);
        assert_eq!(conv.entries[1].content, "response");
    }

    #[test]
    fn rendered_lines_contain_role_markers() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        conv.push_assistant("hi");

        let lines = conv.rendered_lines();
        assert!(lines.iter().any(|l| l.text == "[You]"));
        assert!(lines.iter().any(|l| l.text == "[AI]"));
        assert!(lines.iter().any(|l| l.text == "hello"));
        assert!(lines.iter().any(|l| l.text == "hi"));
    }

    #[test]
    fn collapsed_tool_results() {
        let mut conv = Conversation::new();
        conv.push_tool_call("buffer_read");
        conv.push_tool_result(true, "file contents\nline 2");

        let lines = conv.rendered_lines();
        // Tool call should be collapsed (▸)
        assert!(lines.iter().any(|l| l.text.contains("▸")));
        // Tool result content should not appear when collapsed
        let result_content_lines: Vec<_> = lines
            .iter()
            .filter(|l| l.text.contains("file contents"))
            .collect();
        assert!(result_content_lines.is_empty());

        // Expand it
        conv.toggle_collapsed(1);
        let lines = conv.rendered_lines();
        assert!(lines.iter().any(|l| l.text.contains("file contents")));
    }

    #[test]
    fn line_count_includes_input_prompt() {
        let conv = Conversation::new();
        // Empty conversation: just the input prompt line
        assert_eq!(conv.line_count(), 1);

        let mut conv = Conversation::new();
        conv.push_user("hello");
        // [You] + "hello" + separator + "> "
        assert!(conv.line_count() >= 3);
    }

    #[test]
    fn version_increments() {
        let mut conv = Conversation::new();
        assert_eq!(conv.version(), 0);
        conv.push_user("hello");
        assert_eq!(conv.version(), 1);
        conv.push_assistant("hi");
        assert_eq!(conv.version(), 2);
        conv.append_streaming_chunk(" there");
        assert_eq!(conv.version(), 3);
    }

    #[test]
    fn to_json_round_trip_preserves_entries() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        conv.push_assistant("hi there");
        conv.push_tool_call("buffer_read");
        conv.push_tool_result(true, "the file contents");
        conv.push_system("system note");

        let json = conv.to_json().unwrap();
        let mut restored = Conversation::new();
        restored.load_json(&json).unwrap();

        assert_eq!(restored.entries.len(), 5);
        assert_eq!(restored.entries[0].content, "hello");
        assert_eq!(restored.entries[1].content, "hi there");
        assert!(matches!(
            restored.entries[2].role,
            ConversationRole::ToolCall { ref name } if name == "buffer_read"
        ));
        assert!(matches!(
            restored.entries[3].role,
            ConversationRole::ToolResult { success: true }
        ));
        assert_eq!(restored.entries[4].role, ConversationRole::System);
    }

    #[test]
    fn load_json_replaces_existing_entries() {
        let mut conv = Conversation::new();
        conv.push_user("original");
        let saved = conv.to_json().unwrap();

        let mut other = Conversation::new();
        other.push_user("to be replaced");
        other.push_assistant("also replaced");
        other.load_json(&saved).unwrap();

        assert_eq!(other.entries.len(), 1);
        assert_eq!(other.entries[0].content, "original");
    }

    #[test]
    fn load_json_rejects_unknown_version() {
        let bad = r#"{"version": 99, "entries": []}"#;
        let mut conv = Conversation::new();
        let err = conv.load_json(bad).unwrap_err();
        assert!(err.contains("Unsupported"));
    }

    #[test]
    fn load_json_rejects_garbage() {
        let mut conv = Conversation::new();
        assert!(conv.load_json("not valid json").is_err());
    }

    #[test]
    fn to_json_produces_stable_schema() {
        let mut conv = Conversation::new();
        conv.push_user("hi");
        let json = conv.to_json().unwrap();
        assert!(json.contains("\"version\""));
        assert!(json.contains("\"entries\""));
        assert!(json.contains("\"User\""));
    }
}
