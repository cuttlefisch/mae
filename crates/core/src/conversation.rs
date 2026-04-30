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
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

/// Find the byte offset at approximately `n` display columns, snapped to a char boundary.
/// CJK characters count as 2 columns.
pub fn char_boundary_at(s: &str, n: usize) -> usize {
    let mut col = 0;
    for (byte_idx, ch) in s.char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(1);
        if col + w > n {
            return byte_idx;
        }
        col += w;
    }
    s.len()
}

/// Split text into rows of at most `width` display columns, respecting char boundaries.
pub fn wrap_text_into_rows(text: &str, width: usize) -> Vec<&str> {
    let w = width.max(1);
    if text.is_empty() {
        return vec![text];
    }
    if screen_line_count(text, w) <= 1 {
        return vec![text];
    }
    let mut rows = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        let end = char_boundary_at(remaining, w);
        rows.push(&remaining[..end]);
        remaining = &remaining[end..];
    }
    rows
}

/// Convert a char count into a display column count, accounting for wide (CJK) characters.
pub fn chars_to_display_cols(text: &str, char_count: usize) -> usize {
    text.chars()
        .take(char_count)
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(1))
        .sum()
}

/// State of a tool call in the conversation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCallState {
    /// Queued but not started.
    Pending,
    /// Currently executing.
    #[default]
    Running,
    /// Result received successfully.
    Completed,
    /// Execution failed.
    Error,
}

// Role of a conversation entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversationRole {
    User,
    Assistant,
    ToolCall {
        name: String,
        #[serde(default)]
        state: ToolCallState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
    },
    ToolResult {
        success: bool,
        elapsed_ms: Option<u64>,
    },
    System,
}

/// Per-message token usage from the API response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u32,
    pub output: u32,
    #[serde(default)]
    pub cache_read: u32,
}

impl TokenUsage {
    /// Format as a compact display string: `[in:1.2k out:340]`
    pub fn display_compact(&self) -> String {
        fn fmt_count(n: u32) -> String {
            if n >= 1000 {
                format!("{:.1}k", n as f64 / 1000.0)
            } else {
                n.to_string()
            }
        }
        if self.cache_read > 0 {
            format!(
                "[in:{} out:{} cache:{}]",
                fmt_count(self.input),
                fmt_count(self.output),
                fmt_count(self.cache_read)
            )
        } else {
            format!(
                "[in:{} out:{}]",
                fmt_count(self.input),
                fmt_count(self.output)
            )
        }
    }
}

