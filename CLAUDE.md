# AI Development Guide (CLAUDE.md) — Modern AI Editor (MAE)

> [!CAUTION]
> **MAE is in early Alpha.** AI features and cost guardrails are experimental and may fail. Always monitor your API usage and costs directly in your provider dashboards.

## What This Project Is

An AI-native lisp machine editor — a successor to GNU Emacs where the human user and an AI agent are **peer actors** calling the same Lisp primitives. The editor is built on a Rust core with an embedded Scheme (R7RS-small) runtime. LSP and DAP are first-class protocols exposed to both the Scheme extension layer and the AI agent's tool-calling interface.

The project README (`README.md`) contains the architecture spec and stack rationale. **Read it before starting any work.**

## Stack

- **Language:** Rust (core) + Scheme R7RS-small (extensions)
- **License:** GPL-3.0-or-later
- **Build:** `make check` / `make build` / `make test` / `make ci` from workspace root
  - `make build` now builds with GUI by default (`--features gui`)
  - `make build-tui` for terminal-only build
  - `make ci` still excludes GUI (skia system deps)
  - `make check-config` validates init.scm + config.toml without launching the editor
  - **Daemon** (separate workspace): `cd daemon && cargo build`, `cd daemon && cargo test`, `cd daemon && cargo clippy -- -D warnings`
  - **Container workflow** (no local toolchain required):
    - `make docker-ci` — full CI in container (mirrors GitHub CI exactly)
    - `make docker-new-user` — validate first-run flow in pristine environment
    - `make docker-dev` — interactive dev shell with Rust toolchain
    - `make docker-smoke` — quick binary smoke test
    - `make docker-clean` — remove Docker images and cache
  - Dockerfile: multi-stage (base -> builder -> ci -> runtime), TUI-only (no Skia in container)
  - `docker compose run --rm --build <service>` is the canonical invocation
- **Self-test:** Call the `self_test_suite` MCP tool to get the structured JSON test plan, then execute each test by calling the listed MCP tools and checking assertions. If MCP is unavailable, fall back to `make self-test` (headless). Categories: `introspection`, `editing`, `git`, `help`, `project`, `lsp`, `dap`, `babel`, `guidance`, `performance`, `scrolling`.

## Repository Layout

Two workspaces + shared crates (ADR-014):

```
mae/                              (repo root)
├── Cargo.toml                    (editor workspace — cozo+sled)
├── Cargo.lock                    (editor lock)
├── crates/                       (editor-only crates — 18 crates)
├── daemon/                       (daemon workspace — cozo+sqlite, no rusqlite)
│   ├── Cargo.toml                (daemon workspace + own Cargo.lock)
│   └── src/                      (mae-daemon binary)
└── shared/                       (shared crates — members of editor workspace)
    ├── kb/                       (mae-kb: CozoDB store, org parser, federation)
    ├── sync/                     (mae-sync: yrs CRDT, ropey bridge)
    └── mcp/                      (mae-mcp: JSON-RPC protocol, shim)
```

## Crate Layout

### Editor Workspace (`Cargo.toml`)

| Crate | Purpose | Key Dependencies (planned) |
|---|---|---|
| `mae-core` | Buffer management (rope), event loop, core primitives | `ropey`, `crossbeam` |
| `mae-renderer` | Display/rendering — `Renderer` trait + terminal backend | `ratatui`, `crossterm` |
| `mae-gui` | GUI rendering backend — winit window + Skia 2D + native SVG | `winit`, `skia-safe` (features: `svg`) |
| `mae-scheme` | Embedded Scheme runtime for configuration and packages | purpose-built R7RS-small |
| `mae-lsp` | LSP client — types, references, diagnostics exposed to Scheme + AI | `tower-lsp` or `lsp-types` |
| `mae-dap` | DAP client — breakpoints, call stacks, variables exposed to Scheme + AI | `dap-types` |
| `mae-ai` | AI agent integration — tool-calling transport (Claude/OpenAI/Gemini/DeepSeek) | `reqwest`, `serde_json` |
| `mae-shell` | Embedded terminal emulator (alacritty_terminal) | `alacritty_terminal` |
| `mae-babel` | Org-mode code block execution (12 languages) | `mae-shell` |
| `mae-export` | Org/Markdown → HTML/Markdown export | `mae-kb` |
| `mae-canvas` | Visual buffer (diagrams, drawings) | `mae-core` |
| `mae-snippets` | Snippet expansion engine | `mae-core` |
| `mae-format` | Buffer formatting (external formatters) | `mae-core` |
| `mae-make` | Build system integration (make, cargo, npm) | `mae-core` |
| `mae-lookup` | Online lookup (dictionary, docs) | `reqwest` |
| `mae-spell` | Spell checking integration | `mae-core` |
| `mae` | Binary crate — CLI entry point, config loading, event loops | `clap`, `tokio` |

