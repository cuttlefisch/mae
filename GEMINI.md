# AI Development Guide — Modern AI Editor (MAE)

> This file provides project context for AI coding assistants (Gemini, Copilot, etc.).
> It mirrors the content in `CLAUDE.md` — if they diverge, `CLAUDE.md` is authoritative.

> [!CAUTION]
> **MAE is in early Alpha.** AI features and cost guardrails are experimental and may fail. Always monitor your API usage and costs directly in your provider dashboards.

## What This Project Is

An AI-native lisp machine editor — a successor to GNU Emacs where the human user and an AI agent are **peer actors** calling the same Lisp primitives. The editor is built on a Rust core with an embedded Scheme (R7RS-small) runtime. LSP and DAP are first-class protocols exposed to both the Scheme extension layer and the AI agent's tool-calling interface.

The full architecture spec lives in `README.org`. Read it before starting any work.

## Stack

- **Language:** Rust (core) + Scheme R7RS-small (extensions)
- **License:** GPL-3.0-or-later
- **Build:** `make check` / `make build` / `make test` / `make ci` from workspace root
  - `make build` now builds with GUI by default (`--features gui`)
  - `make build-tui` for terminal-only build
  - `make ci` still excludes GUI (skia system deps)
  - `make check-config` validates init.scm + config.toml without launching the editor

## Crate Layout

| Crate | Purpose |
|---|---|
| `mae-core` | Buffer management (rope), event loop, core primitives |
| `mae-renderer` | Display/rendering — `Renderer` trait + terminal backend |
| `mae-gui` | GUI rendering backend — winit window + Skia 2D |
| `mae-scheme` | Embedded Scheme runtime for configuration and packages |
| `mae-lsp` | LSP client — types, references, diagnostics exposed to Scheme + AI |
| `mae-dap` | DAP client — breakpoints, call stacks, variables exposed to Scheme + AI |
| `mae-ai` | AI agent integration — tool-calling transport (Claude/OpenAI/Gemini/DeepSeek) |
| `mae-kb` | Knowledge base — graph store, org parser, bidirectional links |
| `mae-shell` | Embedded terminal emulator (alacritty_terminal) |
| `mae-mcp` | MCP bridge — Unix socket server, JSON-RPC, stdio shim |
| `mae` | Binary crate — event loop, key handling, CLI entry point |

## Architecture Principles

These are derived from analysis of 35 years of Emacs git history. They are non-negotiable design constraints:

1. **Concurrency from day one.** No Global Interpreter Lock, ever. Rust ownership for the core, concurrent GC for the Scheme runtime.

2. **Modular display layer.** Renderer is a separate crate with a clean trait-based HAL. Platform code lives in backend libraries, not in our codebase.

3. **The AI is a peer, not a plugin.** The AI agent calls the same Scheme functions as the user's keybindings. Same API surface for human and AI.

4. **LSP and DAP are first-class.** Not bolted-on packages. The AI gets structured semantic knowledge and runtime debug state as part of its reasoning context.

5. **Module boundaries enable distributed ownership.** Each crate has a clear responsibility. No 10k+ line files.

6. **Runtime redefinability is sacred.** Users must be able to redefine any function while the editor is running.

## Key Design Decisions

- **Scheme over other Lisps:** R7RS-small — hygienic macros, proper tail calls, first-class continuations.
- **Rust over other cores:** Eliminates the GC problem entirely.
- **GPL-3.0-or-later:** Copyleft ensures the project stays open.
- **Terminal-first:** ratatui/crossterm for initial development. GUI via winit + Skia.

## Development Status

See `ROADMAP.md` for granular milestone tracking. All core phases (1-8) are complete:
- Core editor, Scheme runtime, AI integration, LSP/DAP, syntax highlighting
- Knowledge base, embedded shell, MCP bridge, GUI backend
- v0.5.0: agent reliability (progress checkpoints, watchdog, prompt caching, token dashboard, context compaction redesign, graceful degradation, web fetch, 25 regression tests)
- 1,603 tests, CI green

## File Conventions

- **`.mae/`** — Project-local runtime state (conversations, sessions, plans, memories). Gitignored.
- **`~/.config/mae/`** — User config (config.toml, init.scm, themes).
- **`~/.local/share/mae/`** — User data (transcripts, logs).
- **`CLAUDE.md`** — Authoritative AI dev guide (this file is a mirror).
- **`ROADMAP.md`** — Milestone tracking with completion status.

## Related Resources

- **Steel Scheme:** https://github.com/mattwparas/steel
- **ropey:** https://github.com/cessen/ropey
- **ratatui:** https://github.com/ratatui/ratatui
