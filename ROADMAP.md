# MAE Roadmap

**Current version:** v0.9.0-dev · **Tests:** 3,186 passing · **Status:** Alpha — all 11 phases + Phase G complete, feature crate extraction done.

---

## Phase Summary

| Phase | Status | Summary |
|-------|--------|---------|
| 1. Core + Renderer | ✅ Complete | Buffer (rope), event loop, terminal renderer, vi modal editing |
| 2. Scheme Runtime | ✅ Complete | Steel R7RS-small, `init.scm`, `define-key`, `set-option!`, REPL |
| 3. AI Integration | ✅ Complete | Claude/OpenAI/Gemini/DeepSeek, 450+ tool-calling, conversation UI, permissions |
| 4. LSP + DAP + Syntax | ✅ Complete | Full LSP (rename, format, outline, breadcrumbs, peek), DAP (watches, exceptions), 13-language tree-sitter |
| 5. Knowledge Base | ✅ Complete | SQLite + FTS5, org parser, 200+ nodes, bidirectional links, federation |
| 6. Embedded Shell | ✅ Complete | alacritty_terminal, MCP bridge, file auto-reload, send-to-shell |
| 7. Documentation | ✅ Complete | Tutor (13 lessons), `:describe-configuration`, `--check-config`, `--init-config` |
| 8. GUI Backend | ✅ Complete | winit + Skia 2D, inline images (PNG/JPG/SVG), variable-height, inertial scroll |
| 9. Babel + Export | ✅ Complete | 12-language executor, HTML/Markdown export, noweb, tangle, KB federation |
| 10. AI Agent Efficiency | ✅ Complete | Tiered prompts, provider-aware hints, target dispatch, frame profiling |
| 11. Module System | ✅ Complete | 9 modules extracted (Doom model), `module.toml` manifests, `mae pkg` CLI, flags, live reload |

---

## Known Bugs

- [ ] **AI output buffer cursor invisible in GUI**: After AI responds, the cursor in the `*ai*` conversation output buffer is not visible. Root cause: buffer type / layout metadata mismatch — the conversation buffer doesn't provide the same state that the cursor renderer expects. Low priority (output buffer is read-only, navigation still works).
- [ ] **Theme load failure is silent in headless mode**: If config.toml requests a nonexistent theme, `set_theme_by_name()` shows a status bar message but keeps the current theme. In CI/headless mode the user gets zero feedback. Should log to stderr or return non-zero exit from `--check-config`.

---

## In Progress / Planned

### Phase G: Feature Crate Extraction + Babel Gaps (v0.9.0) — COMPLETE

- [x] `mae-babel` crate extracted from `mae-core` (7 files, zero import changes downstream)
- [x] `mae-export` crate extracted from `mae-core` (3 files, depends on `mae-babel`)
- [x] Block ref resolution: `resolve_block_ref()` reads cached `#+RESULTS:` (was stub)
- [x] Persistent REPL sessions: `SessionManager` with sentinel-based output capture
- [x] Per-language backends: `LanguageBackend` trait + 4 implementations (shell, script, compiled, internal)
- [x] Compiled backend: hash-based binary caching in `~/.cache/mae/babel/`
- [x] `PendingSchemeEval` variant: scheme blocks routed to editor runtime
- [x] Babel edit-special: `SPC m '` opens src block in dedicated buffer with language mode
- [x] Babel edit-commit: `SPC m '` in edit buffer writes body back to source

### Near-term
- [ ] Server-client architecture refactoring and hardening
- [ ] PDF preview (GUI inline rendering via `hayro` pure-Rust rasterizer + midnight mode)
- [ ] Semantic code search (vector embeddings)
- [x] Org ↔ Markdown bidirectional conversion (`:markdown-to-org`, `:org-to-markdown`)
- [ ] Investigate `bincode` unmaintained dependency (RUSTSEC-2025-0141) — transitive via `steel-core`; evaluate alternatives (`bitcode`, `postcard`) or upstream Steel fix

### Doom Parity Roadmap: Future Feature Crates

**Tier 1: High-value, self-contained (next 2-3 releases)**

| Crate | Doom Equivalent | Description |
|-------|----------------|-------------|
| `mae-snippets` | `:editor/snippets` | YASnippet-style templates with tab-stops, mirrors, transforms |
| `mae-spell` | `:checkers/spell` | Spellcheck via hunspell/aspell, inline markers, `z=` suggest |
| `mae-format` | `:editor/format` | Formatter bridge (prettier, black, rustfmt) — complements LSP format |
| `mae-lookup` | `:tools/lookup` | Unified lookup: LSP def + docs URL + man + Dash/Devdocs |
| `mae-make` | `:tools/make` | Build runner: detect Makefile/Cargo.toml/package.json, `SPC c b` |