### Shared Crates (`shared/` — editor workspace members, also used by daemon)

| Crate | Purpose | Key Dependencies |
|---|---|---|
| `mae-kb` | Knowledge base — CozoDB graph store, typed relationships, org parser, federation | `cozo`, `tree-sitter`, `tree-sitter-org` |
| `mae-sync` | Collaborative state — yrs CRDT, ropey bridge, encoding helpers | `yrs`, `serde`, `base64` |
| `mae-mcp` | MCP server — Unix/TCP, JSON-RPC, multi-client, stdio shim, transport-generic I/O | `tokio`, `serde_json` |

### Daemon Workspace (`daemon/Cargo.toml` — separate Cargo.lock)

| Crate | Purpose | Key Dependencies |
|---|---|---|
| `mae-daemon` | Background service — KB persistence, collaborative editing (TCP sync + WAL), maintenance scheduler, JSON-RPC API | `cozo` (sqlite), `sqlite`, `mae-kb`, `mae-mcp`, `mae-sync`, `tokio` |

## Architecture Principles

These are derived from analysis of 35 years of Emacs git history. They are non-negotiable design constraints:

1. **Concurrency from day one.** Emacs spent 23,901 commits across 3 branches trying to retrofit a concurrent GC and still hasn't merged it. We use Rust's ownership model for the core and a purpose-designed concurrent GC for the Scheme runtime. No Global Interpreter Lock, ever.

2. **Modular display layer.** Emacs's `xdisp.c` is 38,605 lines and the most bug-prone file in the codebase. Our renderer is a separate crate with a clean trait-based HAL. Platform-specific code lives in the rendering backend library (crossterm/Skia), not in our codebase.

3. **The AI is a peer, not a plugin.** The AI agent calls the same Scheme functions as the user's keybindings. `(buffer-insert ...)`, `(lsp-references ...)`, `(dap-inspect-variable ...)` — same API surface for human and AI. No separate "AI mode" or simulated keystrokes.

4. **LSP and DAP are first-class.** Not bolted-on packages. The AI gets structured semantic knowledge (types, references, diagnostics from LSP) and runtime debug state (call stacks, variables from DAP) as part of its reasoning context.

5. **Module boundaries enable distributed ownership.** Each crate has a clear responsibility. No 10k+ line files. This is a direct response to Emacs's bus-factor problem (top 5 contributors = 50.8% of all commits, critical subsystems maintained by single individuals).

6. **Runtime redefinability is sacred.** Users must be able to redefine any function while the editor is running. This is the property that makes Emacs irreplaceable. The Scheme layer provides `defadvice`-equivalent, live REPL, and hot reload.

7. **No hardcoding — Scheme-first configurability.** Every user-visible behavior that could reasonably differ between users MUST be exposed as a configurable option via the OptionRegistry. This means:
   - Register in `options.rs` with a `config_key` (enables `:set-save` persistence)
   - Automatically accessible via `(set-option!)` / `(get-option)` in Scheme
   - Automatically accessible via `:set` command at runtime
   - Default values live in the option definition, never as magic constants in rendering code
   - Constants that are truly fixed (buffer sizes, protocol limits) belong in the relevant module, documented with rationale

   **Corollary: No ad-hoc solutions.** Never add a hardcoded workaround for a problem that should be solved architecturally. If you find yourself duplicating logic between TUI and GUI, extract to `render_common` or `text_utils`. If you find a magic number, make it an option. If you find a one-off fix for one backend, fix it properly for both.

8. **Shared computation, backend-specific drawing.** All layout math, content formatting, span computation, and data preparation lives in `mae-core` (specifically `render_common/` and `text_utils`). Backend crates (`mae-renderer`, `mae-gui`) contain ONLY the code that touches platform APIs (ratatui widgets, Skia paint calls). If two renderers compute the same thing, extract it.

