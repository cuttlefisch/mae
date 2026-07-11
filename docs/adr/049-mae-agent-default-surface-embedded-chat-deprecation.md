# ADR-049: `mae-agent` as the default AI interaction surface; embedded chat gated behind a flag

**Status:** Accepted.
**Supersedes (in part):** ADR-046's decision to reject deprecating the embedded conversation
window. ADR-046's freeze-embedded/invest-in-CLI-harness decision otherwise stands unchanged.
**Depends on:** ADR-045 (provider-parity harness), ADR-046 (the CLI/MCP-shim surface this ADR
promotes to default).

## Context

ADR-046 (Proposed, written before the harness existed in its current form) explicitly rejected
deprecating the embedded conversation window: *"no migration cost or risk is justified given
current users are already well-served by it; freezing costs nothing and preserves optionality."*
At the time that was the right call — the CLI harness (`mae-agent`, `crates/agent-cli`) was net
new and unproven.

Since then, the harness has matured substantially within this session's work: a real permission-
tier enforcement fix (the MCP protocol previously never transmitted tool tiers, so every external
client silently treated every tool as `Write`-tier — fixed by transmitting `permission` in
`ToolInfo`, see `shared/mcp/src/protocol.rs`), a `NonInteractiveExecutor` guardrail layer, stage-
policy tracking, 70+ tests, and live verification against multiple real Ollama models (with one
model's real destructive `shell_exec` pushback failure caught, root-caused, and fixed in the
process — see `docs/MODEL_SUPPORT.md`). The user made an explicit, direct decision to flip the
default given this evidence: make `mae-agent` the default AI-interaction surface, and gate the
embedded chat behind an opt-in flag rather than removing it outright.

This is narrower than a full reversal of ADR-046: that ADR's core decision (freeze the embedded
window's feature growth, build new agent-surface work on the CLI/MCP-shim harness) is unaffected.
What changes is only the "kept on and default" question ADR-046 answered when it rejected
deprecation — the calculus behind that specific sub-decision has changed, not the overall
CLI-harness-first strategy.

**Key implementation finding:** MAE already had a fully-wired, tested command for launching an
external agent CLI inside an embedded terminal — `open-ai-agent` (`SPC a a`,
`crates/core/src/editor/dispatch/ui.rs`) spawns whatever command is configured via the `ai_editor`
option as an auto-run child process inside a real terminal buffer (`Buffer::new_shell` +
`agent_shell = true`, which already has auto-close-on-exit and correct window-placement behavior).
It already generically reads `ai_editor` with no MAE-side knowledge of what the command is. So
making `mae-agent` the default required no new terminal-spawning infrastructure — only flipping
`ai_editor`'s default and redirecting the old embedded-chat entry point to the same mechanism.

## Decision

1. **`ai_editor` now defaults to `"mae-agent"`** (was `"claude"`). `SPC a a` / `open-ai-agent`
   launches the harness by default; users can still set `ai_editor` to `claude`, `aider`, or any
   other CLI.
2. **New option `ai_chat_enabled` (Bool, default `false`)** gates the embedded conversation-buffer
   chat (`open_conversation_buffer()`, the custom `Conversation` window/rendering path). Default
   off means the harness is the real out-of-the-box experience on this branch, not a soft rollout.
3. **`ai-prompt`/`ai-chat` (`SPC a p`) redirects transparently** to the same agent-shell mechanism
   as `open-ai-agent` when `ai_chat_enabled` is false, rather than dead-ending or printing a
   message — muscle memory on the old binding still does something useful. Setting
   `:set ai_chat_enabled=true` restores the exact original behavior (conversation buffer +
   not-configured guidance), proving this is a default flip with an escape hatch, not a removal.
4. **Shared infrastructure is untouched.** `BufferKind::Conversation`, `conversation.rs`'s
   `Conversation` type, `render_common/spans.rs`'s `highlight_spans_for_buffer` dispatch, and
   `ai_event_handler.rs`'s `handle_ai_event` all remain exactly as they are — they're also used by
   `delegate()` sub-agent monitor buffers, which are unaffected by this change. Only the primary
   chat's *entry point* is gated; the buffer type and its renderer are not deprecated in the sense
   of being removed or frozen further than ADR-046 already froze them.

## Consequences

**Positive.** New users get the more reliable, model-agnostic, actively-developed surface by
default. No code duplication — the redirect calls the exact same `open_ai_agent_shell()` method
`open-ai-agent` already used. Existing users/configs that rely on the embedded chat are one
`:set-save ai_chat_enabled=true` away from unchanged behavior; nothing is deleted.

**Costs (honest).** Anyone who never customized `ai_editor` and liked the embedded chat now needs
to discover and flip `ai_chat_enabled` after upgrading — a one-time behavior change on default
config, not a breaking removal. The embedded chat's `render_common` gap noted in ADR-046 remains
unresolved (still out of scope; it is now doubly deprioritized since it is no longer even the
default path).

## Alternatives rejected

- **Hard-remove the embedded chat.** Rejected — `conversation.rs`/`BufferKind::Conversation` is
  shared infrastructure with `delegate()` sub-agent monitor buffers; removing the *feature* would
  require either duplicating that infrastructure or breaking delegate monitoring. Gating the entry
  point achieves the user's goal (harness is what people see by default) without that cost.
- **Default `ai_chat_enabled` to true (opt-out later, after a deprecation period).** Rejected per
  explicit user decision — the ask was for `mae-agent` to be the default *now*, on this branch,
  not a phased soft-launch.
- **Leave `ai_editor` defaulting to `claude` and only change `SPC a p`'s redirect target.**
  Rejected — would leave `SPC a a` (the more discoverable, already-documented entry point)
  pointing at a third-party CLI by default, contradicting the "make the harness the default"
  goal.

## Verification

- Fresh default config (`ai_editor` unset, `ai_chat_enabled` unset): `SPC a p` and `SPC a a` both
  spawn the `mae-agent` TUI in an embedded terminal buffer, not the old conversation buffer.
- `:set ai_chat_enabled=true` then `SPC a p`: restores the original embedded conversation-buffer
  behavior, including the not-configured guidance text when AI isn't set up — the escape hatch
  is a real, working code path, not just a flag with no effect.
- `delegate()` sub-agent monitor buffers (which reuse `Conversation`/`BufferKind::Conversation`)
  are unaffected — this ADR gates only the primary-chat *entry point*, not the shared buffer type
  or its renderer.
- Unit tests cover the new option's default value and the `ai-prompt` redirect dispatch behavior
  (`crates/core/src/editor/tests/`).