**Tier 2: Language IDE modules (Scheme, not Rust crates)**

| Module | Doom Equivalent | What it configures |
|--------|----------------|-------------------|
| `lang-python` | `:lang/python` | pyright/pylsp, debugpy, virtualenv, REPL |
| `lang-rust` | `:lang/rust` | rust-analyzer, lldb-dap, cargo, test runner |
| `lang-go` | `:lang/go` | gopls, dlv, go commands |
| `lang-javascript` | `:lang/javascript` | ts-ls, chrome-debugger, npm |
| `lang-cc` | `:lang/cc` | clangd, lldb-dap, cmake |

**Tier 3: Editor enhancement crates (future releases)**

| Crate | Doom Equivalent | Description |
|-------|----------------|-------------|
| `mae-dired` | `:emacs/dired` | Buffer-based file manager |
| `mae-undo-tree` | `:emacs/undo` | Undo tree visualization |
| `mae-workspace` | `:ui/workspaces` | Named workspace sessions, per-project layouts |
| `mae-zen` | `:ui/zen` | Distraction-free writing mode |

### AI
- [x] Semantic tool search (`search_tools` — fuzzy match over 146+ tool names/descriptions)
- [x] Dynamic MCP tool discovery (external MCP server connections, `mcp_{server}_{tool}` namespacing)
- [x] `request_tools` accepts specific tool names (completes search→request workflow)
- [x] Memory synthesis (`synthesize_memory()` in bootstrap.rs — categorizes/deduplicates/budgets memory per model)
- [x] Verification specialist (verifier profile, `AiCommand::Delegate`, scoped tools)
- [ ] AI session playback & undo (step-through replay of code changes)
- [x] Network status command (`:ai-ping`, `connectivity_check()` — HTTP HEAD, no LLM round-trip)
- [ ] `:mcp-status` / `:mcp-reconnect` commands (MCP server management UI)

### Org-mode
- [ ] Table formulas (`#+TBLFM:` with Calc-like syntax)
- [ ] Table sorting (alphabetic, numeric, time)
- [ ] Table import/export (CSV/TSV, org ↔ markdown)

### Editor
- [ ] PR comment summaries (auto-summarize changes on PR amend)
- [ ] Free AI-assisted setup (Gemini free tier for first-run guidance)
- [ ] Step-through command execution (inspect AI tool call stdin/stdout)

### Keymap Architecture Migration

> **Goal**: Kernel provides only vi-modal primitives. All leader-key (SPC) bindings move to keymap flavor modules.
>
> 1. Trim `keymaps.rs` to minimal vi: Escape, hjkl, operators, text objects, `:`, search
> 2. Make `keymap-doom` the sole source of the SPC tree
> 3. Ship `keymap-emacs` and `keymap-minimal` flavor modules
> 4. Auto-load the selected `keymap_flavor` module regardless of `(mae!)` declarations
> 5. Expose `(clear-keymap-prefix)` for users who want to override, not just extend
> 6. Group names (`set-group-name`) should come from the keymap flavor module, not the kernel

### Architecture Debt (v0.9.1+)

- [ ] **Editor struct field extraction**: ~100+ fields accumulating (Emacs buffer.c trajectory). Extract into named sub-structs: `LspContext` (7 fields), `DapContext` (3+ fields), `ModuleContext` (4 fields), `RenderContext` (5+ fields). Keeps LOC flat, improves cohesion.
- [ ] **dispatch/ui.rs split**: At 1,141 lines, "UI" dispatch is a semantic dumping ground (config, themes, terminal, help, registers, options, toggles, projects, AI). Split into dispatch/config.rs, dispatch/terminal.rs, dispatch/project.rs, dispatch/help.rs.
- [ ] **Custom theme filesystem loading**: Only bundled themes work. No user theme search path (~/.config/mae/themes/). Emacs, Vim, Helix all support this.
- [ ] **Binding ownership audit**: Every kernel-dispatched command should have a kernel default binding. Module bindings are for module-specific commands or user-facing overrides only.
- [ ] **Ad-hoc solution review**: Thorough code review for hardcoded values, duplicated logic between TUI/GUI, and workarounds that should be proper abstractions — in prep for server-client architecture.
- [ ] **Which-key idle delay**: Wire `which-key-idle-delay` option to event loop timer (default 0ms = immediate).

---

## Completed Features (collapsed)

<details>
<summary>Phase 3 details — AI Integration + Editor Essentials (v0.3–v0.4.1)</summary>

