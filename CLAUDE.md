# AI Development Guide (CLAUDE.md) — Modern AI Editor (MAE)

> [!CAUTION]
> **MAE is in early Alpha.** AI features and cost guardrails are experimental and may fail. Always monitor your API usage and costs directly in your provider dashboards.

## What This Project Is

An AI-native lisp machine editor — a successor to GNU Emacs where the human user and an AI agent are **peer actors** calling the same Lisp primitives. The editor is built on a Rust core with an embedded Scheme (R7RS-small) runtime. LSP and DAP are first-class protocols exposed to both the Scheme extension layer and the AI agent's tool-calling interface.

The project README is an org-roam node symlinked from `~/RoamNotes/20260415142357-ai_native_lisp_machine_editor.org`. That file contains the full architecture spec, stack rationale, and Emacs git history analysis. **Read it before starting any work.**

## Stack

- **Language:** Rust (core) + Scheme R7RS-small (extensions)
- **License:** GPL-3.0-or-later
- **Build:** `make check` / `make build` / `make test` / `make ci` from workspace root
  - `make build` now builds with GUI by default (`--features gui`)
  - `make build-tui` for terminal-only build
  - `make ci` still excludes GUI (skia system deps)
  - `make check-config` validates init.scm + config.toml without launching the editor
- **Self-test:** Call the `self_test_suite` MCP tool to get the structured JSON test plan, then execute each test by calling the listed MCP tools and checking assertions. If MCP is unavailable, fall back to `make self-test` (headless). Categories: `introspection`, `editing`, `help`, `project`, `lsp`, `scrolling`.

## Crate Layout

| Crate | Purpose | Key Dependencies (planned) |
|---|---|---|
| `mae-core` | Buffer management (rope), event loop, core primitives | `ropey`, `crossbeam` |
| `mae-renderer` | Display/rendering — `Renderer` trait + terminal backend | `ratatui`, `crossterm` |
| `mae-gui` | GUI rendering backend — winit window + Skia 2D + native SVG | `winit`, `skia-safe` (features: `svg`) |
| `mae-scheme` | Embedded Scheme runtime for configuration and packages | `steel` (or purpose-built) |
| `mae-lsp` | LSP client — types, references, diagnostics exposed to Scheme + AI | `tower-lsp` or `lsp-types` |
| `mae-dap` | DAP client — breakpoints, call stacks, variables exposed to Scheme + AI | `dap-types` |
| `mae-ai` | AI agent integration — tool-calling transport (Claude/OpenAI/Gemini/DeepSeek) | `reqwest`, `serde_json` |
| `mae-kb` | Knowledge base — graph store, org parser, bidirectional links | `rusqlite`, `tree-sitter`, `tree-sitter-org` |

## Architecture Principles

These are derived from analysis of 35 years of Emacs git history. They are non-negotiable design constraints:

1. **Concurrency from day one.** Emacs spent 23,901 commits across 3 branches trying to retrofit a concurrent GC and still hasn't merged it. We use Rust's ownership model for the core and a purpose-designed concurrent GC for the Scheme runtime. No Global Interpreter Lock, ever.

2. **Modular display layer.** Emacs's `xdisp.c` is 38,605 lines and the most bug-prone file in the codebase. Our renderer is a separate crate with a clean trait-based HAL. Platform-specific code lives in the rendering backend library (crossterm/wgpu), not in our codebase.

3. **The AI is a peer, not a plugin.** The AI agent calls the same Scheme functions as the user's keybindings. `(buffer-insert ...)`, `(lsp-references ...)`, `(dap-inspect-variable ...)` — same API surface for human and AI. No separate "AI mode" or simulated keystrokes.

4. **LSP and DAP are first-class.** Not bolted-on packages. The AI gets structured semantic knowledge (types, references, diagnostics from LSP) and runtime debug state (call stacks, variables from DAP) as part of its reasoning context.

5. **Module boundaries enable distributed ownership.** Each crate has a clear responsibility. No 10k+ line files. This is a direct response to Emacs's bus-factor problem (top 5 contributors = 50.8% of all commits, critical subsystems maintained by single individuals).

6. **Runtime redefinability is sacred.** Users must be able to redefine any function while the editor is running. This is the property that makes Emacs irreplaceable. The Scheme layer provides `defadvice`-equivalent, live REPL, and hot reload.

