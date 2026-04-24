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
use unicode_width::UnicodeWidthStr;

/// Count how many screen lines a rendered line produces when wrapped to `width` columns.
/// Uses display width (UnicodeWidthStr) so CJK characters count as 2 columns.
pub fn screen_line_count(text: &str, width: usize) -> usize {
    let w = width.max(1);
    if text.is_empty() {
        return 1;
    }
    let cols = UnicodeWidthStr::width(text);
    if cols <= w {
        return 1;
    }
    cols.div_ceil(w)
}

// Role of a conversation entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversationRole {
    User,
    Assistant,
    ToolCall {
        name: String,
    },
    ToolResult {
        success: bool,
        elapsed_ms: Option<u64>,
    },
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
    /// The index of the ConversationEntry this line belongs to, if any.
    pub entry_index: Option<usize>,
}

/// Conversation state for an AI interaction pane.
pub struct Conversation {
    pub entries: Vec<ConversationEntry>,
    pub input_line: String,
    /// Byte offset of the editing cursor within `input_line`.
    pub input_cursor: usize,
    /// Conversation-specific scroll state: 0 = bottom (auto-follow),
    /// positive = scrolled up N lines from the bottom.
    pub scroll: usize,
    pub streaming: bool,
    /// When streaming started, used to display elapsed time in the UI.
    pub streaming_start: Option<std::time::Instant>,
    version: u64,
    /// Pre-computed rendered lines, rebuilt on every content mutation.
    /// Avoids O(N) allocation per frame during scrolling.
    cached_lines: Vec<RenderedLine>,
    /// Per-line screen counts cached alongside rendered lines.
    /// Width-dependent — invalidated when width changes.
    cached_screen_counts: Vec<usize>,
    cached_screen_width: usize,
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

impl Conversation {
    /// Mark streaming as finished and clear the start timestamp.
    pub fn end_streaming(&mut self) {
        self.streaming = false;
        self.streaming_start = None;
    }

    pub fn new() -> Self {
        let mut conv = Conversation {
            entries: Vec::new(),
            input_line: String::new(),
            input_cursor: 0,
            scroll: 0,
            streaming: false,
            streaming_start: None,
            version: 0,
            cached_lines: Vec::new(),
            cached_screen_counts: Vec::new(),
            cached_screen_width: 0,
        };
        conv.rebuild_render_cache();
        conv
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
        self.rebuild_render_cache();
    }

    pub fn push_assistant(&mut self, text: impl Into<String>) {
        self.entries.push(ConversationEntry {
            role: ConversationRole::Assistant,
            content: text.into(),
            collapsed: false,
        });
        self.version += 1;
        self.trim_entries();
        self.rebuild_render_cache();
    }

    pub fn push_tool_call(&mut self, name: impl Into<String>) {
        self.entries.push(ConversationEntry {
            role: ConversationRole::ToolCall { name: name.into() },
            content: String::new(),
            collapsed: true,
        });
        self.version += 1;
        self.trim_entries();
        self.rebuild_render_cache();
    }

    pub fn push_tool_result(
        &mut self,
        success: bool,
        output: impl Into<String>,
        elapsed_ms: Option<u64>,
    ) {
        self.entries.push(ConversationEntry {
            role: ConversationRole::ToolResult {
                success,
                elapsed_ms,
            },
            content: output.into(),
            collapsed: true,
        });
        self.version += 1;
        self.trim_entries();
        self.rebuild_render_cache();
    }

    pub fn push_system(&mut self, text: impl Into<String>) {
        self.entries.push(ConversationEntry {
            role: ConversationRole::System,
            content: text.into(),
            collapsed: false,
        });
        self.version += 1;
        self.trim_entries();
        self.rebuild_render_cache();
    }

