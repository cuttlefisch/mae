# MAE Roadmap

Current state: Phases 1-3 complete, Phase 3e M1-M5+M7(partial) complete (405 tests).
Terminal editor with vi-like modal editing, Scheme runtime, Claude/OpenAI
integration, search, visual mode, change/repeat/replace, scroll, and fuzzy
file picker all working. Count prefix (M4) in progress.

Self-hosting goal: use MAE + Claude to develop MAE itself.

---

## Comprehensive Feature Checklist

### What We Have (338 tests, Phase 3e M1 complete)

| Category | Features |
|----------|----------|
| **Modes** | Normal, Insert, Visual (char/line), Command, ConversationInput, Search |
| **Movement** | hjkl, 0/$, gg/G, w/b/e/W/B/E, f/F/t/T, %, {/} |
| **Search** | /pattern, ?pattern, n/N, *, :s///g, :%s///g, :noh, highlights |
| **Editing** | i/a/A/o/O, x, dd/dw/d$/d0, cc/cw/C/c0, r, `.` repeat, u/Ctrl-r |
| **Yank/Paste** | yy/yw/y$/y0, p/P, register system |
| **Windows** | split v/h, close, focus hjkl, binary tree layout |
| **Buffers** | next/prev/kill/switch, modified tracking |
| **Files** | :e, :w, :wq, :q, :q! |
| **AI** | Claude/OpenAI tool-calling, conversation buffer, streaming |
| **Scheme** | Steel runtime, init.scm, define-key, eval REPL |
| **Themes** | 7 bundled, TOML-based, hot-switchable |
| **Debug** | Self-debug state inspection, DAP protocol types |
| **Renderer** | Line numbers, status bar, which-key popup, multi-window, search highlights, selection highlights |

### Tier 1: Blocking Self-Hosting (must complete for dogfooding)

| # | Feature | Vi Keys | Phase | Status |
|---|---------|---------|-------|--------|
| 1 | Search forward/backward | `/`, `?`, `n`, `N` | 3e M1 | **DONE** |
| 2 | Substitute | `:s///`, `:%s///g` | 3e M1 | **DONE** |
| 3 | Search word under cursor | `*` | 3e M1 | **DONE** |
| 4 | Visual mode (charwise) | `v` + motions + d/y/c | 3e M2 | **DONE** |
| 5 | Visual mode (linewise) | `V` + motions + d/y/c | 3e M2 | **DONE** |
| 6 | Change operator | `c`+motion, `cc`, `C` | 3e M3 | **DONE** |
| 7 | Dot repeat | `.` | 3e M3 | **DONE** |
| 8 | Count prefix | `5j`, `3dd`, `2dw` | 3e M4 | In progress |
| 9 | Scroll commands | Ctrl-U/D/F/B | 3e M5 | **DONE** |
| 10 | Replace char | `r` | 3e M3 | **DONE** |
| 11 | Line join | `J` | 3e M6 | Not started |
| 12 | Indent/dedent | `>>`, `<<` | 3e M6 | Not started |
| 13 | File picker | `SPC f f` (fuzzy) | 3e M7 | **DONE** |
| 14 | Save-as | `:w path` | 3e M7 | **DONE** |
| 15 | Multi-buffer AI tools | AI: open_file, buffer by name | 3f M1 | Not started |
| 16 | Project search | AI: grep across project | 3f M4 | Not started |

### Tier 2: Expected in Vi-like Editor (friction without these)

| # | Feature | Vi Keys | Phase |
|---|---------|---------|-------|
| 17 | Screen-relative cursor | `H`, `M`, `L` | 3e M5 | **DONE** |
| 18 | Scroll position | `zz`, `zt`, `zb` | 3e M5 | **DONE** |
| 19 | Marks | `m`+letter, `'`+letter | 3e M6 |
| 20 | Macros | `q` record, `@` playback | 3e M6 |
| 21 | Case change | `~`, `gU`, `gu` | 3e M6 |
| 22 | Text objects | `ci"`, `di(`, `ya{` | 3e M6 |
| 23 | Alternate file | `Ctrl-^` | 3e M7 |
| 24 | Command history | up/down in `:` mode | 3e M7 |
| 25 | Tab completion | in `:e` paths | 3e M7 | **DONE** |
| 26 | Shell escape | `:!cmd`, `:read !cmd` | 3e M7 |
| 27 | Search highlight clear | `:noh` | 3e M1 | **DONE** |

