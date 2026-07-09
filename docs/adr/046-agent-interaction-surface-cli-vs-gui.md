# ADR-046: Agent interaction surface — CLI/MCP-shim vs. embedded GUI window

**Status:** Proposed.
**Extends:** ADR-045 (the harness this CLI surface runs local models behind).
**Relates to:** `mae-mcp-shim` (`shared/mcp/src/shim.rs`) — this ADR formalizes a decision that
was already de facto true of the shim's design, but never written down.

## Context

MAE has two viable AI-integration surfaces today, and the "hard part" — tool execution and
permission enforcement — is already **100% shared** between them: both the embedded-agent path
and the MCP-shim path call the exact same `execute_tool()`
(`crates/ai/src/executor/tool_dispatch.rs`), the same tool registry, the same `PermissionTier`
enforcement, the same undo tracking (`crates/mae/src/main.rs:1736-1741` builds one shared
`all_tools` Vec consumed by both integration paths).

Where they differ is entirely at the UI layer:

- **Embedded conversation window** (`crates/core/src/conversation.rs`, 1780 lines +
  `crates/gui/src/conversation_render.rs`, 74 lines + a 5-line TUI stub). It is **not** built on
  the shared `render_common` layer that every other MAE UI surface uses (git-status, kb,
  notifications, debug, agenda, file_tree all have one) — a partial violation of principle #8.
  `ConversationPair` is special-cased through the window/split system, session save/restore, and
  mode dispatch. Across its git history: 36 commits touch these files, **8+ of them dedicated
  bugfixes** (scroll desync, split-system interaction, cursor/viewport unification, a division-
  by-zero) — a real, ongoing maintenance tax concentrated entirely in this UI layer.
- **`mae-mcp-shim`** (368 lines) is a tiny, fully generic MCP JSON-RPC-over-stdio ↔ Unix-socket
  bridge. Nothing in its protocol handling is Claude-Code-specific — `clientInfo.name` is logged,
  never gated on. A CLI harness for local models could connect through this exact shim today and
  get identical tool access, with **zero** new MAE-side code required.

The user's own experience already validates the second path in practice: driving MAE's 135+
editor tools via Claude Code CLI + `mae-mcp-shim` "has been immensely successful," and "the tui
functionality integrates well with mae without the overhead of the extensive ui work." The open
strategic question this ADR resolves is whether to keep growing the embedded window to reach
provider parity (including local models), or to invest new agent-surface work in a CLI harness
instead.

No interactive human-in-the-loop permission-approval UI exists on **either** surface today —
enforcement is a static tier-check at tool-dispatch time on both paths, not a live prompt. This
ADR does not change that; it is out of scope here.

## Decision

**Freeze embedded-window feature growth. Invest new agent-surface work in a CLI harness built on
`mae-mcp-shim`.**

Concretely:

- The embedded conversation window continues to receive bugfixes for its current feature set,
  serving current Claude/hosted-API users exactly as it does today. No rip-and-replace, no forced
  migration, no regression risk to existing users.
- All new local-model/harness work (ADR-045's guardrail layer, the reliability-tiered model
  support, the KB-enrichment lifecycle work, multi-agent orchestration in ADR-047) is built as a
  **new, minimal, terminal-native CLI harness** on top of `mae-mcp-shim`: its own tool-calling
  loop, talking to MAE purely as an MCP tool backend, with the ADR-045 guardrail layer sitting
  inside the CLI harness (not inside MAE's core).
- This CLI harness is available as a lighter-weight option for *any* provider going forward, not
  just local models — it is simply the cheaper place to build new agent-surface features from
  here on.

## Consequences

**Positive.** Zero new maintenance burden on the already bug-prone conversation-buffer UI.
Local-model users get a working agent experience without waiting on embedded-GUI parity work that
would otherwise need to touch render_common integration, split-system interaction, and session
save/restore just to catch up. Existing Claude/hosted-API embedded-window users see no change.
New agent-surface features (guardrails, multi-agent orchestration) are built once, outside the
UI-layer maintenance tax entirely.

**Costs (honest).** Local-model users do not get MAE's in-editor chat history/window UX under
this decision — they get an external terminal chat loop instead. Accepted given the user's own
validated experience that the CLI+shim path already works well without the UI investment. The
embedded window's `render_common` gap (principle #8 violation) is *not* fixed by this ADR — it is
explicitly deprioritized, not resolved, since the window is frozen rather than actively developed.

## Alternatives rejected

- **Continue embedded-window parity work for all providers**, including local models. Rejected —
  this duplicates the harness work ADR-045 already centralizes at the CLI-shim layer, and grows
  the single largest existing bugfix surface in the AI-integration code for a UX benefit
  (in-editor chat history) the user has not asked to prioritize.
- **Deprecate the embedded window outright.** Rejected per explicit user choice — no
  migration cost or risk is justified given current users are already well-served by it; freezing
  costs nothing and preserves optionality.
- **Fix the `render_common` gap now, as part of this ADR.** Rejected — it is real technical debt,
  but fixing it does not unblock any Ollama-parity work (the CLI harness bypasses the embedded
  window entirely), so it stays a known, tracked gap rather than blocking-scope here.

## Verification

- A local model can complete a full tool-calling session (list tools → call a tool → receive
  result → respond) through the new CLI harness + `mae-mcp-shim`, exercising the same
  `execute_tool()` path and `PermissionTier` enforcement as the embedded window, with no MAE-core
  changes required to add this surface.
- Existing embedded-window sessions (Claude/hosted-API) show no behavior change — session
  save/restore, split-system interaction, and scroll behavior are unaffected by the new CLI
  harness's existence.
- The CLI harness's tool-calling loop is a separate process/binary from MAE's core — a crash or
  infinite retry loop in the harness cannot corrupt or hang the embedded editor, the concrete
  isolation benefit this decision buys.
