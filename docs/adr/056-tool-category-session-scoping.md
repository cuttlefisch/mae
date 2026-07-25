# ADR-056: `ToolCategory` session-scoped dispatch enforcement

**Status:** Accepted.
**Extends:** ADR-051 (per-session `PermissionPolicy` & `DrivenWindow` isolation — this ADR
adds an orthogonal `ToolCategory` axis to the same session-declaration mechanism
(`ClientSession.declared_permission_ceiling`/`RequesterContext`), not a competing
authorization system). **Relates to:** ADR-055 (headless MAE as an "engine" instance — the
concrete motivating use case for a global, config-driven restriction), ADR-050 D2 (the
mechanically-derived `ToolCategory` taxonomy this ADR reuses unmodified).
**Tracking:** issue #375 (epic tracker, closed-out pass).

## Context

`mae --headless` is already a correctly-architected, GUI-free "engine" process (ADR-055) —
confirmed via direct research into its boot path: no `Renderer` is ever constructed, and
the KB+guidance tool surface an external editor's AI agent needs only ever touches
`editor.kb`/`editor.project`/`editor.git_or_project_root()`, none of it display-dependent.
What's missing is a way to make that engine *only* serve KB+guidance operations to a
connected MCP session.

**The gap, precisely.** `ToolCategory` and `classify_tool_category()` already exist
(`crates/ai/src/tools/categories.rs`), and `ToolCategory::Knowledge` already covers
exactly the intended surface (`kb_*`, `help_open`, `org_*`, `babel_*`, including
`kb_export_guidance`). `parse_categories()` already parses a comma-separated category
string — but today it's consumed **only** by the `request_tools` meta-tool, which affects
what's *advertised* in `tools/list`. Dispatch itself — `execute_tool_dispatch_body`
(`crates/ai/src/executor/tool_dispatch.rs`), confirmed the single enforced MCP/AI dispatch
chokepoint (the same function this session's Scheme-command permission-bypass fix lives
in) — only checks `PermissionTier` (`policy.is_allowed(permission)`). A connected client
that already knows a tool name (or discovers it via `request_tools`/`search_tools`, which
is deliberately not gated behind advertisement) can call **any** of MAE's ~700+ tools
regardless of tiering. A headless instance configured as a KB+guidance-only engine cannot
actually stop a paired agent from calling `shell_exec`, `git_push`, or `execute_command`.

**Why this matters now, not hypothetically.** Once an external editor's AI agent (VS Code
Copilot, or any other MCP client) is paired with a headless MAE instance, the tiering
mechanism (`mcp_tools_tiered_by_default`) that already exists is advertisement-only —
exactly the class of gap CLAUDE.md principle #3 (no separate "AI mode") and the standing
"client confirmation dialog is not a security boundary" finding (ADR-050's own
verification note) already flag: MAE's server-side gate is the *only* real enforcement,
and today there is no server-side gate for "which tool categories," only "which
permission tier."

**Explicitly out of scope.** The KB-federation eager-load-of-all-registered-instances cost
found during the same research pass (`bootstrap.rs`, every registered federated KB opened
synchronously at boot) applies identically to GUI/TUI/headless boot alike — an unrelated
boot-latency question, not a session-scoping concern. Tracked separately, not folded in
here.

## Decision

