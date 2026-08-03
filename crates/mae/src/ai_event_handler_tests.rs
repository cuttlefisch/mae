//! Tests for [`super`] -- `ai_event_handler`'s MCP/Scheme dispatch entry
//! points. Split from `ai_event_handler.rs`'s inline `mod tests` under
//! CLAUDE.md's 500-line test-file ceiling (the combined extraction was
//! 1234 lines). This file keeps the pre-#363 dispatch-mechanics helpers +
//! tests, the #363 companion tests, and the MCP-request builder helpers
//! shared by every sibling file split out below -- see each sibling's own
//! doc comment for why it reaches back here via `use super::super::*;`
//! rather than duplicating these helpers (CLAUDE.md principle #8).

use super::*;

/// Build a bare MCP tool-call request with no declared session ceiling or
/// category allowlist. Shared by the #363 and #372 dispatch-mechanics tests.
fn mcp_request(
    tool_name: &str,
    arguments: serde_json::Value,
) -> (
    mae_mcp::McpToolRequest,
    tokio::sync::oneshot::Receiver<mae_mcp::McpToolResult>,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    (
        mae_mcp::McpToolRequest {
            tool_name: tool_name.to_string(),
            arguments,
            reply: tx,
            requester: mae_mcp::RequesterContext::default(),
        },
        rx,
    )
}

/// Build an MCP tool-call request carrying a declared per-session
/// permission ceiling. Shared by the ADR-051 and ADR-090
/// permission-ceiling tests.
fn mcp_request_with_ceiling(
    tool_name: &str,
    arguments: serde_json::Value,
    session_id: u64,
    declared_permission_ceiling: Option<&str>,
) -> (
    mae_mcp::McpToolRequest,
    tokio::sync::oneshot::Receiver<mae_mcp::McpToolResult>,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    (
        mae_mcp::McpToolRequest {
            tool_name: tool_name.to_string(),
            arguments,
            reply: tx,
            requester: mae_mcp::RequesterContext {
                session_id,
                declared_permission_ceiling: declared_permission_ceiling.map(|s| s.to_string()),
                ..Default::default()
            },
        },
        rx,
    )
}

