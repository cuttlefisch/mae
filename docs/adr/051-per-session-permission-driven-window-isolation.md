# ADR-051: Per-session permission policy & per-session `DrivenWindow` isolation

**Status:** Proposed.
**Extends:** ADR-001 (server-client protocol), CLAUDE.md principle #10 (multi-client
safety by design).
**Closes acknowledged drift:** CLAUDE.md's own "Architecture Debt" language already flags
"two simultaneous MCP clients still share one driven window — candidate for its own ADR."
Per principle #15 (bugs are drift signals), this ADR is that ADR — a promised-but-undelivered
gap being closed, not new scope.
**Tracking:** issue #375 (epic tracker); phase issue #378.

## Context

Multiple concurrent MCP clients attached to one MAE process is about to become the normal
case, not an edge case — a human's own `mae-agent`/embedded tooling plus VS Code Copilot
plus potentially other editors, all against the same running instance (or the same
project's headless instance, per ADR-055). Two gaps make this currently unsafe:

**Gap 1 — permission tiers are process-global.** `PermissionPolicy{auto_approve_up_to}`
(`crates/ai/src/tools/categories.rs`) is computed **once** at startup
(`crates/mae/src/config.rs::resolve_permission_policy`) and threaded as one shared
reference into every dispatch site, ultimately enforced in the single chokepoint
`execute_tool_with_requester` (`crates/ai/src/executor/tool_dispatch.rs:48-58`). There is
no code path that grants a different tool-approval ceiling to, say, a fully-trusted local
session versus a newly-paired external client. Confirmed by direct code read: zero
reference to `ClientSession`/`RequesterContext`/`declared_ai_provider` anywhere in the
tier-check path.

**Gap 2 — `DrivenWindow` is shared process-wide.** `AiState.work_window: DrivenWindow`
(`crates/core/src/editor/ai_state.rs:72`) is a single field on the single process-wide
`Editor`, not keyed by MCP session. `Editor::with_ai_dispatch_scope`/
`ensure_ai_dispatch_target` (`crates/core/src/editor/window_ops.rs:458-503`) enforces this
for **every** MCP tool dispatch regardless of which `ClientSession` issued it. This means
one MCP client's dispatch can never steal the sole visible window from a human (that
specific bug, issue #372, is already fixed) — but two simultaneous MCP clients today
**share the exact same companion window**, so their tool-driven buffer navigation can
visually interleave.

Separately-confirmed research finding: this window bookkeeping is renderer-independent
(`Renderer::render(&mut Editor)` only ever reads editor state, never mutates it) and stays
fully meaningful in headless mode (ADR-055) — it still governs which buffer an AI tool
response targets even with nothing on screen. So this gap is **not** made moot by headless
operation; it is real, load-bearing work regardless of whether a display is attached.

## Decision

1. **Per-session `PermissionPolicy`.** Move from one process-global value to a field on
   `ClientSession` (`shared/mcp/src/session.rs:29-74`), defaulted from the existing global
   config, override-able per connection (e.g. a stricter ceiling for a newly-paired
   external client via the same declared-capability precedent `ClientSession.declared_ai_provider`
   already establishes, ADR-048). **A session policy may only be tightened, never loosened**
   below what ADR-018's KB-role check would independently permit for that principal — this
   stays strictly additive to, not a bypass of, existing RBAC.
2. **Per-session `DrivenWindow`.** Move `AiState.work_window` from one process-wide field
   to a map keyed by the already-unique `ClientSession.id`, so each connected client gets
   its own companion window and never observes or disrupts another session's.
3. **Session identity is sufficient as the isolation key.** The same human running two
   simultaneous connections (their own tooling + VS Code Copilot) is exactly the intended
   case for two independent entries — no additional principal-level bucketing is needed
   for this ADR's scope (principal-level concerns belong to ADR-018/052, not here).
4. **Daemon is explicitly out of scope for this ADR.** Research confirmed the daemon holds
   no mutable "current window/buffer" state at all — it's a CRDT sync engine plus a query
   service — so there is structurally nothing to isolate there; this ADR is scoped to the
   editor's MCP server only.

## Consequences

**Positive.** Closes a real, already-acknowledged gap before it becomes user-visible at
scale (multiple concurrent editor frontends is the explicit near-term scenario this whole
initiative targets). Keeps the fix additive to existing RBAC rather than introducing a
second, parallel authorization concept.

**Costs (honest).** Every call site currently receiving `&PermissionPolicy`/reading
`AiState.work_window` needs to thread a session identifier instead — a mechanical but
wide-reaching change across `crates/ai/src/executor/tool_dispatch.rs`,
`crates/core/src/editor/window_ops.rs`, and their callers. Must be done without breaking
the existing single-session (interactive human, no MCP client) case, which has no
`ClientSession` at all today — needs a well-defined default/synthetic session for that
path.

## Alternatives rejected

- **Leave permission policy process-global and rely on the client-side confirmation
  dialog as the differentiator.** Rejected — a client can set "always allow" in its own UI
  (see ADR-050's verification note), making the server-side gate the *only* real
  enforcement; a process-global policy can't express "this session gets less trust" at
  all.
- **Key `DrivenWindow` isolation on principal instead of session.** Rejected for this
  ADR's scope — two connections from the same principal (e.g. a human's own tooling
  reconnecting) legitimately want independent windows too; principal-level policy
  questions belong to ADR-052's identity-mapping work, not window bookkeeping.

## Verification

- N≥3 simultaneous MCP sessions (mix of permission tiers) running a real concurrent test:
  no session's window-targeting or tool-approval state is ever observed to leak into
  another's — an adversarial "confused deputy" style test, not just a happy-path 2-client
  check.
- The existing `multi_client_concurrent_connections` test
  (`shared/mcp/src/lib.rs:1164-1294`) is extended to assert per-session isolation of both
  permission policy and driven-window state, not just independent connection lifecycle.
- A single-session (no MCP client) interactive-human path is confirmed unaffected —
  existing `self_test_suite` and Scheme test suites stay green.