/// A single entry in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEntry {
    pub role: ConversationRole,
    pub content: String,
    pub collapsed: bool,
    /// Token usage for this message (populated from API response for assistant messages).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,
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
    /// Active/running tool (⟳ spinner color)
    ToolRunning,
    /// Queued/pending tool (○ dimmed)
    ToolPending,
    /// Completed tool (✓ green)
    ToolSuccess,
    /// Failed tool (✗ red)
    ToolError,
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
    /// Dirty flag for screen counts — set on content mutation,
    /// cleared after recomputation. Prevents O(N) char-width ops
    /// per frame when content is unchanged.
    screen_counts_dirty: bool,
    /// Maps entry index → first cached_line index, enabling incremental
    /// re-render of only the last entry during streaming.
    entry_start_indices: Vec<usize>,
    /// Rendered links from markdown stripping — populated during render cache rebuild.
    rendered_links: Vec<crate::link_detect::RenderedLink>,
    /// Whether to strip markdown link markup (show labels only).
    pub link_descriptive: bool,
    /// Whether to render inline bold/italic/code spans.
    pub render_markup: bool,
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
            scroll: 0,
            streaming: false,
            streaming_start: None,
            version: 0,
            cached_lines: Vec::new(),
            cached_screen_counts: Vec::new(),
            cached_screen_width: 0,
            screen_counts_dirty: true,
            entry_start_indices: Vec::new(),
            rendered_links: Vec::new(),
            link_descriptive: true,
            render_markup: true,
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
            token_usage: None,
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
            token_usage: None,
        });
        self.version += 1;
        self.trim_entries();
        self.rebuild_render_cache();
    }

    pub fn push_tool_call(&mut self, name: impl Into<String>) {
        self.push_tool_call_with_state(name, ToolCallState::Running);
    }

    /// Push a tool call entry with an explicit state (Pending, Running, etc.)
    pub fn push_tool_call_with_state(&mut self, name: impl Into<String>, state: ToolCallState) {
        self.entries.push(ConversationEntry {
            role: ConversationRole::ToolCall {
                name: name.into(),
                state,
                elapsed_ms: None,
            },
            content: String::new(),
            collapsed: true,
            token_usage: None,
        });
        self.version += 1;
        self.trim_entries();
        self.rebuild_render_cache();
    }

    /// Update the last ToolCall entry matching `name` to a new state,
    /// or push a new entry if none found. Used to transition Pending → Running
    /// without creating a duplicate entry.
    pub fn update_or_push_tool_call(&mut self, name: &str, state: ToolCallState) {
        // Look for the last Pending/Running ToolCall with this name
        for entry in self.entries.iter_mut().rev() {
            if let ConversationRole::ToolCall {
                name: ref entry_name,
                state: ref mut entry_state,
                ..
            } = entry.role
            {
                if entry_name == name
                    && matches!(entry_state, ToolCallState::Pending | ToolCallState::Running)
                {
                    *entry_state = state;
                    self.version += 1;
                    self.rebuild_render_cache();
                    return;
                }
            }
        }
        // No matching entry — create one
        self.push_tool_call_with_state(name, state);
    }

    /// Complete the last ToolCall entry: set state to Completed/Error,
    /// store the output and elapsed time directly on the ToolCall.
    /// This merges the ToolResult into the ToolCall for compact display.
    pub fn complete_last_tool_call(
        &mut self,
        success: bool,
        output: &str,
        elapsed_ms: Option<u64>,
    ) {
        // Find the last ToolCall entry (scan backwards)
        for entry in self.entries.iter_mut().rev() {
            if let ConversationRole::ToolCall {
                ref mut state,
                elapsed_ms: ref mut ems,
                ..
            } = entry.role
            {
                *state = if success {
                    ToolCallState::Completed
                } else {
                    ToolCallState::Error
                };
                *ems = elapsed_ms;
                entry.content = output.to_string();
                break;
            }
        }
        self.version += 1;
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
            token_usage: None,
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
            token_usage: None,
        });
        self.version += 1;
        self.trim_entries();
        self.rebuild_render_cache();
    }

    /// Append a streaming chunk to the last assistant entry.
    /// If the last entry isn't an assistant entry, creates one.
    ///
    /// Uses incremental cache rebuild: only re-renders the last entry
    /// instead of all entries. Turns O(all_entries) per token into
    /// O(last_entry_lines).
    pub fn append_streaming_chunk(&mut self, text: &str) {
        if let Some(last) = self.entries.last_mut() {
            if last.role == ConversationRole::Assistant {
                last.content.push_str(text);
                self.version += 1;
                self.incremental_rebuild_last_entry();
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
    /// Current wire format version.
    const WIRE_VERSION: u32 = 2;

    pub fn to_json(&self) -> Result<String, String> {
        #[derive(Serialize)]
        struct Wire<'a> {
            version: u32,
            entries: &'a [ConversationEntry],
        }
        serde_json::to_string_pretty(&Wire {
            version: Self::WIRE_VERSION,
            entries: &self.entries,
        })
        .map_err(|e| e.to_string())
    }

    /// Replace entries with those loaded from JSON. Supports v1 (legacy)
    /// and v2 (ToolCallState) formats.
    pub fn load_json(&mut self, json: &str) -> Result<(), String> {
        #[derive(Deserialize)]
        struct Wire {
            version: u32,
            entries: Vec<ConversationEntry>,
        }
        let wire: Wire = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if wire.version != 1 && wire.version != 2 {
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
    pub fn rebuild_render_cache(&mut self) {
        self.rendered_links.clear();
        self.cached_lines = self.compute_rendered_lines();
        self.cached_screen_counts.clear();
        self.screen_counts_dirty = true;
        self.rebuild_entry_start_indices();
    }

    /// Return pre-computed rendered lines (zero-allocation on read).
    pub fn rendered_lines(&self) -> &[RenderedLine] {
        &self.cached_lines
    }

    /// Ensure per-line screen counts are computed for the given width.
    /// Returns `true` if counts were recomputed (content or width changed).
    /// Amortized O(1) when nothing changed — the dirty flag prevents
    /// ~30K char-width ops/sec at idle.
    pub fn ensure_screen_counts(&mut self, width: usize) -> bool {
        let w = width.max(1);
        if !self.screen_counts_dirty
            && self.cached_screen_width == w
            && self.cached_screen_counts.len() == self.cached_lines.len()
        {
            return false;
        }
        self.cached_screen_counts = self
            .cached_lines
            .iter()
            .map(|rl| screen_line_count(&rl.text, w))
            .collect();
        self.cached_screen_width = w;
        self.screen_counts_dirty = false;
        true
    }

    /// Return pre-computed screen counts and total.
    /// Must call `ensure_screen_counts` first.
    pub fn screen_counts_total(&self) -> (&[usize], usize) {
        let total: usize = self.cached_screen_counts.iter().sum();
        (&self.cached_screen_counts, total)
    }

    /// Backwards-compatible alias for `screen_counts_total`.
    pub fn screen_counts(&self) -> (&[usize], usize) {
        self.screen_counts_total()
    }

    /// Width used for the cached screen counts. Returns 0 if not yet computed.
    pub fn cached_screen_width(&self) -> usize {
        self.cached_screen_width
    }

    /// Rebuild the entry_start_indices map from cached_lines.
    fn rebuild_entry_start_indices(&mut self) {
        self.entry_start_indices.clear();
        let mut current_entry: Option<usize> = None;
        for (line_idx, rl) in self.cached_lines.iter().enumerate() {
            if let Some(ei) = rl.entry_index {
                if current_entry != Some(ei) {
                    // Ensure vec is large enough
                    while self.entry_start_indices.len() <= ei {
                        self.entry_start_indices.push(line_idx);
                    }
                    current_entry = Some(ei);
                }
            }
        }
    }

    /// Incrementally rebuild only the last entry's rendered lines + input prompt.
    /// O(last_entry_lines) instead of O(all_entries).
    fn incremental_rebuild_last_entry(&mut self) {
        if self.entries.is_empty() || self.entry_start_indices.is_empty() {
            self.rebuild_render_cache();
            return;
        }
        let last_entry_idx = self.entries.len() - 1;
        let start_line = if last_entry_idx < self.entry_start_indices.len() {
            self.entry_start_indices[last_entry_idx]
        } else {
            // Fallback: full rebuild
            self.rebuild_render_cache();
            return;
        };

        // Truncate cached_lines from the last entry's start
        self.cached_lines.truncate(start_line);

        // If previous entry was a tool entry and this is too, remove the
        // separator between them (it sits right before start_line)
        let this_is_tool = matches!(
            self.entries[last_entry_idx].role,
            ConversationRole::ToolCall { .. } | ConversationRole::ToolResult { .. }
        );
        let prev_is_tool = last_entry_idx > 0
            && matches!(
                self.entries[last_entry_idx - 1].role,
                ConversationRole::ToolCall { .. } | ConversationRole::ToolResult { .. }
            );
        if this_is_tool && prev_is_tool && !self.cached_lines.is_empty() {
            if let Some(last) = self.cached_lines.last() {
                if matches!(last.style, LineStyle::Separator) {
                    self.cached_lines.pop();
                }
            }
        }

        // Skip rendering ToolResult if previous ToolCall already has result merged
        let skip_entry = matches!(
            self.entries[last_entry_idx].role,
            ConversationRole::ToolResult { .. }
        ) && last_entry_idx > 0
            && matches!(
                self.entries[last_entry_idx - 1].role,
                ConversationRole::ToolCall { state, .. }
                    if matches!(state, ToolCallState::Completed | ToolCallState::Error)
            );

        if !skip_entry {
            // Re-render just the last entry into a temp buffer, then extend
            let mut new_lines = Vec::new();
            let entry = self.entries[last_entry_idx].clone();
            self.render_entry_into(&mut new_lines, last_entry_idx, &entry);
            self.cached_lines.extend(new_lines);
        }

        // Only add trailing separator if this is NOT the last entry
        // (matches compute_rendered_lines which skips separator after last).
        if last_entry_idx + 1 < self.entries.len() {
            self.cached_lines.push(RenderedLine {
                text: String::new(),
                style: LineStyle::Separator,
                entry_index: None,
            });
        }

        self.cached_screen_counts.clear();
        self.screen_counts_dirty = true;
        // Update start indices for this entry
        while self.entry_start_indices.len() <= last_entry_idx {
            self.entry_start_indices.push(start_line);
        }
    }

    /// Render a single entry into the provided line buffer.
    fn render_entry_into(
        &mut self,
        lines: &mut Vec<RenderedLine>,
        i: usize,
        entry: &ConversationEntry,
    ) {
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
                let marker = if let Some(ref usage) = entry.token_usage {
                    format!("[AI] {}", usage.display_compact())
                } else {
                    "[AI]".into()
                };
                lines.push(RenderedLine {
                    text: marker,
                    style: LineStyle::RoleMarker,
                    entry_index: Some(i),
                });
                for line in entry.content.lines() {
                    let text = if self.link_descriptive {
                        let (clean, link_positions) =
                            crate::link_detect::strip_markdown_links(line);
                        let line_idx = lines.len();
                        for (start, end, target) in link_positions {
                            self.rendered_links.push(crate::link_detect::RenderedLink {
                                line_idx,
                                byte_start: start,
                                byte_end: end,
                                target,
                                kind: crate::link_detect::LinkKind::Markdown,
                            });
                        }
                        clean
                    } else {
                        line.to_string()
                    };
                    lines.push(RenderedLine {
                        text,
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
            ConversationRole::ToolCall {
                ref name,
                state,
                elapsed_ms,
            } => {
                let (icon, style) = match state {
                    ToolCallState::Pending => ("\u{25cb}", LineStyle::ToolPending), // ○
                    ToolCallState::Running => ("\u{27f3}", LineStyle::ToolRunning), // ⟳
                    ToolCallState::Completed => ("\u{2713}", LineStyle::ToolSuccess), // ✓
                    ToolCallState::Error => ("\u{2717}", LineStyle::ToolError),     // ✗
                };
                let timing = match elapsed_ms {
                    Some(ms) => format!("  {}ms", ms),
                    None if *state == ToolCallState::Running => "  ...".to_string(),
                    _ => String::new(),
                };
                if entry.collapsed {
                    lines.push(RenderedLine {
                        text: format!("{} {:<30}{}", icon, name, timing),
                        style,
                        entry_index: Some(i),
                    });
                } else {
                    lines.push(RenderedLine {
                        text: format!("{} {}{}", icon, name, timing),
                        style,
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
                let marker = if *success { "\u{2713}" } else { "\u{2717}" };
                let header = match elapsed_ms {
                    Some(ms) => format!("  [{} {}ms]", marker, ms),
                    None => format!("  [{}]", marker),
                };
                lines.push(RenderedLine {
                    text: header,
                    style: LineStyle::ToolResultText,
                    entry_index: Some(i),
                });
                if !entry.collapsed {
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
    }

    /// Render all entries + input line into display lines.
    fn compute_rendered_lines(&mut self) -> Vec<RenderedLine> {
        let mut lines = Vec::new();
        let entry_count = self.entries.len();

        for i in 0..entry_count {
            // Skip ToolResult if the previous entry is a ToolCall that already
            // has the result merged (Completed/Error state with content).
            if matches!(self.entries[i].role, ConversationRole::ToolResult { .. }) && i > 0 {
                if let ConversationRole::ToolCall { state, .. } = &self.entries[i - 1].role {
                    if matches!(state, ToolCallState::Completed | ToolCallState::Error) {
                        continue;
                    }
                }
            }

            let entry = self.entries[i].clone();
            self.render_entry_into(&mut lines, i, &entry);

            // Skip separator between consecutive tool entries for compact display
            let next_is_tool = self
                .entries
                .get(i + 1)
                .map(|e| {
                    matches!(
                        e.role,
                        ConversationRole::ToolCall { .. } | ConversationRole::ToolResult { .. }
                    )
                })
                .unwrap_or(false);
            let this_is_tool = matches!(
                self.entries[i].role,
                ConversationRole::ToolCall { .. } | ConversationRole::ToolResult { .. }
            );
            if this_is_tool && next_is_tool {
                continue; // no separator between stacked tool lines
            }

            // Separator between entries (skip after last entry to avoid phantom line)
            if i + 1 < self.entries.len() {
                lines.push(RenderedLine {
                    text: String::new(),
                    style: LineStyle::Separator,
                    entry_index: None,
                });
            }
        }

        // (Input prompt is now rendered separately in the *ai-input* split buffer.)

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
    /// Uses screen-line total if available, falls back to rendered-line count.
    pub fn scroll_to_top(&mut self) {
        let total: usize = if self.cached_screen_counts.is_empty() {
            self.cached_lines.len()
        } else {
            self.cached_screen_counts.iter().sum()
        };
        self.scroll = total;
    }

    /// Generate `HighlightSpan`s for the synced conversation rope.
    ///
    /// Each rendered line maps 1:1 to a rope line (via `sync_conversation_rope`).
    /// The span covers the full line and assigns a theme key based on `LineStyle`.
    pub fn highlight_spans(&self, rope: &ropey::Rope) -> Vec<crate::syntax::HighlightSpan> {
        let rendered = self.rendered_lines();
        let mut spans = Vec::with_capacity(rendered.len());
        for (line_idx, rl) in rendered.iter().enumerate() {
            if line_idx >= rope.len_lines() {
                break;
            }
            let theme_key: &'static str = match &rl.style {
                LineStyle::RoleMarker => {
                    if rl.text.contains("[You]") {
                        "conversation.user"
                    } else if rl.text.contains("[AI]") {
                        "conversation.assistant"
                    } else {
                        "conversation.system"
                    }
                }
                LineStyle::UserText => "conversation.user.text",
                LineStyle::AssistantText => "conversation.assistant.text",
                LineStyle::ToolCallHeader => "conversation.tool",
                LineStyle::ToolResultText => "conversation.tool.result",
                LineStyle::ToolRunning => "conversation.tool",
                LineStyle::ToolPending => "ui.text.dim",
                LineStyle::ToolSuccess => "conversation.tool.result",
                LineStyle::ToolError => "diagnostic.error",
                LineStyle::SystemText => "conversation.system",
                LineStyle::Separator => "ui.text",
                LineStyle::InputPrompt => "conversation.input",
            };
            let line_start = rope.line_to_char(line_idx);
            let line_len = rope.line(line_idx).len_chars();
            // Strip trailing newline for byte range.
            let text_len = if line_idx + 1 < rope.len_lines() {
                line_len.saturating_sub(1)
            } else {
                line_len
            };
            if text_len == 0 {
                continue;
            }
            let byte_start = rope.char_to_byte(line_start);
            let byte_end = rope.char_to_byte(line_start + text_len);
            spans.push(crate::syntax::HighlightSpan {
                byte_start,
                byte_end,
                theme_key,
            });
        }
        spans
    }

    /// Find a rendered link at a given cursor position (line_idx, byte_col).
    pub fn link_at_position(
        &self,
        line_idx: usize,
        byte_col: usize,
    ) -> Option<&crate::link_detect::RenderedLink> {
        self.rendered_links
            .iter()
            .find(|l| l.line_idx == line_idx && byte_col >= l.byte_start && byte_col < l.byte_end)
    }

    /// Access the rendered links list.
    pub fn rendered_links(&self) -> &[crate::link_detect::RenderedLink] {
        &self.rendered_links
    }

    /// Generate highlight spans with inline markdown styling for assistant text.
    ///
    /// Extends the base `highlight_spans()` with `markup.bold`, `markup.literal`,
    /// and `markup.italic` spans for assistant messages. Intentionally excludes
    /// `markup.heading` — adding heading spans would trigger `line_heading_scale()`
    /// in `compute_layout()`, breaking uniform conversation line heights.
    pub fn highlight_spans_with_markup(
        &self,
        rope: &ropey::Rope,
    ) -> Vec<crate::syntax::HighlightSpan> {
        let mut spans = self.highlight_spans(rope);
        let rendered = self.rendered_lines();
        for (line_idx, rl) in rendered.iter().enumerate() {
            if line_idx >= rope.len_lines() {
                break;
            }
            if !matches!(rl.style, LineStyle::AssistantText) {
                continue;
            }
            let line_start_byte = rope.char_to_byte(rope.line_to_char(line_idx));
            let line_text: String = rope.line(line_idx).chars().collect();

            if self.render_markup {
                // Markdown inline spans: **bold**, `code`, *italic*
                for mut span in crate::syntax::compute_markdown_style_spans(&line_text) {
                    span.byte_start += line_start_byte;
                    span.byte_end += line_start_byte;
                    spans.push(span);
                }
                // Org inline spans: *bold*, /italic/, =code=, ~verbatim~
                for mut span in crate::syntax::compute_org_style_spans(&line_text) {
                    span.byte_start += line_start_byte;
                    span.byte_end += line_start_byte;
                    spans.push(span);
                }
            }

            // Link spans for concealed markdown links
            for link in &self.rendered_links {
                if link.line_idx == line_idx {
                    spans.push(crate::syntax::HighlightSpan {
                        byte_start: line_start_byte + link.byte_start,
                        byte_end: line_start_byte + link.byte_end,
                        theme_key: "markup.link",
                    });
                }
            }
        }
        spans.sort_by_key(|s| s.byte_start);
        spans
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_conversation_is_empty() {
        let conv = Conversation::new();
        assert!(conv.entries.is_empty());
        assert_eq!(conv.scroll, 0);
        assert!(!conv.streaming);
        assert_eq!(conv.version(), 0);
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
    fn no_trailing_separator_after_last_entry() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        conv.push_assistant("world");
        let lines = conv.rendered_lines();
        assert!(!lines.is_empty());
        let last = lines.last().unwrap();
        // Last line should be content, not an empty separator
        assert!(
            !matches!(last.style, LineStyle::Separator) || !last.text.is_empty(),
            "trailing empty separator found after last entry"
        );
        // flat_text should not end with a newline (which would cause phantom line)
        let flat = conv.flat_text();
        assert!(
            !flat.ends_with('\n'),
            "flat_text ends with trailing newline"
        );
    }

    #[test]
    fn separator_between_non_last_entries() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        conv.push_assistant("world");
        conv.push_user("again");
        let lines = conv.rendered_lines();
        // Should have separators between entries but not after the last
        let separator_count = lines
            .iter()
            .filter(|l| matches!(l.style, LineStyle::Separator) && l.text.is_empty())
            .count();
        assert_eq!(
            separator_count, 2,
            "expected separator between each pair of entries"
        );
    }

    #[test]
    fn flat_text_empty_conversation() {
        let conv = Conversation::new();
        let flat = conv.flat_text();
        // Empty conversation: no entries, no input prompt (input is in separate buffer)
        assert!(flat.is_empty());
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
        // Tool call should be collapsed with state icon (⟳ for Running)
        assert!(lines.iter().any(|l| l.text.contains("buffer_read")));
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
    fn line_count_no_input_prompt() {
        let conv = Conversation::new();
        // Empty conversation: no entries, no input prompt
        assert_eq!(conv.line_count(), 0);

        let mut conv = Conversation::new();
        conv.push_user("hello");
        // [You] + "hello" (no trailing separator after last entry)
        assert!(conv.line_count() >= 2);
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
            ConversationRole::ToolCall { ref name, .. } if name == "buffer_read"
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

    // ---- char_boundary_at tests (migrated from GUI) ----

    #[test]
    fn char_boundary_at_basic() {
        assert_eq!(char_boundary_at("hello", 3), 3);
        assert_eq!(char_boundary_at("hello", 10), 5);
        assert_eq!(char_boundary_at("", 5), 0);
    }

    #[test]
    fn char_boundary_at_multibyte() {
        let s = "héllo"; // é is 2 bytes
        let boundary = char_boundary_at(s, 2);
        assert!(s.is_char_boundary(boundary));
    }

    #[test]
    fn char_boundary_at_cjk() {
        let s = "日本語テスト"; // 6 chars, 12 display columns
        let boundary = char_boundary_at(s, 4); // 4 display cols = 2 CJK chars
        assert_eq!(boundary, 6); // 2 chars × 3 bytes
        assert!(s.is_char_boundary(boundary));
    }

    // ---- wrap_text_into_rows tests (migrated from GUI) ----

    #[test]
    fn wrap_text_into_rows_basic() {
        let text = "a".repeat(20);
        let rows = wrap_text_into_rows(&text, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 10);
        assert_eq!(rows[1].len(), 10);
    }

    #[test]
    fn wrap_text_into_rows_exact() {
        let text = "a".repeat(10);
        let rows = wrap_text_into_rows(&text, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 10);
    }

    #[test]
    fn wrap_text_into_rows_short() {
        let rows = wrap_text_into_rows("hello", 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], "hello");
    }

    #[test]
    fn wrap_text_into_rows_empty() {
        let rows = wrap_text_into_rows("", 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], "");
    }

    // ---- screen_line_count regression tests ----

    #[test]
    fn screen_line_count_basic_regression() {
        assert_eq!(screen_line_count("hello", 80), 1);
        assert_eq!(screen_line_count("", 80), 1);
        assert_eq!(screen_line_count(&"a".repeat(20), 10), 2);
        assert_eq!(screen_line_count(&"a".repeat(30), 10), 3);
    }

    #[test]
    fn screen_line_count_cjk() {
        // 6 CJK chars = 12 display columns
        assert_eq!(screen_line_count("日本語テスト", 12), 1);
        assert_eq!(screen_line_count("日本語テスト", 6), 2);
        assert_eq!(screen_line_count("日本語テスト", 4), 3);
    }

    #[test]
    fn screen_line_count_edge_cases() {
        assert_eq!(screen_line_count("", 1), 1);
        assert_eq!(screen_line_count("a", 1), 1);
        assert_eq!(screen_line_count("ab", 1), 2);
        // width=0 is clamped to 1
        assert_eq!(screen_line_count("abc", 0), 3);
        // Mixed ASCII + CJK: "hi日" = 2 + 2 = 4 display cols
        assert_eq!(screen_line_count("hi日", 4), 1);
        assert_eq!(screen_line_count("hi日", 3), 2);
    }

    #[test]
    fn ensure_screen_counts_cache_invalidation() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        conv.ensure_screen_counts(80);
        let (counts, _total) = conv.screen_counts_total();
        let count_len_1 = counts.len();
        assert!(count_len_1 > 0);
        let ver1 = conv.version();

        conv.push_assistant("world");
        assert!(conv.version() > ver1);
        // After mutation, counts must be recomputed
        conv.ensure_screen_counts(80);
        let (counts2, _) = conv.screen_counts_total();
        assert!(counts2.len() > count_len_1);
    }

    #[test]
    fn ensure_screen_counts_width_change() {
        let mut conv = Conversation::new();
        conv.push_assistant("a".repeat(20));
        conv.ensure_screen_counts(80);
        let (_, total80) = conv.screen_counts_total();
        conv.ensure_screen_counts(10);
        let (_, total10) = conv.screen_counts_total();
        // At width 10, the 20-char line wraps to 2 screen lines
        assert!(total10 > total80);
    }

    #[test]
    fn screen_counts_cover_all_rendered_lines() {
        let mut conv = Conversation::new();
        conv.push_user("test");
        conv.push_assistant("response");
        conv.ensure_screen_counts(80);

        let rendered = conv.rendered_lines();
        let (counts, total) = conv.screen_counts_total();
        assert_eq!(counts.len(), rendered.len());
        assert!(total > 0);
    }

    /// Regression: cached_screen_width must reflect the width used for
    /// screen count computation, so renderers can detect mismatches.
    #[test]
    fn cached_screen_width_tracks_computation() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        assert_eq!(conv.cached_screen_width(), 0); // not yet computed
        conv.ensure_screen_counts(42);
        assert_eq!(conv.cached_screen_width(), 42);
        conv.ensure_screen_counts(80);
        assert_eq!(conv.cached_screen_width(), 80);
    }

    // ---- dirty flag + incremental cache tests ----

    #[test]
    fn ensure_screen_counts_dirty_flag_skips_recompute() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        // First call: recomputes
        assert!(conv.ensure_screen_counts(80));
        // Second call at same width: no-op
        assert!(!conv.ensure_screen_counts(80));
        // Width change: recomputes
        assert!(conv.ensure_screen_counts(40));
        // Same width again: no-op
        assert!(!conv.ensure_screen_counts(40));
        // Content change: marks dirty, recomputes
        conv.push_assistant("world");
        assert!(conv.ensure_screen_counts(40));
        assert!(!conv.ensure_screen_counts(40));
    }

    #[test]
    fn incremental_streaming_cache() {
        let mut conv = Conversation::new();
        conv.push_user("question");
        conv.push_assistant("start");
        let lines_before = conv.rendered_lines().len();

        // Streaming append should incrementally update
        conv.append_streaming_chunk(" more text");
        let lines_after = conv.rendered_lines().len();
        // Same number of lines (single-line content)
        assert_eq!(lines_before, lines_after);
        // Content should be merged
        assert_eq!(conv.entries[1].content, "start more text");
        // Should have AI text in rendered output
        assert!(conv
            .rendered_lines()
            .iter()
            .any(|l| l.text.contains("start more text")));
    }

    #[test]
    fn incremental_streaming_multi_line() {
        let mut conv = Conversation::new();
        conv.push_assistant("line1");
        conv.append_streaming_chunk("\nline2");
        conv.append_streaming_chunk("\nline3");
        assert_eq!(conv.entries.len(), 1);
        assert_eq!(conv.entries[0].content, "line1\nline2\nline3");
        // Should have 3 content lines + role marker + separator + input prompt
        let assistant_lines: Vec<_> = conv
            .rendered_lines()
            .iter()
            .filter(|l| l.style == LineStyle::AssistantText)
            .collect();
        assert_eq!(assistant_lines.len(), 3);
    }

    // ---- chars_to_display_cols tests ----

    #[test]
    fn chars_to_cols_ascii() {
        assert_eq!(chars_to_display_cols("hello", 3), 3);
    }

    #[test]
    fn chars_to_cols_cjk() {
        assert_eq!(chars_to_display_cols("日本語", 2), 4);
    }

    #[test]
    fn chars_to_cols_mixed() {
        // "hi日本" — 2 ASCII (2 cols) + 1 CJK (2 cols) = 4 cols for 3 chars
        assert_eq!(chars_to_display_cols("hi日本", 3), 4);
    }

    #[test]
    fn token_usage_display_compact() {
        let usage = TokenUsage {
            input: 1200,
            output: 340,
            cache_read: 0,
        };
        assert_eq!(usage.display_compact(), "[in:1.2k out:340]");
    }

    #[test]
    fn token_usage_display_with_cache() {
        let usage = TokenUsage {
            input: 500,
            output: 200,
            cache_read: 8000,
        };
        assert_eq!(usage.display_compact(), "[in:500 out:200 cache:8.0k]");
    }

    #[test]
    fn token_usage_serde_roundtrip() {
        let usage = TokenUsage {
            input: 100,
            output: 50,
            cache_read: 0,
        };
        let json = serde_json::to_string(&usage).unwrap();
        let parsed: TokenUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.input, 100);
        assert_eq!(parsed.output, 50);
    }

    #[test]
    fn conversation_entry_with_token_usage_renders() {
        let mut conv = Conversation::new();
        conv.push_assistant("Hello");
        // Set token usage on last entry.
        conv.entries.last_mut().unwrap().token_usage = Some(TokenUsage {
            input: 1500,
            output: 300,
            cache_read: 0,
        });
        conv.rebuild_render_cache();
        let lines = conv.rendered_lines();
        let marker = lines.iter().find(|l| l.style == LineStyle::RoleMarker);
        assert!(marker.is_some());
        assert!(
            marker.unwrap().text.contains("[in:1.5k out:300]"),
            "role marker should contain token display: {}",
            marker.unwrap().text
        );
    }

    #[test]
    fn highlight_spans_covers_all_lines() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        conv.push_assistant("world");
        let rope = ropey::Rope::from_str(&conv.flat_text());
        let spans = conv.highlight_spans(&rope);
        // Every non-empty rendered line should have a span.
        let non_empty = conv
            .rendered_lines()
            .iter()
            .filter(|l| !l.text.is_empty())
            .count();
        assert_eq!(spans.len(), non_empty);
    }

    #[test]
    fn highlight_spans_role_markers() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        conv.push_assistant("world");
        let rope = ropey::Rope::from_str(&conv.flat_text());
        let spans = conv.highlight_spans(&rope);
        // First span should be [You] role marker → conversation.user
        assert_eq!(spans[0].theme_key, "conversation.user");
        // Find the [AI] marker span
        let ai_span = spans
            .iter()
            .find(|s| s.theme_key == "conversation.assistant");
        assert!(ai_span.is_some());
    }

    #[test]
    fn highlight_spans_with_markup_adds_inline_styles() {
        let mut conv = Conversation::new();
        conv.push_user("hello");
        conv.push_assistant("This is **bold** and `code`");

        let rope = ropey::Rope::from_str(&conv.flat_text());
        let spans = conv.highlight_spans_with_markup(&rope);

        // Should contain base conversation spans
        assert!(spans
            .iter()
            .any(|s| s.theme_key == "conversation.assistant.text"));
        // Should also contain inline markup spans
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.bold"),
            "expected markup.bold from **bold**"
        );
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.literal"),
            "expected markup.literal from `code`"
        );
        // Must NOT contain heading spans
        assert!(
            !spans.iter().any(|s| s.theme_key == "markup.heading"),
            "highlight_spans_with_markup must not produce heading spans"
        );
    }

    // --- Rendered link tests ---

    #[test]
    fn rendered_lines_strip_markdown_links() {
        let mut conv = Conversation::new();
        conv.link_descriptive = true;
        conv.push_assistant("See [docs](https://docs.rs) for info");
        let lines = conv.rendered_lines();
        let text_line = lines.iter().find(|l| l.style == LineStyle::AssistantText);
        assert!(text_line.is_some());
        assert_eq!(text_line.unwrap().text, "See docs for info");
    }

    #[test]
    fn link_at_position_finds_link() {
        let mut conv = Conversation::new();
        conv.link_descriptive = true;
        conv.push_assistant("See [docs](https://docs.rs) for info");
        // The rendered text is "See docs for info"
        // "docs" starts at byte 4, ends at byte 8
        // Line index: 0=[AI], 1="See docs for info"
        let link = conv.link_at_position(1, 4);
        assert!(link.is_some(), "expected link at position (1, 4)");
        assert_eq!(link.unwrap().target, "https://docs.rs");
        // No link at position 0 (before the label)
        assert!(conv.link_at_position(1, 0).is_none());
    }

    #[test]
    fn highlight_spans_include_link_style() {
        let mut conv = Conversation::new();
        conv.link_descriptive = true;
        conv.push_assistant("See [docs](https://docs.rs) here");
        let rope = ropey::Rope::from_str(&conv.flat_text());
        let spans = conv.highlight_spans_with_markup(&rope);
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.link"),
            "expected markup.link span for concealed link"
        );
    }

    #[test]
    fn link_descriptive_disabled_shows_raw() {
        let mut conv = Conversation::new();
        conv.link_descriptive = false;
        conv.push_assistant("See [docs](https://docs.rs) for info");
        let lines = conv.rendered_lines();
        let text_line = lines.iter().find(|l| l.style == LineStyle::AssistantText);
        assert!(text_line.is_some());
        assert_eq!(
            text_line.unwrap().text,
            "See [docs](https://docs.rs) for info"
        );
        assert!(conv.rendered_links().is_empty());
    }

    #[test]
    fn render_markup_disabled_no_markup_spans() {
        let mut conv = Conversation::new();
        conv.render_markup = false;
        conv.push_assistant("This is **bold** and `code`");
        let rope = ropey::Rope::from_str(&conv.flat_text());
        let spans = conv.highlight_spans_with_markup(&rope);
        // Should NOT have markup.bold or markup.literal since render_markup is off
        assert!(
            !spans.iter().any(|s| s.theme_key == "markup.bold"),
            "expected no markup.bold when render_markup=false"
        );
        assert!(
            !spans.iter().any(|s| s.theme_key == "markup.literal"),
            "expected no markup.literal when render_markup=false"
        );
    }
}
