use crate::types::*;

/// Tool tiers for payload optimization — only core tools are sent by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolTier {
    /// Always sent (~15 tools). Essential for basic editing workflows.
    Core,
    /// Sent on request via `request_tools` meta-tool.
    Extended,
}

/// Tool categories for the `request_tools` meta-tool.
///
/// A category answers "what subject is this tool about?" — **not** "how much
/// damage can it do?" That second question is `PermissionTier`'s, and the two
/// axes are independent and both enforced (ADR-085, ADR-056). Do not encode
/// risk here, and do not assume a category grant bounds blast radius.
///
/// The one property a category must have is that it does not *span* blast
/// radii, so that restricting by subject is not silently a lie about risk.
/// [`ToolCategory::is_read_flavoured`] plus the registry-wide invariant test in
/// this module's tests enforce that mechanically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCategory {
    Lsp,
    Dap,
    Knowledge,
    /// Tools that execute code, run arbitrary queries, or touch the network/
    /// filesystem beyond ordinary editing as their *purpose*, split out of
    /// `Knowledge` by ADR-085. `babel_execute` runs org source blocks in twelve
    /// languages and `babel_tangle` writes them to arbitrary paths; `org_export`
    /// can shell out (mermaid rendering); `kb_register`/`kb_reimport` do
    /// filesystem discovery/rebuild of a KB instance; `kb_raw_query` runs
    /// arbitrary Datalog; `kb_enrich` and `kb_export_subgraph_html` make real
    /// network calls (embedding provider / `npx`). All are genuinely
    /// knowledge-work operations, which is exactly why classifying by subject
    /// alone put them inside a category an operator reads as "only my notes".
    Execution,
    ShellMgmt,
    Commands,
    Git,
    Web,
    Ai,
    Visual,
    Debug,
    Mcp,
}

impl ToolCategory {
    /// Every category, so registry-wide invariants can iterate rather than
    /// depend on someone remembering to extend a hand-written list.
    pub const ALL: &'static [ToolCategory] = &[
        ToolCategory::Lsp,
        ToolCategory::Dap,
        ToolCategory::Knowledge,
        ToolCategory::Execution,
        ToolCategory::ShellMgmt,
        ToolCategory::Commands,
        ToolCategory::Git,
        ToolCategory::Web,
        ToolCategory::Ai,
        ToolCategory::Visual,
        ToolCategory::Debug,
        ToolCategory::Mcp,
    ];

    /// Does this category's *name* invite an operator to read it as
    /// non-destructive?
    ///
    /// Read-flavoured categories may not contain tools above `Write` tier — the
    /// ADR-085 invariant. This is a judgement, deliberately written down as code
    /// and asserted by a test rather than left implicit in a taxonomy nobody
    /// re-reads. When adding a category, decide this explicitly: the honest
    /// answer is "would someone allowlisting only this category be surprised to
    /// get shell access?"
    pub fn is_read_flavoured(self) -> bool {
        match self {
            // Query/inspect surfaces. Their names promise looking, not doing.
            ToolCategory::Lsp | ToolCategory::Knowledge | ToolCategory::Debug => true,
            // Names that already say they act, or that span subsystems whose
            // whole point is mutation. `Web` sits here, not above: its only
            // current member, `web_fetch`, is a real network fetch (Shell
            // tier) -- "web" does not promise looking any more than "git"
            // does, and treating it as read-flavoured was the same
            // subject-vs-blast-radius conflation ADR-085 fixes for `babel_`.
            ToolCategory::Execution
            | ToolCategory::Dap
            | ToolCategory::ShellMgmt
            | ToolCategory::Commands
            | ToolCategory::Git
            | ToolCategory::Web
            | ToolCategory::Ai
            | ToolCategory::Visual
            | ToolCategory::Mcp => false,
        }
    }
}