### Tier 3: Quality of Life (defer past self-hosting)

| # | Feature | Phase |
|---|---------|-------|
| 28 | System clipboard | `"+y`, `"+p` | 3e M7 |
| 29 | Auto-reload on external change | 3e future |
| 30 | Backup/swap files | 3e future |
| 31 | Read-only file detection | 3e future |
| 32 | CRLF line ending handling | 3e future |
| 33 | `:set` options | 3e future |
| 34 | Mouse support | 3e future |
| 35 | Code folding | 4c+ |
| 36 | Multiple cursors | Future |
| 37 | Session persistence | 3f M3 |

---

## Phase 3d: Dogfood the AI

Validate that Claude works inside MAE. Fix what breaks. This is the gate
to self-hosting.

### M1: Smoke test
- [ ] `ANTHROPIC_API_KEY=... cargo run` — verify startup
- [ ] `SPC a a` → conversation buffer opens
- [ ] Type "what buffer am I in?" → Claude calls `cursor_info`, responds
- [ ] Type "read the first 5 lines" → Claude calls `buffer_read`
- [ ] Type "add a comment on line 1" → Claude calls `buffer_write`
- [ ] Tool calls visible in conversation as `[Tool: ...]` entries
- [ ] `:ai-status` shows provider/model/connected
- [ ] Without API key: helpful error message

### M2: Fix what breaks
- [ ] Bug fixes from M1 testing (unknown scope)
- [ ] Tune system prompt based on Claude's actual behavior
- [ ] Verify streaming display works (chunks appear incrementally)
- [ ] Verify cancel (`C-c` in ConversationInput) stops generation

---

## Phase 3e: Editor Essentials

Missing vi operations needed before MAE is usable for real editing.

### M1: Search ✅ COMPLETE (338 tests)
- [x] `/pattern` forward search (regex-based)
- [x] `?pattern` backward search
- [x] `n` / `N` next/previous match (wrapping)
- [x] `*` search word under cursor
- [x] `:s/old/new/g` substitute (current line)
- [x] `:%s/old/new/g` substitute (whole buffer)
- [x] `:noh` / `:nohlsearch` clear highlights
- [x] Search highlights in renderer (yellow bg)
- [x] Search mode with `/` and `?` prompts in command line
- [x] "N of M" match status display

### M2: Visual Mode ✅ COMPLETE (364 tests)
- [x] `v` character-wise visual selection
- [x] `V` line-wise visual selection
- [x] Selection highlight in renderer (`ui.selection` style)
- [x] `d`/`x`, `y`, `c` operate on selection
- [x] Visual mode + motion (extend selection with hjkl, w, b, etc.)
- [x] Toggle v/V between char/line, Esc exits to Normal
- [x] 25 tests covering mode transitions, selection range, operators, keybindings

### M3: Change + Repeat + Replace ✅ COMPLETE (405 tests)
- [x] `c`+motion, `cc`, `C`, `c0` (change operator)
- [x] `.` (dot repeat — replay last edit with inserted text)
- [x] `r` (replace single char)
- [x] 17 tests covering change ops, replace, dot repeat, keybindings

### M4: Count Prefix
- [ ] `5j`, `3dd`, `2dw`, `10G` etc.
- [ ] Pervasive: touches dispatch loop and all motion/operator commands

### M5: Scroll + Screen Movement ✅ COMPLETE (376→388 tests)
- [x] Ctrl-U/D (half page), Ctrl-F/B (full page)
- [x] `zz`/`zt`/`zb` (center/top/bottom)
- [x] `H`/`M`/`L` (screen top/middle/bottom)
- [x] `viewport_height` field on Editor, updated from renderer each frame
- [x] 12 tests covering scroll, screen-relative cursor, boundary clamping

