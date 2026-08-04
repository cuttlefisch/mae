//! ADR-090 tests: what the external-MCP surface does with the three
//! permission states (allow/ask/deny). Split from
//! `ai_event_handler_tests.rs` to stay under the 500-line test ceiling;
//! `mcp_request_with_ceiling` / `mcp_request_with_categories` are defined
//! there, two module levels up.

#[cfg(test)]
mod tests {
    use super::super::*;

    // --- ADR-090: what the external-MCP surface does with the three states ---

    /// Run one MCP tool call under `global_policy` (optionally with a declared
    /// session ceiling) and return the reply. Shared by the ADR-090 cases so
    /// each test states only what it is trying to break.
    fn mcp_reply(
        global_policy: &PermissionPolicy,
        tool: &str,
        args: serde_json::Value,
        declared_ceiling: Option<&str>,
    ) -> mae_mcp::McpToolResult {
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "line one\nline two\n");
        let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();
        let (req, mut rx) = mcp_request_with_ceiling(tool, args, 7, declared_ceiling);
        let (lsp_tx, _lsp_rx) = tokio::sync::mpsc::channel(1);
        let mut deferred = Vec::new();
        handle_mcp_request(
            &mut editor,
            req,
            &[],
            global_policy,
            &lsp_tx,
            &mut deferred,
            &mut scheme,
        );
        rx.try_recv().expect("reply must have been sent")
    }

    /// ADR-090 D3, the explicit mapping: the external-MCP surface has no human
    /// to prompt, so an `Ask` becomes a denial — and the message says *why*,
    /// naming the ceiling an operator can raise. The property that matters is
    /// that it is never a success.
    #[tokio::test]
    async fn the_mcp_surface_maps_ask_to_a_denial_that_names_the_ceiling() {
        let global = PermissionPolicy {
            auto_approve_up_to: PermissionTier::ReadOnly,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        let result = mcp_reply(
            &global,
            "execute_command",
            serde_json::json!({"command": "move-down"}),
            None,
        );
        assert!(
            !result.success,
            "an Ask must never reach execution on a surface with no human: {}",
            result.output
        );
        assert!(
            result.output.contains("no human to confirm"),
            "the non-interactive mapping must be visible: {}",
            result.output
        );
        assert!(
            result.output.contains("ReadOnly"),
            "the message must name the ceiling to raise: {}",
            result.output
        );
    }

    /// The counterpart: a real `Deny` must NOT read like a raise-the-ceiling
    /// hint. A session-declared ceiling is binding, and telling the operator to
    /// raise a ceiling would be actively wrong advice.
    #[tokio::test]
    async fn a_session_declared_ceiling_denies_differently_from_an_unaskable_ask() {
        let global = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Privileged,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        let result = mcp_reply(
            &global,
            "execute_command",
            serde_json::json!({"command": "move-down"}),
            Some("ReadOnly"),
        );
        assert!(!result.success, "{}", result.output);
        assert!(
            result.output.contains("declared ceiling"),
            "a session-declared ceiling must be named as such: {}",
            result.output
        );
        assert!(
            !result.output.contains("no human to confirm"),
            "a real denial is not a missing-human problem: {}",
            result.output
        );
    }

    /// ADR-090 D2 + ADR-084 D4: an unparseable declaration is a `Deny`, not an
    /// `Ask` — over several realistic typos, not one hand-picked value.
    /// Softening this into `Ask` would mean a misspelt restriction becomes a
    /// prompt on an interactive surface, i.e. an escalation via typo.
    #[tokio::test]
    async fn an_unparseable_session_declaration_denies_and_says_so() {
        let global = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Privileged,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        for bad in ["not-a-real-tier", "Reed-Only", "privelaged", "  "] {
            let result = mcp_reply(
                &global,
                "execute_command",
                serde_json::json!({"command": "move-down"}),
                Some(bad),
            );
            assert!(!result.success, "{bad:?}: {}", result.output);
            assert!(
                result.output.contains("could not be parsed"),
                "{bad:?} must be reported as a parse failure, not as an ordinary ceiling: {}",
                result.output
            );
        }
    }

    /// A category restriction stays a `Deny` through the MCP path too, and is
    /// reported as a category denial rather than a tier one — the two axes must
    /// remain distinguishable to whoever reads the error.
    #[tokio::test]
    async fn a_category_restriction_denies_on_the_mcp_surface() {
        let global = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Privileged,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        let mut editor = Editor::new();
        let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();
        let (req, mut rx) = mcp_request_with_categories(
            "execute_command",
            serde_json::json!({"command": "move-down"}),
            9,
            Some("knowledge"),
        );
        let (lsp_tx, _lsp_rx) = tokio::sync::mpsc::channel(1);
        let mut deferred = Vec::new();
        handle_mcp_request(
            &mut editor,
            req,
            &[],
            &global,
            &lsp_tx,
            &mut deferred,
            &mut scheme,
        );
        let result = rx.try_recv().expect("reply must have been sent");
        assert!(!result.success, "{}", result.output);
        assert!(
            result.output.contains("Category denied"),
            "a category restriction must not be reported as a tier problem: {}",
            result.output
        );
    }

    /// The ambient Scheme tier (ADR-084 D2/D7) that the MCP path hands the VM
    /// is the `Allow` line of the session's *effective* policy — so a session
    /// that declared a ceiling gets evaluated Scheme bounded by it, not by the
    /// server's global policy. Exhaustive over the declared values that parse,
    /// because a single case cannot distinguish "min applied" from "declared
    /// ignored" when the two happen to coincide.
    #[test]
    fn the_mcp_ambient_scheme_tier_is_the_effective_allow_line() {
        let global = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
            allowed_categories: None,
            ..PermissionPolicy::default()
        };
        for (declared, expected) in [
            (None, PermissionTier::Shell),
            (Some("ReadOnly"), PermissionTier::ReadOnly),
            (Some("Write"), PermissionTier::Write),
            (Some("Shell"), PermissionTier::Shell),
            // A declaration ABOVE the global is not an escalation.
            (Some("Privileged"), PermissionTier::Shell),
            // ...and an unparseable one clamps to the most restrictive tier.
            (Some("gibberish"), PermissionTier::ReadOnly),
        ] {
            let effective = effective_permission_policy(&global, declared, None);
            assert_eq!(
                effective.ambient_scheme_tier(),
                expected,
                "declared={declared:?}"
            );
        }
    }

    /// The human keypress path is deliberately NOT bounded by the AI policy —
    /// the user already has a shell. Asserted explicitly so a future change
    /// that lowers it is a deliberate decision rather than an accident, and so
    /// the asymmetry is visible next to the AI-path test above.
    #[test]
    fn the_human_keypress_path_evaluates_scheme_at_full_authority() {
        assert_eq!(HUMAN_AMBIENT_TIER, PermissionTier::Privileged);
        assert!(
            HUMAN_AMBIENT_TIER > PermissionPolicy::default().ambient_scheme_tier(),
            "the human path must not be bounded by the AI's shipped default"
        );
    }
}