/// ADR-056 analogue of `mcp_request_with_ceiling` above, for a
/// session-declared tool-category allowlist.
fn mcp_request_with_categories(
    tool_name: &str,
    arguments: serde_json::Value,
    session_id: u64,
    declared_tool_categories: Option<&str>,
) -> (
    mae_mcp::McpToolRequest,
    tokio::sync::oneshot::Receiver<mae_mcp::McpToolResult>,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    (
        mae_mcp::McpToolRequest {
            tool_name: tool_name.to_string(),
            arguments,
            reply: tx,
            requester: mae_mcp::RequesterContext {
                session_id,
                declared_tool_categories: declared_tool_categories.map(|s| s.to_string()),
                ..Default::default()
            },
        },
        rx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A minimal fake provider whose `send()` is never actually invoked by
    /// the `Verified` test below — it only needs a distinctive `name()` so
    /// the test can prove the returned provider is functional and unwrapped
    /// by identity of behavior, not by downcasting (`AgentProvider` isn't
    /// `Any`).
    struct FakeProvider(&'static str);

    #[async_trait::async_trait]
    impl mae_ai::AgentProvider for FakeProvider {
        async fn send(
            &self,
            _: &[mae_ai::Message],
            _: &[mae_ai::ToolDefinition],
            _: &str,
        ) -> Result<mae_ai::ProviderResponse, mae_ai::ProviderError> {
            unimplemented!("not exercised by this test")
        }
        fn name(&self) -> &str {
            self.0
        }
    }

    /// A fake provider that always returns a completely empty response (no
    /// text, no tool calls) and counts how many times `send()` actually ran
    /// on this concrete instance.
    ///
    /// This is the vehicle for the "prove wrapping actually happened" test.
    /// `GuardrailProvider` cannot be distinguished from a bare provider via
    /// `AgentProvider`'s public surface by identity alone — `.name()` just
    /// forwards transparently to the inner provider's name, and the trait
    /// isn't `Any`, so downcasting is not an option. What *is* observable is
    /// behavior: `GuardrailProvider::send` treats a completely empty
    /// response as "the model produced nothing usable" and issues exactly
    /// one corrective retry nudge — a *second* call into the inner provider
    /// (see `crates/ai/src/guardrail.rs`'s "targeted retry nudge" pillar).
    /// So one call through a *wrapped* provider drives 2 calls into this
    /// counter, while one call through the *unwrapped* provider drives
    /// exactly 1. That difference is the only honest, non-cherry-picked way
    /// to prove `guardrail_wrap_if_needed`'s `matches!` polarity is correct.
    struct CountingEmptyProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl mae_ai::AgentProvider for CountingEmptyProvider {
        async fn send(
            &self,
            _: &[mae_ai::Message],
            _: &[mae_ai::ToolDefinition],
            _: &str,
        ) -> Result<mae_ai::ProviderResponse, mae_ai::ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(mae_ai::ProviderResponse {
                text: None,
                tool_calls: vec![],
                stop_reason: mae_ai::StopReason::EndTurn,
                usage: None,
            })
        }
        fn name(&self) -> &str {
            "counting-empty"
        }
    }

    #[test]
    fn verified_model_is_returned_unwrapped() {
        let provider: Box<dyn mae_ai::AgentProvider> = Box::new(FakeProvider("distinctive-name"));
        let result = guardrail_wrap_if_needed(mae_ai::ModelVerification::Verified, provider);

        // NOTE on what this test can and can't prove: `.name()` alone can't
        // distinguish "unwrapped" from "wrapped" because `GuardrailProvider`
        // forwards `name()` straight to its inner provider. So this test
        // only proves the `Verified` branch returns a working provider that
        // preserves identity via its name, without panicking or swapping in
        // something else. The actual behavioral proof that non-`Verified`
        // models get wrapped (and `Verified` models are NOT subjected to
        // guardrail behavior) lives in
        // `unverified_model_is_wrapped_and_retries_on_empty_response` below,
        // which can observe wrapping through a real behavioral difference.
        assert_eq!(result.name(), "distinctive-name");
    }

    #[tokio::test]
    async fn unverified_model_is_wrapped_and_retries_on_empty_response() {
        let calls = Arc::new(AtomicUsize::new(0));
        let inner = CountingEmptyProvider {
            calls: calls.clone(),
        };
        let provider: Box<dyn mae_ai::AgentProvider> = Box::new(inner);

        // `Testing`/`Untested` are the two non-`Verified` variants this
        // function must wrap.
        let wrapped = guardrail_wrap_if_needed(mae_ai::ModelVerification::Untested, provider);
        let _ = wrapped.send(&[], &[], "system prompt").await.unwrap();

        // A wrapped provider's empty response triggers exactly one retry
        // nudge, so the inner `CountingEmptyProvider` sees 2 calls for our
        // single `send()` call. If `guardrail_wrap_if_needed` had (wrongly)
        // NOT wrapped for `Untested`, this would be 1 instead.
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "expected the guardrail's empty-response retry nudge to fire a second send()"
        );
    }

    // --- #363: execute_command MCP tool must dispatch Scheme-sourced commands ---

    /// The policy the pre-ADR-090 dispatch-mechanics tests below run under.
    /// They assert *routing* (which dispatch path a command takes, which window
    /// it lands in), not permissions, so they declare a permissive ceiling
    /// explicitly. Inheriting `PermissionPolicy::default()` would silently turn
    /// each of them into an approval-prompt test the moment the default moves.
    /// ADR-090's own tests build their policies deliberately.
    pub(super) fn dispatch_mechanics_policy() -> mae_ai::PermissionPolicy {
        mae_ai::PermissionPolicy {
            auto_approve_up_to: PermissionTier::Privileged,
            ..mae_ai::PermissionPolicy::default()
        }
    }

    #[tokio::test]
    async fn execute_command_dispatches_a_scheme_sourced_command() {
        let mut editor = Editor::new();
        let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();
        scheme
            .eval(r#"(define (my-greet) (buffer-insert "hi")) (define-command "my-greet" "test" "my-greet")"#)
            .unwrap();
        scheme.apply_to_editor(&mut editor);
        assert_eq!(
            editor.commands.get("my-greet").map(|c| &c.source),
            Some(&mae_core::CommandSource::Scheme("my-greet".into())),
            "sanity: registration must have landed before dispatch is exercised"
        );

        let (req, mut rx) = mcp_request(
            "execute_command",
            serde_json::json!({"command": "my-greet"}),
        );
        let (lsp_tx, _lsp_rx) = tokio::sync::mpsc::channel(1);
        let mut deferred = Vec::new();
        let resolved = handle_mcp_request(
            &mut editor,
            req,
            &[],
            &dispatch_mechanics_policy(),
            &lsp_tx,
            &mut deferred,
            &mut scheme,
        );

        assert!(resolved, "execute_command must resolve immediately");
        let result = rx.try_recv().expect("reply must have been sent");
        assert!(result.success, "expected success: {}", result.output);
        let idx = editor.active_buffer_idx();
        assert_eq!(
            editor.buffers[idx].rope().to_string(),
            "hi",
            "the scheme command's body must have actually run, not just been looked up"
        );
    }

    /// Adversarial regression test (found via an independent security
    /// review of this branch, not by a happy-path pass): before this fix,
    /// the Scheme-sourced-command bridge above dispatched with NO
    /// permission check at all — it never reached
    /// `execute_tool_dispatch_body`'s `policy.is_allowed(...)` gate, the
    /// only enforcement point in the parallel builtins-only path. A session
    /// that declared a ReadOnly ceiling at `initialize` (ADR-051's own
    /// headline feature, and the exact mechanism
    /// `session_declared_ceiling_denies_a_call_the_global_policy_alone_
    /// would_allow` above already proves for a Rust builtin command) could
    /// still execute ANY Scheme-sourced command with full effect via
    /// `execute_command`. Proves both properties: the call is denied, AND
    /// the command's body never actually ran (buffer unchanged) — a test
    /// that only checked `result.success == false` without also checking
    /// for a real side effect would pass even if denial were reported but
    /// dispatch happened anyway.
    #[tokio::test]
    async fn execute_command_denies_a_scheme_sourced_command_above_the_declared_ceiling() {
        let mut editor = Editor::new();
        let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();
        scheme
            .eval(r#"(define (my-greet) (buffer-insert "hi")) (define-command "my-greet" "test" "my-greet")"#)
            .unwrap();
        scheme.apply_to_editor(&mut editor);
        assert_eq!(
            editor.commands.get("my-greet").map(|c| &c.source),
            Some(&mae_core::CommandSource::Scheme("my-greet".into())),
            "sanity: registration must have landed before dispatch is exercised"
        );

        let global_policy = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        let (req, mut rx) = mcp_request_with_ceiling(
            "execute_command",
            serde_json::json!({"command": "my-greet"}),
            1,
            Some("ReadOnly"),
        );
        let (lsp_tx, _lsp_rx) = tokio::sync::mpsc::channel(1);
        let mut deferred = Vec::new();
        handle_mcp_request(
            &mut editor,
            req,
            &[],
            &global_policy,
            &lsp_tx,
            &mut deferred,
            &mut scheme,
        );

        let result = rx.try_recv().expect("reply must have been sent");
        assert!(
            !result.success,
            "a session with a declared ReadOnly ceiling must be denied a Scheme-sourced \
             command dispatched via execute_command, got success: {}",
            result.output
        );
        assert!(
            result.output.contains("Permission denied"),
            "denial reason should be a permission error, got: {}",
            result.output
        );
        let idx = editor.active_buffer_idx();
        assert_eq!(
            editor.buffers[idx].rope().to_string(),
            "",
            "the Scheme command's body must NOT have run -- denial that still executes the \
             command would be a strictly worse bug than reporting no error at all"
        );
    }

    #[tokio::test]
    async fn execute_command_unknown_name_still_errors() {
        let mut editor = Editor::new();
        let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();

        let (req, mut rx) = mcp_request(
            "execute_command",
            serde_json::json!({"command": "totally-unregistered-command"}),
        );
        let (lsp_tx, _lsp_rx) = tokio::sync::mpsc::channel(1);
        let mut deferred = Vec::new();
        handle_mcp_request(
            &mut editor,
            req,
            &[],
            &dispatch_mechanics_policy(),
            &lsp_tx,
            &mut deferred,
            &mut scheme,
        );

        let result = rx.try_recv().expect("reply must have been sent");
        assert!(
            !result.success,
            "an unregistered command must still error, not silently succeed: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn execute_command_builtin_still_dispatches_via_the_original_path() {
        // Regression guard: the #363 bridge must not break the pre-existing
        // builtins path it falls through to.
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "line one\nline two\n");
        let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();

        let (req, mut rx) = mcp_request(
            "execute_command",
            serde_json::json!({"command": "move-down"}),
        );
        let (lsp_tx, _lsp_rx) = tokio::sync::mpsc::channel(1);
        let mut deferred = Vec::new();
        handle_mcp_request(
            &mut editor,
            req,
            &[],
            &dispatch_mechanics_policy(),
            &lsp_tx,
            &mut deferred,
            &mut scheme,
        );

        let result = rx.try_recv().expect("reply must have been sent");
        assert!(result.success, "expected success: {}", result.output);
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 1);
    }
}

#[cfg(test)]
#[path = "ai_event_handler_permission_tests.rs"]
mod ai_event_handler_permission_tests;

#[cfg(test)]
#[path = "ai_event_handler_session_permission_tests.rs"]
mod ai_event_handler_session_permission_tests;

#[cfg(test)]
#[path = "ai_event_handler_adr090_tests.rs"]
mod ai_event_handler_adr090_tests;
