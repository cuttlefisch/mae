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

### Near-term: Server-Client Architecture

- [ ] **Multi-AI file contention protocol**: When multiple AI-assisted editors (MAE, VS Code + Copilot, Cursor, aider) operate on the same project simultaneously, file writes race, LSP state goes stale, and undo histories diverge. Short-term: git worktree isolation (each agent in its own worktree, merge at commit time). Medium-term: advisory file locks (`.mae.lock`), inotify coordination to detect external changes and pause AI operations. Long-term: canonical state server (see below).
- [x] **State server v1** (`mae-state-server` binary): Standalone CRDT sync server over TCP (port 9473). Per-document locking, WAL-first SQLite persistence, periodic compaction, transport-generic I/O (reuses `mae_mcp` primitives). Sync protocol: `sync/update`, `sync/state_vector`, `sync/full_state`, `sync/diff`. No auth (trusted LAN only).
- [x] **State server v1.5** (scalability + UX): Sharded SQLite pool (4 shards), save protocol (SHA-256 content-hash), event sequence tracking (wal_seq), background compaction + idle eviction. Editor: 7 commands (SPC C prefix), 4 AI tools, status bar segment, 5 options, doctor integration, audit_configuration collab section. New methods: `sync/resync`, `docs/stats`, `docs/save_intent`, `docs/save_committed`, `$/debug`.
- [x] **Client ID echo filtering**: Server `broadcast_except()` skips the originating session on `sync/update`. Eliminates wasted bandwidth/CPU from self-echo and prevents share duplication race.
- [ ] **State server v2** (Phase F): Awareness protocol (cursor sharing), per-user undo, auth tiers (PSK → SSH → OAuth/OIDC), update compression (msgpack), multi-machine sync.
- [ ] **Enterprise KB server**: Shared KB instance serving development teams + AI agents. Scaling tiers:
  - *Tier 1* (5-20 users, <20K nodes): Shared SQLite in WAL mode + connection pool + TCP proxy. ~1 week effort.
  - *Tier 2* (20-100 users, <100K nodes): Dedicated `mae-kb-server` microservice with HTTP/gRPC API, write-ahead buffer, read replicas, vector embeddings for semantic search. ~1 month.
  - *Tier 3* (100+ users, 500K+ nodes): PostgreSQL + pgvector, write sharding by namespace, event sourcing for real-time sync. ~3 months.
  - Key bottlenecks: SQLite single-writer ceiling (~50 writes/sec), FTS5 index size at scale (~400MB at 100K nodes), network latency for RAG workflows (5-10 KB queries per AI turn × 30 concurrent agents = ~600 node fetches/sec peak).
- [x] **CRDT collaborative editing (yrs/YATA)**: Sync engine: yrs (Yjs Rust port). Per-user cursors via Awareness protocol, per-user undo via yrs UndoManager, conflict-free merge for concurrent AI and human edits. Dual structure: yrs YText + ropey mirror. See ADR-002, ADR-005, ADR-006.
  - Phase A: `mae-sync` crate (yrs dependency, document schemas, ropey bridge) ✅
  - Phase B: Buffer integration (sync_doc field, local edits → yrs transactions) ✅
  - Phase C: MCP sync methods (state_vector, apply_update) ✅
  - Phase D: Push-based sync event broadcasting ✅
  - Phase E (state-server): TCP transport, WAL persistence, per-doc locking ✅
  - Phase F: Awareness protocol, per-user undo, multi-machine sync

### KB Enterprise Readiness & Hardening

