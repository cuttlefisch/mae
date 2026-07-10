//! Ratatui TUI for `mae-agent` (ADR-046) — the "proven UX patterns" surface:
//! a scrollable transcript with collapsed-by-default tool-call blocks, a fixed
//! bottom input box, and an inline confirm prompt for permission-gated tool
//! calls. No existing MAE code is reusable here (`MiniDialogKind::Confirm` is
//! GUI/render-system-coupled) — built fresh, modeled stylistically on Claude
//! Code/Gemini CLI's shape.
//!
//! Streaming is explicitly deferred (per the session's own decision) — a
//! `busy` spinner covers the wait between submitting input and the full-turn
//! response arriving.

mod confirm;
mod input;
mod statusline;
mod transcript;

pub use confirm::{
    needs_confirmation, parse_confirm_key, ConfirmChoice, PendingConfirm, PermissionMode,
};
pub use transcript::TranscriptEntry;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::Frame;

/// All mutable UI state. Rendering functions take `&AppState`; only the
/// key-handling / agent-event-handling methods here mutate it — keeping state
/// transitions unit-testable independent of any real terminal.
pub struct AppState {
    pub transcript: Vec<TranscriptEntry>,
    pub input: String,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub permission_mode: PermissionMode,
    pub pending_confirm: Option<PendingConfirm>,
    pub model: String,
    pub provider: String,
    pub round: usize,
    pub busy: bool,
    pub should_quit: bool,
    pub input_history: Vec<String>,
    pub history_cursor: Option<usize>,
    /// Set by the key-handling loop when Enter submits a non-empty line;
    /// drained by the event loop, which decides whether it's a slash command
    /// or a real user turn to hand to the agent.
    pub pending_submit: Option<String>,
    /// Compact one-line summary of the most recent round's
    /// `AgentEvent::RoundDiagnostics` (tools offered, stop reason, tokens) --
    /// shown in the status line rather than the transcript so it's always
    /// available for troubleshooting without cluttering the conversation.
    pub last_diagnostics: Option<String>,
}

impl AppState {
    pub fn new(model: String, provider: String, permission_mode: PermissionMode) -> Self {
        Self {
            transcript: Vec::new(),
            input: String::new(),
            cursor: 0,
            scroll_offset: 0,
            permission_mode,
            pending_confirm: None,
            model,
            provider,
            round: 0,
            busy: false,
            should_quit: false,
            input_history: Vec::new(),
            history_cursor: None,
            pending_submit: None,
            last_diagnostics: None,
        }
    }

    pub fn set_diagnostics(&mut self, summary: String) {
        self.last_diagnostics = Some(summary);
    }

    pub fn push_user(&mut self, text: String) {
        self.transcript.push(TranscriptEntry::User(text));
    }

    pub fn push_assistant(&mut self, text: String) {
        self.transcript.push(TranscriptEntry::Assistant(text));
    }

    pub fn push_system_note(&mut self, text: String) {
        self.transcript.push(TranscriptEntry::SystemNote(text));
    }

    pub fn push_tool_call_started(&mut self, name: String, arguments: serde_json::Value) {
        self.transcript.push(TranscriptEntry::ToolCall {
            name,
            arguments,
            result: None,
            expanded: false,
        });
    }

    /// Attach a result to the most recent still-pending tool call matching
    /// `name`. If none is found (shouldn't happen in practice), the result is
    /// silently dropped rather than panicking.
    pub fn complete_tool_call(&mut self, name: &str, success: bool, output: String) {
        if let Some(TranscriptEntry::ToolCall { result, .. }) = self
            .transcript
            .iter_mut()
            .rev()
            .find(|e| matches!(e, TranscriptEntry::ToolCall { name: n, result: None, .. } if n == name))
        {
            *result = Some((success, output));
        }
    }

    /// Toggle expand/collapse on the transcript entry at `index`, if it's a
    /// tool call. No-op otherwise.
    pub fn toggle_tool_call_expanded(&mut self, index: usize) {
        if let Some(TranscriptEntry::ToolCall { expanded, .. }) = self.transcript.get_mut(index) {
            *expanded = !*expanded;
        }
    }

    /// Submit the current input line: returns it (trimmed) and clears the
    /// input box, recording it in history. Returns `None` for an all-
    /// whitespace line (nothing to submit).
    pub fn submit_input(&mut self) -> Option<String> {
        let trimmed = self.input.trim().to_string();
        self.input.clear();
        self.cursor = 0;
        self.history_cursor = None;
        if trimmed.is_empty() {
            return None;
        }
        self.input_history.push(trimmed.clone());
        Some(trimmed)
    }

    /// Recall the previous history entry (up-arrow), if any.
    pub fn recall_history_prev(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let next_idx = match self.history_cursor {
            None => self.input_history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_cursor = Some(next_idx);
        self.input = self.input_history[next_idx].clone();
        self.cursor = self.input.len();
    }

    /// Recall the next history entry (down-arrow), clearing the input once
    /// past the newest entry.
    pub fn recall_history_next(&mut self) {
        match self.history_cursor {
            None => {}
            Some(i) if i + 1 < self.input_history.len() => {
                self.history_cursor = Some(i + 1);
                self.input = self.input_history[i + 1].clone();
                self.cursor = self.input.len();
            }
            Some(_) => {
                self.history_cursor = None;
                self.input.clear();
                self.cursor = 0;
            }
        }
    }
}

/// Parsed result of a slash command (`/help`, `/clear`, etc.) or `None` if
/// `line` isn't a slash command at all (an ordinary chat message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Help,
    Clear,
    Model,
    Permissions,
    Quit,
    Unknown(String),
}