/// Classify a tool into Core or Extended tier.
pub fn classify_tool_tier(name: &str) -> ToolTier {
    match name {
        // Core tools — always sent
        "buffer_read"
        | "buffer_write"
        | "cursor_info"
        | "open_file"
        | "switch_buffer"
        | "create_file"
        | "close_buffer"
        | "list_buffers"
        | "editor_state"
        | "project_search"
        | "project_files"
        | "project_info"
        | "shell_exec"
        | "get_option"
        | "set_option"
        | "help_open"
        | "file_read"
        | "self_test_suite"
        | "introspect"
        | "perf_stats"
        | "perf_benchmark"
        | "window_layout"
        | "ai_permissions"
        | "input_lock"
        | "git_status"
        | "git_diff"
        | "git_log"
        | "org_cycle"
        | "org_todo_cycle"
        | "org_open_link"
        | "babel_execute"
        | "babel_tangle"
        | "org_export"
        | "kb_instances"
        | "kb_register"
        | "kb_unregister"
        | "kb_reimport"
        | "kb_search_context"
        | "kb_shortest_path"
        | "kb_neighborhood"
        | "kb_add_link"
        | "kb_raw_query"
        | "command_list"
        | "editor_save_state"
        | "editor_restore_state"
        | "github_pr_status"
        | "ask_user"
        | "rename_file"
        | "ai_save"
        | "ai_load"
        | "create_plan"
        | "update_plan"
        | "save_memory"
        | "debug_state"
        | "read_messages"
        | "syntax_tree"
        | "switch_project"
        | "toggle_file_tree"
        | "audit_configuration"
        | "list_modules"
        | "format_buffer"
        | "run_build"
        | "run_test"
        | "spell_check"
        | "lookup_online"
        | "next_error"
        | "search_tools"
        | "keymap_query" => ToolTier::Core,
        // Everything else is extended
        _ => ToolTier::Extended,
    }
}

/// Classify a tool into its category for request_tools.
pub fn classify_tool_category(name: &str) -> Option<ToolCategory> {
    if name.starts_with("mcp_") || name.starts_with("collab_") {
        return Some(ToolCategory::Mcp);
    }
    // Decision #6 relocations. These are the `kb_` tools that grant, revoke,
    // or relax another principal's access to a knowledge base — raised to
    // `Privileged` because an authorization change is not an edit. That raise
    // makes them illegal members of `Knowledge`, which `is_read_flavoured`
    // declares read-flavoured and the invariant test below caps at `Write`:
    // an operator who allowlists "knowledge" for a note-taking agent must not
    // thereby hand it the ability to add a member to a shared KB.
    //
    // Relocated to `Mcp` (the peer-collaboration/external category) rather
    // than `Execution`, because that is what they are *about* — `collab_share`,
    // the buffer-level sibling of `kb_share`, already lives there via the
    // `collab_` prefix. Named explicitly rather than splitting the `kb_`
    // prefix wholesale, following ADR-085's own precedent for the
    // `kb_raw_query`/`kb_enrich` outliers a few branches down. The read-only
    // `kb_sharing_status` deliberately stays in `Knowledge`: it is
    // introspection, the invariant does not touch it, and moving it would take
    // "who are the members?" away from knowledge-scoped sessions for no
    // security gain.
    if super::authorization::is_authorization_change(name) {
        return Some(ToolCategory::Mcp);
    }
    if name.starts_with("lsp_") || name == "syntax_tree" {
        Some(ToolCategory::Lsp)
    } else if name.starts_with("dap_") || name == "debug_state" {
        Some(ToolCategory::Dap)
    } else if name.starts_with("babel_")
        || matches!(
            name,
            "org_export"
                | "kb_enrich"
                | "kb_register"
                | "kb_reimport"
                | "kb_raw_query"
                | "kb_export_subgraph_html"
        )
    {
        // ADR-085: subject-matter prefix rules (`babel_`, `kb_`, `org_`) put
        // these tools in Knowledge, but each is Shell/Privileged tier --
        // genuine code execution, arbitrary Datalog, filesystem
        // registration/reimport, or a shell-out to `npx` for mermaid
        // rendering. Knowledge is read-flavoured (`is_read_flavoured`), so
        // leaving them there would let `mcp_tool_category_allowlist =
        // "knowledge"` reach them -- the exact defect this ADR fixes for
        // `babel_*`. The `babel_` prefix moves wholesale (D5: every
        // babel_ tool is Shell-tier). The handful of `kb_`/`org_` outliers
        // are named explicitly rather than splitting those prefixes
        // wholesale, because the overwhelming majority of `kb_`/`org_`
        // tools are genuinely ReadOnly/Write -- this mirrors the existing
        // `shell_exec`/`ai_permissions` exact-name carve-outs a few
        // branches down, not a departure from the mechanical-prefix
        // design. The registry-wide invariant test below (`is_read_flavoured`
        // + tier <= Write) is what actually enforces this, not this comment.
        Some(ToolCategory::Execution)
    } else if name.starts_with("kb_") || name == "help_open" || name.starts_with("org_") {
        Some(ToolCategory::Knowledge)
    } else if name.starts_with("shell_") && name != "shell_exec" {
        Some(ToolCategory::ShellMgmt)
    } else if name.starts_with("command_") {
        Some(ToolCategory::Commands)
    } else if name.starts_with("git_") || name.starts_with("github_") {
        Some(ToolCategory::Git)
    } else if name.starts_with("web_") {
        Some(ToolCategory::Web)
    } else if name.starts_with("ai_") && name != "ai_permissions" {
        // ai_set_mode, ai_set_profile, ai_set_budget, ai_save, ai_load
        Some(ToolCategory::Ai)
    } else if name.starts_with("visual_buffer_") {
        Some(ToolCategory::Visual)
    } else if matches!(
        name,
        "delegate"
            | "save_memory"
            | "create_plan"
            | "update_plan"
            | "ask_user"
            | "log_activity"
            | "read_transcript"
            | "propose_changes"
    ) {
        Some(ToolCategory::Ai)
    } else if matches!(
        name,
        "theme_inspect" | "mouse_event" | "render_inspect" | "event_recording" | "trigger_hook"
    ) {
        Some(ToolCategory::Debug)
    } else {
        None
    }
}

