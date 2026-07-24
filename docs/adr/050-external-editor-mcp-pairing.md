# ADR-050: External-editor MCP pairing — VS Code/Copilot & cross-editor compatibility

**Status:** Accepted (implemented — D1 MVP local pairing + D3 generic docs via Phase B
(#377); D2 tool annotations via Phase A (#376); D4 guidance-delivery fallback via Phase H
(#383); D1's full "MAE for VS Code" extension via Phase I (#384). See "Verification" and the
"Implementation note" below).
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

## Implementation note (added during Phase I implementation, principle #15)

Three small additions surfaced during Phase I planning that this ADR's original text didn't
anticipate, recorded here rather than left as undocumented drift:

- **New `mae --headless --print-socket-path` flag** (`crates/mae/src/cli.rs`). The
  extension needs to know a workspace's stable headless socket path *before* deciding
  whether to spawn an instance — rather than have the TypeScript side reimplement
  `headless_loop.rs`'s project-hashing scheme (a real drift risk, principle #8), this flag
  reuses `stable_socket_path()` verbatim, printing only the resolved path and exiting
  immediately with no other side effect. This is now the single source of truth any
  external tool can rely on instead of guessing the hash.
- **New `MAE_MCP_PERMISSION_CEILING` env var on `mae-mcp-shim`**
  (`shared/mcp/src/shim.rs`/`build_shim_initialize_params` in `shared/mcp/src/lib.rs`).
  ADR-051's `permissionCeiling` (`initialize` params) had no way to be set by the shim
  itself — a real gap for the extension, which is the natural place to want a tightened
  ceiling for a given VS Code session. Forwarding is opaque and additive (the shim doesn't
  validate the value itself; the editor's existing tightening-only `min()` enforcement is
  unchanged and unaware of which pathway a ceiling arrived through) — proven end-to-end
  against the real session/dispatch machinery in `shared/mcp/src/lib.rs`'s
  `permission_ceiling_built_by_the_shim_helper_threads_losslessly_to_the_real_requester_context`.
- **The extension lives at `editors/vscode/`** (new top-level directory, MAE's first
  non-Rust deliverable) and deliberately **never reads or writes `.vscode/mcp.json`** —
  `vscode.lm.registerMcpServerDefinitionProvider` is a dynamic, in-memory registration API,
  which structurally sidesteps the JSONC-mutation-safety concern this ADR's Context section
  flagged for a file-editing approach, rather than requiring careful parsing to avoid it.
- **The actual "capability declaration abuse" adversarial test target turned out to be
  workspace settings, not MCP protocol fields.** Every other phase's escalation tests target
  a network/protocol-level claim (a JWT's audience, a session's declared permission tier).
  Phase I's genuinely new attack surface is different in kind: a cloned, untrusted
  repository's `.vscode/settings.json` can set `mae.shimPath`/`mae.headlessBinaryPath` to
  anything it wants, and this extension is the first MAE surface that spawns local
  processes based on a workspace-supplied path. The primary defense is structural
  (`shell: false` with an argv array on every spawn call this extension makes, so shell
  metacharacters in a configured value are inert regardless of content — not a
  denylist/regex, which would be exactly the kind of ad-hoc workaround CLAUDE.md principle
  #7's corollary warns against); `resolveExecutable`'s existing-executable-file validation
  is the complementary guard against a bogus/nonexistent value being silently accepted. See
  `editors/vscode/src/shimCommand.ts`'s module doc and its adversarial unit tests
  (`test/unit/shimCommand.test.ts`, `test/unit/headlessDiscovery.test.ts`) for the exact
  proof — including the case where a maliciously-named file *does* exist (a legal Unix
  filename), showing the spawn call still treats it as one literal argv element, never
  shell-interpreted.

## Verification

- A real VS Code session with Copilot in Agent mode, connected via `mae-mcp-shim`, lists
  MAE's tools, calls `kb_search`/`kb_get`, and sees correctly-annotated results with **no**
  confirmation prompt on read-only calls — while the server-side permission gate is
  independently verified to still enforce correctly regardless of client-side "always
  allow" settings (see ADR-051's adversarial tests; a client-side dialog is never MAE's
  security boundary). **Partially done** — the server-side gate's independence from
  client-side behavior is proven (ADR-051), and the extension's own real-host smoke test
  (`editors/vscode/test/integration/extension.test.ts`) proves `activate()`/provider
  registration succeed in a genuine VS Code instance. The live Copilot Agent-mode
  round-trip itself needs a human's interactive check — no browser/GUI automation is
  available to this agent, the same honest caveat already recorded for Phases B/H
  (#377/#383).
- Guidance content is demonstrably present in Copilot's context, via `instructions` or the
  fallback file, whichever the D4 verification step finds necessary. **Done** — Phase H
  (#383): `kb_export_guidance` MCP/agent tool, additive-merge-safe, verified via a live MCP
  round-trip.
- The same setup steps, written generically, are smoke-tested against at least one
  non-VS-Code MCP client to prove D3 isn't VS-Code-only in practice. **Done** — Phase B
  (#377): `scripts/mcp-shim-stdio-smoke.{sh,py}`, a generic "any MCP client" doc, verified
  live against a freshly built, isolated headless instance.
- A CI audit test enumerates every registered tool's `PermissionTier` against its derived
  `readOnlyHint` and fails the build on any inconsistency. **Done** — Phase A (#376):
  `every_registered_tool_annotation_matches_its_permission_tier`,
  `crates/ai/src/executor/mod_tests.rs`.
- **D1 full extension (Phase I, #384):** the extension auto-spawns a headless instance via
  `McpServerDefinitionProvider` when none is running (never a GUI window), never touches
  `.vscode/mcp.json`, and the required "capability declaration abuse" adversarial test
  (a hostile workspace's `mae.shimPath`/`mae.headlessBinaryPath`) passes. **Done** —
  `editors/vscode/`, unit suite (`npm run test:unit`, 16 tests incl. the adversarial pair
  in `shimCommand.test.ts`/`headlessDiscovery.test.ts`) and a real-extension-host smoke
  test (`npm run test:integration`), both in default CI (`vscode-extension` job). Live
  interactive VS Code+Copilot verification remains the one open item, per the first bullet
  above.