- Tool-calling transport (Claude, OpenAI, Gemini, DeepSeek APIs)
- 450+ commands mapped to AI tool definitions
- Conversation buffer with streaming, tool call display
- Permission tiers (ReadOnly/Write/Shell/Privileged)
- Full vi modal editing: visual mode, text objects, marks, macros, dot-repeat
- Multi-file AI tools, conversation persistence
- Editor.rs split into 9 submodules, AI security (blocklist, circuit breaker)
- Registers, clipboard, vim-surround, command palette
- Second modularization pass (6 god files → module directories)

</details>

<details>
<summary>Phase 5 details — AI Reliability (v0.5.0)</summary>

- Progress checkpoint system (semantic stagnation detection)
- Softened oscillation detector (warn-then-abort)
- Watchdog recovery (cancel AI on prolonged stall)
- Prompt caching (Claude cache_control, OpenAI cached_prompt_tokens)
- Token budget dashboard (cache hit rate, context utilization)
- Context compaction (extractive summarization before hard trimming)
- Graceful degradation (auto-shed tools at >85%/92% context pressure)
- Web fetch tool, ANSI-only themes, XDG transcript logging

</details>

<details>
<summary>Phase 6 details — Embedded Shell + MCP</summary>

- Terminal emulator via `alacritty_terminal` (full VT100, colors)
- ShellInsert mode, Ctrl-\ Ctrl-n exit, process lifecycle
- MCP bridge: Unix socket server, JSON-RPC, stdio shim
- File auto-reload: mtime tracking, dirty buffer warning
- Send-to-shell: `SPC e s` (line), `SPC e S` (region)

</details>

<details>
<summary>Phase 8 details — GUI Backend (v0.6.0–v0.7.0)</summary>

- winit + skia-safe rendering, full keyboard/mouse input
- Pixel-based variable-height lines, cached lazy theme resolution
- Native SVG via `skia_safe::svg::Dom`
- Code folding, file tree sidebar, Magit-style git status
- Org/Markdown structural editing (cycle, promote/demote, narrow/widen)
- Autosave, swap files, crash recovery
- Multi-cursor, inline markup rendering, display overlays
- Desktop launcher (.desktop + SVG icon)
- Large document performance (binary search, graceful degradation)
- Per-phase render timing, frame profiling, scroll stress benchmarks

</details>

<details>
<summary>Phase 9–10 details — Babel, Export, Agent Efficiency</summary>

- 12-language babel executor, noweb expansion, tangle directive
- HTML/Markdown export with TOC, syntax highlighting, tag filtering
- Tiered prompt system (Full/Compact), provider-aware hints
- AI target dispatch (`save-excursion` pattern)
- Org table editing (align, navigate, insert/delete row/column)
- LSP+DAP polish: rename, format, symbol outline, breadcrumbs, watch expressions

</details>

<details>
<summary>Phase 11 details — Module System + KB Federation (v0.9.0)</summary>

- 9 modules extracted: dashboard, surround, marks-jumps, search, registers, macros, tables, multicursor, file-tree
- Doom Emacs-inspired three-file model: `module.toml`, `autoloads.scm`, `init.scm`
- `mae pkg` CLI: list, doctor, info, create, sync, upgrade
- Module flags (`+flag` syntax), dependency resolution, live reload
- `describe-module`, `describe-mode`, `describe-bindings` introspection commands
- KB federation fully wired: `:kb-register`, `:kb-unregister`, `:kb-reimport`, `:kb-instances`
- Recursive org-dir import with health metrics (orphans, broken links, namespaces)
- Federated search across local KB + N external instances
- AI tools: `kb_register`, `kb_unregister`, `kb_reimport` with structured JSON responses
- Registry persistence (`~/.config/mae/kb-registry.toml`), startup auto-load
- KB documentation: `concept:kb-federation`, `concept:kb-workflows`, `concept:kb-vs-alternatives`
- Tutorial: `lesson:kb-import-roam` (Lesson 13)
- Self-test categories: `modules`, `federation`

</details>

---

## Architecture Invariants

These are non-negotiable constraints derived from Emacs git history analysis:

1. **No file exceeds 3,500 lines** — Emacs's `xdisp.c` is 38,605 lines. We enforce module splits. (Exception: `window.rs` at ~3,434 lines — variable-height line math and smooth scrolling justify the size. Under active monitoring.)
2. **No Global Interpreter Lock** — Emacs spent 23,901 commits on GC retrofit. We use Rust ownership.
3. **AI is a peer, not a plugin** — same `dispatch_builtin()` for human and AI.
4. **Module boundaries enable distributed ownership** — 11 crates with clear responsibilities.
5. **Runtime redefinability is sacred** — Scheme `defadvice`, live REPL, hot reload.