/// Parse category names from a comma-separated string.
pub fn parse_categories(input: &str) -> Vec<ToolCategory> {
    input
        .split(',')
        .filter_map(|s| match s.trim().to_ascii_lowercase().as_str() {
            "lsp" => Some(ToolCategory::Lsp),
            "dap" => Some(ToolCategory::Dap),
            "knowledge" | "kb" => Some(ToolCategory::Knowledge),
            "execution" | "exec" => Some(ToolCategory::Execution),
            "shell" | "shell_mgmt" => Some(ToolCategory::ShellMgmt),
            "commands" | "command" => Some(ToolCategory::Commands),
            "git" | "github" => Some(ToolCategory::Git),
            "web" => Some(ToolCategory::Web),
            "ai" | "agent" => Some(ToolCategory::Ai),
            "visual" | "canvas" => Some(ToolCategory::Visual),
            "debug" | "profiling" => Some(ToolCategory::Debug),
            "mcp" | "external" => Some(ToolCategory::Mcp),
            _ => None,
        })
        .collect()
}

/// Build the `request_tools` meta-tool definition.
pub fn request_tools_definition() -> ToolDefinition {
    super::tool_def::ToolDefBuilder::new(
        "request_tools",
        "Request additional tools by category or specific name. Use search_tools first to discover tool names, then request them here. Categories: lsp, dap, knowledge, execution, shell, commands, git, web, ai, visual, debug, mcp.",
    )
    .prop(
        "categories",
        "string",
        "Comma-separated categories: lsp, dap, knowledge, execution, shell, commands, git, web, ai, visual, debug, mcp",
    )
    .prop("tools", "string", "Comma-separated tool names to add (e.g. from search_tools results)")
    .required(["categories"])
    .permission(PermissionTier::ReadOnly)
    .build()
}

/// Classify a command's permission tier based on its name.
pub fn classify_command_permission(name: &str) -> PermissionTier {
    match name {
        // Movement and read-only state changes
        n if n.starts_with("move-") => PermissionTier::ReadOnly,
        "enter-normal-mode"
        | "enter-insert-mode"
        | "enter-command-mode"
        | "enter-insert-mode-after"
        | "enter-insert-mode-eol" => PermissionTier::ReadOnly,

        // Editing commands
        n if n.starts_with("delete-") || n.starts_with("open-line-") => PermissionTier::Write,
        "undo" | "redo" => PermissionTier::Write,
        "save" | "save-and-quit" => PermissionTier::Write,

        // Dangerous operations
        "quit" | "force-quit" => PermissionTier::Privileged,

        // Authorization changes (decision #6). Placed AFTER the explicit arms
        // above so the list stays the single source of truth for this one
        // question and does not quietly reclassify anything else. Without
        // this, `command_kb_share` — generated from the registry and landing
        // on the `_ => Write` default below — is a Write-tier path to the
        // exact effect `kb_share` was raised to Privileged to gate, and it
        // needs no arguments: it shares the *active* KB.
        n if super::authorization::is_authorization_change(n) => PermissionTier::Privileged,

        // Default to Write for unknown commands
        _ => PermissionTier::Write,
    }
}

