# MAE Roadmap

Current state: Phases 1-3 complete, Phase 3e COMPLETE, Phase 3e M6/M7 COMPLETE (506 tests).
Terminal editor with vi-like modal editing, Scheme runtime, Claude/OpenAI/Ollama
integration, search, visual mode, text objects, change/repeat/replace, scroll,
indent/dedent, case change, line join, fuzzy file picker, command history, shell
escape, and horizontal scroll all working.

Self-hosting goal: use MAE + Claude/Ollama to develop MAE itself.

---

## Comprehensive Feature Checklist

### What We Have (506 tests)

| Category | Features |
|----------|----------|
| **Modes** | Normal, Insert, Visual (char/line), Command, ConversationInput, Search, FilePicker |
| **Movement** | hjkl, 0/$, gg/G, w/b/e/W/B/E, f/F/t/T, %, {/}, H/M/L |
| **Search** | /pattern, ?pattern, n/N, *, :s///g, :%s///g, :noh, highlights |
| **Editing** | i/a/A/o/O, x, dd/dw/d$/d0, cc/cw/C/c0, r, J, >>/<\<, ~, gUU/guu, `.` repeat, u/Ctrl-r |
| **Text Objects** | ci"/di(/ya{/iw/aw/iW/aW + all paired delimiters + quotes |
| **Yank/Paste** | yy/yw/y$/y0, p/P, register system |
| **Count Prefix** | 5j, 3dd, 2dw, 10G — pervasive across motions and operators |
| **Scroll** | Ctrl-U/D/F/B, zz/zt/zb, horizontal scroll in split windows |
| **Windows** | split v/h, close, focus hjkl, binary tree layout |
| **Buffers** | next/prev/kill/switch, Ctrl-^ alternate, modified tracking |
| **Files** | :e (tab complete), :w, :w path, :wq, :q, :q!, SPC f f (fuzzy picker) |
| **Commands** | :!cmd (shell escape), command history (up/down), :ai-status |
| **AI** | Claude/OpenAI/Ollama tool-calling, conversation buffer, streaming, elapsed timer |
| **Scheme** | Steel runtime, init.scm, define-key, eval REPL |
| **Themes** | 7 bundled, TOML-based, hot-switchable |
| **Debug** | Self-debug state inspection, DAP protocol types |
| **Renderer** | Line numbers, status bar, which-key popup, multi-window, search/selection highlights |
| **CI** | GitHub Actions (check/test/clippy/fmt), tag-based release, dependabot, git-cliff changelog |

### Remaining Tier 1: Blocking Self-Hosting

| # | Feature | Phase | Status |
|---|---------|-------|--------|
| 1 | Multi-buffer AI tools (open_file, buffer by name) | 3f M1 | Not started |
| 2 | Project search (AI: grep across project) | 3f M4 | Not started |
| 3 | Marks (`m`+letter, `'`+letter) | 3e M6 | Deferred |
| 4 | Macros (`q` record, `@` playback) | 3e M6 | Deferred |

### Tier 2: Quality of Life

| # | Feature | Phase |
|---|---------|-------|
| 5 | System clipboard (`"+y`, `"+p`) | 3e M7 |
| 6 | Auto-reload on external change | future |
| 7 | `:set` options | future |
| 8 | Mouse support | future |
| 9 | `:read !cmd` | future |
| 10 | Multiple cursors | future |
| 11 | Session persistence | 3f M3 |

---

## Phase 3e: Editor Essentials ✅ COMPLETE (506 tests)

### M1: Search ✅ (338 tests)
- [x] /pattern, ?pattern, n/N, *, :s///g, :%s///g, :noh, highlights

### M2: Visual Mode ✅ (364 tests)
- [x] v/V, selection highlight, d/y/c operators, motion extension

### M3: Change + Repeat + Replace ✅ (405 tests)
- [x] c+motion, cc, C, c0, `.` dot repeat, `r` replace

### M4: Count Prefix ✅ (426 tests)
- [x] 5j, 3dd, 2dw, 10G — pervasive across all motions and operators

### M5: Scroll + Screen Movement ✅ (433 tests)
- [x] Ctrl-U/D/F/B, zz/zt/zb, H/M/L, horizontal scroll in split windows

### M6: Operators + Text Objects ✅ (506 tests)
- [x] J (line join), >> << (indent/dedent), ~ gUU guu (case change)
- [x] Text objects: ci" di( ya{ iw aw iW aW + all paired delimiters
- [x] Ctrl-^ (alternate file), command history, :!cmd (shell escape)

---

## Phase 3f: AI Multi-File

Extend AI tools so the AI agent can operate across multiple files and buffers.
Required for self-hosting (AI needs to edit multiple crate files).