### Rendering Pipeline
The GUI renderer uses a three-phase pipeline: `compute_layout()` produces
a `FrameLayout`, `render_buffer_content()` draws text, and `render_cursor()`
positions the cursor. All three MUST consume the same `HighlightSpan` set.
See `crates/gui/src/RENDERING.md` for detailed rules.

## Development Priorities

Start terminal-only. Skip GUI until the model works.
Granular milestone tracking lives in **ROADMAP.md**.

### Phase 1: Core + Renderer (MVP) — COMPLETE
- Buffer type using ropey (insert, delete, cursor movement)
- Event loop (keyboard input → command dispatch)
- Terminal renderer via ratatui/crossterm
- Basic modal editing (vi-like normal/insert modes)
- Single-file editing with save/load

### Phase 2: Scheme Runtime — COMPLETE
- Steel embedded as the extension language
- Buffer operations exposed to Scheme
- Config file loading (`init.scm`)
- Command binding from Scheme (`(define-key ...)`)
- REPL / eval-expression (`:eval`)

### Phase 3: AI Integration — COMPLETE
- Tool-calling transport (Claude, OpenAI, Gemini, and DeepSeek APIs)
- Scheme API surface mapped to AI tool definitions (10 tools + all commands)
- AI can read/edit buffers, navigate, execute commands, inspect editor state
- Conversation buffer with streaming, tool call display
- Permission tiers (ReadOnly/Write/Shell/Privileged)

### Phases 3d–3h: Editor Essentials + AI Multi-File + Hardening + Vim Parity — COMPLETE (1148 tests)
- Full vi modal editing: visual mode, text objects, marks, macros, count prefix, search, dot-repeat
- Multi-file AI tools: open_file, switch_buffer, project_search, create_file
- Conversation persistence: :ai-save / :ai-load with versioned JSON schema
- Architecture hardened: editor.rs split into 9 submodules, all growth bounded,
  AI security (blocklist, circuit breaker, backpressure), error handling audited
- Registers & clipboard, vim-surround, Scheme REPL, AI prompt UX, command palette
- **v0.4.1**: Second modularization pass — 6 god files split into module directories (key_handling, main, tools, executor, session), 12 code smell fixes, model-agnostic system prompt (1,590 tests)
- **v0.5.0**: Agent reliability — progress checkpoint system (semantic stagnation detection), softened oscillation detector (warn-then-abort), self-test mode, watchdog recovery (cancel AI on prolonged stall), prompt caching (Claude cache_control), token budget dashboard (cache hit rate, context utilization), context compaction (extractive summarization before hard trimming), graceful degradation (auto-shed tools at >85%/92% context pressure), web_fetch tool, ANSI-only themes (light-ansi, dark-ansi), XDG-compliant transcript logging, DAP observability (enriched timeouts, protocol tracing, failure guidance) (1,673 tests)

### Phase 4: LSP + DAP + Syntax + KB — COMPLETE (M1-M4 each)
- LSP client: connection, navigation (gd/gr/K), diagnostics, completion popup ✅
- DAP client: protocol types, breakpoints, step/continue, AI debug tools ✅
- Tree-sitter syntax highlighting: 13 languages, structural selection ✅
- Gutter rendering: breakpoints, execution line, diagnostic severity markers ✅
- Knowledge base: in-memory graph, SQLite persistence, org-mode parser, help system, AI KB tools ✅
- LSP AI tools: `lsp_definition`, `lsp_references`, `lsp_hover`, `lsp_workspace_symbol`, `lsp_document_symbols` ✅
- Debug panel UI complete ✅

### Phase 5: Knowledge Base — COMPLETE
- SQLite-backed graph store with FTS5
- Org-mode parser (hand-rolled, multi-node files)
- Bidirectional link primitives
- KB queries from Scheme and AI
- Help buffer with navigation, link following, neighborhood display

### Phase 6: Embedded Shell — COMPLETE (M1-M4 + MCP bridge + file auto-reload)
- Terminal emulator via `alacritty_terminal` (full VT100, colors, attributes) ✅
- ShellInsert mode, Ctrl-\ Ctrl-n exit, process lifecycle handling ✅
- Scheme hooks (7 hook points) + `set-option!` configuration ✅
- Shell integration + README overhaul ✅
- Scheme shell functions: `shell-cwd`, `shell-read-output`, `*shell-buffers*` ✅
- Send-to-shell: `SPC e s` (line), `SPC e S` (region) ✅
- MCP bridge: Unix socket server, JSON-RPC, stdio shim, tool re-export ✅
- File auto-reload: mtime tracking, clean buffer reload, dirty buffer warning, `file-changed-on-disk` hook ✅

