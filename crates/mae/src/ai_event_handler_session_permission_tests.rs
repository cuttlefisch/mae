//! ADR-051's session-isolation tests: the async, multi-session cases that
//! call `mcp_request_with_ceiling` / `mcp_request_with_categories` (both
//! defined in `ai_event_handler_tests.rs`, two module levels up -- see
//! `ai_event_handler_permission_tests.rs`'s doc comment for why that needs
//! `use super::super::*;`). Split out from that file's sync half to stay
//! under the 500-line test ceiling on its own.

#[cfg(test)]
mod tests {
    use super::super::*;

    /// ADR-056, primary adversarial target: `execute_command` is itself
    /// uncategorized (`classify_tool_category` returns `None` for it), so a
    /// Knowledge-only session must be denied calling it -- the highest-value
    /// bypass a restricted session would try first, since it can indirectly
    /// reach almost anything else via a registered command.
    #[tokio::test]
    async fn knowledge_only_session_denies_execute_command() {
        let global_policy = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        let mut editor = Editor::new();
        let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();
        let (req, mut rx) = mcp_request_with_categories(
            "execute_command",
            serde_json::json!({"command": "move-down"}),
            1,
            Some("knowledge"),
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
            "a Knowledge-only session must be denied execute_command (uncategorized, fail-closed)"
        );
        assert!(
            result.output.contains("Category denied"),
            "expected a category-denial message, got: {}",
            result.output
        );
    }