### M1: Buffer & File Tools
- [ ] `open_file` tool — AI can open a file into a new buffer
- [ ] `switch_buffer` tool — AI can switch the active buffer
- [ ] `close_buffer` tool — AI can close a buffer
- [ ] `buffer_read` accepts optional `buffer_name` param (not just active)

### M2: Multi-File Editing
- [ ] AI can read from any open buffer by name
- [ ] AI can write to any open buffer by name
- [ ] `create_file` tool — AI creates new file + buffer
- [ ] Undo per-buffer (already works, just verify with AI)

### M3: Conversation Persistence
- [ ] Save conversation to file (`:ai-save`)
- [ ] Load conversation from file (`:ai-load`)
- [ ] Conversation history survives buffer kill + reopen

### M4: Project Awareness
- [ ] `project_files` tool — list files in project (git ls-files)
- [ ] `project_search` tool — grep across project (ripgrep)
- [ ] Working directory awareness in system prompt
- [ ] Git status awareness in system prompt

---

## Phase 3g: Hardening

Architecture review (April 2026) identified structural debt that must be
addressed before the codebase grows further. Informed by lessons from Emacs's
xdisp.c monolith, Xi Editor's over-engineering, and Remacs's accumulated debt.

### M1: Split editor.rs
- [ ] editor.rs is 3926+ lines — split into editor/mod.rs (state), dispatch.rs,
      motion.rs, edit_ops.rs, search_ops.rs
- [ ] Preserve all 506+ tests during refactor
- [ ] No functional changes — pure structural refactor

### M2: Error Handling
- [ ] Replace all production unwrap()/expect() with proper error handling
- [ ] Mutex locks: use parking_lot (no poisoning) or catch panics
- [ ] Bounds-check window→buffer indexing in renderer

### M3: Resource Bounding
- [ ] Bound undo/redo stacks (1000 entries)
- [ ] Bound command history (500 entries)
- [ ] Bound conversation entries (5000 entries or ~50MB)
- [ ] Clear search matches on buffer change

### M4: AI Security & Robustness
- [ ] Validate AI tool arguments against typed schemas (not serde_json::Value)
- [ ] Sanitize shell_exec: allowlist or direct execve (no shell injection)
- [ ] Add backpressure to AI event channels (warn when near capacity)
- [ ] Add message history truncation (keep last N messages + system prompt)
- [ ] Add circuit breaker with exponential backoff on provider errors

