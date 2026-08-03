//! Session-scoped agent state accessors (ADR-091).
//!
//! `dispatch_tool` had no session handle at all, which is why the six
//! session-scoped AI tools (`ai_set_mode`, `ai_set_profile`, `ai_set_budget`,
//! `log_activity`, `read_transcript`, `web_fetch`) were advertised over MCP
//! and yet fell through to `Unknown tool`: the state they touch lived only on
//! the embedded `AgentSession`'s tokio task.
//!
//! The handle is the pair (`Editor`, the MCP session id in scope). The id is
//! recorded by `Editor::with_ai_dispatch_scope_for_session`
//! (`window_ops.rs`) — the enforced boundary every MCP-originated dispatch
//! already routes through — for the extent of the call, exactly like the
//! `ai_dispatch_depth` counter beside it. Everything here resolves against it.
//!
//! @ai-caution: [dispatch] `dispatch_session_id` is identity and routing,
//! never authority. It answers "which session is calling", not "may it do
//! this" — the tier and category gates in `execute_tool_dispatch_body` answer
//! that, and nothing here may be used to widen them.

use super::Editor;

impl Editor {
    /// The MCP session id in scope for the currently-running dispatch, or
    /// `None` when there is none (embedded AI path, `--self-test`, a human
    /// keybinding). See [`crate::editor::ai_state::AiState::dispatch_session_id`].
    pub fn dispatch_session_id(&self) -> Option<u64> {
        self.ai.dispatch_session_id
    }

    /// Is the running dispatch on behalf of an external MCP client?
    ///
    /// The discriminator for surfaces that must differ between an external
    /// client and MAE's own embedded agent — notably ADR-091's exclusion of
    /// the inherently-interactive tools from external discovery, which must
    /// NOT hide them from the embedded session that can actually run them.
    pub fn is_external_mcp_dispatch(&self) -> bool {
        self.ai.dispatch_session_id.is_some()
    }

    /// Session-scoped agent state for the dispatch in scope (ADR-091): the
    /// per-MCP-session record when a session id is in scope, the process-wide
    /// record otherwise.
    pub fn agent_session(&self) -> &crate::editor::ai_state::AgentSessionState {
        match self.ai.dispatch_session_id {
            Some(sid) => match self.ai.mcp_sessions.get(&sid) {
                Some(state) => &state.agent,
                // Not yet populated: reading before any write. The
                // process-wide record is the right answer for a reader —
                // it holds the editor's own defaults — and creating an entry
                // on a *read* would let a read path grow the map.
                None => &self.ai.agent_session,
            },
            None => &self.ai.agent_session,
        }
    }

    /// Mutable counterpart of [`Self::agent_session`]. Creates the
    /// per-session record on first write, subject to the same coarse size
    /// bound as the window state it shares a record with.
    pub fn agent_session_mut(&mut self) -> &mut crate::editor::ai_state::AgentSessionState {
        let Some(sid) = self.ai.dispatch_session_id else {
            return &mut self.ai.agent_session;
        };
        if !self.ai.mcp_sessions.contains_key(&sid)
            && self.ai.mcp_sessions.len() >= super::ai_state::MAX_TRACKED_MCP_SESSION_WINDOWS
        {
            if let Some(&evict) = self.ai.mcp_sessions.keys().next() {
                self.ai.mcp_sessions.remove(&evict);
            }
        }
        &mut self.ai.mcp_sessions.entry(sid).or_default().agent
    }

    /// Set the agent session's operating mode (`ai_set_mode`).
    ///
    /// With an MCP session in scope the value is session-scoped and does not
    /// touch the editor's global `ai_mode` — two connected agents can be in
    /// different modes. With no session in scope the handle *is* the embedded
    /// agent's, so the editor option moves too, matching exactly what
    /// `AiEvent::UpdateMode` does when the embedded session runs the same
    /// tool.
    pub fn set_agent_mode(&mut self, mode: &str) -> Result<String, String> {
        if self.ai.dispatch_session_id.is_none() {
            self.set_option("ai_mode", mode)?;
        } else {
            // Validate against the same allowed set the option enforces, so a
            // per-session value can never be one the editor would reject.
            const VALID: [&str; 3] = ["standard", "plan", "auto-accept"];
            if !VALID.contains(&mode) {
                return Err(format!(
                    "Invalid AI mode: '{mode}' (expected: standard, plan, auto-accept)"
                ));
            }
        }
        self.agent_session_mut().mode = Some(mode.to_string());
        Ok(format!("AI mode set to {mode}"))
    }

    /// Set the agent session's prompt profile (`ai_set_profile`). Same
    /// session-scoped / write-through split as [`Self::set_agent_mode`].
    pub fn set_agent_profile(&mut self, profile: &str) -> Result<String, String> {
        if self.ai.dispatch_session_id.is_none() {
            self.set_option("ai_profile", profile)?;
        }
        self.agent_session_mut().profile = Some(profile.to_string());
        Ok(format!("AI profile set to {profile}"))
    }

    /// The mode in force for the dispatch in scope: the session's own value
    /// if it set one, otherwise the editor's global mode.
    pub fn agent_mode(&self) -> String {
        self.agent_session()
            .mode
            .clone()
            .unwrap_or_else(|| self.ai.mode.clone())
    }

    /// The profile in force for the dispatch in scope. See [`Self::agent_mode`].
    pub fn agent_profile(&self) -> String {
        self.agent_session()
            .profile
            .clone()
            .unwrap_or_else(|| self.ai.profile.clone())
    }
}