    /// A Knowledge-only session must also be denied real mutating tools in
    /// OTHER wrong categories -- proves the restriction isn't scoped just to
    /// `execute_command`.
    #[tokio::test]
    async fn knowledge_only_session_denies_shell_exec_git_push_buffer_write() {
        let global_policy = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        for tool in ["shell_exec", "git_push", "buffer_write"] {
            let mut editor = Editor::new();
            let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();
            let (req, mut rx) =
                mcp_request_with_categories(tool, serde_json::json!({}), 1, Some("knowledge"));
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
                "a Knowledge-only session must be denied {tool}, got success: {}",
                result.output
            );
            assert!(
                result.output.contains("Category denied"),
                "expected a category-denial message for {tool}, got: {}",
                result.output
            );
        }
    }

    /// The allowlist must not be accidentally denying everything: real
    /// Knowledge-category tools must still be reachable (not blocked by the
    /// category gate -- functional success/failure on a fresh, unconfigured
    /// `Editor` is a separate concern from whether the PERMISSION gate let
    /// the call through, which is what this test verifies).
    #[tokio::test]
    async fn knowledge_only_session_allows_knowledge_tools_through_the_gate() {
        let global_policy = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        for tool in ["kb_search", "kb_export_guidance", "help_open"] {
            let mut editor = Editor::new();
            let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();
            let (req, mut rx) = mcp_request_with_categories(
                tool,
                serde_json::json!({"query": "x", "topic": "x"}),
                1,
                Some("knowledge"),
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
                !result.output.contains("Category denied"),
                "a Knowledge-only session must NOT be denied {tool} by the category gate, got: {}",
                result.output
            );
            assert!(
                !result.output.contains("Permission denied"),
                "unexpected permission denial for {tool} (tier, not category): {}",
                result.output
            );
        }
    }

    /// Global-instance restriction composes with a looser (or absent)
    /// per-session declaration as an INTERSECTION, never an override -- a
    /// session cannot escalate past what the instance-wide config already
    /// restricts just by declaring (or not declaring) something looser.
    #[tokio::test]
    async fn global_category_restriction_is_not_widened_by_a_looser_session_declaration() {
        let global_policy = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
            allowed_categories: Some([mae_ai::ToolCategory::Knowledge].into_iter().collect()),
            ..PermissionPolicy::default()
        };
        let mut editor = Editor::new();
        let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();
        // Session declares NO restriction of its own -- the instance-wide
        // Knowledge-only restriction must still apply.
        let (req, mut rx) =
            mcp_request_with_categories("shell_exec", serde_json::json!({}), 1, None);
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
            "a session with no declared restriction must still be bound by the instance-wide \
             Knowledge-only config, not escalate past it"
        );
        assert!(result.output.contains("Category denied"));
    }

    /// ADR-056 correction, discovered writing this test (principle #15): the
    /// Scheme-sourced-command bridge below matches BEFORE ever reaching
    /// `execute_tool_dispatch_body`'s category check, so it needs (and now
    /// has) its own independent category check -- this proves that fix, not
    /// the generic-tool-dispatch path.
    #[tokio::test]
    async fn knowledge_only_session_denies_execute_command_naming_a_scheme_sourced_command() {
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
        let (req, mut rx) = mcp_request_with_categories(
            "execute_command",
            serde_json::json!({"command": "my-greet"}),
            1,
            Some("knowledge"),
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
            "a Knowledge-only session must be denied a Scheme-sourced command via execute_command"
        );
        assert!(
            result.output.contains("Category denied"),
            "expected a category-denial message, got: {}",
            result.output
        );
        let idx = editor.active_buffer_idx();
        assert_eq!(
            editor.buffers[idx].rope().to_string(),
            "",
            "the Scheme command's body must NOT have run -- a denied call must have zero side effects"
        );
    }

    /// Adversarial end-to-end proof (ADR-051's own required test): simulate
    /// two sessions against the SAME global policy and the SAME (untiered,
    /// so it defaults to `Write`) tool -- one with no declared ceiling
    /// (allowed, matching today's behavior exactly) and one that declared a
    /// stricter `ReadOnly` ceiling (must be DENIED). This is not testing
    /// client-side UI; it calls `handle_mcp_request` directly, exactly as if
    /// a client skipped its own confirmation dialog and called `tools/call`
    /// straight through -- proving the server-side gate is the real
    /// boundary regardless of client behavior.
    #[tokio::test]
    async fn session_declared_ceiling_denies_a_call_the_global_policy_alone_would_allow() {
        let global_policy = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Write,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };

        // Session 1: no declared ceiling -- Write-tier (untiered default)
        // call is allowed, unchanged from pre-ADR-051 behavior.
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "line one\nline two\n");
        let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();
        let (req1, mut rx1) = mcp_request_with_ceiling(
            "execute_command",
            serde_json::json!({"command": "move-down"}),
            1,
            None,
        );
        let (lsp_tx, _lsp_rx) = tokio::sync::mpsc::channel(1);
        let mut deferred = Vec::new();
        handle_mcp_request(
            &mut editor,
            req1,
            &[],
            &global_policy,
            &lsp_tx,
            &mut deferred,
            &mut scheme,
        );
        let result1 = rx1.try_recv().expect("reply must have been sent");
        assert!(
            result1.success,
            "session with no declared ceiling must be unaffected: {}",
            result1.output
        );

        // Session 2: declared a stricter ReadOnly ceiling -- the identical
        // Write-tier call must now be denied, even though the global policy
        // alone would have allowed it.
        let (req2, mut rx2) = mcp_request_with_ceiling(
            "execute_command",
            serde_json::json!({"command": "move-down"}),
            2,
            Some("ReadOnly"),
        );
        let mut deferred2 = Vec::new();
        handle_mcp_request(
            &mut editor,
            req2,
            &[],
            &global_policy,
            &lsp_tx,
            &mut deferred2,
            &mut scheme,
        );
        let result2 = rx2.try_recv().expect("reply must have been sent");
        assert!(
            !result2.success,
            "session with a declared ReadOnly ceiling must have its Write-tier call denied"
        );
        assert!(
            result2.output.contains("Permission denied"),
            "denial reason should be a permission error, got: {}",
            result2.output
        );
    }

    /// Combined adversarial proof for ADR-051's literal DoD wording (#378):
    /// N>=3 real MCP sessions, at least 2 with *differing* declared
    /// permission ceilings, asserted to have BOTH properties hold
    /// simultaneously -- not just window isolation (already proven at N=3
    /// with a single shared tier by `with_ai_dispatch_scope_for_session_
    /// isolates_three_concurrent_sessions`) and not just ceiling enforcement
    /// (already proven, but only at N=2, by the test just above). A real
    /// confused-deputy bug could plausibly only reproduce when a permission
    /// *denial* and a window-isolation dispatch interleave across 3+
    /// sessions -- this is the literal scenario neither existing test alone
    /// could catch.
    #[tokio::test]
    async fn three_plus_sessions_with_differing_ceilings_stay_isolated_on_both_axes() {
        let global_policy = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Write,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        let mut editor = Editor::new();
        editor.buffers[0].name = "*AI:claude*".to_string();
        editor.buffers[0].agent_shell = true;
        editor.buffers[0].insert_text_at(0, "line one\nline two\n");
        let original_id = editor.window_mgr.focused_id();
        let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();
        let (lsp_tx, _lsp_rx) = tokio::sync::mpsc::channel(1);

        // Session 1: no declared ceiling -- a Write-tier call is allowed.
        let (req1, mut rx1) = mcp_request_with_ceiling(
            "execute_command",
            serde_json::json!({"command": "move-down"}),
            101,
            None,
        );
        let mut deferred1 = Vec::new();
        handle_mcp_request(
            &mut editor,
            req1,
            &[],
            &global_policy,
            &lsp_tx,
            &mut deferred1,
            &mut scheme,
        );
        let result1 = rx1.try_recv().expect("reply must have been sent");
        assert!(result1.success, "session 101 (no ceiling) must be allowed");

        // Session 2: a stricter ReadOnly ceiling -- the identical Write-tier
        // call must be denied, even mid-way through other sessions' activity.
        let (req2, mut rx2) = mcp_request_with_ceiling(
            "execute_command",
            serde_json::json!({"command": "move-down"}),
            102,
            Some("ReadOnly"),
        );
        let mut deferred2 = Vec::new();
        handle_mcp_request(
            &mut editor,
            req2,
            &[],
            &global_policy,
            &lsp_tx,
            &mut deferred2,
            &mut scheme,
        );
        let result2 = rx2.try_recv().expect("reply must have been sent");
        assert!(
            !result2.success,
            "session 102 (ReadOnly ceiling) must be denied a Write-tier call"
        );

        // Session 3: no declared ceiling again -- must be allowed exactly
        // like session 101, unaffected by session 102's denial.
        let (req3, mut rx3) = mcp_request_with_ceiling(
            "execute_command",
            serde_json::json!({"command": "git-status"}),
            103,
            None,
        );
        let mut deferred3 = Vec::new();
        handle_mcp_request(
            &mut editor,
            req3,
            &[],
            &global_policy,
            &lsp_tx,
            &mut deferred3,
            &mut scheme,
        );
        let result3 = rx3.try_recv().expect("reply must have been sent");
        assert!(
            result3.success,
            "session 103 (no ceiling) must be unaffected by session 102's denial: {}",
            result3.output
        );

        // Window isolation: all three sessions -- including the DENIED
        // one -- must each have established their own distinct companion
        // window (window setup wraps the dispatch body, so it happens
        // before the permission check runs inside it).
        let target_101 = editor
            .ai
            .mcp_sessions
            .get(&101)
            .and_then(|s| s.windows.target_window_id)
            .expect("session 101 should have an established target");
        let target_102 = editor
            .ai
            .mcp_sessions
            .get(&102)
            .and_then(|s| s.windows.target_window_id)
            .expect("session 102 (denied) must still have its own established target");
        let target_103 = editor
            .ai
            .mcp_sessions
            .get(&103)
            .and_then(|s| s.windows.target_window_id)
            .expect("session 103 should have an established target");

        assert_ne!(
            target_101, target_102,
            "sessions 101 and 102 must not share a window"
        );
        assert_ne!(
            target_102, target_103,
            "sessions 102 and 103 must not share a window"
        );
        assert_ne!(
            target_101, target_103,
            "sessions 101 and 103 must not share a window"
        );
        assert_ne!(target_101, original_id);
        assert_ne!(target_102, original_id);
        assert_ne!(target_103, original_id);

        // Confused-deputy check: re-dispatching for session 101 must reuse
        // ITS OWN window, never session 102's or 103's, even after a denial
        // happened in between.
        let (req1b, mut rx1b) = mcp_request_with_ceiling(
            "execute_command",
            serde_json::json!({"command": "move-down"}),
            101,
            None,
        );
        let mut deferred1b = Vec::new();
        handle_mcp_request(
            &mut editor,
            req1b,
            &[],
            &global_policy,
            &lsp_tx,
            &mut deferred1b,
            &mut scheme,
        );
        let result1b = rx1b.try_recv().expect("reply must have been sent");
        assert!(result1b.success);
        let target_101_again = editor
            .ai
            .mcp_sessions
            .get(&101)
            .and_then(|s| s.windows.target_window_id)
            .expect("session 101 should still have a target");
        assert_eq!(
            target_101_again, target_101,
            "session 101 must reuse its own window on a second dispatch, not session 102's or 103's"
        );
    }
}