### Phase 7: Embedded Documentation — IN PROGRESS (M1-M3)
- Scheme primitive KB nodes (scheme:*) for ~45 functions + 18 variables ✅
- Progressive getting-started tutorial (3 tracks: vim, beginner, AI) ✅
- `:help` fuzzy completion with expanded namespace fallback ✅
- `:help` Tab completion → HelpSearch palette ✅
- WhichKeyEntry doc field populated from CommandRegistry ✅
- User help nodes from `~/.config/mae/help/*.org` ✅
- `:help-edit` command for authoring user help ✅
- Org internal link jump bugfix ✅
- `reload-config` command implementation ✅
- AI Agent/Chat naming UX (which-key, splash, tutorial) ✅
- Layered init.scm loading (user → project, error isolation) ✅
- `after-load` hook ✅
- `--debug-init` CLI flag + verbose init logging ✅
- `:describe-configuration` health report command ✅
- `audit_configuration` MCP tool (structured JSON) ✅
- `--check-config --report` CLI extension ✅

### Phase 8: GUI Rendering Backend — M1-M7 COMPLETE (2,389 tests)
- `Renderer` trait extracted: backend-agnostic HAL for terminal + GUI ✅
- `InputEvent` type: backend-agnostic input abstraction in mae-core ✅
- `mae-gui` crate: winit + skia-safe, monospace text, theme colors ✅
- Configurable shell exit sequence: shell-insert keymap (not hardcoded) ✅
- Configurable AI permission tier: config.toml + `MAE_AI_PERMISSIONS` env var ✅
- GUI event loop: `run_app` + `EventLoopProxy<MaeEvent>` (Alacritty pattern) ✅
- Full keyboard input in GUI: all modes, shell-insert, modifier tracking ✅
- CI exclusion: `mae-gui` excluded from workspace CI (skia system deps) ✅
- init.scm fix: inject editor state before Scheme evaluation ✅
- Self-test suite: `:self-test`, `self_test_suite` MCP tool, `--self-test` CLI flag ✅
- Input lock during AI operations (Esc/Ctrl-C to cancel) ✅
- CWD-based project detection at startup ✅
- AI tool parity: `ai_save`, `ai_load`, `rename_file`, `close_buffer` force param ✅
- Ex-command registry parity: 6 commands registered for AI access ✅
- LSP AI tools: deferred async resolution for definition/references/hover/symbols ✅
- Event loop refactor: shared `ai_event_handler` + `shell_lifecycle` modules ✅
- GUI visual polish: cursor, status bar, splash screen, mouse, shell scrollback ✅
- Desktop launcher (.desktop + SVG icon) for GNOME/sway ✅
- Font size config, FPS overlay, :set/:edit-config, ZZ/ZQ ✅
- OptionRegistry: single source of truth for all editor options ✅
- `describe-option` command + `SPC h o` binding ✅
- `:set-save` — persist option changes to config.toml ✅
- Event loop refactor: `run_app` + `EventLoopProxy<MaeEvent>` replaces `pump_app_events` ✅
- `GuiApp` owns all state, `bridge_task` on background tokio thread ✅
- `main()` is plain `fn` — tokio runtime built manually ✅
- v0.3.0 polish: BackTab, font zoom, `:read !cmd`, per-buffer projects, status line (git/LSP/tier), AI agent launcher, session persistence, sample config ✅
- Tutor→KB: 11 linked lesson nodes, `:tutor` opens help with Tab/Enter/C-o navigation ✅
- Shell auto-close on exit (no blank frames), agent shells tagged for distinct cleanup ✅
- Shell CPU idle: generation-based dirty tracking (30%→~0%) ✅
- `find-file` uses project root with CWD fallback ✅
- Debug stats show FPS instead of μs frame timing ✅
- Debugging powerhouse: watchdog thread, introspect AI tool, event recording, DAP attach/evaluate ✅
- Conditional/logpoint breakpoints, lock contention tracking, anomaly detection ✅
- Doom-style init.scm: 8 sections, all 14 options, hooks, keybindings, AI config ✅
- Tutor KB: 12 lessons (added Debugging + Observability), 4 new concept nodes ✅
- `--check-config` CLI flag + CI E2E validation step ✅
- Magit-style status buffer, swap files, display optimization, variable-height polish ✅
- Mouse focus: click-to-focus, scroll-under-mouse, idle deferred work ✅
- Inline images: PNG/JPG/SVG rendering below text lines, org-mode `[[file:...]]` auto-preview ✅
- Native SVG rendering via skia `svg::Dom` — vector text, perfect scaling, same font stack as editor ✅
- Smooth sub-line scrolling past images, viewport clipping, scroll guard fixes ✅
- Rich content: multi-cursor edits, TUI shift normalization ✅
- **Next: Org core interactivity (9 M1), PDF preview (8 M8)**

