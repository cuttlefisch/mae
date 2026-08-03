//! Issue #372's companion-window-protection case for the Scheme-command
//! bridge, plus ADR-051's synchronous per-session permission-ceiling
//! tests. Split from `ai_event_handler_tests.rs` to stay under the
//! 500-line test ceiling; the MCP-request builder helpers these tests
//! call (`mcp_request`, `mcp_request_with_ceiling`) are defined two module
//! levels up in `ai_event_handler_tests.rs` -- the `#[path]` indirection
//! that loads this file adds one level over a plain inline `mod`, so
//! `use super::super::*;` (not `use super::*;`) is what reaches them.

#[cfg(test)]
mod tests {
    use super::super::*;
    // One definition, in ai_event_handler_tests; not copied (principle #8).
    use super::super::tests::dispatch_mechanics_policy;

    // --- Issue #372: the Scheme-command bridge must also get companion-window protection ---

    #[tokio::test]
    async fn execute_command_scheme_sourced_gets_companion_window_protection() {
        // This is the one MCP dispatch path that bypasses
        // `execute_tool_with_requester` (and thus its `with_ai_dispatch_scope`
        // wrap) entirely, since `crates/ai` has no `SchemeRuntime` in scope.
        // Prove the bridge branch at line ~864 wraps itself the same way.
        let mut editor = Editor::new();
        editor.buffers[0].name = "*AI:claude*".to_string();
        editor.buffers[0].agent_shell = true;
        let original_id = editor.window_mgr.focused_id();
        assert_eq!(editor.window_mgr.iter_windows().count(), 1);

        let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();
        scheme
            .eval(r#"(define (my-greet) (buffer-insert "hi")) (define-command "my-greet" "test" "my-greet")"#)
            .unwrap();
        scheme.apply_to_editor(&mut editor);

        let (req, mut rx) = mcp_request(
            "execute_command",
            serde_json::json!({"command": "my-greet"}),
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

        // A companion window must have been established, and the original
        // agent-shell window/buffer must be untouched and still visible.
        assert_eq!(
            editor.window_mgr.iter_windows().count(),
            2,
            "Scheme-command dispatch must establish a companion window, same as the builtins path"
        );
        let orig_win = editor.window_mgr.window(original_id).unwrap();
        assert_eq!(orig_win.buffer_idx, 0);
        assert!(editor.buffers[orig_win.buffer_idx].agent_shell);
        // ADR-051: the bridge is now session-scoped (`mcp_request`'s
        // `RequesterContext::default()` -> session_id 0), so the companion
        // window lands in the per-session map, not the process-global
        // `target_window_id` field -- that field must in fact stay
        // untouched by MCP-session dispatch now (see
        // `with_ai_dispatch_scope_for_session_isolates_three_concurrent_sessions`
        // in `crates/core`'s test suite for the multi-session isolation proof).
        assert!(
            editor
                .ai
                .mcp_sessions
                .get(&0)
                .and_then(|s| s.windows.target_window_id)
                .is_some(),
            "companion window must be recorded under the requesting session's own id"
        );
        assert!(
            editor.ai.target_window_id.is_none(),
            "session-scoped dispatch must not leak into the global no-session target"
        );
        assert_eq!(
            editor.window_mgr.focused_id(),
            original_id,
            "focus must be restored to the agent-shell window after the scope exits"
        );
    }

    // --- ADR-051: per-session permission-ceiling tightening ---

    #[test]
    fn effective_permission_policy_with_no_declared_ceiling_is_unchanged() {
        let global = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        let effective = effective_permission_policy(&global, None, None);
        assert_eq!(effective.auto_approve_up_to, PermissionTier::Shell);
    }

    #[test]
    fn effective_permission_policy_tightens_when_declared_ceiling_is_lower() {
        let global = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        let effective = effective_permission_policy(&global, Some("ReadOnly"), None);
        assert_eq!(effective.auto_approve_up_to, PermissionTier::ReadOnly);
    }

    #[test]
    fn effective_permission_policy_never_loosens_beyond_global() {
        // A session declaring a HIGHER ceiling than the server's own global
        // policy must never escalate -- this is the core safety property
        // ADR-051 requires: a self-declared ceiling can only ever tighten.
        let global = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Write,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        let effective = effective_permission_policy(&global, Some("Privileged"), None);
        assert_eq!(
            effective.auto_approve_up_to,
            PermissionTier::Write,
            "declaring a higher ceiling than the global policy must not escalate"
        );
    }

    /// ADR-084 D4. This previously asserted the opposite — that an unparseable
    /// ceiling fell back to the *global* policy. Since the ceiling is
    /// tighten-only, that meant a session which tried to restrict itself and
    /// misspelled the value received no restriction at all.
    #[test]
    fn unparseable_declared_ceiling_resolves_to_the_most_restrictive_tier() {
        let global = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        for bad in ["not-a-real-tier", "readonly", "read-only", "", "SHELL"] {
            let effective = effective_permission_policy(&global, Some(bad), None);
            assert_eq!(
                effective.auto_approve_up_to,
                PermissionTier::ReadOnly,
                "{bad:?}: an unparseable ceiling must restrict, not fall through to global"
            );
        }
    }

    /// Declaring nothing is different from declaring nonsense: a session that
    /// never asked to be restricted keeps the global policy.
    #[test]
    fn a_session_that_declares_no_ceiling_keeps_the_global_policy() {
        let global = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        let effective = effective_permission_policy(&global, None, None);
        assert_eq!(effective.auto_approve_up_to, PermissionTier::Shell);
    }

    /// The fail-closed path must not become an escalation path: restricting to
    /// ReadOnly is a `min` against the global, so a garbage declaration can
    /// never raise a ceiling that was already lower.
    #[test]
    fn an_unparseable_ceiling_cannot_escalate_an_already_restricted_global() {
        let global = PermissionPolicy {
            auto_approve_up_to: PermissionTier::ReadOnly,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        let effective = effective_permission_policy(&global, Some("Privileged!!"), None);
        assert_eq!(effective.auto_approve_up_to, PermissionTier::ReadOnly);
    }

    /// Same rule on the category axis: a declared allowlist that parses to
    /// nothing denies everything rather than reverting to unrestricted.
    #[test]
    fn a_category_allowlist_parsing_to_nothing_denies_rather_than_unrestricts() {
        let global = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        let effective = effective_permission_policy(&global, None, Some("not-a-category"));
        let cats = effective
            .allowed_categories
            .expect("a declared allowlist must produce a restriction, not None");
        assert!(
            cats.is_empty(),
            "an unrecognised category list must deny, got {cats:?}"
        );
    }
}
