# ADR-055: Headless MAE as a first-class release/service target

**Status:** Proposed.
**Extends:** ADR-014 (binary architecture, editor vs. daemon workspaces — this ADR adds a
third *mode* of the *editor* binary; the daemon workspace structurally cannot host it,
since `daemon/Cargo.toml` has no `mae-core` dependency at all), ADR-035 (editor↔daemon
boundary — headless-vs-daemon is an orthogonal axis: a headless instance obeys
`daemon_mode` exactly as a GUI/TUI instance does; this ADR never merges the two process
types).
**Relates to:** ADR-046 (the CLI-harness-on-`mae-mcp-shim` direction — headless mode is
the natural host process for exactly that harness when no interactive terminal exists).
**Tracking:** issue #375 (epic tracker); phase issue #380.

## Context

VS Code (and other IDE) users pairing with MAE will not want an actual MAE GUI/TUI window
floating around when MAE isn't their primary editor. This needs to be a carefully,
natively designed capability — not a flag bolted onto the interactive binary as an
afterthought — including deciding whether a dedicated service/entrypoint is needed, and
shipping it as a fully tested, first-class release target.

Research found **no persistent "full Editor engine, zero renderer, long-running" mode
exists today**, but two existing code paths prove the exact shape already works:

- `mae --self-test` runs the **complete** bootstrap (buffers/windows/KB federation/AI/LSP/
  DAP/MCP primary+PSK sockets/external-MCP-client-manager —
  `crates/mae/src/main.rs:602-834`) with the only two `Renderer`-constructing call sites
  (`gui_app::run_gui`/`run_terminal_loop`) sitting **textually after** the self-test exit
  point. For the run's ~5-minute duration, MAE genuinely operates as "full engine + full
  MCP server + zero window" — it just always self-terminates afterward.
- The window/buffer/`DrivenWindow` machinery (`crates/core/src/driven_window.rs`,
  `Editor.window_mgr`) has **zero renderer dependency** — `Renderer::render(&mut Editor)`
  only ever reads editor state, never mutates it, and `Editor::new()` unconditionally
  creates one buffer+window regardless of whether any renderer will ever attach. So
  headless mode retains a fully meaningful internal window model (still governing which
  buffer an AI tool response targets, per ADR-051), it's just never drawn.
- `mae --check-config` is confirmed **one-shot-validator-only** — it builds an `Editor`
  purely to check `init.scm`/theme, never touches AI/LSP/DAP/MCP, and always exits. Not a
  candidate to extend into a long-running mode.
- `mae-daemon` is **structurally separate** — no `mae-core` dependency — and therefore
  cannot host the full Editor/AI/LSP/DAP/KB-tool-execution surface a VS Code pairing
  needs. The headless engine and the daemon remain distinct processes with distinct
  purposes: daemon = collab/KB persistence hub; headless engine = the full per-project
  agent-facing engine, no window.
- The specific tool surface a VS Code/Copilot pairing needs (KB tools, git tools,
  `project_search`) only ever touches `editor.kb`/`editor.project`/
  `editor.git_or_project_root()` — never `editor.windows`/`editor.buffers` — confirming no
  "slimmed-down Editor" variant is needed; the cost of the window/buffer machinery is
  already paid unconditionally by every `Editor::new()` today.

Building a persistent headless mode is therefore a modest, low-risk lift on proven code,
not new architecture — but "modest lift to build" is not the same as "trivially safe to
run for weeks unattended," which is why this ADR's success criteria focus heavily on
adversarial and longevity testing rather than just the happy-path bootstrap.

## Decision

1. **New `mae --headless` entry point on the existing TUI binary** (`make build-tui`,
   already Skia-free — the right base). Reuses the proven `--self-test` bootstrap shape,
   replacing the exit-after-test loop with a long-running serve loop and graceful SIGTERM
   shutdown.
