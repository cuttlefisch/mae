# ADR-050: External-editor MCP pairing — VS Code/Copilot & cross-editor compatibility

**Status:** Proposed.
**Extends:** ADR-001 (server-client protocol), ADR-046 (`mae-mcp-shim` is already a fully
generic, provider-agnostic stdio↔Unix-socket bridge — "nothing in its protocol handling is
Claude-Code-specific").
**Relates to:** ADR-035 (editor↔daemon boundary — a paired external editor is still bound
by `daemon_mode`), ADR-049 (`mae-agent`/CLI-first stance — this ADR is the externally-facing
arm of the same "more external-client integrations are the intended direction" trajectory).
**Depends on:** ADR-055 (headless mode — the extension-managed pairing path spawns a
headless instance, never a GUI window).
**Tracking:** issue #375 (epic tracker).

## Context

MAE's MCP server already lets Claude Code attach as an external tool-calling client via
`mae-mcp-shim`. The next step is making MAE a general-purpose MCP backend for *any*
editor's AI agent — starting with VS Code + GitHub Copilot's agent mode — so Copilot's
tool calls are steered by MAE's KB search/CRUD and dev-guidance-KB mechanism, with VS Code
as the human's editing surface and MAE running invisibly underneath.

Research into VS Code Copilot's current MCP client behavior (mid-2026) found:
- MCP tools only activate in Copilot's **Agent mode** (not Ask/Edit).
- Config lives in `.vscode/mcp.json` (workspace-shareable, **JSONC** — comments are legal)
  or user profile; supports `command`+`args` (stdio) or `url`+`headers` (remote HTTP, SSE
  fallback), an `oauth` config object, an `inputs` array for secrets, and a `sandbox`
  object (macOS/Linux) for filesystem/network restriction.
- VS Code's MCP client supports tool **`annotations`** (`title`, `readOnlyHint`, and by
  extension `destructiveHint`/`idempotentHint`) — `readOnlyHint: true` bypasses the
  confirmation dialog for safe reads. MAE's wire protocol sends **none of these** today —
  confirmed by direct read of `shared/mcp/src/protocol.rs:171-187`, where `ToolInfo` has
  only `name`/`description`/`input_schema`/a custom `permission` string. Without this,
  every MAE tool call — including harmless KB reads — triggers a confirmation prompt in
  VS Code, which would make the pairing unusably noisy.
- Extensions register MCP servers programmatically via
  `vscode.lm.registerMcpServerDefinitionProvider` (`McpServerDefinitionProvider`,
  `provideMcpServerDefinitions`/`resolveMcpServerDefinition`/
  `onDidChangeMcpServerDefinitions`), returning an `McpStdioServerDefinition` or
  `McpHttpServerDefinition`.
- GitHub Copilot also has its own custom-instructions convention independent of MCP:
  `AGENTS.md`, `.github/copilot-instructions.md`, `.github/instructions/**.instructions.md`
  (with an `applyTo` glob).

Separately, MAE already has a transport-agnostic guidance mechanism
(`crates/ai/src/guidance.rs`, `ai_guidance_kb` option) that flows into the MCP
`initialize.instructions` field for *every* connected client with zero extra code
(`crates/mae/src/main.rs:717-786`). Whether VS Code's MCP client actually forwards that
field into Copilot's model context is unverified — this ADR treats that as an open
empirical question with a fallback, not an assumption.

## Decision

**D1 — Local pairing architecture.** VS Code registers `mae-mcp-shim` directly as a
`command`+`args` stdio server for the MVP. This requires **zero MAE-side protocol
changes** — `shared/mcp/src/shim.rs` already does socket auto-discovery, a real
handshake-verify-before-trust step, and reconnect-with-backoff, and is confirmed
provider-agnostic by ADR-046's own text. The fuller "MAE for VS Code" extension (a later
phase) wraps this via `McpServerDefinitionProvider`, auto-spawning a **headless** (ADR-055)
instance — never a GUI window — when none is running for the current workspace.

