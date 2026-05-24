# AI Development Guide — Modern AI Editor (MAE)

> This file provides project context for AI coding assistants (Gemini, Copilot, etc.).
> It mirrors the content in `CLAUDE.md` — if they diverge, `CLAUDE.md` is authoritative.

> [!CAUTION]
> **MAE is in early Alpha.** AI features and cost guardrails are experimental and may fail. Always monitor your API usage and costs directly in your provider dashboards.

## What This Project Is

An AI-native lisp machine editor — a successor to GNU Emacs where the human user and an AI agent are **peer actors** calling the same Lisp primitives. The editor is built on a Rust core with an embedded Scheme (R7RS-small) runtime. LSP and DAP are first-class protocols exposed to both the Scheme extension layer and the AI agent's tool-calling interface.

The project README (`README.md`) contains the architecture spec and stack rationale.

## Stack

- **Language:** Rust (core) + Scheme R7RS-small (extensions)
- **License:** GPL-3.0-or-later
- **Build:** `make check` / `make build` / `make test` / `make ci` from workspace root
  - `make build` builds with GUI by default (`--features gui`)
  - `make build-tui` for terminal-only build
  - `make ci` excludes GUI (skia system deps)
  - `make audit` runs `cargo-deny` for license/advisory/ban checks
  - `make check-config` validates init.scm + config.toml without launching the editor
  - **Container workflow** (no local toolchain required):
    - `make docker-ci` — full CI in container (mirrors GitHub CI exactly)
    - `make docker-new-user` — validate first-run flow in pristine environment
    - `make docker-dev` — interactive dev shell with Rust toolchain
    - `make docker-smoke` — quick binary smoke test
    - `make docker-clean` — remove Docker images and cache
    - `make docker-collab-test` — collab CRDT E2E test (state-server + 2 clients + verifier)
  - Dockerfile: multi-stage (base -> builder -> ci -> runtime), TUI-only (no Skia in container)
  - `docker compose run --rm --build <service>` is the canonical invocation

## Crate Layout

| Crate | Purpose |
|---|---|
| `mae-core` | Buffer management (rope), editor state, commands, keymap, syntax |
| `mae-renderer` | Display/rendering — `Renderer` trait + terminal backend |
| `mae-gui` | GUI rendering backend — winit window + Skia 2D + native SVG |
| `mae-scheme` | Embedded Scheme runtime for configuration and packages |
| `mae-lsp` | LSP client — types, references, diagnostics exposed to Scheme + AI |
| `mae-dap` | DAP client — breakpoints, call stacks, variables exposed to Scheme + AI |
| `mae-ai` | AI agent integration — tool-calling transport (Claude/OpenAI/Gemini/DeepSeek) |
| `mae-kb` | Knowledge base — graph store, org parser, bidirectional links |
| `mae-shell` | Embedded terminal emulator (alacritty_terminal) |
| `mae-mcp` | MCP server — Unix/TCP, JSON-RPC, multi-client, stdio shim, transport-generic I/O |
| `mae-sync` | Collaborative state — yrs CRDT, ropey bridge, encoding helpers |
| `mae-state-server` | Standalone collab state server — TCP sync, WAL persistence, per-doc locking |
| `mae-babel` | Org-babel executor — 12 languages, persistent sessions, language backends |
| `mae-export` | Org/Markdown export — HTML, Markdown, TOC, syntax highlighting |
| `mae-snippets` | YASnippet-style templates — tab-stops, mirrors, transforms |
| `mae-format` | Formatter bridge — prettier, black, rustfmt (complements LSP format) |
| `mae-make` | Build runner — Makefile/Cargo.toml/package.json detection |
| `mae-lookup` | Unified lookup — LSP def + docs URL + man pages |
| `mae-spell` | Spellcheck — hunspell/aspell integration, inline markers |
| `mae` | Binary crate — event loop, key handling, CLI entry point |

## Architecture Principles

These are derived from analysis of 35 years of Emacs git history. They are non-negotiable design constraints:

1. **Concurrency from day one.** No Global Interpreter Lock, ever. Rust ownership for the core, concurrent GC for the Scheme runtime.

2. **Modular display layer.** Renderer is a separate crate with a clean trait-based HAL. Platform code lives in backend libraries, not in our codebase.

3. **The AI is a peer, not a plugin.** The AI agent calls the same Scheme functions as the user's keybindings. Same API surface for human and AI.

4. **LSP and DAP are first-class.** Not bolted-on packages. The AI gets structured semantic knowledge and runtime debug state as part of its reasoning context.

5. **Module boundaries enable distributed ownership.** Each crate has a clear responsibility. No 10k+ line files.

6. **Runtime redefinability is sacred.** Users must be able to redefine any function while the editor is running.

7. **No hardcoding — Scheme-first configurability.** Every user-visible behavior exposed as a configurable option via the OptionRegistry.