### M6: Operators + Marks + Macros
- [ ] `J` (line join), `>>` `<<` (indent/dedent)
- [ ] `~` `gU` `gu` (case change)
- [ ] `m`/`'`/`` ` `` (marks)
- [ ] `q`/`@` (macros)
- [ ] `ci"` `di(` `ya{` (text objects — inner/around)

### M7: File & Buffer UX (partial) ✅ PARTIAL (388→405 tests)
- [x] `SPC f f` (fuzzy file picker — centered popup, recursive scan, subsequence matching)
- [x] `:e` tab completion (Tab cycles through matches)
- [x] `:w path` (save-as)
- [x] 12 tests for file scanning, fuzzy matching, tab completion
- [ ] `Ctrl-^` (alternate file), command history (up/down in `:`)
- [ ] `:!cmd` (shell escape), `:read !cmd`
- [ ] System clipboard (`"+y`, `"+p`)

### Previously Completed (from prior plans)
- Word motions: w/b/e/W/B/E/f/F/t/T/%/{/} ✅
- Yank/Paste: yy/yw/y$/y0/p/P, registers ✅
- Delete ops: dd/dw/d$/d0 ✅
- Buffer mgmt: next/prev/kill/switch ✅

---

## Phase 3f: AI Multi-File

Extend AI tools so Claude can operate across multiple files and buffers.
Required for self-hosting (Claude needs to edit multiple crate files).

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
- [ ] AI-assisted completion (Claude suggests, LSP validates)

### M5: Scheme + AI Exposure
- [ ] Scheme functions: `(lsp-definition)`, `(lsp-references)`, `(lsp-hover)`
- [ ] AI system prompt updated with LSP tool descriptions
- [ ] AI can query "what type is this variable?" via LSP

---

## Phase 4b: DAP Client

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

### M4: Debug UI
- [ ] Split layout: source | variables | call stack
- [ ] PUDB-style TUI with resizable panes
- [ ] Thread switcher
- [ ] Expandable variable tree rendering

### M5: AI Debug Tools
- [ ] AI tools: `dap_continue`, `dap_step`, `dap_inspect_variable`
- [ ] AI can set breakpoints, inspect state, suggest fixes
- [ ] Scheme exposure: `(dap-continue)`, `(dap-inspect)`
- [ ] System prompt updated with debug tool docs

---

## Phase 4c: Syntax Highlighting

Tree-sitter integration for structural editing and display.

### M1: Tree-sitter Core
- [ ] tree-sitter dependency, grammar loading
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

## Milestone Dependencies

```
Phase 3d (dogfood)
    │
    ├─→ Phase 3e (editor essentials)  ← M1 search, M2 visual COMPLETE
    │       │
    │       └─→ Phase 3f (AI multi-file)  ← needed for self-hosting
    │
    ├─→ Phase 4c (syntax highlighting)  ← independent, high visual impact
    │
    └─→ Phase 4a (LSP)  ← needed for real coding
            │
            └─→ Phase 4b (DAP)  ← depends on LSP patterns
                    │
                    └─→ Phase 5 (KB)  ← lowest priority
```

Phases 3e, 3f, and 4c can be interleaved. LSP (4a) is the biggest single
unlock for self-hosting — once Claude has semantic understanding, it can
navigate and refactor effectively.

---

## Test Targets

| Phase | Current | Target |
|-------|---------|--------|
| 3e M1 | 338     | 338 ✅ (search: 22 unit + 13 integration) |
| 3e M2 | 364     | 364 ✅ (visual mode: 25 tests) |
| 3e M3-M7 | —   | +35 (change, count, scroll, text objects) |
| 3f    | —       | +15 (multi-file AI tools) |
| 4a    | —       | +25 (LSP connection, navigation, diagnostics) |
| 4b    | 8       | +20 (DAP lifecycle, breakpoints, state) |
| 4c    | —       | +10 (tree-sitter parse, highlight) |
| 5     | —       | +15 (SQLite, org parser, search) |
| **Total** | **338** | **~483** |