9. **Every change must consider downstream impact.** Before implementing any change, assess:
   - **Bug risk**: What existing behavior could break? What edge cases does this touch?
   - **Performance impact**: Does this add work to a hot path? Is it O(1), O(n), or O(n²)?
   - **Type safety at boundaries**: When extracting shared code, verify that type conversions (e.g., `usize` ↔ `u16`) don't silently truncate.
   - **Regression guard**: If the change touches rendering or input handling, verify both TUI and GUI backends. If it touches options, verify the Scheme API + `:set` + `:set-save` persistence (which writes `init.scm` — the primary config surface; `config.toml` is legacy bootstrap for AI provider + theme only) all work.

10. **Multi-client safety by design.** Any state mutation must be safe for concurrent observation. The MCP server may have N connected clients. Editor state changes emit events to a broadcast channel. Clients that can't keep up are dropped (bounded queues, write timeouts). File writes use content-hash verification + advisory locks. No operation assumes single-client.

11. **CRDT-first sync (yrs/YATA).** All collaborative state flows through yrs (Yjs Rust port). Text buffers use `YText`, visual documents use `YMap`/`YArray`, KB nodes are yrs documents. The ropey rope is a read-only rendering mirror rebuilt from yrs on remote changes. Local edits generate yrs transactions (attributed, undoable via per-user `UndoManager`). This is the universal substrate — no separate sync mechanism for different content types. See ADR-002, ADR-005, ADR-006. Local undo/redo uses `reconcile_to()` (character-level LCS diff) to generate CRDT-safe deltas instead of full-state replacements.

12. **Local-first by design.** MAE satisfies 5 of 7 Ink & Switch local-first ideals today (no spinners, multi-device, network optional, collaboration without conflict, user ownership). P2P collaboration and E2E encryption will complete the remaining two. The daemon is an optimization for persistence and discovery, not a requirement for collaboration.

13. **Cross-platform parity (macOS + Linux) is a development constraint, not an afterthought.** MAE is developed and run across macOS and Linux *simultaneously* (often on the same branch, same day). Every script, path-resolution, and tool invocation MUST behave identically on both — or fail loudly with a portable fallback, never silently no-op on one platform. A "fix" that only works on one developer's machine is not a fix; it manufactures the stop-and-go cross-machine debugging this principle exists to prevent. Concretely:
    - **Directory resolution is XDG-first on ALL platforms.** Honor `XDG_CONFIG_HOME` / `XDG_DATA_HOME` when set, then fall back to the platform default. The bare `dirs` / `directories` crate follows Apple conventions on macOS (`~/Library/Application Support`) and *ignores* XDG — so calling `dirs::config_dir()` / `dirs::data_dir()` directly breaks env-var test isolation and contradicts the documented `~/.config/mae` + `~/.local/share/mae` contract. Use the XDG-first helpers (`mae-mcp::identity::default_collab_dir`, `mae-mcp::keystore`, editor `pkg/paths.rs::{dirs_candidate,data_dir_candidate}`), never raw `dirs::*` for primary config/data paths.
    - **Shell scripts use portable tooling.** No Linux-only commands without a fallback: `ss` → `lsof` → `netstat`; `timeout` → `gtimeout` → optional/omitted; avoid GNU-only behavior (`sed -i` arg differences, `readlink -f`, `mktemp` templates, `date` flags). Prefer POSIX; gate platform branches on capability (`command -v`), not `uname`. Keep the Linux path first so CI/driver behavior is unchanged.
    - **CI must exercise both OSes** for anything touching paths, sockets, or scripts — the collab e2e (`scripts/collab-*-e2e.sh`) especially, since that's where this bites.
    This is the cross-machine corollary to principles #8 (shared computation) and #9 (downstream impact): verify the change on *both* platforms, not just the one in front of you.

### Rendering Pipeline
The GUI renderer uses a three-phase pipeline: `compute_layout()` produces
a `FrameLayout`, `render_buffer_content()` draws text, and `render_cursor()`
positions the cursor. All three MUST consume the same `HighlightSpan` set.
See `crates/gui/src/RENDERING.md` for detailed rules.

## Development Priorities

Start terminal-only. Skip GUI until the model works.
Granular milestone tracking lives in **ROADMAP.md**.

All phases below are COMPLETE. See ROADMAP.md for granular milestone details.