    /// Append a streaming chunk to the last assistant entry.
    /// If the last entry isn't an assistant entry, creates one.
    pub fn append_streaming_chunk(&mut self, text: &str) {
        if let Some(last) = self.entries.last_mut() {
            if last.role == ConversationRole::Assistant {
                last.content.push_str(text);
                self.version += 1;
                self.rebuild_render_cache();
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
            self.rebuild_render_cache();
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
        self.rebuild_render_cache();
        Ok(())
    }

    /// Rebuild the cached rendered lines. Called after every content mutation.
    fn rebuild_render_cache(&mut self) {
        self.cached_lines = self.compute_rendered_lines();
        self.cached_screen_counts.clear();
    }

    /// O(1) update of just the input prompt line in the cache.
    /// Used by input editing methods to avoid O(N) full rebuild.
    fn update_input_in_cache(&mut self) {
        if let Some(last) = self.cached_lines.last_mut() {
            if last.style == LineStyle::InputPrompt {
                last.text = format!("> {}", self.input_line);
                self.cached_screen_counts.clear();
                return;
            }
        }
        self.rebuild_render_cache();
    }

    /// Return pre-computed rendered lines (zero-allocation on read).
    pub fn rendered_lines(&self) -> &[RenderedLine] {
        &self.cached_lines
    }

    /// Ensure per-line screen counts are computed for the given width.
    /// Returns `(per_line_counts, total_screen_lines)`.
    /// Amortized O(1) when width and content are unchanged.
    pub fn ensure_screen_counts(&mut self, width: usize) -> (&[usize], usize) {
        let w = width.max(1);
        if self.cached_screen_counts.len() != self.cached_lines.len()
            || self.cached_screen_width != w
        {
            self.cached_screen_counts = self
                .cached_lines
                .iter()
                .map(|rl| screen_line_count(&rl.text, w))
                .collect();
            self.cached_screen_width = w;
        }
        let total: usize = self.cached_screen_counts.iter().sum();
        (&self.cached_screen_counts, total)
    }

    /// Return pre-computed screen counts (must call `ensure_screen_counts` first).
    /// Returns `(per_line_counts, total)`. If not yet computed, returns empty.
    pub fn screen_counts(&self) -> (&[usize], usize) {
        let total: usize = self.cached_screen_counts.iter().sum();
        (&self.cached_screen_counts, total)
    }

    /// Render all entries + input line into display lines.
    fn compute_rendered_lines(&self) -> Vec<RenderedLine> {
        let mut lines = Vec::new();

        for (i, entry) in self.entries.iter().enumerate() {
            match &entry.role {
                ConversationRole::User => {
                    lines.push(RenderedLine {
                        text: "[You]".into(),
                        style: LineStyle::RoleMarker,
                        entry_index: Some(i),
                    });
                    for line in entry.content.lines() {
                        lines.push(RenderedLine {
                            text: line.to_string(),
                            style: LineStyle::UserText,
                            entry_index: Some(i),
                        });
                    }
                    if entry.content.is_empty() {
                        lines.push(RenderedLine {
                            text: String::new(),
                            style: LineStyle::UserText,
                            entry_index: Some(i),
                        });
                    }
                }
                ConversationRole::Assistant => {
                    lines.push(RenderedLine {
                        text: "[AI]".into(),
                        style: LineStyle::RoleMarker,
                        entry_index: Some(i),
                    });
                    for line in entry.content.lines() {
                        lines.push(RenderedLine {
                            text: line.to_string(),
                            style: LineStyle::AssistantText,
                            entry_index: Some(i),
                        });
                    }
                    if entry.content.is_empty() {
                        lines.push(RenderedLine {
                            text: String::new(),
                            style: LineStyle::AssistantText,
                            entry_index: Some(i),
                        });
                    }
                }
                ConversationRole::ToolCall { name } => {
                    if entry.collapsed {
                        lines.push(RenderedLine {
                            text: format!("▸ [Tool: {}]", name),
                            style: LineStyle::ToolCallHeader,
                            entry_index: Some(i),
                        });
                    } else {
                        lines.push(RenderedLine {
                            text: format!("▾ [Tool: {}]", name),
                            style: LineStyle::ToolCallHeader,
                            entry_index: Some(i),
                        });
                        for line in entry.content.lines() {
                            lines.push(RenderedLine {
                                text: format!("  {}", line),
                                style: LineStyle::ToolResultText,
                                entry_index: Some(i),
                            });
                        }
                    }
                }
                ConversationRole::ToolResult {
                    success,
                    elapsed_ms,
                } => {
                    let marker = if *success { "✓" } else { "✗" };
                    let header = match elapsed_ms {
                        Some(ms) => format!("  [{} {}ms]", marker, ms),
                        None => format!("  [{}]", marker),
                    };
                    if entry.collapsed {
                        lines.push(RenderedLine {
                            text: header,
                            style: LineStyle::ToolResultText,
                            entry_index: Some(i),
                        });
                    } else {
                        lines.push(RenderedLine {
                            text: header,
                            style: LineStyle::ToolResultText,
                            entry_index: Some(i),
                        });
                        for line in entry.content.lines() {
                            lines.push(RenderedLine {
                                text: format!("  {}", line),
                                style: LineStyle::ToolResultText,
                                entry_index: Some(i),
                            });
                        }
                    }
                }
                ConversationRole::System => {
                    lines.push(RenderedLine {
                        text: "[System]".into(),
                        style: LineStyle::RoleMarker,
                        entry_index: Some(i),
                    });
                    for line in entry.content.lines() {
                        lines.push(RenderedLine {
                            text: line.to_string(),
                            style: LineStyle::SystemText,
                            entry_index: Some(i),
                        });
                    }
                }
            }

            // Separator between entries
            lines.push(RenderedLine {
                text: String::new(),
                style: LineStyle::Separator,
                entry_index: None,
            });
        }

        // Input prompt
        lines.push(RenderedLine {
            text: format!("> {}", self.input_line),
            style: LineStyle::InputPrompt,
            entry_index: None,
        });

        lines
    }

    /// Flatten all rendered lines into a single string for visual mode operations.
    /// This is the text that visual mode selection coordinates map to.
    pub fn flat_text(&self) -> String {
        self.cached_lines
            .iter()
            .map(|rl| rl.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Total rendered line count (for scroll calculations).
    pub fn line_count(&self) -> usize {
        self.cached_lines.len()
    }

    /// Split the input prompt into spans for cursor rendering.
    ///
    /// Returns `(prefix, before_cursor, cursor_char, after_cursor)` where
    /// `cursor_char` is the character under the cursor or `" "` at end of line.
    /// The renderer applies `ui.cursor` style to `cursor_char`.
    pub fn input_cursor_spans(&self) -> (&str, &str, String, &str) {
        let input = &self.input_line;
        let cursor_byte = self.input_cursor.min(input.len());
        let before = &input[..cursor_byte];
        if cursor_byte < input.len() {
            let rest = &input[cursor_byte..];
            let ch_len = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            let cursor_ch = input[cursor_byte..cursor_byte + ch_len].to_string();
            let after = &input[cursor_byte + ch_len..];
            ("> ", before, cursor_ch, after)
        } else {
            ("> ", before, " ".to_string(), "")
        }
    }

    // -----------------------------------------------------------------------
    // Input readline editing
    // -----------------------------------------------------------------------

    /// Insert `ch` at `input_cursor`, advancing the cursor.
    pub fn input_insert_char(&mut self, ch: char) {
        self.input_line.insert(self.input_cursor, ch);
        self.input_cursor += ch.len_utf8();
        self.update_input_in_cache();
    }

    /// Delete the char immediately before the cursor (Backspace / C-h).
    pub fn input_backspace(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let before = &self.input_line[..self.input_cursor];
        let (prev_start, _) = before.char_indices().next_back().unwrap();
        let removed_len = self.input_cursor - prev_start;
        self.input_line.remove(prev_start);
        self.input_cursor -= removed_len;
        self.update_input_in_cache();
    }

    /// Delete the char at the cursor (C-d / Delete).
    pub fn input_delete_forward(&mut self) {
        if self.input_cursor >= self.input_line.len() {
            return;
        }
        self.input_line.remove(self.input_cursor);
        self.update_input_in_cache();
    }

    /// Move cursor to start of input (C-a / Home).
    pub fn input_move_home(&mut self) {
        self.input_cursor = 0;
    }

    /// Move cursor to end of input (C-e / End).
    pub fn input_move_end(&mut self) {
        self.input_cursor = self.input_line.len();
    }

    /// Move cursor one char backward (C-b / Left).
    pub fn input_move_backward(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let before = &self.input_line[..self.input_cursor];
        let (prev_start, _) = before.char_indices().next_back().unwrap();
        self.input_cursor = prev_start;
    }

    /// Move cursor one char forward (C-f / Right).
    pub fn input_move_forward(&mut self) {
        if self.input_cursor >= self.input_line.len() {
            return;
        }
        let ch = self.input_line[self.input_cursor..].chars().next().unwrap();
        self.input_cursor += ch.len_utf8();
    }

    /// Delete backward to the last whitespace boundary (C-w, bash-style).
    pub fn input_kill_word_backward(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let before = &self.input_line[..self.input_cursor];
        // Strip trailing whitespace, then find last whitespace before the word.
        let trimmed_end = before.trim_end().len();
        let word_start = if trimmed_end == 0 {
            0
        } else {
            self.input_line[..trimmed_end]
                .rfind(|c: char| c.is_whitespace())
                .map(|i| i + 1)
                .unwrap_or(0)
        };
        self.input_line.drain(word_start..self.input_cursor);
        self.input_cursor = word_start;
        self.update_input_in_cache();
    }

    /// Delete from start of input to cursor (C-u).
    pub fn input_kill_to_start(&mut self) {
        self.input_line.drain(..self.input_cursor);
        self.input_cursor = 0;
        self.update_input_in_cache();
    }

    /// Delete from cursor to end of input (C-k).
    pub fn input_kill_to_end(&mut self) {
        self.input_line.truncate(self.input_cursor);
        self.update_input_in_cache();
    }

    // -----------------------------------------------------------------------
    // Scroll control
    // -----------------------------------------------------------------------

    /// Scroll conversation history up by `n` lines (toward older content).
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_add(n);
    }

    /// Scroll conversation history down by `n` lines (toward newer content).
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_sub(n);
    }

    /// Jump to the bottom of the conversation (re-enables auto-scroll).
    pub fn scroll_to_bottom(&mut self) {
        self.scroll = 0;
    }

    /// Jump to the top of the conversation history.
    pub fn scroll_to_top(&mut self) {
        self.scroll = self.cached_lines.len();
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
        assert_eq!(conv.input_cursor, 0);
        assert_eq!(conv.scroll, 0);
        assert!(!conv.streaming);
        assert_eq!(conv.version(), 0);
    }

    #[test]
    fn input_insert_and_move() {
        let mut conv = Conversation::new();
        conv.input_insert_char('h');
        conv.input_insert_char('i');
        assert_eq!(conv.input_line, "hi");
        assert_eq!(conv.input_cursor, 2);

        conv.input_move_home();
        assert_eq!(conv.input_cursor, 0);
        conv.input_insert_char('!');
        assert_eq!(conv.input_line, "!hi");
        assert_eq!(conv.input_cursor, 1);
    }

    #[test]
    fn input_backspace_moves_cursor() {
        let mut conv = Conversation::new();
        conv.input_insert_char('a');
        conv.input_insert_char('b');
        conv.input_insert_char('c');
        conv.input_backspace();
        assert_eq!(conv.input_line, "ab");
        assert_eq!(conv.input_cursor, 2);

        conv.input_move_home();
        conv.input_backspace(); // no-op at start
        assert_eq!(conv.input_line, "ab");
        assert_eq!(conv.input_cursor, 0);
    }

    #[test]
    fn input_delete_forward() {
        let mut conv = Conversation::new();
        conv.input_insert_char('a');
        conv.input_insert_char('b');
        conv.input_move_home();
        conv.input_delete_forward();
        assert_eq!(conv.input_line, "b");
        assert_eq!(conv.input_cursor, 0);
    }

    #[test]
    fn input_kill_word_backward() {
        let mut conv = Conversation::new();
        for ch in "hello world".chars() {
            conv.input_insert_char(ch);
        }
        conv.input_kill_word_backward();
        assert_eq!(conv.input_line, "hello ");
        assert_eq!(conv.input_cursor, 6);

        // Kill with trailing spaces
        for ch in "  ".chars() {
            conv.input_insert_char(ch);
        }
        conv.input_kill_word_backward();
        assert_eq!(conv.input_line, "");
    }

    #[test]
    fn input_kill_to_start_and_end() {
        let mut conv = Conversation::new();
        for ch in "abcdef".chars() {
            conv.input_insert_char(ch);
        }
        conv.input_move_home();
        conv.input_move_forward();
        conv.input_move_forward();
        conv.input_move_forward(); // cursor at 3
        assert_eq!(conv.input_cursor, 3);

        conv.input_kill_to_end();
        assert_eq!(conv.input_line, "abc");

        conv.input_move_forward(); // still at end, no-op
        conv.input_move_home();
        conv.input_move_forward(); // cursor at 1
        conv.input_kill_to_start();
        assert_eq!(conv.input_line, "bc");
        assert_eq!(conv.input_cursor, 0);
    }

    #[test]
    fn scroll_up_down_clamps() {
        let mut conv = Conversation::new();
        assert_eq!(conv.scroll, 0);
        conv.scroll_up(5);
        assert_eq!(conv.scroll, 5);
        conv.scroll_down(3);
        assert_eq!(conv.scroll, 2);
        conv.scroll_down(100);
        assert_eq!(conv.scroll, 0);
        conv.scroll_to_bottom();
        assert_eq!(conv.scroll, 0);
    }

    #[test]
    fn push_entries_ordering() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        conv.push_assistant("hi there");
        conv.push_tool_call("buffer_read");
        conv.push_tool_result(true, "content here", None);

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
    fn flat_text_joins_rendered_lines() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        conv.push_assistant("world");
        let flat = conv.flat_text();
        assert!(flat.contains("[You]"));
        assert!(flat.contains("hello"));
        assert!(flat.contains("[AI]"));
        assert!(flat.contains("world"));
        // Lines are joined with newlines
        assert!(flat.contains('\n'));
    }

    #[test]
    fn flat_text_empty_conversation() {
        let conv = Conversation::new();
        let flat = conv.flat_text();
        // Should just be the input prompt
        assert!(flat.contains("> "));
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

        // Verify entry_index is populated
        assert_eq!(lines[0].entry_index, Some(0)); // [You]
        assert_eq!(lines[1].entry_index, Some(0)); // hello
        assert_eq!(lines[2].entry_index, None); // separator
        assert_eq!(lines[3].entry_index, Some(1)); // [AI]
    }

    #[test]
    fn collapsed_tool_results() {
        let mut conv = Conversation::new();
        conv.push_tool_call("buffer_read");
        conv.push_tool_result(true, "file contents\nline 2", None);

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
        conv.push_tool_result(true, "the file contents", None);
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
            ConversationRole::ToolResult { success: true, .. }
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

    // ---- input_cursor_spans tests ----

    #[test]
    fn cursor_spans_empty_input() {
        let conv = Conversation::new();
        let (prefix, before, cursor, after) = conv.input_cursor_spans();
        assert_eq!(prefix, "> ");
        assert_eq!(before, "");
        assert_eq!(cursor, " "); // block cursor at end
        assert_eq!(after, "");
    }

    #[test]
    fn cursor_spans_at_end() {
        let mut conv = Conversation::new();
        conv.input_line = "hello".into();
        conv.input_cursor = 5;
        let (_, before, cursor, after) = conv.input_cursor_spans();
        assert_eq!(before, "hello");
        assert_eq!(cursor, " "); // block at end
        assert_eq!(after, "");
    }

    #[test]
    fn cursor_spans_in_middle() {
        let mut conv = Conversation::new();
        conv.input_line = "hello".into();
        conv.input_cursor = 2;
        let (_, before, cursor, after) = conv.input_cursor_spans();
        assert_eq!(before, "he");
        assert_eq!(cursor, "l");
        assert_eq!(after, "lo");
    }

    #[test]
    fn cursor_spans_at_start() {
        let mut conv = Conversation::new();
        conv.input_line = "abc".into();
        conv.input_cursor = 0;
        let (_, before, cursor, after) = conv.input_cursor_spans();
        assert_eq!(before, "");
        assert_eq!(cursor, "a");
        assert_eq!(after, "bc");
    }

    #[test]
    fn cursor_spans_multibyte() {
        let mut conv = Conversation::new();
        conv.input_line = "héllo".into();
        conv.input_cursor = 1; // before 'é' (byte offset 1)
        let (_, before, cursor, after) = conv.input_cursor_spans();
        assert_eq!(before, "h");
        assert_eq!(cursor, "é");
        assert_eq!(after, "llo");
    }
}