### M5: Scheme Runtime Boundary
- [ ] Define trait-based API between core and scheme (insurance if Steel doesn't scale)
- [ ] Benchmark Steel under realistic load (1000 rapid edits, 50 buffers, sustained REPL)
- [ ] Document Steel limitations and workarounds

---

## Phase 4a: LSP Client

Language server integration. AI gets semantic code intelligence.

### M1: Connection Management
- [ ] Spawn language server subprocess from config
- [ ] Content-Length framed transport (reuse DAP transport pattern)
- [ ] Initialize handshake (capabilities negotiation)
- [ ] `textDocument/didOpen`, `didChange`, `didSave` notifications
- [ ] Graceful shutdown on editor exit

### M2: Navigation
- [ ] `textDocument/definition` — go to definition (`gd`)
- [ ] `textDocument/references` — find references (`gr`)
- [ ] `textDocument/hover` — show type/docs
- [ ] Results displayed in status bar or preview buffer
- [ ] Expose to AI: `lsp_definition`, `lsp_references`, `lsp_hover` tools

### M3: Diagnostics
- [ ] `textDocument/publishDiagnostics` → editor diagnostic store
- [ ] Gutter markers (error/warning indicators)
- [ ] `SPC d l` diagnostic list buffer
- [ ] AI tool: `lsp_diagnostics` — read current file diagnostics
- [ ] Jump to next/prev diagnostic

### M4: Completion
- [ ] `textDocument/completion` triggered on input
- [ ] Completion popup in renderer
- [ ] Tab/Enter to accept, Esc to dismiss

### M5: Scheme + AI Exposure
- [ ] Scheme functions: `(lsp-definition)`, `(lsp-references)`, `(lsp-hover)`
- [ ] AI system prompt updated with LSP tool descriptions
- [ ] AI can query "what type is this variable?" via LSP

---

## Phase 4b: Syntax Highlighting (Tree-sitter)

Tree-sitter integration for structural editing and display. Moved up in
priority — proven killer feature in Helix and Zed. Can be developed
concurrently with LSP.

### M1: Tree-sitter Core
- [ ] tree-sitter dependency, grammar loading (Rust, Scheme, TOML, Markdown)
- [ ] Parse buffer on change (incremental)
- [ ] Syntax tree stored per-buffer

### M2: Highlight
- [ ] Theme-aware syntax highlighting (theme keys: `syntax.keyword`,
      `syntax.string`, `syntax.comment`, etc.)
- [ ] Incremental re-highlight on edit
- [ ] Language detection from file extension

### M3: Structural Operations
- [ ] Select syntax node at cursor
- [ ] Expand/contract selection by tree level
- [ ] AI tool: `syntax_tree` — read syntax tree for current selection

---

## Phase 4c: DAP Client

Debug adapter integration. Wires existing protocol types to live debuggers.

### M1: Connection & Lifecycle
- [ ] Spawn debug adapter subprocess from config
- [ ] Wire `DapTransport` to async event loop (tokio::select!)
- [ ] Initialize → configurationDone handshake
- [ ] Launch/attach request support
- [ ] Terminate/disconnect cleanup

### M2: Breakpoints & Execution
- [ ] `setBreakpoints` request wired to editor breakpoints
- [ ] `continue`, `next`, `stepIn`, `stepOut` commands
- [ ] Stopped event → update editor debug_state
- [ ] Gutter breakpoint indicators in renderer
- [ ] Current execution line highlight

### M3: State Inspection
- [ ] `threads` → populate thread list
- [ ] `stackTrace` → populate stack frames
- [ ] `scopes` + `variables` → populate variable tree
- [ ] Variable hover (show value at cursor)
- [ ] Watch expressions

### M4: AI Debug Tools
- [ ] AI tools: `dap_continue`, `dap_step`, `dap_inspect_variable`
- [ ] AI can set breakpoints, inspect state, suggest fixes
- [ ] Scheme exposure: `(dap-continue)`, `(dap-inspect)`

---

## Phase 5: Knowledge Base

SQLite-backed graph store with org-mode parser.

### M1: Storage
- [ ] SQLite database (rusqlite)
- [ ] Node CRUD operations
- [ ] Link (edge) CRUD operations
- [ ] Full-text search via FTS5

### M2: Org-Mode Parser
- [ ] tree-sitter-org grammar integration
- [ ] Parse org files to extract: headings, links, properties, tags
- [ ] Bidirectional link extraction (`[[id:...]]` style)

### M3: Editor Integration
- [ ] `:kb-search` command
- [ ] Backlink buffer (show what links to current file)
- [ ] AI tool: `kb_search`, `kb_backlinks`
- [ ] Scheme functions: `(kb-search)`, `(kb-insert-link)`

---

## Future Considerations (from editor history research)

These are architectural investments informed by studying Neovim, Helix, Zed,
Xi, and other editor projects. Not scheduled yet.

| Consideration | Source | Notes |
|---------------|--------|-------|
| Atomic transaction model for buffer edits | Helix | Simplifies undo/redo, gives AI clean edit history |
| MCP (Model Context Protocol) compatibility | Zed | Becoming standard for AI tool integration |
| Remote UI protocol (renderer detachment) | Neovim | Enables future GUI frontends without architecture change |
| Streaming diff protocol for AI edits | Zed | Token-by-token buffer updates during AI generation |
| WASI plugin system | Lapce | Language-agnostic plugins beyond Scheme (Phase 5+) |

---

## Milestone Dependencies

```
Phase 3e (editor essentials) ✅ COMPLETE
    │
    ├─→ Phase 3f (AI multi-file) ← needed for self-hosting
    │       │
    │       └─→ Phase 3g (hardening) ← before codebase grows further
    │
    ├─→ Phase 4b (syntax highlighting) ← high visual impact, concurrent with LSP
    │
    └─→ Phase 4a (LSP) ← biggest unlock for self-hosting
            │
            └─→ Phase 4c (DAP) ← depends on LSP patterns
                    │
                    └─→ Phase 5 (KB) ← lowest priority
```

Phases 3f, 3g, and 4b can be interleaved. LSP (4a) is the biggest single
unlock for self-hosting — once the AI has semantic understanding, it can
navigate and refactor effectively.

---

## Test Targets

| Phase | Current | Target |
|-------|---------|--------|
| 3e    | 506     | 506 ✅ (search, visual, change, count, scroll, text objects, M6, M7) |
| 3f    | —       | +15 (multi-file AI tools) |
| 3g    | —       | +0 (refactor, no new features — preserve existing 506+) |
| 4a    | —       | +25 (LSP connection, navigation, diagnostics) |
| 4b    | —       | +10 (tree-sitter parse, highlight) |
| 4c    | 8       | +20 (DAP lifecycle, breakpoints, state) |
| 5     | —       | +15 (SQLite, org parser, search) |
| **Total** | **506** | **~591** |