/// Mechanically derive MCP tool-annotation hints from a tool's
/// `PermissionTier` (ADR-050 D2). Returns `(read_only_hint, destructive_hint,
/// idempotent_hint)`. This is the single source of truth for the mapping --
/// never hand-author a tool's annotations elsewhere, since doing so per tool
/// across 700+ registered tools would be an unauditable drift risk (a false
/// `read_only_hint: true` on a mutating tool would make external clients
/// like VS Code's Copilot skip their own confirmation dialog on a real
/// write). `ReadOnly` tools are read-only and idempotent by construction;
/// `Write` tools mutate but are ordinary, reversible editing operations;
/// `Shell`/`Privileged` tools can perform effects MAE cannot reason about or
/// undo (arbitrary shell commands, host filesystem/network access), so both
/// are flagged destructive.
pub fn annotations_for_tier(tier: PermissionTier) -> (bool, bool, bool) {
    match tier {
        PermissionTier::ReadOnly => (true, false, true),
        PermissionTier::Write => (false, false, false),
        PermissionTier::Shell => (false, true, false),
        PermissionTier::Privileged => (false, true, false),
    }
}

/// Policy for auto-approving or prompting for tool calls.
#[derive(Debug, Clone)]
pub struct PermissionPolicy {
    /// Maximum tier that is auto-approved without user confirmation.
    pub auto_approve_up_to: PermissionTier,
    /// ADR-056: categories this session/instance is restricted to. `None`
    /// (default, backward compatible) = unrestricted. `Some(set)` = only
    /// tools whose `classify_tool_category` is in `set` may be dispatched.
    /// A tool with NO classified category (`classify_tool_category` returns
    /// `None` — notably `execute_command`, `shell_exec`) is DENIED when a
    /// restriction is active: fail-closed, not fail-open, since an
    /// uncategorized tool is exactly the case where the taxonomy hasn't made
    /// a judgment yet and this is a trust boundary. Orthogonal to
    /// `auto_approve_up_to` — tier answers "how mutating," category answers
    /// "which subsystem"; both gates must pass.
    pub allowed_categories: Option<std::collections::HashSet<ToolCategory>>,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        // Container-first: auto-approve up to Shell tier.
        PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
            allowed_categories: None,
        }
    }
}

impl PermissionPolicy {
    /// Check if a permission tier is auto-approved.
    pub fn is_allowed(&self, tier: PermissionTier) -> bool {
        tier <= self.auto_approve_up_to
    }

    /// Check if `tool_name` is allowed under this policy's category
    /// restriction (ADR-056). Always `true` when unrestricted.
    pub fn is_category_allowed(&self, tool_name: &str) -> bool {
        match &self.allowed_categories {
            None => true,
            // request_tools/search_tools are pure discovery (return JSON,
            // invoke nothing) -- exempt so a restricted session can still
            // see what it's missing. Escalation is still blocked: calling
            // the *discovered* tool re-enters this same check.
            Some(_) if matches!(tool_name, "request_tools" | "search_tools") => true,
            Some(set) => classify_tool_category(tool_name)
                .map(|c| set.contains(&c))
                .unwrap_or(false),
        }
    }
}

#[cfg(test)]
mod annotation_tests {
    use super::*;

    #[test]
    fn read_only_tier_is_read_only_and_idempotent_never_destructive() {
        let (read_only, destructive, idempotent) = annotations_for_tier(PermissionTier::ReadOnly);
        assert!(read_only);
        assert!(!destructive);
        assert!(idempotent);
    }

    #[test]
    fn write_tier_is_neither_read_only_nor_flagged_destructive() {
        let (read_only, destructive, idempotent) = annotations_for_tier(PermissionTier::Write);
        assert!(!read_only);
        assert!(!destructive);
        assert!(!idempotent);
    }

    #[test]
    fn shell_and_privileged_tiers_are_flagged_destructive_never_read_only() {
        for tier in [PermissionTier::Shell, PermissionTier::Privileged] {
            let (read_only, destructive, _) = annotations_for_tier(tier);
            assert!(!read_only, "{tier:?} must never be read_only_hint: true");
            assert!(
                destructive,
                "{tier:?} must be flagged destructive_hint: true"
            );
        }
    }

    /// Exhaustive consistency check across every `PermissionTier` variant:
    /// `read_only_hint` must be true if and only if the tier is `ReadOnly`.
    /// This is what makes the mapping in `annotations_for_tier` a genuine
    /// single source of truth rather than something that could silently
    /// drift from `PermissionTier` if a variant is ever added -- add the new
    /// variant to this array and the compiler/test forces the mapping to be
    /// considered.
    #[test]
    fn read_only_hint_is_exactly_read_only_tier() {
        for tier in [
            PermissionTier::ReadOnly,
            PermissionTier::Write,
            PermissionTier::Shell,
            PermissionTier::Privileged,
        ] {
            let (read_only, _, _) = annotations_for_tier(tier);
            assert_eq!(
                read_only,
                tier == PermissionTier::ReadOnly,
                "read_only_hint mismatch for {tier:?}"
            );
        }
    }
}