2. **Discovery:** project-scoped, short-lived instances (e.g. spawned per-workspace by a
   VS Code extension) keep the existing `/tmp/mae-{PID}.sock` convention unchanged —
   `mae-mcp-shim`'s auto-discovery already handles this with no changes. A new **stable,
   project-keyed socket path** (`~/.local/share/mae/headless/{project-hash}.sock`,
   XDG-first per principle #13) is added for long-lived, explicitly-started instances that
   need to be found without tracking a PID that changes across restarts.
3. **One headless instance per project is the default.** `Editor` has no internal
   multi-project model today (`editor.git_or_project_root()` is inherently
   single-project); building one would be new, untested surface disproportionate to the
   problem this ADR solves. A shared multi-project instance is explicitly out of scope —
   revisit only if a real memory-pressure measurement from the soak test below justifies
   it.
4. **Systemd unit** extending `assets/mae-daemon.service`'s proven shape (`Type=simple`,
   `Restart=on-failure`, `ProtectSystem=strict`, scoped `ReadWritePaths`) into a templated
   `mae-headless@.service` (one instance per project hash), plus a launchd plist for macOS
   parity (principle #13).
5. **Release packaging:** extend `.github/workflows/release.yml`'s existing
   `mae-linux-x86_64` bundle (already ships the TUI binary + `mae-daemon.service` +
   `install.sh`) to also bundle the new service unit — no new binary, no new pipeline.
6. **Coexistence is allowed, not blocked.** A GUI/TUI editor and a headless instance may
   be open on the same project simultaneously — they're just two more MCP-connected
   `Editor` processes, each with their own buffers, converging through the existing CRDT
   machinery for shared KB content. This is exactly why ADR-051's per-session isolation and
   this ADR's adversarial tests below matter.

## Consequences

**Positive.** VS Code/other-IDE users get a genuinely invisible MAE backend, not a
window they have to manage or hide. Reuses proven bootstrap code rather than inventing new
architecture. Fits existing release/packaging conventions with no new binary or pipeline.

**Costs (honest).** A long-running process has failure modes a 5-minute `--self-test` run
never exercises: memory growth, file-descriptor accumulation, orphaned sockets on unclean
shutdown, and races between multiple instances/processes on the same project. These are
real risks this ADR's success criteria treat as first-class, not incidental — see the
"Assumptions, pitfalls & antipatterns" section of the initiative's tracking issue (P4:
render-loop-shaped work burning idle CPU; P5: memory/fd leaks over days/weeks of uptime;
both must be measured, not assumed away by "no `Renderer` is constructed").

## Alternatives rejected

- **A single shared headless instance serving multiple projects.** Rejected for this
  phase — `Editor` has no multi-project model, and building one is new, untested surface
  disproportionate to the memory/process overhead it would save; per-project instances are
  cheap given the already-proven `--self-test` bootstrap cost.
- **Extend `mae --check-config` into the long-running mode.** Rejected — confirmed to be a
  one-shot validator by design (never touches AI/LSP/DAP/MCP); repurposing it would
  conflate two very different semantics under one flag.
- **Host headless functionality inside `mae-daemon`.** Rejected — structurally impossible
  without adding `mae-core`/`mae-ai`/`mae-lsp`/`mae-dap` dependencies to a workspace that
  was deliberately kept minimal (ADR-014); would blur a boundary the project has
  intentionally maintained.
- **Block simultaneous GUI+headless instances on the same project.** Rejected — the
  existing CRDT machinery already handles concurrent writers correctly; blocking would add
  complexity to prevent a scenario that's already safe, and the adversarial test below
  verifies that assumption rather than taking it on faith.

## Verification

- **Idle CPU (P4):** a headless instance with no active MCP session, sampled over a
  sustained period, shows CPU usage that is bounded and near-zero — not periodic
  full-tilt polling. Any tick/animation logic assuming a viewport
  (`Editor::on_idle_tick`, `crates/core/src/editor/idle_ops.rs`, and any GUI-only
  animation scheduling) is audited and confirmed inert (or explicitly gated off) when no
  `Renderer` is attached.
- **Soak test (P5):** sustained realistic load (repeated KB queries, buffer opens/closes,
  LSP requests, connect/disconnect churn simulating many short-lived VS Code sessions) run
  for a bounded-but-long duration, with RSS and fd count sampled over time. Success is
  flat/bounded memory and fd usage — any growth must be traced to a specific structure and
  either bounded by an eviction policy or justified as legitimate content growth.
- **Kill -9 mid-write** recovers cleanly on next start (WAL/CRDT integrity intact).
- **Two headless instances racing to bind the same stable socket path** — the second fails
  loudly, never silently overwrites the first.
- **A GUI editor and a headless instance open on the same project simultaneously**, each
  performing conflicting KB writes, converge correctly via existing CRDT machinery — no
  corruption.
- **An orphaned socket** from an ungracefully-killed VS Code session (no clean MCP
  `shutdown`) is cleaned up on the next launch — extends the existing
  `cleanup_stale_mcp_sockets` (`crates/mae/src/terminal_loop.rs:727`) to the new stable-path
  convention.
