# MAE Roadmap

**Current version:** v0.9.0-dev · **Tests:** 2,748 passing · **Status:** Alpha — all 11 phases complete, KB federation ready for production use.

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

---

## In Progress / Planned

### Near-term
- [ ] PDF preview (GUI inline rendering)
- [ ] Semantic code search (vector embeddings)
- [ ] Org ↔ Markdown bidirectional conversion
- [ ] Investigate `bincode` unmaintained dependency (RUSTSEC-2025-0141) — transitive via `steel-core`; evaluate alternatives (`bitcode`, `postcard`) or upstream Steel fix

### AI
- [ ] Memory synthesis (sub-agent reads persistent memory into context)
- [ ] Dynamic MCP tool discovery (fuzzy search across servers)
- [ ] Verification specialist (isolated test execution sub-agent)
- [ ] AI session playback & undo (step-through replay of code changes)
- [ ] Network status command (`:ai-status` with connectivity diagnostics)

### Org-mode
- [ ] Table formulas (`#+TBLFM:` with Calc-like syntax)
- [ ] Table sorting (alphabetic, numeric, time)
- [ ] Table import/export (CSV/TSV, org ↔ markdown)

### Editor
- [ ] PR comment summaries (auto-summarize changes on PR amend)
- [ ] Free AI-assisted setup (Gemini free tier for first-run guidance)
- [ ] Step-through command execution (inspect AI tool call stdin/stdout)

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

1. **No file exceeds 3,000 lines** — Emacs's `xdisp.c` is 38,605 lines. We enforce module splits.
2. **No Global Interpreter Lock** — Emacs spent 23,901 commits on GC retrofit. We use Rust ownership.
3. **AI is a peer, not a plugin** — same `dispatch_builtin()` for human and AI.
4. **Module boundaries enable distributed ownership** — 11 crates with clear responsibilities.
5. **Runtime redefinability is sacred** — Scheme `defadvice`, live REPL, hot reload.