1. **Extend `PermissionPolicy` with an orthogonal `allowed_categories` field**
   (`crates/ai/src/tools/categories.rs`) rather than building a parallel authorization
   mechanism (principle #8) — `PermissionPolicy` is already the single source of truth
   threaded through `ClientSession`/`RequesterContext`/`effective_permission_policy` into
   the one enforced dispatch chokepoint. `None` (default) = unrestricted, fully backward
   compatible. `Some(set)` restricts dispatch to tools whose `classify_tool_category` is
   in `set`.
2. **Fail-closed for uncategorized tools.** A tool `classify_tool_category` returns `None`
   for (notably `execute_command`, `shell_exec`) is **denied** when a restriction is
   active — an uncategorized tool is exactly the case the taxonomy hasn't judged yet, and
   this is a trust boundary, not a place to default open. `execute_command` specifically
   is the primary adversarial-test target: it's the highest-value bypass a restricted
   session would try first (it can indirectly reach almost anything else via a registered
   Scheme/Rust command).
3. **`request_tools`/`search_tools` stay reachable under any restriction.** They are pure
   discovery (return JSON describing tools, invoke nothing) — a restricted session can
   still see what it's missing. The escalation attempt itself is still blocked: calling
   the *discovered* tool re-enters the same dispatch chokepoint and is denied there.
4. **Orthogonal to, not merged with, `PermissionTier`.** Tier answers "how mutating";
   category answers "which subsystem." Both gates run at the same chokepoint (tier check,
   then category check) and both must pass — a Knowledge-only + ReadOnly session still
   correctly rejects `kb_delete` (in-category, wrong tier).
   **Correction found while writing this ADR's own required adversarial test**
   (principle #15 — record, don't silently drift): `execute_tool_dispatch_body` is *not*
   the only enforced path. `handle_mcp_request`'s Scheme-sourced-command bridge
   (`crates/mae/src/ai_event_handler.rs`, the same bridge this session's earlier
   Scheme-command permission-bypass fix targets) matches and dispatches Scheme-sourced
   commands *before* ever falling through to `execute_tool_with_requester`/
   `execute_tool_dispatch_body` — it is a second, independent chokepoint with its own
   `PermissionTier` check, and the category check must be duplicated there too, not
   assumed to compose "for free" via a single chokepoint. Both checks now live at both
   sites, same fail-closed semantics.
5. **Declarable per-session (mirrors `declared_permission_ceiling`) and per-instance
   (config-driven, principle #7).** Per-session: a new `toolCategoryAllowlist` `initialize`
   param, `ClientSession.declared_tool_categories`, forwarded through
   `mae-mcp-shim` via a new `MAE_MCP_TOOL_CATEGORY_ALLOWLIST` env var — identical shape to
   the already-proven `permissionCeiling`/`MAE_MCP_PERMISSION_CEILING` mechanism. Global:
   a new `mcp_tool_category_allowlist` `OptionRegistry` entry, read once at boot
   (`crates/mae/src/main.rs`) alongside the existing `mcp_tools_tiered` read, seeding the
   server's global policy before any session connects. **The effective policy is always
   the intersection of global and per-session declarations** — a session can only ever
   further restrict itself, never escalate past what the instance-wide config already
   allows, and the instance-wide config is exactly the mechanism that makes a headless
   engine's "KB+guidance only" intent structurally enforced rather than aspirational.

## Consequences

**Positive.** Closes the one real gap standing between "headless mode boots without a
GUI" (already true) and "a headless instance can be safely paired with an external agent
and *actually* restricted to KB+guidance operations" (not true before this ADR). Reuses
100% existing taxonomy/parsing/session-declaration infrastructure — no new authorization
concept, no new config surface shape unfamiliar to anyone who already understands
`permissionCeiling`.

**Costs (honest).** A second orthogonal axis on `PermissionPolicy` is one more thing a
future contributor must reason about when adding a new tool — every new tool must get a
correct (or intentionally absent) `classify_tool_category` classification, same discipline
already required for `classify_tool_tier`. The fail-closed default for uncategorized tools
means a tool that's genuinely safe under a category restriction but not yet classified
will be denied until someone adds it to `classify_tool_category` — a deliberate
conservative default, not a bug, but worth naming as friction.

## Alternatives rejected

- **A separate, parallel allowlist mechanism outside `PermissionPolicy`.** Rejected — would
  either need its own chokepoint (duplicating the `ClientSession`/`RequesterContext`/
  `effective_permission_policy` wiring `PermissionPolicy` already has) or bolt awkwardly
  onto the existing one anyway. Better to add one field to the type that's already the
  single source of truth for "what can this session do."
- **Advertisement-only enforcement (extend `mcp_tools_tiered_by_default` further).**
  Rejected — this is exactly the status quo gap. Advertisement controls what a client sees
  by default; it does not and structurally cannot stop a client that already knows (or
  discovers via `request_tools`) a tool name outside the advertised set.
  `mcp_tools_tiered_by_default` and `mcp_tool_category_allowlist` solve different problems
  and both stay in place, unmerged.
- **Fail-open for uncategorized tools.** Rejected — would silently grant a category-
  restricted session access to any tool the taxonomy hasn't classified yet, including
  every future tool added without an explicit category, defeating the restriction's whole
  purpose over time as the tool surface grows.

## Verification

- A Knowledge-only session **must be denied** `execute_command`, `shell_exec`, `git_push`,
  `buffer_write` — the uncategorized/wrong-category fail-closed cases, verified via a real
  `execute_tool_dispatch_body`/`handle_mcp_request` call returning a genuine failure
  result, not documented intent.
- The same session **must be allowed** `kb_search`, `kb_export_guidance`, `help_open` —
  proving the allowlist isn't accidentally blocking everything.
- Global-config restriction plus a looser per-session declaration still enforces the
  tighter global value (intersection, not override) — mirrors the existing
  `effective_permission_policy` tier tests' shape, extended to categories.
- Composition case: a Knowledge-only session calling `execute_command` naming a real
  Scheme-sourced command is denied by the Scheme-bridge's OWN category check (added
  alongside its existing tier check, per the correction above — this call never reaches
  `execute_tool_dispatch_body` at all, since the bridge branch is matched first) —
  proves the category check was actually duplicated at both chokepoints, not assumed to
  compose for free through one shared code path.
- At least one denial test is verified to genuinely fail without the fix (temporarily
  neutering `is_category_allowed`'s call site, confirming the test catches it, reverting) —
  matches this session's established verify-both-directions discipline.

## Verification note (evidence, moved to Accepted)

All six tests above are implemented in `crates/mae/src/ai_event_handler.rs`'s test module:
`knowledge_only_session_denies_execute_command`,
`knowledge_only_session_denies_shell_exec_git_push_buffer_write`,
`knowledge_only_session_allows_knowledge_tools_through_the_gate`,
`global_category_restriction_is_not_widened_by_a_looser_session_declaration`, and
`knowledge_only_session_denies_execute_command_naming_a_scheme_sourced_command`, plus 5 unit
tests in `crates/ai/src/tools/categories.rs`'s `category_allowlist_tests` module. **Both**
independent chokepoints (`execute_tool_dispatch_body`'s step 2b, and the Scheme-command
bridge's own check in `handle_mcp_request`) were verified to genuinely fail when their
respective check was temporarily neutered — the Scheme-bridge check existing at all is
itself a direct product of that verification catching the gap the Decision-4 correction
above describes. `cargo fmt --check`/`cargo clippy --workspace --all-targets -- -D
warnings`/`cargo test --workspace` clean across the editor workspace.