## Key Design Decisions Already Made

- **Scheme over other Lisps:** R7RS-small is close enough to elisp for a compatibility shim, has hygienic macros (superior to elisp's `defmacro`), proper tail calls, and first-class continuations. Janet was too limited on macros. Racket has the best language but worst embedding story. Fennel/LuaJIT is proven (Neovim) but fragile upstream.

- **Rust over other cores:** Eliminates the GC problem entirely. Zig was considered (simpler FFI, comptime) but has a smaller ecosystem and less mature async story. C/C++ would repeat Emacs's mistakes.

- **GPL-3.0-or-later:** Copyleft ensures the project stays open. No FSF copyright assignment — contributions are owned by their authors.

- **Terminal-first:** ratatui/crossterm for initial development. GPU rendering (wgpu) is a future enhancement, not a prerequisite.

## Emacs Lessons (Reference Data)

These findings from analyzing the Emacs git repo at `~/src/emacs` motivated our architecture:

- **Fix ratio climbed from 15% to 32%** over 35 years — a complexity ceiling from C + untyped Lisp. Rust's type system structurally prevents this.
- **`xdisp.c`: 38,605 lines, 20k+ commits/decade** — the display engine is a monolithic maintenance black hole. We use a modular renderer crate.
- **IGC/MPS: 23,901 commits across `feature/igc`, `igc2`, `igc3`** — still unmerged after 3 iterations. GC retrofit is intractable. We avoid needing one.
- **Bus factor ~4 people** — top 5 = 50.8% of commits. Single-person dependencies on native-comp (Corallo), tree-sitter (Yuan Fu), Android (Po Lu), Tramp (Albinus). We enforce module boundaries.
- **~10% of all commits are platform support** — separate `*term.c` files per platform. We delegate to crossterm/wgpu.
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

## Developing MAE Inside MAE (MCP Tools)

All 130+ MAE editor tools are exposed via MCP with full parity — the same tools the built-in AI agent uses. When developing MAE with Claude Code connected via the MCP shim (`mae-mcp-shim`), prefer these tools over raw file reads for structured editor operations.

### Connection

Socket path: `$XDG_RUNTIME_DIR/mae-mcp.sock` (typically `/run/user/$UID/mae-mcp.sock`).
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
| `kb_get` | Fetch a specific node by ID |
| `kb_links_from` / `kb_links_to` | Navigate the link graph |
| `kb_graph` | Neighborhood subgraph around a node |

Node ID namespaces: `cmd:*` (commands), `concept:*` (architecture), `lesson:*` (tutorial), `scheme:*` (Scheme API), `option:*` (editor options).

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

### Validation

`self_test_suite` returns the structured JSON test plan. Execute each test by calling the listed tools and checking assertions. Categories: `introspection`, `editing`, `help`, `project`, `lsp`, `scrolling`.

### When to Use

- **Navigating MAE's own code**: `lsp_definition` / `lsp_references` over raw grep — structured results, no false positives.
- **Understanding architecture**: `kb_search "window group"` or `kb_get "concept:window"` — curated docs, not raw source.
- **Debugging MAE**: `dap_start` with `lldb-dap` for Rust, `debug_state` for stack inspection.
- **Testing changes**: `execute_command` to trigger commands, `self_test_suite` for structured E2E.

## Related Resources

- **Full architecture spec:** `README.org` (symlink to org-roam node)
- **Emacs source for reference:** `~/src/emacs` (clone of emacs-mirror/emacs, `emacs-30` branch)
- **Declarative project config:** `.project` in repo root (for declarative-project-mode in Emacs)
- **Steel Scheme:** https://github.com/mattwparas/steel — primary candidate for embedded Scheme runtime
- **ropey:** https://github.com/cessen/ropey — rope data structure for buffer management
- **ratatui:** https://github.com/ratatui/ratatui — terminal UI framework
- **tree-sitter-org:** org-mode grammar for tree-sitter
