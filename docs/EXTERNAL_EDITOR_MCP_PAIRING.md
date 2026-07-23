# Pairing MAE with VS Code, Copilot, and Other MCP Clients

> Last updated: 2026-07-23 (v0.14.53). Design: [ADR-050](adr/050-external-editor-mcp-pairing.md)
> (D1, D3 — this doc is that ADR's Verification artifact for Phase B / issue #377).
> Related: [MCP_ARCHITECTURE.md](MCP_ARCHITECTURE.md) (wire protocol reference).

MAE can act as a general-purpose MCP backend for **any** MCP-capable editor's AI agent —
not just Claude Code. This doc covers the two supported paths: pairing with **VS Code +
GitHub Copilot's Agent mode**, and pairing with **any other stdio-capable MCP client**.
Both use the exact same mechanism Claude Code already validates in this repo: MAE's
per-process Unix socket, bridged over stdio by the `mae-mcp-shim` binary
(`shared/mcp/src/shim.rs`) — **zero MAE-side protocol changes** are required for either
path (ADR-050 D1).

## What this gets you

Once paired, your editor's AI agent gets the same ~700+ tool surface the built-in `mae`
agent uses: `kb_search`/`kb_get`/`kb_agenda` (your knowledge base), `lsp_definition`/
`lsp_references` (semantic code navigation), `execute_command` (any editor command), and
more. MAE's dev-guidance-KB mechanism (`ai_guidance_kb`) can steer the paired agent's
behavior — see "Which MAE config matters" below.

**Scope today:** this pairs with a KB running **locally on the same machine** as your
editor (the KB an `mae`/`mae --headless` instance has open). Reading a **shared,
access-controlled hub KB** you haven't fully replicated locally is a separate, later
capability (ADR-053) — not yet available through this path.

## Prerequisites

- MAE built and installed: `make build && make install` (installs `mae`, `mae-daemon`,
  `mae-mcp-shim` to `~/.local/bin` — see the repo README for the full setup).
- A running MAE instance for the project you want the agent to work in — either your
  normal `mae`/`mae --gui` session, or a headless instance (`mae --headless`,
  [ADR-055](adr/055-headless-service-mode.md)) if you don't want a GUI window open at
  all. Either way, `mae-mcp-shim` auto-discovers it — see "Which instance gets used?"
  below.

## Path 1: VS Code + GitHub Copilot (Agent mode)

1. Create `.vscode/mcp.json` in your project (a real, working example is committed at the
   root of this repo — `.vscode/mcp.json` — open this repo in VS Code to try it against
   MAE's own codebase):

   ```jsonc
   {
     // MAE MCP pairing — docs/EXTERNAL_EDITOR_MCP_PAIRING.md.
     // Requires `mae-mcp-shim` on PATH (`make install`) and a running `mae`/
     // `mae --headless` instance for this project. Comments are fine here —
     // .vscode/mcp.json is JSONC, not strict JSON.
     "servers": {
       "mae-editor": {
         "type": "stdio",
         "command": "mae-mcp-shim"
       }
     }
   }
   ```

2. Open the **Chat** view, switch to **Agent** mode (MCP tools are only exposed in Agent
   mode — not Ask or Edit mode), and open the tools picker (🔧) to confirm `mae-editor`'s
   tools are listed.
3. Ask it to do something that exercises a MAE tool, e.g. "search the knowledge base for
   X" or "what does `kb_search` return for Y". Read-only MAE tools (`kb_search`,
   `kb_get`, `lsp_definition`, …) are annotated `readOnlyHint: true`
   ([ADR-050 D2](adr/050-external-editor-mcp-pairing.md), mechanically derived from every
   tool's `PermissionTier` — audited by a CI test, `every_registered_tool_annotation_matches_its_permission_tier`)
   — VS Code Copilot skips the confirmation dialog for these and prompts for anything
   else, same as any other MCP server.

**Minimum VS Code version:** ADR-050's research found broad MCP support landing around VS
Code 1.99. This is a fast-moving area of VS Code — if `mae-editor`'s tools don't appear,
check `Help > About` against the current VS Code release notes before assuming a MAE-side
problem.

## Path 2: Any other stdio-capable MCP client

The exact same `mae-mcp-shim` binary works for Zed, Cursor, JetBrains' MCP support, a
hand-rolled client, or anything else that can spawn a `command` over stdio and speak
newline-delimited JSON-RPC 2.0 — `mae-mcp-shim`'s stdio surface has nothing VS-Code- or
Claude-Code-specific in it (confirmed directly against `shared/mcp/src/shim.rs`; this is
also ADR-046's own conclusion about the shim). Point your client's MCP config at the
`mae-mcp-shim` binary the same way you'd point it at any other local MCP server —
consult your client's own docs for its config file's exact shape (this is precisely the
config-fragmentation risk noted below).

**Verifying the mechanism without a specific branded client**: `scripts/mcp-shim-stdio-smoke.sh`
in this repo drives the shim exactly as any generic MCP host would — spawns it, does a
real `initialize` → `notifications/initialized` → `tools/list` → `tools/call` round trip
over stdio, and asserts `kb_search` carries a correct `readOnlyHint: true` annotation.
Run it against a live MAE instance to confirm the pairing mechanism itself is sound before
troubleshooting a specific host's own MCP client:

```sh
scripts/mcp-shim-stdio-smoke.sh
# or, if mae-mcp-shim isn't on PATH yet:
scripts/mcp-shim-stdio-smoke.sh ./target/release/mae-mcp-shim
```

This is what "smoke-tested against a generic host" means for this phase in practice: the
wire protocol contract every host depends on is fully exercised by this script; a
specific third-party host's own chat UI/approval behavior is out of scope for an
automated check and is instead verified per-host as in Path 1 above.

## Which instance gets used?

`mae-mcp-shim` auto-discovers a live MAE socket by scanning `/tmp/mae-*.sock` for one
whose PID is still alive, picking the most recently modified match if more than one is
running. If you have multiple MAE instances open (e.g. one per project) and want to pin
which one your editor's agent talks to, set `MAE_MCP_SOCKET` explicitly — in VS Code, via
the `env` field:

```jsonc
"servers": {
  "mae-editor": {
    "type": "stdio",
    "command": "mae-mcp-shim",
    "env": { "MAE_MCP_SOCKET": "/tmp/mae-12345.sock" }
  }
}
```

A long-lived `mae --headless` instance also has a **stable, project-keyed** socket path
(`~/.local/share/mae/headless/{project-hash}.sock`, ADR-055) that doesn't change across
restarts — useful for exactly this pinning, once you know the path (`mae --headless` logs
it at startup — `MCP headless stable socket started`).

## Config-format fragmentation: what to expect

Every MCP host has its **own** config schema, and this feature is evolving month to
month across the ecosystem, not a stable, frozen spec — do not treat anything in this doc
as permanent:

- **`.vscode/mcp.json` is JSONC, not JSON** — comments are legal and expected (see the
  example above). If you ever write tooling that edits this file programmatically
  (planned for a future "MAE for VS Code" extension, Phase I of this initiative), it
  **must** use a JSONC-tolerant parser and merge under a clearly-owned key, preserving
  everything else byte-for-byte — never naively `json.dump()` over a user's existing file.
- Zed, Cursor, and JetBrains each have their own, structurally different config
  surfaces — there is no single format to document once and reuse.
- **No automated capability probe exists yet** to detect whether a given host actually
  supports annotations/`instructions` forwarding/etc. before relying on it — for now,
  `scripts/mcp-shim-stdio-smoke.sh` is the manual equivalent (it directly asserts
  annotation support is present on the MAE side; whether a *specific host* consumes that
  is verified per-host, as in Path 1's confirmation-dialog check). An automated per-host
  capability check is future work, not yet built.

## Which MAE config matters — and which doesn't

Once your editor's own AI agent (Copilot, etc.) is the one acting, it brings its **own**
model and never touches MAE's AI executor. Most of MAE's `ai_*` provider/model settings
(`ai_provider`, `ai_model`, API keys, …) are **irrelevant to this pairing** — do not set
these up expecting them to affect Copilot in any way.

**What still matters, server-side:**

- **`ai_guidance_kb`** — if set (MAE ships a default of `"MaePractices"`; check via
  `:describe-option ai-guidance-kb`), its content is surfaced in the MCP `initialize`
  response's `instructions` field to *every* connected client, including a paired VS Code
  session — for free, no extra config. Whether VS Code's Copilot MCP client actually
  forwards `instructions` into the model's context is tracked separately
  ([ADR-050 D4](adr/050-external-editor-mcp-pairing.md), Phase H of this initiative) — a
  fallback exporter to `.github/copilot-instructions.md`/`AGENTS.md` is planned for
  whichever hosts don't.
- **The server-side permission policy** (`MAE_AI_PERMISSIONS` env var, or `config.toml`'s
  `[ai] auto_approve_tier`; default `"trusted"` = auto-approves up through Shell-tier
  tools with **no server-side confirmation at all**) — this, not VS Code's own
  confirmation dialog, is MAE's actual security boundary. See the note below.
- **The KB registry** (`kb_register`/`kb_instances` — which KBs MAE has open and
  searchable).

**What's irrelevant:** MAE's own `ai_provider`/`ai_model`/API-key settings, the embedded
AI chat (`ai_chat_enabled`), and anything else that only affects the *built-in* `mae`
agent — none of it is read or needed by an external editor's own agent pairing over MCP.

## Security note: the client's confirmation dialog is not MAE's security boundary

VS Code (and most hosts) let a user permanently "always allow" a tool in the *client* UI.
Once that's set, there is no client-side prompt standing between the model and that tool
— **MAE's own server-side `PermissionPolicy`/`kb_access` checks are the only real
enforcement**, regardless of what any client's UI does or doesn't show. Don't rely on
"Copilot will ask before doing anything destructive" as your actual safety net; set
`MAE_AI_PERMISSIONS`/`auto_approve_tier` to a tier you're actually comfortable auto-
approving for *any* MCP client, paired or not. (Per-session permission ceilings — a
connecting client can *further restrict*, never loosen, its own ceiling via
`initialize`'s `permissionCeiling` param — exist for exactly this kind of scoped pairing;
see [ADR-051](adr/051-per-session-permission-driven-window-isolation.md). VS Code's own MCP client doesn't
expose a way to set this today, but a hand-rolled or scripted client can.)

## Troubleshooting

- **No tools listed / `mae-editor` shows disconnected**: confirm a MAE instance is
  actually running for this project (`ls /tmp/mae-*.sock`), and that `mae-mcp-shim` is on
  `PATH` (`which mae-mcp-shim`). Run `mae-mcp-shim --check` for a connectivity diagnostic,
  or `scripts/mcp-shim-stdio-smoke.sh` for a full protocol-level check.
- **Tools listed but every call needs confirmation**: you're likely on a MAE build
  predating [ADR-050 D2](adr/050-external-editor-mcp-pairing.md)'s tool annotations
  (check `mae --version`; annotations shipped alongside this doc) — rebuild.
- **Debug logging**: set `MAE_MCP_SHIM_LOG=/path/to/log` before launching your editor to
  trace all shim traffic (all clients share the process-wide default,
  `/tmp/mae-shim.log`, if unset — expect it to be noisy with multiple clients connected).