- [x] **Change management**: `node_changelog` table with full audit trail (create/update/delete, old/new values, timestamps, author, reason). Schema v6 migration.
- [x] **Incremental sync**: `sync_to_sqlite()` — only writes changed nodes, records all mutations in changelog.
- [x] **Structured timestamps**: `created_at` / `updated_at` INTEGER columns on `nodes`. Enables `ORDER BY updated_at` without JSON parsing.
- [x] **Changelog query API**: `node_history()`, `changes_since()` for auditing and time-travel.
- [ ] **Point-in-time restore**: `kb_restore` command + MCP tool to revert a node to any prior state from changelog.
- [ ] **Node blame**: Per-change author tracking. Requires session identity propagation from MCP client → KB write path.
- [ ] **Changelog pruning**: Configurable retention policy (default: 90 days). `kb-changelog-prune` command.
- [ ] **KB backup/export**: `kb-export` dumps full KB + changelog to portable format (SQLite file or JSON). `kb-import` restores.
- [ ] **Conflict detection**: When multi-client writes land on same node, detect via version counter and surface conflict to user (not silent last-write-wins).
- [ ] **KB replication**: Read replicas for high-read-throughput scenarios (AI agents doing 600+ node fetches/sec). WAL mode enables this natively for same-host.

### Near-term: Other
- [ ] **Version compatibility policy**: Semver enforcement on upgrade — protocol version negotiation in state-server (`initialize` params), config schema migration on major bumps, `make install-upgrade` blocking on incompatible major versions (currently warns only). Prerequisite for v1.0.
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
- [ ] **Which-key floating popup mode**: Option to render which-key as a centered floating popup (like find-file/command-palette) instead of docked to bottom. Controlled by a `which-key-display` option (`docked` | `floating`).
- [ ] **Scheme configurability audit**: Audit ALL OptionRegistry entries for missing `config_key` (prevents `:set-save` persistence). Verify every option round-trips through config.toml. Document full option surface in `:help concept:options` KB node.
- [ ] **Performance regression testing**: Build a benchmark suite (`criterion` in `benches/` or `make bench`). Key metrics: startup time, frame render time (TUI + GUI at 50/500/5000 lines), which-key popup open latency, KB search at 100/1K/10K nodes, AI tool dispatch round-trip, memory usage under sustained editing. Integrate with CI to catch regressions per-PR.
- [ ] **KB search scoping**: Allow per-project KB search that excludes MAE internal nodes (scheme:*, cmd:*, option:*). Add `kb_search_scope` option: `"all"` (default), `"user"` (exclude internal), `"project"` (only project-registered KBs). AI tools respect scope; `:help` always searches all.
- [ ] **KB node visibility**: Add `visibility` property to nodes: `public` (default), `internal` (MAE system nodes), `private` (user personal notes). Internal nodes hidden from user-facing search unless explicitly queried with `:help` or `kb_get` by ID.
- [ ] **Per-workspace KB isolation**: When multiple projects are open, `kb_search` defaults to the active project's registered KB instances. Cross-project search available via `kb_search --all` or `(kb-search-all query)` Scheme API.
- [ ] **KB tangle pipeline**: `make docs-tangle` extracts ADR markdown from KB concept nodes. CI job validates freshness (same as code-map pattern). Enables KB as single source of truth for architecture docs.

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
- Session detach/resume (tmux-style): persist editor state, reconnect from another terminal
- Shared P2P sessions with focus handoff: collaborative cursor, presence indicators
- Granular KB connection/search configuration: users can select/deselect which KB instances are active by default, run scoped queries across a subset of KBs, AI tool parity (e.g. `kb_search` accepts optional `instances` filter param)

</details>

---

## Architecture Invariants

These are non-negotiable constraints derived from Emacs git history analysis:

1. **No file exceeds 3,500 lines** — Emacs's `xdisp.c` is 38,605 lines. We enforce module splits. (Exception: `window.rs` at ~3,434 lines — variable-height line math and smooth scrolling justify the size. Under active monitoring.)
2. **No Global Interpreter Lock** — Emacs spent 23,901 commits on GC retrofit. We use Rust ownership.
3. **AI is a peer, not a plugin** — same `dispatch_builtin()` for human and AI.
4. **Module boundaries enable distributed ownership** — 11 crates with clear responsibilities.
5. **Runtime redefinability is sacred** — Scheme `defadvice`, live REPL, hot reload.