8. **Shared computation, backend-specific drawing.** All layout math lives in `mae-core`. Backends contain ONLY platform API calls.

9. **CRDT-first sync (yrs/YATA).** All collaborative state flows through yrs (Yjs Rust port). The ropey rope is a read-only rendering mirror. See ADR-002.

10. **Local-first by design.** MAE satisfies 5 of 7 Ink & Switch local-first ideals today. The state server is an optimization, not a requirement.

## Key Design Decisions

- **Scheme over other Lisps:** R7RS-small — hygienic macros, proper tail calls, first-class continuations.
- **Rust over other cores:** Eliminates the GC problem entirely.
- **GPL-3.0-or-later:** Copyleft ensures the project stays open.
- **Terminal-first:** ratatui/crossterm for initial development. GUI via winit + Skia.

## Keybinding Architecture

- **Kernel keymaps** (`keymaps.rs`): vi-modal primitives (hjkl, operators, text objects, Escape, `:`). Currently also has SPC leader bindings as a transitional default — migrating to keymap flavor modules.
- **Keymap flavor modules** (`modules/keymap-doom/`, future `keymap-emacs/`, `keymap-minimal/`): define the full SPC leader tree. Selected via `keymap_flavor` option (default: "doom").
- **Feature modules** (dailies, git-status, etc.): add bindings ONLY for module-specific commands not covered by the keymap flavor.
- **Scheme API**: `(define-key MAP KEY CMD)` and `(set-group-name MAP PREFIX LABEL)` are the canonical binding APIs. Both work at init time and REPL time.
- **`(mae!)` block**: Declarative module selection in `init.scm`. Only declared modules load.
- **Never duplicate** bindings between kernel and modules without a documented migration path.

## Sync Engine (yrs/YATA)

Collaborative state uses **yrs** (Yjs Rust port, YATA algorithm). `mae-sync` wraps yrs with MAE-specific document schemas and provides the ropey bridge.

- Text buffers use `YText`, KB nodes are yrs documents
- Built-in `UndoManager` with per-user stacks
- Transport: JSON-RPC 2.0 with Content-Length framing over TCP and Unix sockets
- `DocAddress` enum: `File { project_hash, rel_path }`, `Shared { name }`, `KbNode { node_id }`
- Local undo/redo uses `reconcile_to()` (character-level LCS diff) for CRDT-safe deltas

## State Server (`mae-state-server`)

Standalone binary for multi-machine collaborative editing.

**Usage:** `mae-state-server [--bind 0.0.0.0:9473] [--unix-socket /path] [--check-config] [doctor]`

**Architecture:**
- Per-document locking (`RwLock<HashMap<String, Arc<Mutex<DocEntry>>>>`)
- SQLite connection pool: FNV-1a hash-sharded (default 4 shards, WAL mode)
- WAL-first persistence: append to SQLite WAL before in-memory apply
- Compaction + idle eviction background tasks

**Join-save model:** Joined buffers have no local file path by default. Users use `:saveas` to persist locally. `collab_auto_resolve_paths` enables prompted project-root mapping. Server-side suffix matching resolves bare filenames.

**Persistent doc_id:** MAE's doc_ids persist across sessions (unique in the industry). Enables asynchronous collaboration — documents survive host disconnection. P2P collaboration via mDNS is planned.

**Editor commands:** `collab-start` (SPC C s), `collab-connect` (SPC C c), `collab-share` (SPC C S), `collab-join` (SPC C j), `collab-status` (SPC C i), `collab-doctor` (SPC C D)

**Collab options (11):** `collab_server_address`, `collab_auto_connect`, `collab_auto_share`, `collab_reconnect_interval`, `collab_user_name`, `collab_write_timeout_ms`, `collab_max_pending_updates`, `collab_reconnect_backoff_factor`, `collab_max_reconnect_attempts`, `collab_batch_update_ms`, `collab_auto_resolve_paths`, `collab_default_save_dir`, `collab_save_on_remote_update`

## Scheme Testing Framework

MAE has a headless test runner. Tests boot a real editor (no mocks) and exercise the same Scheme API surface available to users.

```bash
mae --test tests/crdt/              # CRDT sync tests
mae --test tests/editor/            # Editor feature tests
make test-scheme-all                # All local tests
```

Architecture: `scheme/lib/mae-test.scm` (BDD library) + `crates/mae/src/test_runner.rs` (Rust orchestrator). TAP v14 output for CI.

## Development Status

**v0.11.0-dailies** — 3,638+ tests, 20 crates, 19 modules. All phases through 8 complete.

See `ROADMAP.md` for granular milestone tracking.

### Key Modules

- **`crates/core/src/editor/dispatch/`** — command dispatch split into 10 submodules
- **`crates/core/src/diff.rs`** — LCS-based unified diff
- **`crates/core/src/syntax.rs`** — tree-sitter syntax highlighting + incremental reparse
- **`crates/gui/src/canvas.rs`** — Skia canvas with font pre-scaling cache

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