pub fn parse_slash_command(line: &str) -> Option<SlashCommand> {
    let line = line.trim();
    let rest = line.strip_prefix('/')?;
    Some(match rest {
        "help" => SlashCommand::Help,
        "clear" => SlashCommand::Clear,
        "model" => SlashCommand::Model,
        "permissions" => SlashCommand::Permissions,
        "quit" | "exit" => SlashCommand::Quit,
        other => SlashCommand::Unknown(other.to_string()),
    })
}

/// Top-level frame layout: transcript (flexible) / input box (3 rows) /
/// status line (1 row), with the confirm dialog (if pending) drawn as an
/// overlay on top.
pub fn draw(frame: &mut Frame, app: &AppState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    transcript::render(frame, chunks[0], app);
    input::render(frame, chunks[1], app);
    statusline::render(frame, chunks[2], app);

    if let Some(pending) = &app.pending_confirm {
        confirm::render_overlay(frame, area, pending);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState::new(
            "qwen3:8b".into(),
            "ollama".into(),
            PermissionMode::default(),
        )
    }

    #[test]
    fn submit_input_trims_and_clears() {
        let mut app = state();
        app.input = "  hello there  ".to_string();
        let submitted = app.submit_input();
        assert_eq!(submitted, Some("hello there".to_string()));
        assert!(app.input.is_empty());
        assert_eq!(app.input_history, vec!["hello there".to_string()]);
    }

    #[test]
    fn submit_empty_input_returns_none_and_records_nothing() {
        let mut app = state();
        app.input = "   ".to_string();
        assert_eq!(app.submit_input(), None);
        assert!(app.input_history.is_empty());
    }

    #[test]
    fn history_recall_prev_then_next_round_trips() {
        let mut app = state();
        app.input = "first".to_string();
        app.submit_input();
        app.input = "second".to_string();
        app.submit_input();

        app.recall_history_prev();
        assert_eq!(app.input, "second");
        app.recall_history_prev();
        assert_eq!(app.input, "first");
        // At the oldest entry — recalling prev again stays put.
        app.recall_history_prev();
        assert_eq!(app.input, "first");

        app.recall_history_next();
        assert_eq!(app.input, "second");
        app.recall_history_next();
        assert_eq!(app.input, "", "past the newest entry clears the input");
    }

    #[test]
    fn tool_call_lifecycle_starts_pending_then_completes() {
        let mut app = state();
        app.push_tool_call_started("kb_search".into(), serde_json::json!({"query": "x"}));
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::ToolCall { result: None, .. })
        ));
        app.complete_tool_call("kb_search", true, "3 results".into());
        assert!(matches!(
            app.transcript.last(),
            Some(TranscriptEntry::ToolCall { result: Some((true, output)), .. }) if output == "3 results"
        ));
    }

    #[test]
    fn complete_tool_call_matches_most_recent_pending_by_name() {
        // Two concurrent-in-transcript calls to the same tool name — completing
        // must resolve the most recent PENDING one, not the already-resolved one.
        let mut app = state();
        app.push_tool_call_started("kb_get".into(), serde_json::json!({"id": "a"}));
        app.complete_tool_call("kb_get", true, "a-result".into());
        app.push_tool_call_started("kb_get".into(), serde_json::json!({"id": "b"}));
        app.complete_tool_call("kb_get", false, "b-failed".into());

        let results: Vec<_> = app
            .transcript
            .iter()
            .filter_map(|e| match e {
                TranscriptEntry::ToolCall { result, .. } => result.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(
            results,
            vec![
                (true, "a-result".to_string()),
                (false, "b-failed".to_string())
            ]
        );
    }

    #[test]
    fn toggle_expanded_only_affects_tool_calls() {
        let mut app = state();
        app.push_user("hi".into());
        app.push_tool_call_started("kb_get".into(), serde_json::json!({}));
        app.toggle_tool_call_expanded(0); // user entry — no-op
        app.toggle_tool_call_expanded(1);
        assert!(matches!(
            app.transcript[1],
            TranscriptEntry::ToolCall { expanded: true, .. }
        ));
        app.toggle_tool_call_expanded(1);
        assert!(matches!(
            app.transcript[1],
            TranscriptEntry::ToolCall {
                expanded: false,
                ..
            }
        ));
    }

    #[test]
    fn slash_command_parsing() {
        assert_eq!(parse_slash_command("/help"), Some(SlashCommand::Help));
        assert_eq!(parse_slash_command("/clear"), Some(SlashCommand::Clear));
        assert_eq!(parse_slash_command("/quit"), Some(SlashCommand::Quit));
        assert_eq!(parse_slash_command("/exit"), Some(SlashCommand::Quit));
        assert_eq!(
            parse_slash_command("/bogus"),
            Some(SlashCommand::Unknown("bogus".to_string()))
        );
        assert_eq!(parse_slash_command("not a command"), None);
        assert_eq!(parse_slash_command(""), None);
    }
}