| Phase | Summary | Tests |
|-------|---------|-------|
| 1. Core + Renderer | ropey buffer, event loop, ratatui/crossterm, vi-modal editing | — |
| 2. Scheme Runtime | R7RS-small, `init.scm`, `(define-key ...)`, REPL | — |
| 3. AI Integration | Claude/OpenAI/Gemini/DeepSeek, tool-calling, permission tiers | 1,148 |
| 3d–3h. Hardening | Full vim, multi-file AI, agent reliability, context compaction | 1,673 |
| 4. LSP + DAP + Syntax | LSP nav/completion, DAP debugging, tree-sitter (13 langs), KB | — |
| 5. Knowledge Base | CozoDB graph (Datalog), federated queries, org parser, HNSW vectors | — |
| 6. Embedded Shell | alacritty_terminal, MCP server, file auto-reload | — |
| 7. Documentation | Help system (862 KB nodes), tutorials, `:describe-configuration` | — |
| 8. GUI Backend | winit + Skia, inline images, multi-cursor, magit-style git | 2,629 |

**Current:** 5,863+ tests. **Next:** Org export & babel, PDF preview, module system.

## Key Design Decisions Already Made

- **Scheme over other Lisps:** R7RS-small is close enough to elisp for a compatibility shim, has hygienic macros (superior to elisp's `defmacro`), proper tail calls, and first-class continuations. Janet was too limited on macros. Racket has the best language but worst embedding story. Fennel/LuaJIT is proven (Neovim) but fragile upstream.

- **Rust over other cores:** Eliminates the GC problem entirely. Zig was considered (simpler FFI, comptime) but has a smaller ecosystem and less mature async story. C/C++ would repeat Emacs's mistakes.

- **GPL-3.0-or-later:** Copyleft ensures the project stays open. No FSF copyright assignment — contributions are owned by their authors.

- **Terminal-first:** ratatui/crossterm for initial development. GPU rendering (Skia) is now the primary target.

## Keybinding Architecture

- **Kernel keymaps** (`keymaps.rs`): vi-modal primitives ONLY (hjkl, operators, text objects, Escape, `:`, `C-w` window + resize, `C-c` capture) + the empty `leader`/`command`/etc. keymaps. The kernel defines **no** SPC leader bindings — enforced by `kernel_keymap_has_no_leader_bindings`.
- **Shared leader tree** (`modules/keymap-leader/`, embedded): the single source of truth for the mae which-key menu, bound into the kernel-created **`leader` keymap** WITHOUT an `SPC` prefix (`(define-key "leader" "b s" "save")`). Every flavor depends on it.
- **Keymap flavor modules** (`modules/keymap-doom/` = modal default, `modules/keymap-nonmodal/` = non-modal/CUA; both embedded). A flavor depends on `keymap-leader` and only wires its ENTRY into the transient keypad + its default mode: doom binds `SPC` (normal/visual) → `leader-dispatch` (Normal default); nonmodal sets `default_mode=insert`, binds `C-;` (insert) → `leader-dispatch`, + CUA chords. Selected via `keymap_flavor` option (default "doom"); switch live with `:keymap-set-flavor <name>` (resets keymaps to kernel + reloads — no stale bindings).
- **Transient keypad** (`leader_active` overlay, `leader-dispatch` command): a God-Mode/Meow-Keypad layer that does NOT mutate the base mode. While active, keys resolve against the shared `leader` keymap (which-key renders, N levels deep via pending-key accumulation); resolving one command or cancelling (`Esc`/`C-g`/unbound) pops the overlay, restoring the base mode (Normal for doom, Insert for nonmodal). Traversal is flavor-independent; restoration is flavor-specific by construction.
- **Extensibility** (user-facing, no kernel patches):
  - *New flavor*: drop `modules/keymap-<name>/` (`[dependencies] keymap-leader = "*"`), set `default_mode` + an entry binding to `leader-dispatch`. Ships embedded if in repo `modules/`; users add flavors via `~/.local/share/mae/modules` or `MAE_MODULES_PATH`.
  - *New which-key command*: `(define-key "leader" "x y" "cmd")` + `(set-group-name "leader" "x" "+label")` in any module or `config.scm` — appears in EVERY flavor's keypad and survives flavor switches.
  - *Hooks*: `leader-open` / `leader-execute` (keypad-resolved command) / `leader-cancel`, `keymap-flavor-changed`, plus generic `command-pre`/`command-post` + per-command `:before`/`:after` advice.
- **Feature modules** (dailies, git-status, etc.): bind leader entries into the `leader` keymap (not `normal`/`SPC`), so they appear in the keypad regardless of flavor.
- **Scheme API**: `(define-key MAP KEY CMD)`, `(set-group-name MAP PREFIX LABEL)`, `(define-keymap NAME PARENT)`, `(undefine-key! MAP KEY)` — work at init + REPL (runtime redefinable). `:reload-modules` (alias `mae-reload`) re-runs module loading live.
- **`(mae!)` block**: Declarative module selection in `init.scm`. Keymap flavor + its dep closure + language modules auto-enable. Belongs in `init.scm` (read before module loading; `config.scm` is too late for `keymap_flavor`/`default_mode`).
- **Never duplicate** leader bindings between kernel and modules. The `leader` keymap is the sole owner. New leader bindings go in `keymap-leader` (or a feature module / user config), never `keymaps.rs`.
- **Never add ad-hoc solutions**: Prefer proper architectural solutions over hardcoded workarounds. When you find yourself duplicating logic between TUI and GUI renderers, extract shared code.
- **Every option must be Scheme-accessible**: If a behavior is configurable, it goes through OptionRegistry. No config.toml-only settings, no env-var-only settings, no compile-time-only flags for user-facing behavior.

## Emacs Lessons (Reference Data)

These findings from analyzing the Emacs git repo (clone of emacs-mirror/emacs) motivated our architecture:

- **Fix ratio climbed from 15% to 32%** over 35 years — a complexity ceiling from C + untyped Lisp. Rust's type system structurally prevents this.
- **`xdisp.c`: 38,605 lines, 20k+ commits/decade** — the display engine is a monolithic maintenance black hole. We use a modular renderer crate.
- **IGC/MPS: 23,901 commits across `feature/igc`, `igc2`, `igc3`** — still unmerged after 3 iterations. GC retrofit is intractable. We avoid needing one.
- **Bus factor ~4 people** — top 5 = 50.8% of commits. Single-person dependencies on native-comp (Corallo), tree-sitter (Yuan Fu), Android (Po Lu), Tramp (Albinus). We enforce module boundaries.
- **~10% of all commits are platform support** — separate `*term.c` files per platform. We delegate to crossterm/Skia.
- **Emacs 31 direction:** VC/git (1,048 commits = 16%), completions, TTY child frames, newcomer presets, `elisp-scope.el` (static analysis). QoL is the frontier.
- **Development velocity peaked in 2022 (9,647 commits) and declined to ~3,356 in 2024.** The 2025 pace is even lower. Whether this is stabilization or contributor burnout is unclear.

## Development Dependencies

Required for full self-test coverage (DAP and LSP categories):

| Package | Purpose | Install |
|---------|---------|---------|
| `lldb` | DAP adapter for C/C++/Rust (provides `lldb-dap`) | `sudo dnf install lldb` (Fedora), `sudo apt install lldb` (Debian/Ubuntu), `brew install llvm` (macOS) |
| `rust-analyzer` | LSP server for Rust | `rustup component add rust-analyzer` |
| `debugpy` | DAP adapter for Python | `pip install debugpy` |

Quick setup: `make setup-dev` (auto-detects package manager).

Environment variable overrides for adapter/server paths:
- **DAP:** `MAE_DAP_LLDB`, `MAE_DAP_CODELLDB`, `MAE_DAP_DEBUGPY`
- **LSP:** `MAE_LSP_RUST`, `MAE_LSP_PYTHON`, `MAE_LSP_TYPESCRIPT`, `MAE_LSP_GO`

## Scheme Testing Framework

MAE has a headless test runner inspired by Emacs ERT/Buttercup and Neovim Plenary. Tests boot a real editor (no mocks) and exercise the same Scheme API surface available to users.

### Running Tests
```bash
mae --test tests/crdt/              # CRDT sync tests
mae --test tests/editor/            # Editor feature tests
mae --test tests/collab-e2e/test_smoke.scm  # Single file
make test-scheme-crdt               # CRDT tests (builds first)
make test-scheme-editor             # Editor tests
make test-scheme-all                # All local tests
```

### Architecture (3 layers)
1. **`scheme/lib/mae-test.scm`** — BDD library: `describe-group`/`it-test`/`should`/TAP output
2. **`crates/mae/src/test_runner.rs`** — Rust orchestrator: iterates tests, syncs state between steps
3. **`crates/scheme/src/runtime.rs`** — Scheme primitives for buffer mutation + state inspection

### Writing Tests
```scheme
(describe-group "Feature name"
  (lambda ()
    (it-test "setup"
      (lambda ()
        (create-buffer "*test-feature*")))
    (it-test "do something"
      (lambda ()
        (buffer-insert "hello")))
    (it-test "verify result"
      (lambda ()
        (should-equal (buffer-string) "hello")))))
;; No (run-tests) — Rust-side iteration handles state refresh
```

### Design Principles
- **Real editor, not mocks.** Tests boot headless with full event loop. Same API for tests and users.
- **Real event loops for event-loop behavior.** When behavior depends on the event loop (hooks firing, async yields, mode transitions with side effects), tests MUST exercise the actual event loop — not synthetic flushes or manual drain calls. A test that manually calls `drain_hook_evals` is testing the drain function, not the hook system. If behavior is tied to the event loop, spawn a real editor instance (PTY or MCP) and test through it. Never create synthetic event triggers to avoid using the event loop.
- **One pending op per test step.** Each `it-test` is one eval→apply cycle. `buffer-insert` + `goto-char` in the same step may execute in unexpected order. Split into separate steps.
- **SharedState pattern for cross-test reads.** Functions like `buffer-string`, `buffer-sync-enabled?`, `current-mode`, and `get-buffer-by-name` read from `Arc<Mutex<SharedState>>` (not closure-captured snapshots) so they see fresh state after `sync_scheme_state`.
- **Assertions signal errors.** `should`/`should-equal`/`should-contain` signal Scheme errors caught by the runner. Use `should-mode` for mode checks.
- **File-boundary state isolation.** The runner snapshots global editor state (mode, keymap_flavor, default_mode, line_numbers, word_wrap) before each test file and auto-restores after. Cross-file pollution is caught and warned: `# warning: test_foo.scm leaked global state (auto-restored): mode: Normal → Insert`. Tests that change flavor/mode/options should still restore them (the snapshot is a safety net, not a substitute for proper cleanup).
- **TAP v14 output.** Machine-parseable, CI-friendly.
- **Rust-side iteration preferred.** Don't add `(run-tests)` at end of test files. The runner calls `run-nth-test` with `apply_to_editor` + `sync_scheme_state` between each step.
- **Clean environment for e2e tests.** The e2e tests run in CI with no user config (`init.scm`, `config.toml`) and no on-disk modules. When testing locally, use a clean HOME: `HOME=/tmp/mae-test XDG_CONFIG_HOME=/tmp/mae-test/.config XDG_DATA_HOME=/tmp/mae-test/.local/share ./target/release/mae --test tests/editor/`

### Adding New Test Primitives
- **Read-only state**: Add to `SharedState`, register Rust function in `new()` that reads from SharedState, update SharedState in `inject_editor_state`.
- **Mutations**: Add pending field to `SharedState`, register Scheme function that sets it, process in `apply_to_editor`.

## Developing MAE Inside MAE (MCP Tools)

All 130+ MAE editor tools are exposed via MCP with full parity — the same tools the built-in AI agent uses. When developing MAE with Claude Code connected via the MCP shim (`mae-mcp-shim`), prefer these tools over raw file reads for structured editor operations.

### Connection

Socket path: `/tmp/mae-{PID}.sock` (per-process, stale sockets cleaned on startup).
Shim: `mae-mcp-shim` — translates MCP JSON-RPC over stdio to the Unix socket.

### Code Navigation (LSP)

| Tool | Purpose |
|------|---------|
| `lsp_definition` | Go to definition (structured file + position) |
| `lsp_references` | Find all references to symbol at point |
| `lsp_hover` | Type info / docs for symbol |
| `lsp_workspace_symbol` | Search symbols across workspace |
| `lsp_document_symbols` | List all symbols in current buffer |
| `lsp_diagnostics` | Current errors/warnings from LSP |

### Debugging (DAP)

| Tool | Purpose |
|------|---------|
| `dap_start` | Launch or attach debug session |
| `dap_set_breakpoint` | Set breakpoint (conditional/logpoint) |
| `dap_continue` / `dap_step` | Control execution |
| `debug_state` | Inspect stack frames, variables |

### Knowledge Base

| Tool | Purpose |
|------|---------|
| `kb_search` | Full-text search across all KB nodes |
| `kb_get` | Fetch a specific node by ID (supports block-level: `concept:buffer#3`) |
| `kb_links_from` / `kb_links_to` | Navigate the typed link graph |
| `kb_graph` | Neighborhood subgraph around a node |
| `kb_search_context` | RAG-style ranked excerpts for architecture questions |
| `kb_agenda` | Agenda queries: todo, priority, tag, stale, orphan, dead-end, custom Datalog |
| `kb_health` | Structured health report (node/link counts, orphans, broken links, hubs) |
| `kb_history` | Node version history (snapshots on each update) |
| `kb_restore` | Restore a node to a previous version |
| `kb_view_query` | Execute a stored Datalog view (kanban, backlog, sprint, agenda) |
| `kb_raw_query` | Execute arbitrary CozoDB Datalog against the KB |
| `kb_vector_search` | Vector similarity search (HNSW index, requires embeddings) |

Node ID namespaces: `cmd:*` (commands), `concept:*` (architecture), `lesson:*` (tutorial), `scheme:*` (Scheme API), `option:*` (editor options), `category:*` (categories), `task:*` (tasks), `view:*` (views), `meta:*` (meta-nodes).

### Collaboration / KB Sharing

| Tool | Purpose |
|------|---------|
| `collab_status` | Connection state, peer count, synced docs |
| `collab_connect` | Connect to a daemon for collab |
| `collab_share` | Share a buffer for collaborative editing |
| `collab_doctor` | Run connectivity diagnostics |
| `collab_list` | List shared documents on the server |
| `collab_discover` | Discover MAE peers via mDNS |
| `kb_sharing_status` | Introspect KBs + members/roles/policy/pending/my-role (call before managing) |
| `kb_share` | Share a KB for collaborative editing |
| `kb_join` | Join a shared KB from the server |
| `kb_leave` | Leave a shared KB (local copy preserved) |
| `kb_add_member` / `kb_remove_member` | Add/remove a member by fingerprint (owner-only) |
| `kb_approve` | Approve a pending join request as a role (owner-only) |
| `kb_set_policy` | Set join policy: restrictive\|invite\|permissive (owner-only) |

KB-sharing lifecycle is also first-class in Scheme: `(kb-share)`, `(kb-join)`,
`(kb-leave)`, `(kb-add-member)`, `(kb-remove-member)`, `(kb-approve)`,
`(kb-set-policy)`, `(kb-sharing-status)`. The `*KB Sharing*` buffer (`SPC C K m`),
the Scheme primitive, and the MCP tool all read the same introspection snapshot.

### Buffer / Editor

| Tool | Purpose |
|------|---------|
| `buffer_read` / `buffer_write` | Read/edit buffer contents |
| `project_search` | Ripgrep across project files |
| `command_list` | List all registered commands |
| `execute_command` | Dispatch any editor command |
| `eval_scheme` | Evaluate Scheme expression |
| `audit_configuration` | Structured config health report |
| `introspect` | Diagnostic snapshot of editor state |

### Model Exam

| Tool | Purpose |
|------|---------|
| `model_exam` | Run deterministic tool-calling exam (`action=plan` / `action=grade`) |

### Validation

`self_test_suite` returns the structured JSON test plan. Execute each test by calling the listed tools and checking assertions. Categories: `introspection`, `editing`, `git`, `help`, `project`, `lsp`, `dap`, `babel`, `guidance`, `performance`, `scrolling`.

`model_exam` provides a 12-test deterministic exam (6 categories) for validating model tool-calling capabilities. Results auto-save to `~/.local/share/mae/exam-results/`. See [MODEL_SUPPORT.md](docs/MODEL_SUPPORT.md).

### When to Use

- **Navigating MAE's own code**: `lsp_definition` / `lsp_references` over raw grep — structured results, no false positives.
- **Understanding architecture**: `kb_search "window group"` or `kb_get "concept:window"` — curated docs, not raw source.
- **Debugging MAE**: `dap_start` with `lldb-dap` for Rust, `debug_state` for stack inspection.
- **Testing changes**: `execute_command` to trigger commands, `self_test_suite` for structured E2E.

### Tool Selection: LSP vs Grep

When developing **inside MAE** (connected via `mae-mcp-shim`):
- **Prefer LSP tools** (`lsp_definition`, `lsp_references`, `lsp_hover`, `lsp_workspace_symbol`) for navigating Rust code — they give precise file+line+column with no false positives
- **Use `project_search`** (ripgrep) for cross-language text patterns, string literals, config values
- **Use `kb_search`/`kb_get`** for architectural concepts and documented workflows

When developing **outside MAE** (Claude Code directly on filesystem):
- Use built-in Grep/Glob/Read tools (faster, no event loop round-trip)
- LSP tools require the editor to be running with rust-analyzer connected

## Security

See `SECURITY.md` for the full security posture. Key points for development:

- **Permission tiers** are enforced before tool execution — no bypass vectors exist
- Use `api_key_command` with a password manager, not plaintext `api_key` in config
- MCP socket (`/tmp/mae-{PID}.sock`) uses Unix permissions only — no per-client auth
- Transcripts in `~/.local/share/mae/transcripts/` contain raw tool output (no secret scrubbing)
- Shell blocklist is substring-based and bypassable — defense in depth, not a sandbox

## Server-Client Architecture

MAE's MCP server supports multiple concurrent clients over Unix domain sockets.
Each client gets its own session with capability negotiation and state subscriptions.

### Protocol
- JSON-RPC 2.0 with Content-Length framing (LSP-compatible)
- Session lifecycle: `initialize` → `notifications/initialized` → ready → `shutdown`
- Heartbeat: `$/ping` returns `"pong"`, idle detection via `last_activity`
- Backpressure: per-client bounded queues (100 events), write timeout (5s)

### State Notifications
Clients subscribe to event types via `notifications/subscribe`: `buffer_edit`,
`cursor_move`, `diagnostics`, `mode_change`, `buffer_open`, `buffer_close`,
`sync_update`, `peer_joined`, `peer_left`, `save_committed`.
Events carry version numbers for ordering. Slow clients are dropped, not blocked.

### File Safety
- Content-hash verification on save (SHA-256, catches mtime failures)
- Advisory file locks (`.{name}.mae.lock` with PID/hostname)
- inotify-based external change detection (existing `notify` infrastructure)
- Git worktree isolation for multi-AI workflows

### Architecture Decision Records
ADRs live in `docs/adr/` and as KB concept nodes (`concept:adr-*`).
See ADR-001 (protocol), ADR-002 (text sync — accepted: yrs), ADR-003 (file safety), ADR-004 (KB scaling), ADR-005 (KB CRDT), ADR-006 (collaborative state engine), ADR-007 (save coordination), ADR-008 (CRDT target metrics).

### Sync Engine (yrs — Accepted)
Collaborative state uses **yrs** (Yjs Rust port, YATA algorithm). Decision rationale:
- Handles text (`YText`), visual documents (`YMap`/`YArray`), and KB nodes
- Built-in `UndoManager` with per-user stacks
- Proven at scale: Notion (200M+ users), Excalidraw, TLDraw
- Dual structure: yrs is source of truth, ropey is rendering mirror

Transport: JSON-RPC 2.0 with Content-Length framing over TCP (port 9473) and Unix sockets.
Planned upgrade path: msgpack wire format (Content-Type negotiation).

`mae-sync` wraps yrs with MAE-specific document schemas and provides the
ropey bridge. See ADR-006 for full architecture.

### Daemon (`mae-daemon`)

Unified background service: KB persistence (Unix socket) + collaborative editing (TCP). Replaces the former `mae-state-server` (merged in v0.13.2).

**Usage:**
```bash
mae-daemon                          # KB (Unix socket) + collab (TCP 9473)
mae-daemon --check-config           # validate configuration
mae-daemon doctor                   # run diagnostics
```

**Architecture:**
- Dual listener: Unix socket for KB queries, TCP for collab sync
- Per-document locking, WAL-first SQLite persistence, background compaction
- PSK mutual authentication (HMAC-SHA256, `mae_mcp::auth`)
- Transport-generic I/O: `mae_mcp::{read_message, write_framed, handle_request}`

**Config:** `~/.config/mae/daemon.toml` (TOML, XDG-compliant). Legacy: auto-reads `state-server.toml` if `daemon.toml` not found.

**Editor commands (SPC C prefix, doom keymap):**
- `collab-start` (SPC C s), `collab-connect` (SPC C c), `collab-disconnect` (SPC C d)
- `collab-status` (SPC C i), `collab-share` (SPC C S), `collab-sync` (SPC C y), `collab-doctor` (SPC C D)

**Systemd:** `assets/mae-daemon.service` (user unit)

## API Stability

These APIs are intended to remain stable through v1.0:

- **Scheme API:** ~50 functions + ~25 variables (see `:help concept:scheme-api`)
- **Hooks:** 25 hook points (see `:help concept:hooks`)
- **MCP tools:** 135+ tools, categorized (core/lsp/dap/kb/shell/ai/commands/git/web/visual/debug/collab)
- **Config options:** 91+ registered, persistable via `:set-save`

## Related Resources

- **Full architecture spec:** `README.md`
- **Emacs source for reference:** the Emacs source tree (clone of emacs-mirror/emacs, `emacs-30` branch)
- **Declarative project config:** `.project` in repo root (for declarative-project-mode in Emacs)
- **ropey:** https://github.com/cessen/ropey — rope data structure for buffer management
- **ratatui:** https://github.com/ratatui/ratatui — terminal UI framework
- **tree-sitter-org:** org-mode grammar for tree-sitter