**D2 — Tool-schema/annotation compatibility.** Add MCP-standard `annotations`
(`title`, `readOnlyHint`, `destructiveHint`, `idempotentHint`) to `ToolInfo`
(`shared/mcp/src/protocol.rs:171-187`), derived **mechanically** from the existing
`PermissionTier` (`crates/ai/src/tools/categories.rs`) — never hand-authored per tool,
to avoid drift across ~700+ registered tools (per CLAUDE.md principle #7's "no hardcoding"
corollary: a derived, single-source-of-truth mapping, not per-tool manual annotation).
Extend the flat `ToolParameters`/`ToolProperty` JSON-Schema subset
(`crates/ai/src/types.rs:24-49` — scalar properties only today, no nested objects/arrays,
no `oneOf`, string enums only) additively with nested-object/array support for richer KB
tool parameters (e.g. structured search filters), without breaking any existing flat tool
definition.

**D3 — Cross-editor generality.** The wire protocol, annotations, schema, and
guidance-instructions delivery are shared and host-agnostic. A documented, tested, generic
"MAE via MCP" setup path (any stdio- or HTTP-capable MCP client) is the baseline; the VS
Code extension's lifecycle UI and Copilot-instructions-file fallback are VS-Code-specific
polish on top, not a fork of the protocol.

**D4 — Guidance delivery robustness.** Verify empirically whether VS Code's Copilot MCP
client forwards `initialize.instructions` into the model's context. Regardless of the
outcome, ship a fallback exporter (`kb-export-guidance` command/tool) that writes the
existing `build_guidance_context()` output (`crates/ai/src/guidance.rs:90-109`) to
`.github/copilot-instructions.md` and/or `AGENTS.md`, additive-merged below a clearly
delimited MAE-managed block, never clobbering hand-written content. One-time/on-demand
export by default (matching the existing opt-in `ai_guidance_kb` philosophy), with a
`:set-save`-able option for teams that want it live-synced.

## Consequences

**Positive.** The MVP local-pairing path requires no new MAE protocol code — it's
config-and-docs work reusing already-shipped, already-tested infrastructure. Mechanically
deriving annotations from the existing permission-tier system means zero ongoing
maintenance burden as new tools are added. The design stays genuinely editor-agnostic
rather than accreting VS-Code-specific protocol branches.

**Costs (honest).** VS Code's MCP feature surface is new and evolving month-to-month
(broad support only since ~1.99); this ADR's external-research claims should be
re-verified against current VS Code docs before implementation, not trusted as a frozen
spec. The flat-to-nested JSON-Schema extension is new wire surface that must remain
backward compatible with existing flat tool consumers (Claude Code, `mae-agent`) — any
regression here breaks an already-shipped integration, not just the new one.

## Alternatives rejected

- **Hand-authored per-tool `readOnlyHint`.** Would allow finer-grained truth (a tool
  that's usually read-only but occasionally not) but is a real drift risk across ~700+
  tools with no single source of truth to audit against — rejected in favor of mechanical
  derivation from `PermissionTier`.
- **Always spawning a GUI-capable `mae` process and just not showing its window.**
  Rejected — CLAUDE.md principle #12's framing ("daemon is a value-add, never a required
  floor") applied here to windows means the lightest correct default is headless-only, per
  ADR-055.

## Verification

- A real VS Code session with Copilot in Agent mode, connected via `mae-mcp-shim`, lists
  MAE's tools, calls `kb_search`/`kb_get`, and sees correctly-annotated results with **no**
  confirmation prompt on read-only calls — while the server-side permission gate is
  independently verified to still enforce correctly regardless of client-side "always
  allow" settings (see ADR-051's adversarial tests; a client-side dialog is never MAE's
  security boundary).
- Guidance content is demonstrably present in Copilot's context, via `instructions` or the
  fallback file, whichever the D4 verification step finds necessary.
- The same setup steps, written generically, are smoke-tested against at least one
  non-VS-Code MCP client to prove D3 isn't VS-Code-only in practice.
- A CI audit test enumerates every registered tool's `PermissionTier` against its derived
  `readOnlyHint` and fails the build on any inconsistency.