#[cfg(test)]
mod category_allowlist_tests {
    use super::*;
    use crate::tools::{ai_specific_tools, tools_from_registry};

    /// Every tool MAE actually registers -- the command-derived registry
    /// surface plus the AI-specific meta-tools -- mirrors the enumeration
    /// pattern `crates/ai/src/executor/mod_tests.rs::all_tools()` uses for
    /// ADR-050 D2's annotation-consistency audit. ADR-085 D4: tests in this
    /// module assert properties of the *whole* registry, not a hand-picked
    /// sample -- three cherry-picked tool names is exactly the "unicorn
    /// values" shape (principle #14) that let `babel_execute`/`babel_tangle`
    /// sit inside `Knowledge` undetected.
    fn all_tools() -> Vec<ToolDefinition> {
        let mut tools = tools_from_registry(&mae_core::CommandRegistry::with_builtins());
        tools.extend(ai_specific_tools(&mae_core::OptionRegistry::new()));
        tools
    }

    #[test]
    fn unrestricted_policy_allows_everything() {
        let policy = PermissionPolicy::default();
        assert!(policy.is_category_allowed("kb_search"));
        assert!(policy.is_category_allowed("execute_command"));
        assert!(policy.is_category_allowed("shell_exec"));
    }

    /// ADR-085 D2, the invariant that generalises the babel fix: a
    /// read-flavoured category (`ToolCategory::is_read_flavoured`) may never
    /// contain a tool declared above `Write` tier. Iterates the entire live
    /// tool registry -- not a sample -- so a future tool added under a
    /// knowledge-ish/lsp-ish/web-ish/debug-ish prefix that happens to be
    /// Shell/Privileged tier fails the build instead of silently becoming
    /// reachable from a `knowledge`-only (or `lsp`-only, `web`-only, ...)
    /// session. Do NOT add an allowlist/exception to make a future failure
    /// here pass -- fix the tool's classification instead (move it to a
    /// non-read-flavoured category, per ADR-085 D5/D6).
    #[test]
    fn read_flavoured_categories_never_exceed_write_tier() {
        let tools = all_tools();
        assert!(
            tools.len() > 100,
            "sanity check: expected hundreds of registered tools, got {}",
            tools.len()
        );

        let offenders: Vec<String> = tools
            .iter()
            .filter_map(|tool| {
                let category = classify_tool_category(&tool.name)?;
                if !category.is_read_flavoured() {
                    return None;
                }
                let tier = tool.permission?;
                (tier > PermissionTier::Write)
                    .then(|| format!("{} (category {category:?}, tier {tier:?})", tool.name))
            })
            .collect();

        assert!(
            offenders.is_empty(),
            "read-flavoured categories must not contain a tool above Write tier, found:\n{}",
            offenders.join("\n")
        );
    }

    /// The regression test for the specific bug ADR-085 fixes: a session
    /// allowlisted to `knowledge` must not be able to reach either babel
    /// tool. The fix is that they are never classified as `Knowledge` in the
    /// first place (not that they're offered and then separately refused by
    /// tier), so this asserts both the classification and the gate.
    #[test]
    fn knowledge_only_session_cannot_reach_babel_execution_tools() {
        let policy = PermissionPolicy {
            allowed_categories: Some([ToolCategory::Knowledge].into_iter().collect()),
            ..PermissionPolicy::default()
        };
        for tool in ["babel_execute", "babel_tangle"] {
            assert_ne!(
                classify_tool_category(tool),
                Some(ToolCategory::Knowledge),
                "{tool} must not classify as Knowledge (ADR-085)"
            );
            assert!(
                !policy.is_category_allowed(tool),
                "a knowledge-only session must not reach {tool}"
            );
        }
    }

    /// Positive case, derived from the live registry rather than three
    /// hand-picked names: every tool that actually classifies as `Knowledge`
    /// today is reachable under a `knowledge`-only restriction.
    #[test]
    fn knowledge_only_allows_every_registered_knowledge_tool() {
        let policy = PermissionPolicy {
            allowed_categories: Some([ToolCategory::Knowledge].into_iter().collect()),
            ..PermissionPolicy::default()
        };
        let knowledge_tools: Vec<_> = all_tools()
            .into_iter()
            .filter(|t| classify_tool_category(&t.name) == Some(ToolCategory::Knowledge))
            .collect();
        assert!(
            knowledge_tools.len() > 10,
            "sanity check: expected a healthy number of Knowledge tools, got {}",
            knowledge_tools.len()
        );
        for tool in &knowledge_tools {
            assert!(
                policy.is_category_allowed(&tool.name),
                "{} classifies as Knowledge but is denied under a knowledge-only restriction",
                tool.name
            );
        }
    }

    #[test]
    fn knowledge_only_denies_a_wrong_category_tool() {
        let policy = PermissionPolicy {
            allowed_categories: Some([ToolCategory::Knowledge].into_iter().collect()),
            ..PermissionPolicy::default()
        };
        assert!(!policy.is_category_allowed("git_push"));
        assert!(!policy.is_category_allowed("lsp_hover"));
    }

    // The highest-value bypass: an uncategorized tool (classify_tool_category
    // returns None) must fail CLOSED under a restriction, not open.
    #[test]
    fn knowledge_only_denies_uncategorized_tools_fail_closed() {
        let policy = PermissionPolicy {
            allowed_categories: Some([ToolCategory::Knowledge].into_iter().collect()),
            ..PermissionPolicy::default()
        };
        assert_eq!(
            classify_tool_category("execute_command"),
            None,
            "sanity: must be uncategorized"
        );
        assert!(!policy.is_category_allowed("execute_command"));
        assert_eq!(
            classify_tool_category("shell_exec"),
            None,
            "sanity: must be uncategorized"
        );
        assert!(!policy.is_category_allowed("shell_exec"));
    }

    #[test]
    fn discovery_tools_stay_reachable_under_any_restriction() {
        let policy = PermissionPolicy {
            allowed_categories: Some([ToolCategory::Knowledge].into_iter().collect()),
            ..PermissionPolicy::default()
        };
        assert!(policy.is_category_allowed("request_tools"));
        assert!(policy.is_category_allowed("search_tools"));
    }

    /// `ToolCategory::ALL` exists so registry-wide invariants (like the two
    /// tests above) can iterate rather than depend on someone remembering to
    /// extend a hand-written list -- so `ALL` itself needs a check that
    /// actually catches a missing/duplicated entry. A test that merely loops
    /// `for c in ToolCategory::ALL { match c { ... } }` would NOT catch a
    /// variant missing from `ALL`: the loop simply never visits what isn't
    /// there, so the match is vacuously exhaustive over an incomplete set.
    /// Instead, the array literal below is independent of `ALL` and is
    /// checked against the real enum by the exhaustive `match` inside the
    /// loop -- adding a `ToolCategory` variant without adding it here fails
    /// to *compile*, and the `ALL.contains` + length assertions then catch a
    /// variant present in the enum but missing (or duplicated) in `ALL`.
    #[test]
    fn tool_category_all_covers_every_variant_exactly_once() {
        let every_variant = [
            ToolCategory::Lsp,
            ToolCategory::Dap,
            ToolCategory::Knowledge,
            ToolCategory::Execution,
            ToolCategory::ShellMgmt,
            ToolCategory::Commands,
            ToolCategory::Git,
            ToolCategory::Web,
            ToolCategory::Ai,
            ToolCategory::Visual,
            ToolCategory::Debug,
            ToolCategory::Mcp,
        ];
        for category in every_variant {
            // Exhaustive match over the REAL enum: adding a `ToolCategory`
            // variant that isn't listed in `every_variant` above leaves this
            // match non-exhaustive, so the test file fails to compile until
            // both the array above and this match are extended.
            match category {
                ToolCategory::Lsp
                | ToolCategory::Dap
                | ToolCategory::Knowledge
                | ToolCategory::Execution
                | ToolCategory::ShellMgmt
                | ToolCategory::Commands
                | ToolCategory::Git
                | ToolCategory::Web
                | ToolCategory::Ai
                | ToolCategory::Visual
                | ToolCategory::Debug
                | ToolCategory::Mcp => {}
            }
            assert!(
                ToolCategory::ALL.contains(&category),
                "{category:?} is a real ToolCategory variant but is missing from \
                 ToolCategory::ALL -- registry-wide invariant tests silently skip it"
            );
        }
        assert_eq!(
            ToolCategory::ALL.len(),
            every_variant.len(),
            "ToolCategory::ALL has a different number of entries than there are real \
             variants (duplicate or stale entry) -- update ALL to match exactly"
        );
    }
}
