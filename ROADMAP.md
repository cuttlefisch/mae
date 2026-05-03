# MAE Roadmap

Current state: Phases 1-6 complete, Phase 8 M1-M7 COMPLETE, Phase 9 M1 COMPLETE, v0.7.0-dev (2,354 tests). GUI renders and accepts input. All Tier 1 self-hosting blockers done. v0.7.0: cursor_set primary sync fix (multi-cursor rendering bug), inline image display (org/md image link detection, GUI Skia rendering with aspect-ratio scaling, TUI placeholder, #+attr_html/:width directives, per-image fold, buffer-wide toggle), image metadata AI tooling (image_info/image_list tools, EXIF extraction, imagesize dimensions), terminal-at-file AI tool + terminal-here command, graceful image error handling (missing/corrupt/oversized). v0.6.0: code folding, incremental reparse, dispatch modularization, org/md structural editing (three-state cycle, promote/demote, move subtree, narrow/widen, insert heading), FrameLayout unified text positioning, ex-command tokenizer, `:set` vim-style parsing, pixel-precise mouse clicks, vertical scrollbar, nyan mode, autosave + swap files + crash recovery, diff syntax highlighting, file tree sidebar (NERDTree-style), inline markup rendering (bold/code/italic/strikethrough), display overlays (link concealment), `BufferView` enum + `BufferMode` trait, keymap overlay architecture, Magit parity (multi-level fold, hunk ops, push/pull/fetch/branch/stash), file-type mode hooks (shebang/modeline), RedrawLevel display optimization, variable-height polish (heading spacing, code block backgrounds, italic typeface), unified code block syntax highlighting (shared per-block tree-sitter for md+org), help buffer code block highlighting, rendering dedup (render_common), code map tool.

---

## Known Bugs

- [ ] **AI output buffer cursor invisible in GUI**: After AI responds, the cursor in the `*ai*` conversation output buffer is not visible. Root cause suspected: buffer type / component info plumbing mismatch — the conversation buffer doesn't provide the same layout metadata (line count, cursor position, scroll state) that the cursor renderer expects. Likely needs a refactor of how buffer types expose state to FrameLayout and cursor computation. Low priority (output buffer is read-only, navigation still works).

---

## AI Parity & Tooling Gaps (v0.4.0 Review)

Identified gaps in MAE's AI peer capabilities compared to industry standards (Claude Code, Gemini, Cursor).

- [ ] **Memory Synthesis**: Sub-agent pattern to read and synthesize persistent memory into concise context.
- [x] **Context Compaction**: Automated summarization of conversation history to preserve token budget (v0.5.0).
- [ ] **Dynamic Tool Discovery (MCP Search)**: Enhanced `request_tools` with fuzzy search across all registered MCP servers.
- [ ] **Semantic Code Search**: Integration with vector embeddings for "meaning-based" codebase navigation.
- [x] **Web Fetch**: Tool for reading raw code/docs from URLs (GitHub, documentation sites) (v0.5.0).
- [ ] **Verification Specialist**: Dedicated sub-agent/tool for isolated test execution and PASS/FAIL verdicts.
- [x] **Tool-Level Defenses**: Explicit anti-looping and boundary guardrails in tool descriptions (v0.4.0).
- [x] **UX Mode Cycling**: Shift-Tab to cycle between `manual`, `auto-accept`, and `plan` modes (v0.4.0).
- [x] **Stateful Interruption**: Double-Esc to cancel AI while preserving context for resumption (v0.4.0).

---

## AI UX & Reliability (v0.5.0)

Agent reliability improvements from crash log analysis and self-test failures.

- [x] **Progress Checkpoint System**: Semantic progress evaluation every N rounds (score 0-6) replaces blunt max_rounds as primary stagnation detection. Catches runaway loops without killing complex legitimate tasks (v0.5.0).
- [x] **Softened Oscillation Detector**: A-B-A-B patterns now warn first, abort only after reaching stagnant threshold (was: immediate abort on 2+ repeats) (v0.5.0).
- [x] **Self-Test Mode**: Wider checkpoint interval (15) and higher tolerance (4 stagnant) for `--self-test` runs (v0.5.0).
- [x] **Watchdog Recovery**: Prolonged stalls (>10s) now set a recovery flag; main loop checks on wake and cancels pending AI work + forces redraw (was: log-only) (v0.5.0).
- [x] **Prompt Caching**: Leverage provider-specific prompt caching (Claude cache_control, OpenAI cached_prompt_tokens) for system prompt + tool definitions to reduce costs (v0.5.0).
- [x] **Token Budget Dashboard**: Real-time context window usage visualization in status bar — cache hit rate, color-coded context utilization (v0.5.0).
- [x] **Graceful Degradation**: Auto-reduce tool set when approaching context limits rather than hard-failing. One-way degradation: Normal → ToolsShed (>85%) → Minimal (>92%) (v0.5.0).
- [x] **ANSI-Only Themes**: `light-ansi` and `dark-ansi` themes for terminal environments where RGB hex doesn't map well (v0.5.0).
- [x] **XDG Transcript Logging**: Session transcripts saved to `~/.local/share/mae/transcripts/` (XDG-compliant) instead of project-local `.mae/` (v0.5.0).
- [x] **Tool Tier System**: Core (~43 tools, always sent) vs Extended (on-demand via `request_tools` meta-tool). 10 categories: lsp, dap, knowledge, shell, commands, git, web, ai, visual, debug (v0.5.0).
- [x] **Editor State Save/Restore**: `editor_save_state`/`editor_restore_state` tools for deterministic session state capture (v0.5.0).
- [x] **Tool Visibility Fix**: All 107 tools reachable — 27 previously invisible tools added to Core tier or categories (v0.5.0).
- [x] **Conversation Buffer Compaction**: Skip separators between consecutive tool entries, merge ToolCall+ToolResult display (v0.5.0).

---

## AI UX & Editing (v0.6.0)

User-facing AI interaction quality — from org-roam exploration notes (2026-04-23).

### Editing UX
- [x] **Diff Display Per Edit**: Claude Code / Gemini-style diff view for proposed and applied changes. LCS-based unified diff with `diff.*` theme keys in all 8 themes, syntax-highlighted (v0.6.0).
- [x] **Clickable Links in Output**: `gx` opens URL/file path under cursor in any buffer. Mouse clicks detect links on-the-fly. File paths with `:line:col` navigate to position. (v0.6.0)
- [x] **Rendered Links**: Display markdown links and org links as rendered/clickable (not raw markup) in conversation and document buffers. (v0.6.0: display regions with link concealment, cursor reveal, `gx` link following)
- [ ] **AI Session Playback & Undo**: Code changes from an AI session saved to a tmp file for step-through replay. GC policy: storage limit, file count limit, or age-based expiry.

### Agent Quality
- [ ] **Prompt Reliability**: Favor small introspection tools (list available commands) over hardcoded command args in prompts. Tool context cleanup: fetch response, clear extra tool-call context, keep only response info.
- [ ] **KB Prompt Integration**: AI reads prompts from KB as source of truth. Prompt library with forking, testing, selection of active prompts. Versioned KB nodes for user modifications.
- [ ] **Network Status Command**: Diagnose connectivity issues — no network, API unreachable, agent communication error. Surface in status bar or `:ai-status`.

### Tool Inspection
- [ ] **Step Through Command Execution**: Inspect each tool call's stdin/stdout/stderr and command args. Debug view for AI actions.
- [ ] **Session Record/Replay with DAP**: Full session recording with DAP introspection for post-hoc debugging of agent behavior.

### Git Workflow
- [x] **Git Stash Tools**: `git_stash_push`, `git_stash_pop`, `git_stash_list` (v0.5.0).
- [x] **Branch Management Tools**: `git_checkout`, `git_branch_list`, `git_branch_delete`, `git_merge`, `git_pull`, `git_push` (v0.5.0).
- [ ] **PR Comment Summaries**: When amending an open PR with new commits, auto-summarize changes in a comment.

### Vim Parity
- [x] **C-e / C-y**: Single-line scroll down/up (v0.5.0 M4.5).
- [x] **C-o in Insert Mode**: Execute one normal-mode command then return to insert (*Practical Vim* ch. 15) (v0.5.0).
- [x] **Chained Ex Command Abbreviations**: Tokenizer framework in `ex_parse.rs` — parses `w`/`q`/`x`/`a`/`!` grammar into structured `ExWriteQuit` actions. Supports `:wq`, `:wq!`, `:wqa`, `:wqa!`, `:xa`, `:xa!` and all combinations (v0.6.0).

### Setup & Onboarding
- [ ] **Free AI-Assisted Setup**: Gemini free tier running in embedded shell for guided first-run config. API key storage via `pass` (Linux standard password manager) or platform keychain.

### Project Navigation
- [x] **File Tree Sidebar (NERDTree)**: Persistent project tree pane with expand/collapse, file ops, NERDTree-style keymap (j/k/gg/G/C-e/C-y/C-d/C-u, S-Tab fold cycling), git status markers (v0.6.0).

### Org Mode
- [ ] **Org ↔ Markdown Conversion**: Bidirectional conversion between org-mode and markdown formats.
- [ ] **Org Table Mode** (Emacs `org-table-mode` parity): Full table editing and formatting.
  - **Styling**: Column alignment with `|` delimiters, auto-align on type, horizontal rules (`|---|---|`), cell highlighting, column width detection
  - **Navigation**: Tab to next cell, S-Tab to previous cell, RET to next row, auto-create new row at end
  - **Structure editing**: `M-Left`/`M-Right` move column, `M-Up`/`M-Down` move row, `M-S-Left` delete column, `M-S-Right` insert column, `M-S-Up` delete row, `M-S-Down` insert row
  - **Sorting**: Sort by column (alphabetic, numeric, time)
  - **Formulas**: `#+TBLFM:` field/column formulas with Calc-like syntax, recalculation
  - **Import/Export**: Convert region to table, export to CSV/TSV, org table → markdown table
- [x] **Help Buffer Heading Scaling**: Apply org heading tiered scaling (1.5x/1.3x/1.15x) to help/tutor buffers. KB nodes use org format — headings should render at scaled sizes for readability, same as standalone `.org` files. (v0.6.0)
- [x] **Org Heading Depth Manipulation**: `M-h`/`M-l` and `M-Left`/`M-Right` to promote/demote heading depth. Evil-org parity. (v0.6.0)
- [x] **Org Heading Movement**: `M-j`/`M-k` and `M-Up`/`M-Down` to move heading subtree up/down. Fold-aware (clears folds in affected range). (v0.6.0)
- [x] **Three-State Org Heading Cycle**: TAB cycles SUBTREE→FOLDED→CHILDREN→SUBTREE (Doom Emacs parity). Leaf headings two-state toggle. (v0.6.0)
- [x] **Org/Markdown Narrow/Widen**: `SPC m s n` narrows to subtree, `SPC m s w` widens. Cursor clamped, status bar shows `[Narrowed]`. (v0.6.0)
- [x] **Markdown Structural Editing Parity**: `#` headings get the same UX as org `*` headings — three-state cycle, promote/demote, move subtree, fold-all, narrow/widen, heading font scaling. Markdown keymap with normal fallback. (v0.6.0)
- [x] **heading_scale Option**: `:set heading_scale false` to disable heading font scaling. (v0.6.0)
- [x] **zM/zR for Org and Markdown**: `close-all-folds`/`open-all-folds` dispatch to heading scan for org/markdown buffers. (v0.6.0)

### Rendering Infrastructure
- [x] **Pixel-Based Variable-Height Lines**: Pixel-Y accumulator in the GUI buffer renderer. Each line advances by `scale * cell_height` pixels (exact). Canvas `_at_y` pixel-positioned methods; gutter/cursor use `FrameLayout`. Enables zero-gap heading rendering, future inline images, code block padding.
- [x] **FrameLayout Unified Layout Pass**: Single source of truth for text positioning (`layout.rs`). `compute_layout()` runs once per frame per window; renderer, cursor, and completion popup all consume the same `FrameLayout`. Fold-aware, scale-aware, wrap-aware. Eliminated `PixelYMap`, `accumulated_scaled_col()`, `heading_extra_rows()`.
- [x] **Popup Pixel-Y Migration**: Completion popup now uses `FrameLayout::display_row_of()` for fold/scale-aware cursor positioning.
- [x] **Canvas Clip**: `set_clip_height()` prevents descender overflow at window bottom edge.
- [x] **Mouse Click FrameLayout**: `pixel_to_buffer_position()` in FrameLayout for pixel-precise mouse conversion. GUI caches `last_focused_layout`, falls back to grid math. Scrollbar click detection included (v0.6.0).

### Buffer Safety
- [x] **Autosave**: Timer-based auto-save for dirty buffers. Idle debounce (5s after last edit), configurable interval via `:set autosave_interval`. Swap file crash recovery (`.swp`), session index, `:recover`/`:delete-swap`. Emacs `auto-save-mode` equivalent (v0.6.0: swap files + timer).

### Editor Modes & Buffer-Local Options
- [x] **Per-Buffer Word Wrap (`BufferLocalOptions`)**: `BufferLocalOptions` struct on `Buffer` with `Option<T>` overrides. `:setlocal word_wrap true` for per-buffer override. Conversation, Help, Messages buffers default to `word_wrap=true`. `toggle-word-wrap` flips buffer-local value. Infrastructure supports `line_numbers`, `relative_line_numbers` too (v0.6.0).
- [ ] **Mode Refactoring & Initialization**: Restructure `Mode` enum and mode transitions. Per-mode configuration in user config (e.g. mode-specific keybindings, default options per mode). Emacs `define-derived-mode` equivalent for Scheme layer. Mode-line indicators for active minor modes.
- [x] **Buffer-Local Options Expansion**: Extended `BufferLocalOptions` with `break_indent`, `show_break`, `heading_scale`. Accessors + `:setlocal` support for all 6 options (v0.6.0).
- [x] **File-Type Mode Hooks**: Shebang/modeline/extension priority chain, `language_for_buffer()`, parameterized hooks (`buffer-open:rust`), `set-local-option!` Scheme function, `*buffer-language*`/`*buffer-file-path*` injection, per-language default options (v0.6.0).

### Self-Test Infrastructure
- [x] **Atomic Self-Test Categories**: Each category wrapped with `editor_save_state`/`editor_restore_state` — buffers cleaned up per-category (v0.6.0).

### Project Intelligence
- [ ] **LSP Code Map**: Generate a visual symbol map from `textDocument/documentSymbols` + `textDocument/references`. Output formats: JSON (machine-readable), Mermaid (renders in GitHub), SVG (high-fidelity). Auto-publish to git on minor/major releases via CI. Shows module hierarchy, function signatures, cross-references, and dependency graph. Enables architecture documentation that stays in sync with the code.

### Test Infrastructure
- [x] **Test Suite Breakout**: Split monolithic test files into smaller focused modules. Improve LLM processability of test code (v0.5.0).

---

## Comprehensive Feature Checklist

### What We Have (1,949 tests)

| Category | Features |
|----------|----------|
| **Modes** | Normal, Insert, Visual (char/line), Command, ConversationInput, Search, FilePicker, FileBrowser |
| **Movement** | hjkl, 0/$, gg/G, w/b/e/W/B/E, f/F/t/T, %, {/}, H/M/L |
| **Search** | /pattern, ?pattern, n/N, *, :s///g, :%s///g, :noh, highlights |
| **Editing** | i/a/A/o/O, x, dd/dw/d$/d0, cc/cw/C/c0, r, J, >>/<\<, ~, gUU/guu, `.` repeat, u/Ctrl-r |
| **Text Objects** | ci"/di(/ya{/iw/aw/iW/aW + all paired delimiters + quotes |
| **Yank/Paste** | yy/yw/y$/y0, p/P, register system |
| **Count Prefix** | 5j, 3dd, 2dw, 10G — pervasive across motions and operators |
| **Scroll** | Ctrl-U/D/F/B, zz/zt/zb, horizontal scroll in split windows |
| **Windows** | split v/h, close, focus hjkl, binary tree layout |
| **Buffers** | next/prev/kill/switch, Ctrl-^ alternate, modified tracking |
| **Files** | :e (tab complete), :w, :w path, :wq, :q, :q!, SPC f f (fuzzy picker), ex-command tokenizer (w/q/x/a/! compounds) |
| **Commands** | :!cmd (shell escape), command history (up/down), :ai-status |
| **AI** | Gemini/Claude/OpenAI/DeepSeek tool-calling, transactional callstack, conversation buffer, streaming, elapsed timer, multi-file tools, project search, structured git tools, web fetch, prompt caching, context compaction, graceful degradation, token budget dashboard, tool tiers (Core/Extended), 10 tool categories, `request_tools`, editor state save/restore, 107 AI tools |
| **Scheme** | Steel runtime, init.scm, history.scm persistence, define-key, eval REPL, 12 hooks |
| **Themes** | 9 bundled (incl. light-ansi, dark-ansi), TOML-based, hot-switchable, cached lazy resolution (palette mutation → cache rebuild), Scheme `set_palette_color`/`set_style` API |
| **Debug** | Self-debug, DAP protocol, debug panel, watchdog, event recording, introspect, DAP attach/evaluate, lock contention tracking |
| **Terminal** | Full VT100 via alacritty_terminal, ShellInsert mode, MCP bridge, agent bootstrap, file auto-reload |
| **LSP** | Connection, go-to-definition, references, hover, diagnostics, completion, workspace/document symbols |
| **DAP** | Adapter presets (lldb/debugpy/codelldb), breakpoints (incl. conditional/logpoint), step/continue, attach, evaluate, 13 AI debug tools |
| **KB/Help** | SQLite-backed graph, org parser, help buffer with links, Tab/Enter/C-o navigation, AI kb_* tools |
| **GUI** | winit+Skia, mouse (click+scroll, FrameLayout pixel-precise), splash screen, font config/zoom, FPS overlay, desktop launcher, vertical scrollbar, nyan mode |
| **Renderer** | Line numbers, status bar (git/LSP/tier), which-key popup, multi-window, search/selection highlights, FPS display |
| **CI** | GitHub Actions (check/test/clippy/fmt/e2e), tag-based release, dependabot, git-cliff changelog, `--check-config` validation |

### Remaining Tier 1: Blocking Self-Hosting

| # | Feature | Phase | Status |
|---|---------|-------|--------|
| 1 | Multi-buffer AI tools (open_file, buffer by name) | 3f M1 | **DONE** |
| 2 | Project search (AI: grep across project) | 3f M4 | **DONE** |
| 3 | Marks (`m`+letter, `'`+letter) | 3e M6 | **DONE** |
| 4 | Macros (`q` record, `@` playback) | 3e M6 | **DONE** |

### Tier 2: Quality of Life

| # | Feature | Phase |
|---|---------|-------|
| 5 | System clipboard (`"+y`, `"+p`) | 3h M5 ✅ |
| 6 | Auto-reload on external change | Phase 6 ✅ |
| 7 | `:set` options (`set-option!`) | Phase 6 M1b ✅ |
| 8 | Mouse support | future |
| 9 | `:read !cmd` | v0.3.0 ✅ |
| 10 | Multiple cursors | future |
| 11 | Session persistence (history.scm) | v0.3.0 | **DONE** |
| 12 | README badges (CI status, Rust version, license, crate count) | v0.3.0 ✅ |
| 13 | File tree sidebar (NERDTree/neotree): persistent project tree pane with expand/collapse, file ops | future |
| 14 | Doom-style `init.scm`: documented API reference, keybinding examples, hook usage, option config, module system | v0.3.0 ✅ |
| 15 | Privileged scope escalation: TRAMP-style sudo for editing protected files, timed sudo sessions, AI privilege elevation UX | future |
| 16 | Security & vulnerability audit: enterprise hardening, dependency audit, shell injection review, AI permission boundary testing, sandboxing | future |
| 17 | Per-buffer project roots, `active_project()`, multi-project support | v0.3.0 ✅ |
| 18 | Status line enhancements (git branch, LSP, file type, AI tier) | v0.3.0 ✅ |
| 19 | AI agent launcher (`SPC a a`, ai_editor option) | v0.3.0 ✅ |
| 20 | Font zoom (Ctrl+=/-/0) | v0.3.0 ✅ |
| 21 | BackTab / Shift-Tab support | v0.3.0 ✅ |
| 22 | KB project nodes (`.project` → KB graph) | v0.3.0 ✅ |
| 23 | KB-linked tutorial (`:tutor` → 11 help nodes with cross-links) | v0.3.0 ✅ |
| 24 | Sample config template (`assets/sample-config.toml`) | v0.3.0 ✅ |
| 25 | Shell auto-close on exit (no more blank `[exited]` frames) | v0.3.0 ✅ |
| 26 | Shell CPU idle fix (generation-based dirty tracking, 30%→~0%) | v0.3.0 ✅ |
| 27 | `find-file` uses project root instead of CWD | v0.3.0 ✅ |
| 28 | Debug stats show FPS instead of frame timing | v0.3.0 ✅ |
| 29 | Autosave: timer-based auto-save for dirty buffers (idle debounce, configurable interval, `:set autosave`) | future |
| 30 | LSP code map: generate visual symbol map (JSON/SVG/Mermaid) from `documentSymbols` + cross-references, publish to git on minor/major releases | future |
| 31 | Org table mode: alignment, Tab-to-cell, structure editing, sorting, formulas, import/export | future |
| 32 | Help buffer heading scaling: org heading tiered scale (1.5x/1.3x/1.15x) in help/tutor buffers | v0.6.0 ✅ |
| 33 | Pixel-based variable-height lines: replace `extra_rows_for_scale` with pixel-Y accumulator for exact heading heights | v0.5.1 ✅ |
| 34 | Chained ex command abbreviations: `:wqa`, `:xa`, `:wa`, `:qa!` — ex-command tokenizer framework | v0.6.0 ✅ |
| 35 | Vertical scrollbar: pixel-precise track+thumb, theme-aware, 1-col allocation | v0.6.0 ✅ |
| 36 | Nyan mode: rainbow progress bar in status line (`:set nyan_mode true`) | v0.6.0 ✅ |
| 37 | `:set` vim-style parsing: `no`-prefix, `!` toggle, `?` query, quoted values, value tab completion | v0.6.0 ✅ |
| 38 | Mouse click FrameLayout: pixel-precise positioning for scaled/folded lines | v0.6.0 ✅ |
| 39 | Insert heading (M-Enter): respects level, inserts after subtree | v0.6.0 ✅ |
| 35 | Cached lazy theme resolution: unresolved style strings → palette-aware cache rebuild on theme cycle/mutation | v0.5.1 ✅ |
| 40 | V-line conversation buffer fix: sync viewport start with `win.scroll_offset` for visual-mode selection | v0.6.0 ✅ |
| 41 | Autosave config.toml + idle debounce: `autosave_interval` in config, 5s idle guard | v0.6.0 ✅ |
| 42 | Diff display syntax highlighting: `diff.added`/`removed`/`hunk`/`header` theme keys, all 8 themes | v0.6.0 ✅ |
| 43 | File tree sidebar: `SPC f t`, icons, expand/collapse, j/k/Enter/o/R/q, AI `toggle_file_tree` tool | v0.6.0 ✅ |
| 44 | AI chat scroll tracking: auto-scroll during streaming, scroll-lock on user scroll-up, smooth catch-up on resume | future |

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

### M1: Buffer & File Tools ✅
- [x] `open_file` tool — AI can open a file into a new buffer
- [x] `switch_buffer` tool — AI can switch the active buffer
- [x] `close_buffer` tool — AI can close a buffer
- [x] `buffer_read` accepts optional `buffer_name` param (not just active)

### M2: Multi-File Editing ✅
- [x] AI can read from any open buffer by name
- [x] AI can write to any open buffer by name
- [x] `create_file` tool — AI creates new file + buffer
- [ ] Undo per-buffer (already works, just verify with AI)

### M3: Conversation Persistence ✅ (560 tests)
- [x] Save conversation to file (`:ai-save <path>`)
- [x] Load conversation from file (`:ai-load <path>`)
- [x] Wire struct pattern with version=1 schema; rejects unknown versions loudly
- [x] Editor::conversation()/conversation_mut() accessors; consolidated callers

### M4: Project Awareness ✅
- [x] `project_files` tool — list files in project (git ls-files)
- [x] `project_search` tool — grep across project (ripgrep)
- [x] Working directory awareness in system prompt
- [x] Git status awareness in system prompt

---

## Phase 3g: Hardening

Architecture review (April 2026) identified structural debt that must be
addressed before the codebase grows further. Informed by lessons from Emacs's
xdisp.c monolith, Xi Editor's over-engineering, and Remacs's accumulated debt.

### M1: Architecture Splits ✅
- [x] editor.rs (4589 lines) → editor/mod.rs + 8 submodules + tests.rs (all ≤910 lines)
- [x] main.rs (1063 lines) → main.rs (232) + bootstrap.rs (269) + key_handling.rs (580)
- [x] executor.rs (1164 lines) → executor.rs (707, mostly tests) + tool_impls/ (4 modules)
- [x] All 521 tests preserved, zero warnings

### M2: Error Handling ✅
- [x] Audited all production unwrap()/expect() — only 2 dangerous, both fixed
- [x] search.rs: replaced `matches.last().unwrap()` with `matches.last().copied()`
- [x] dispatch.rs: replaced `debug_state.as_mut().unwrap()` with `if let Some(state)`
- [x] Mutex locks: all safe (no panics while holding lock), parking_lot deferred
- [x] Renderer has zero unwrap() calls — already safe

### M3: Resource Bounding ✅
- [x] Bound undo stack (1000 entries, oldest trimmed on push)
- [x] Bound command history (500 entries)
- [x] Bound conversation entries (5000 entries, oldest trimmed on push)
- [x] Clear search matches on buffer edit (via record_edit/record_edit_with_count)

### M4: AI Security & Robustness ✅ (525 tests)
- [x] Shell command blocklist (rm -rf /, fork bombs, mkfs, dd destructive)
- [x] Shell timeout capped at 120s regardless of AI request
- [x] Backpressure warning when AI event channel near capacity (<4 remaining)
- [x] Message history truncation (keep first message + last N, default 200)
- [x] Circuit breaker with exponential backoff (up to 3 retries, 0.5s/1s/2s)
- [ ] Validate AI tool arguments against typed schemas — deferred (serde_json::Value works, typed schemas add complexity without blocking self-hosting)

### M5: Scheme Runtime Boundary — DEFERRED
- Steel is working well for current use case (config loading, REPL, define-key/define-command)
- Trait extraction is insurance for hypothetical future; not blocking self-hosting
- Will revisit if Steel shows scaling issues under real workloads

---

## Phase 3g-v2: v0.4.1 Modularization & Code Smell Audit ✅

Second round of architecture splits — 6 god files broken into module directories, plus 12 code smell fixes across AI providers, session management, and tool execution.

### File Splits (6 files → module directories)
- [x] `key_handling.rs` (2,056 lines) → `key_handling/` directory (10 mode-specific modules)
- [x] `main.rs` → extracted `terminal_loop`, `lsp_bridge`, `dap_bridge`, `shell_keys` modules
- [x] `tools.rs` + `executor.rs` → `tools/` and `executor/` module directories
- [x] `session.rs` (2,791 lines) → `session/` directory with focused submodules
- [x] All 1,590 tests preserved, zero warnings

### Code Smell Audit (12 fixes)
- [x] Provider modules: removed dead code, simplified error paths, fixed clippy lints
- [x] Session management: extracted serialization logic, reduced coupling
- [x] Executor: clarified tool dispatch, removed redundant matches
- [x] Model-agnostic system prompt: works across Claude, OpenAI, Gemini, DeepSeek

### Deferred Items
- [x] **Rendering dedup** — `render_common/` module with shared span selection, help rendering, git status, messages, file tree, shell palette (v0.6.0)
- [ ] **Packaging readiness** — audit mae-dap, mae-lsp, mae-kb for crates.io publishability (→ Phase 10)

---

## Phase 3h: Vim/Emacs Keybinding Parity & QoL

Deep feature parity with Vim (as documented in *Practical Vim* by Drew Neil)
and Doom Emacs–style discoverability. The guiding principles:

- **Vim's composability**: operator + motion + text-object is the grammar.
  Everything should compose. `cgn` (change-next-match + dot repeat) is as
  powerful as a global replace. From *Practical Vim*: "prefer repeatable,
  undoable units over one-shot commands."
- **Doom's discoverability**: SPC SPC is a fuzzy command palette (M-x with
  completion). Every command is findable without memorizing the binding.
  Which-key annotates the tree with group names so the user learns naturally.
- **Readline/terminal conventions**: users spend time in terminals; the insert
  and command modes should honour C-a/C-e/C-w/C-u/C-k/C-d so muscle memory
  from bash, zsh, and readline transfers directly.

### M0: AI Prompt UX QoL ✅
First-class editor behavior in the AI conversation prompt. The input field
must match the readline/Evil editing experience users get everywhere else.

- [x] `input_cursor: usize` — byte-offset cursor tracking in `Conversation.input_line`
- [x] `scroll: usize` — conversation history scroll state (0 = auto-follow bottom)
- [x] `C-a` / `Home` — move to start of input
- [x] `C-e` / `End` — move to end of input
- [x] `C-b` / `Left` — move cursor one char backward
- [x] `C-f` / `Right` — move cursor one char forward
- [x] `Backspace` / `C-h` — delete char before cursor
- [x] `Delete` / `C-d` — delete char at cursor
- [x] `C-w` — delete word backward (bash-style: to last whitespace)
- [x] `C-u` — kill to start of input
- [x] `C-k` — kill to end of input
- [x] `PageUp` / `PageDown` — scroll conversation history (stay in input mode)
- [x] Normal-mode `j` / `k` — scroll conversation when focused (j=down, k=up)
- [x] Normal-mode `G` — jump to bottom of conversation
- [x] Normal-mode `i` / `a` — re-enter ConversationInput mode
- [x] Enter — submit prompt, reset cursor, scroll to bottom
- [x] Cursor rendered at correct column (char count to `input_cursor`, not `.len()`)
- [x] Cursor hidden when scrolled up (prompt not visible)
- [x] 27 new tests (852 total)

### M1: Terminal Keybinds in Insert Mode ✅
Standard readline/Emacs editing bindings that users expect from any Unix program.

- [x] `C-a` — move to beginning of line (mirrors readline)
- [x] `C-e` — move to end of line
- [x] `C-w` — delete word backward (bash behaviour: delete to last whitespace)
- [x] `C-u` — delete to beginning of line
- [x] `C-k` — delete to end of line (kill-line)
- [x] `C-d` — delete char forward (equiv. `x` in normal mode)
- [x] `C-h` — backspace alias
- [x] `C-j` — newline (alternative to Enter; muscle memory from readline)
- [x] `C-r {register}` — paste from named register while in insert mode
       (from *Practical Vim* ch. 15 — "use registers in insert mode").
       Implemented in M5 via `pending_insert_register` + `insert_from_register`.
- [x] `C-o` — execute one normal-mode command then return to insert
       (from *Practical Vim* ch. 15 — "Run a Normal Mode Command Without Leaving Insert Mode") (v0.5.0)

### M2: Terminal Keybinds in Command Mode ✅
Command line (`:` prompt) should behave like a readline/zsh command line.

- [x] `C-a` / `C-e` — home / end of command line
- [x] `C-w` — delete word backward
- [x] `C-u` — clear command line
- [x] `C-k` — delete to end
- [x] `C-b` / `C-f` — move cursor left / right one char
- [x] `C-p` / `C-n` — history cycle aliases
- [x] `C-d` — delete char forward in command line
- [x] `C-h` — backspace in command line

### M3: Normal Mode Gaps (Practical Vim)
Motions and operators that Vim users rely on but we haven't implemented.

- [x] `s` / `S` — substitute char (`cl`) / line (`cc`) shortcuts
       (*Practical Vim* tip 2: "Think in terms of repeatable units")
- [x] `^` — first non-blank char of line (complement to `0` / `$`)
- [x] `+` / `-` — first non-blank of next / previous line
- [x] `_` — first non-blank of current line (useful with operators: `d_`)
- [x] `ge` / `gE` — backward word-end (complement to `e`/`E`)
- [x] `gf` — go to file under cursor (open in new buffer). Uses a
       filename-char classifier (alphanumerics + `_-./~+:@`) wider than
       word chars. Resolution: literal path first (absolute or relative
       to cwd), fall back to active buffer's parent dir. `~/…` expanded
       via `$HOME`. Pushes a jump before opening so `Ctrl-o` returns.
- [x] `gi` — re-enter insert mode at last insert position
- [x] `g;` / `g,` — jump backward/forward through change list
       (*Practical Vim* ch. 9 — "Traverse the Change List").
       Each edit (via `record_edit` / `record_edit_with_count` /
       `finalize_insert_for_repeat`) pushes the cursor position onto a
       bounded 100-entry list. `g;` walks backward (pushing the current
       position on first step so `g,` can return); `g,` walks forward.
       Dedupes consecutive entries; new edit truncates forward history.
       Cross-buffer via path-resolve with clamp-past-EOF on restore.
       Module mirrors `jumps.rs` pattern.
- [x] `Ctrl-o` / `Ctrl-i` — jump list backward / forward
       (*Practical Vim* ch. 9 — "Navigate the Jump List")
       Push sites: `gg`/`G`, `%`, `{`/`}`, `n`/`N`/`*`, `'<mark>`, `gd`, `]d`/`[d`.
       Bounded at 100 entries; dedupes consecutive pushes; truncates forward
       history on new push. Cross-buffer navigation via path-resolve.
- [x] `@:` — repeat last ex command. Rides the existing `replay-macro`
       await channel: when the register char is `:`, pulls the last entry
       off `command_history` and re-runs it through `execute_command`.
       Count-prefixed (`3@:` re-runs 3 times). Empty-history case
       surfaces "No previous command line" status.
- [x] `gn` / `gN` — visual select next/prev search match (737 tests)
       (*Practical Vim* tip 86 — `cgn<text><Esc>` + `.` as one-key global replace).
       Operator variants: `dgn`/`dgN`, `cgn`/`cgN`, `ygn`/`ygN`. `cgn` is
       dot-repeatable so `.` re-runs the whole select-delete-insert cycle
       from the new cursor position. Primitive lives in
       `search::find_match_at_or_adjacent` (cursor inside a match selects
       that match — i.e. "at or after/before the cursor"), with wrap-around.
- [x] `:changes` command — display change list (newest-first, marks
       current index with `>`). Dispatched via `show-changes-buffer`
       builtin; opens/replaces `*Changes*` scratch buffer.
- [x] Ranger/dired-style directory browser (`SPC f d`) — spatial
       traversal complement to the fuzzy `SPC f f` picker. New
       `Mode::FileBrowser` backed by `mae_core::FileBrowser`; single-pane
       listing with dirs sorted first, Enter/`l` to descend or open,
       `h`/Backspace to ascend (re-selecting the child you came from),
       incremental filter as you type, cleared on descent. Hidden and
       skip-dirs (`.git`/`target`/…) are pruned. 11 unit + 3 integration
       tests. (751 total.)

### M4: Leader Key Command Palette (Doom Emacs-style SPC SPC)
The current which-key shows a key-sequence tree. Users also need a fuzzy
command launcher where they can type any substring of a command name or
description and select from live candidates — the Emacs M-x experience.

Key UX targets from Doom Emacs:
- `SPC SPC` — open command palette (all registered commands, filterable)
- `SPC :` — open command-line (`:` alias; muscle memory from Doom)
- `SPC h k` — describe key binding (what does `gd` do?)
- `SPC h c` — describe command by name (what does `lsp-hover` do?)
- `SPC t t` — switch theme via palette (type "catppuccin", see candidates)
- All existing SPC bindings get meaningful which-key group names with docs

Implementation:
- [x] `CommandPalette` overlay — reuse `FilePicker` infrastructure (same
      fuzzy-match + scrollable list pattern)
- [x] Source: `CommandRegistry::list_commands()` → `(name, doc)` pairs, fuzzy-ranked
- [x] Accept with Enter executes the command; Esc dismisses
- [x] `SPC SPC` binding in normal keymap
- [x] `SPC h k` → describe-key; arms an `awaiting_key_description` flag,
      intercepts the next key sequence in `handle_normal_mode`, looks it
      up in the normal keymap, and opens the bound command's `cmd:<name>`
      help page on Exact (or reports "Key not bound" on None). Esc/Ctrl-C
      cancel.
- [x] `SPC h c` → describe-command; opens the command palette with
      `PalettePurpose::Describe`. Same fuzzy-selection UI as `SPC SPC`,
      but Enter opens the selected command's `cmd:<name>` help page
      instead of executing it.
- [x] Audit all `SPC *` group names in which-key — all 9 current
      prefixes (+buffer, +file, +window, +ai, +theme, +debug, +help,
      +quit, +syntax) have group labels; pinned by a test that walks
      `which_key_entries(SPC)` and fails if any group renders as the
      fallback "+...".

### M5: Registers & Clipboard ✅ (Practical Vim ch. 10)
Named registers are central to Vim's cut/copy/paste model. *Practical Vim*
devotes a full chapter to them as a core feature, not an edge case.

- [x] `"a`–`"z` — yank/delete/paste to/from named registers (`"ayy`, `"ap`).
      All yank/delete/paste call sites centralized through `save_yank` /
      `save_delete` / `paste_text` in `register_ops.rs`. `"<char>` prefix
      captured via `pending_register_prompt` → `active_register`.
- [x] `"A`–`"Z` — append to named registers (uppercase = append).
      `write_named_register` detects uppercase, lowercases the key,
      and appends to the existing entry.
- [x] `"0` — yank register (always the last yank; `save_yank` writes `"0`,
      `save_delete` skips it — so deletes don't clobber yank history)
- [x] `"_` — black-hole register (early return in save_yank/save_delete/paste_text)
- [x] `"+` / `"*` — system clipboard integration. Shell-out shim in
      `clipboard.rs`: tries `wl-copy`/`wl-paste` (Wayland), `xclip` (X11),
      `pbcopy`/`pbpaste` (macOS). Falls back to local mirror on failure.
- [x] `:reg` / `:registers` / `:display-registers` — opens `*Registers*`
      scratch buffer with all non-empty registers, ordered deterministically.
      Newlines rendered as `↵`, tabs as `⇥`.
- [x] `Ctrl-r {register}` in insert mode — `pending_insert_register` flag
      captures the register char, `insert_from_register` inserts its
      contents at the cursor. Clipboard registers query the live clipboard.
- [x] 8 unit tests in `register_ops.rs` + 6 integration tests in `tests.rs`

### M6: Surrounds ✅ (vim-surround)
`vim-surround` is one of the most-installed Vim plugins because it fills a
genuine gap. The operations are composable with operators and dot-repeat.

- [x] `ds{char}` — delete surrounding delimiter. Uses the existing
      `text_object_range` (around) to find the pair, then removes the
      two delimiter chars. Cursor positioned at the old open position.
- [x] `cs{from}{to}` — change surrounding delimiter. Two-char await
      via `pending_surround_from` + `change-surround-1`/`change-surround-2`
      chain through `pending_char_command`. `surround_pair()` maps target
      chars (including `b`→`(`, `B`→`{`, symmetric quotes) to
      `(open, close)`.
- [x] `yss{char}` — surround current line content with char (excludes
      trailing newline). Close inserted at end, open at start.
- [x] `S{char}` in Visual mode — surround selection with char. Works
      with both charwise and linewise selections.
- [x] Integrates with existing text-object infrastructure —
      `text_object_range` provides the range, `surround_pair` maps aliases.
      All four commands are dot-repeatable via `record_edit`.
- [x] 10 unit tests in `surround.rs`

### M7: Vim Quick Wins Batch ✅
Batch of high-value muscle-memory features that fill remaining vim parity gaps.

- [x] `D` → delete-to-line-end (alias for d$)
- [x] `Y` → yank-line (alias for yy, standard vim behavior)
- [x] `X` → delete-char-backward (command existed, wasn't bound)
- [x] `;` / `,` — repeat last f/F/t/T motion / reverse. Tracks
      `last_find_char: Option<(char, String)>` in editor state. Direction
      flipping: forward↔backward, till/find preserved.
- [x] `#` — search word under cursor backward (mirror of `*`)
- [x] `gv` — reselect last visual selection. Saves
      `(anchor_row, anchor_col, cursor_row, cursor_col, VisualType)` on
      every visual exit.
- [x] Visual `>` / `<` — indent/dedent selection by 4 spaces
- [x] Visual `J` — join all lines in selection
- [x] Visual `p` / `P` — paste replacing selection (saves paste text
      before deleting; deleted text goes to black-hole register so paste
      register isn't clobbered)
- [x] Visual `o` — swap cursor and anchor (other end of selection)
- [x] Visual `u` / `U` — lowercase/uppercase selection
- [x] 7 new tests

### M8: Scheme REPL & Lisp Machine ✅
The defining feature: MAE is a lisp machine. Every editor operation is
callable from Scheme, and users can live-evaluate and redefine behavior
while the editor runs — the same property that makes Emacs irreplaceable.

**New Scheme API surface** (registered in `SchemeRuntime::new`):
- [x] `(buffer-insert TEXT)` — insert text at cursor (write-side, applied
      after eval via SharedState pattern)
- [x] `(cursor-goto ROW COL)` — move cursor to absolute position
- [x] `(open-file PATH)` — open a file in a new buffer
- [x] `(run-command NAME)` — dispatch any registered command by name
- [x] `(message TEXT)` — append to *Messages* log
- [x] `(buffer-line N)` — read a specific line (0-indexed; captured as
      a closure over a snapshot of all lines at inject time)
- [x] `*buffer-text*` — full buffer text (new global)
- [x] `*buffer-count*` — number of open buffers (new global)
- [x] `*mode*` — current mode name as string (new global)

**REPL buffer + eval commands:**
- [x] `*Scheme*` output buffer — accumulates prompt/result history.
      Created on first use; `SPC e o` to open/switch.
- [x] `SPC e l` → eval-line (eval current line as Scheme)
- [x] `SPC e r` → eval-region (eval visual selection as Scheme)
- [x] `SPC e b` → eval-buffer (eval entire buffer as Scheme)
- [x] `:eval <code>` — existing inline eval (unchanged)
- [x] +eval which-key group for discoverability
- [x] `eval_for_repl` method — formats `> code\n; => result\n` for
      REPL output; errors formatted as `; error: <msg>`
- [x] Binary drains `pending_scheme_eval` after every key dispatch
      (same intent-queue pattern as LSP/DAP)
- [x] Short results → status bar; all results → appended to `*Scheme*`

**init.scm enriched** with documented API reference, example custom
commands (`insert-timestamp`, `buffer-info`), and example keybinding
customization.

- [x] 8 new scheme runtime tests + 6 scheme_ops tests

---

## Phase 4a: LSP Client

Language server integration. AI gets semantic code intelligence.

### M1: Connection Management ✅ (551 tests)
- [x] Spawn language server subprocess from config
- [x] Content-Length framed transport (reuse DAP transport pattern)
- [x] Initialize handshake (capabilities negotiation)
- [x] `textDocument/didOpen`, `didChange`, `didSave`, `didClose` notifications
- [x] Graceful shutdown on editor exit
- [x] JSON-RPC 2.0 protocol types (Request/Notification/Response)
- [x] Server capabilities parsing (text document sync kind)
- [x] Language ID detection from file extension
- [x] `file://` URI conversion
- [x] Async reader/writer tasks with event channel

### M2: Navigation ✅ (603 tests)
- [x] `textDocument/definition` — go to definition (`gd`)
- [x] `textDocument/references` — find references (`gr`)
- [x] `textDocument/hover` — show type/docs (`K`)
- [x] Results displayed in status bar; cross-file definitions open new buffer
- [x] `LspManager` multi-language coordinator + `run_lsp_task` in binary
- [x] `LspIntent` queue drained each event-loop tick
- [x] Auto `didOpen` on CLI/`:e`, auto `didSave` on `:w`
- [x] Configurable servers via env (MAE_LSP_RUST, MAE_LSP_PYTHON, etc.)
- [ ] Expose to AI: `lsp_definition`, `lsp_references`, `lsp_hover` tools (M5)

### M3: Diagnostics ✅ (633 tests)
- [x] `textDocument/publishDiagnostics` → editor diagnostic store
- [x] Gutter markers (error/warning indicators)
- [x] `:diagnostics` buffer listing every diagnostic grouped by file
- [x] Jump to next/prev diagnostic (`]d` / `[d`)
- [x] AI tool: `lsp_diagnostics` — structured JSON, scope=buffer|all

### M4: Completion ✅ (825 tests)
- [x] `textDocument/completion` triggered on word-char input in insert mode
- [x] `CompletionItem` / `CompletionResponse` with two LSP shapes (array + CompletionList)
- [x] `textEdit` support for servers that send a replacement range
- [x] Kind sigils (`f`=function, `v`=variable, `t`=type, `k`=keyword, `s`=snippet, `m`=module)
- [x] Popup overlay below cursor: up to 10 items, selected item highlighted, flips above edge
- [x] Tab=accept (replaces word prefix), Ctrl-n/Ctrl-p navigate, non-word chars dismiss

### M5: Scheme + AI Exposure ✅ (partial — AI done, Scheme deferred)
- [x] AI tool: `lsp_diagnostics` (structured JSON, done as part of M3)
- [x] AI tools: `lsp_definition`, `lsp_references`, `lsp_hover` — deferred
      execution via `ExecuteResult::Deferred` + oneshot relay pattern. Tools
      queue `LspIntent`, main loop holds reply channel, completes it when
      `LspTaskEvent` arrives. Structured JSON output (1-indexed positions).
- [x] AI system prompt updated with LSP tool descriptions
- [ ] Scheme functions: `(lsp-definition)`, `(lsp-references)`, `(lsp-hover)` — deferred

### M6: LSP UI Parity (lsp-ui / VSCode equivalents)
Rich presentation of LSP results — currently we show hover in the status
bar and references in a scratch buffer. This milestone brings the UX up to
lsp-ui-mode (Emacs) / VSCode inline standards, with evil-style navigation.

- [ ] Floating hover popup: multi-line type signature + docs in a bordered
      overlay near the cursor. Dismiss on motion, `q`, or Escape.
- [ ] Peek definition: inline split showing the target file's context without
      leaving the current buffer. `gd` with peek prefix (e.g. `gpd`), navigate
      with `j`/`k`, Enter to jump, `q` to dismiss.
- [ ] Peek references: same inline split UX for `gr`, cycling through
      locations with `]r`/`[r` or `j`/`k` inside the peek window.
- [ ] Inline diagnostics: underline/highlight the diagnostic range in the
      buffer with severity-colored markers. Show the message on the same line
      (sideline) or on hover. Toggle with `SPC t d`.
- [ ] Code action menu: `SPC c a` opens a popup list of available actions
      (quickfix, refactor, etc.). `j`/`k` to navigate, Enter to apply.
- [ ] Symbol outline / imenu: `SPC c o` opens a sidebar or popup with
      `textDocument/documentSymbol` results. Jump on Enter.
- [ ] Breadcrumbs: optional header line showing the symbol path at cursor
      (function > struct > field). Uses `textDocument/documentSymbol`.
- [ ] Signature help: `textDocument/signatureHelp` shown as a floating tooltip
      when typing function arguments in insert mode.
- [ ] Rename preview: `SPC c R` shows a diff of all affected locations before
      applying the rename. Confirm with `y`, cancel with `n`.

---

## Phase 4b: Syntax Highlighting (Tree-sitter)

Tree-sitter integration for structural editing and display. Moved up in
priority — proven killer feature in Helix and Zed. Can be developed
concurrently with LSP.

### M1: Tree-sitter Core ✅ (648 tests)
- [x] tree-sitter dependency, grammar loading (Rust, TOML, Markdown)
- [x] Parse buffer on edit (full reparse — incremental deferred)
- [x] Syntax tree + highlight spans stored per-buffer in `SyntaxMap`
- [x] Expanded language set: Python, JavaScript, TypeScript/TSX, Go,
      JSON, Bash, Scheme, YAML
- [x] Markdown block highlights working end-to-end — capture names
      like `@text.title`, `@text.literal`, `@text.uri` routed to
      `markup.heading` / `markup.literal` / `markup.link` theme keys
- [x] Org-mode fallback highlighter (regex-based) — tree-sitter-org
      1.3.3 is incompatible with tree-sitter 0.25; swap when fixed

### M2: Highlight ✅
- [x] Theme-aware syntax highlighting — reuses existing bare theme keys
      (`keyword`, `string`, `comment`, `function`, `type`, etc.)
- [x] Re-highlight on edit via `SyntaxMap::invalidate` wired into
      `record_edit`, `record_edit_with_count`, and `finalize_insert_for_repeat`
- [x] Language detection from file extension (auto-attached on `open_file`
      and `with_buffer`)
- [x] Selection/search highlights correctly override syntax colors

### M3: Structural Operations ✅
- [x] Select syntax node at cursor (`SPC s s`)
- [x] Expand/contract selection by tree level (`SPC s e` / `SPC s c`,
      also bound inside Visual mode)
- [x] AI tool: `syntax_tree` — returns full S-expression or just the
      node kind at cursor; 18 AI tools total

---

## Phase 4c: DAP Client

Debug adapter integration. Wires existing protocol types to live debuggers.
Also the substrate for AI-agent driven E2E testing of the editor itself.

### M1: Connection & Lifecycle ✅ (684 tests)
- [x] Spawn debug adapter subprocess from config (`DapServerConfig`)
- [x] Async reader/writer tasks — reader routes responses by `request_seq`
- [x] Initialize handshake — parses `Capabilities` from adapter
- [x] Launch/attach request support (adapter-specific JSON pass-through)
- [x] `configurationDone` flow gated on `initialized` event
- [x] setBreakpoints / threads / stackTrace / scopes / variables
- [x] continue / next / stepIn / stepOut
- [x] terminate / disconnect (with `terminateDebuggee` flag)
- [x] Event channel surfaces `stopped`, `output`, `terminated`, `exited`, etc.
- [x] Request timeout cleans up pending-response map
- [x] 12 client tests using in-memory duplex streams + mock adapter script
- [x] `DapManager` (`DapCommand` / `DapTaskEvent` / `run_dap_task`) — mirrors
      `LspManager` so the editor's event loop stays uniform. Translates raw
      DAP events into editor-friendly variants (Stopped, Continued, Output,
      Terminated, ThreadsResult, StackTraceResult, ScopesResult,
      VariablesResult, BreakpointsSet). +10 manager tests.
- [ ] Editor wiring: main.rs event loop, `:debug-start` commands,
      `:debug` buffer with stack/variables panes (M1.5)

### M2: Breakpoints & Execution ✅ (764 tests)
- [x] `setBreakpoints` request wired to editor breakpoints (via `DapIntent` queue)
- [x] `continue`, `next`, `stepIn`, `stepOut` commands
- [x] Stopped event → update editor debug_state (`apply_dap_stopped` + auto-refresh)
- [x] Gutter breakpoint indicators in renderer (`●` glyph, `debug.breakpoint` theme)
- [x] Current execution line highlight (`▶` gutter + `debug.current_line` background)
- [x] Marker priority: Stopped > Breakpoint > Diagnostic (`resolve_gutter_marker`)
- [x] Stopped-line bg shows through syntax highlights (`Style::patch` merge)

### M3: State Inspection (debug panel)
- [x] `*Debug*` buffer with `DebugView` + `DebugLineItem` line map
- [x] `threads` → thread list with active marker + status
- [x] `stackTrace` → stack frames with source:line, selected marker
- [x] `scopes` + `variables` → scope-grouped variable tree with expand/collapse
- [x] Variable expansion: `▶`/`▼` markers, lazy-loaded children via DAP
- [x] `debug-panel` command + `SPC d p` keybinding
- [x] Panel key handling: j/k navigate, Enter select/expand, q close, o toggle output
- [x] Output log view toggle (o key)
- [x] Auto-refresh on DAP events (`debug_panel_refresh_if_open`)
- [x] GUI + terminal debug panel renderers
- [ ] Variable hover (show value at cursor in source) — deferred
- [ ] Watch expressions — deferred

### M4: AI Debug Tools ✅
- [x] AI tools: `dap_start`, `dap_set_breakpoint`, `dap_continue`, `dap_step`, `dap_inspect_variable`
- [x] AI tools (new): `dap_remove_breakpoint`, `dap_list_variables`, `dap_expand_variable`, `dap_select_frame`, `dap_select_thread`, `dap_output`
- [x] `dap_list_variables` includes expanded children from debug panel
- [x] `dap_select_frame` updates `DebugView.selected_frame_id`
- [x] Action-oriented design — read-side view already covered by `debug_state`
- [x] Permission tiers: `dap_start` Privileged, breakpoint/continue/step Write, inspect ReadOnly
- [x] Idempotent breakpoint set; explicit errors (not no-ops) on stale-state calls
- [x] Shared `dap_start_with_adapter` entry point — command & AI tool agree on preconditions
- [x] `StepKind` enum replaces stringly-typed step dispatch
- [x] `DebugState::find_variable` encapsulates scope iteration (no leak to tool layer)
- [x] `editor_state` reports `debug_panel_open` + `breakpoint_count`
- [x] Self-test suite: `dap` category with 6 tests (conditional, skippable)
- [x] `dap_evaluate` AI tool — evaluate expressions in debug context
- [x] `dap_disconnect` AI tool — disconnect from debug session
- [x] `:debug-attach <adapter> <pid>` — attach to running process
- [x] `:debug-eval <expr>` — evaluate in debug context
- [x] Conditional breakpoints (condition, hitCondition, logMessage)
- [x] `introspect` AI tool — diagnostic snapshot (threads/perf/locks/buffers/shell/ai)
- [x] `event_recording` AI tool — dump/save event recordings
- [x] Watchdog thread: heartbeat monitoring, stall detection, /proc thread dumps
- [x] Lock contention tracking (FairMutex wait times, holder info)
- [ ] Scheme exposure: `(dap-continue)`, `(dap-inspect)` — deferred

---

## Phase 4d: Knowledge Base Foundation + Help System ✅

Built first as an in-memory graph store that powers the built-in help
system. Human (`:help`) and AI (`kb_*` tools) read the same nodes — the
"AI as peer" design point at its most literal.

### M1: In-Memory KB ✅
- [x] `mae-kb` crate with `Node`, `KnowledgeBase`, `NodeKind`
- [x] `[[target]]` / `[[target|display]]` link parsing
- [x] Reverse index (`links_in`) so `links_to()` is O(1) — even for dangling targets
- [x] 20 unit tests

### M2: Help Buffer ✅
- [x] `BufferKind::Help` + `HelpView` (current + back/forward stacks + scroll + focused_link)
- [x] `cmd:<name>` nodes auto-seeded from `CommandRegistry` on startup
- [x] Hand-authored `concept:*`, `key:*`, and `index` nodes
- [x] `:help [topic]` with namespace fallback (literal → `cmd:<topic>` → `concept:<topic>`)
- [x] `:describe-command <name>` opens `cmd:<name>`
- [x] Help buffer keys: Enter=follow, Tab=next link, Shift-Tab=prev, q=close, C-o=back, C-i=forward, j/k=scroll
- [x] Renderer: title header + body with styled `[[link]]` segments + focus highlight

### M3: AI KB Tools ✅
- [x] `kb_get`, `kb_search`, `kb_list`, `kb_links_from`, `kb_links_to` (all ReadOnly)
- [x] `kb_graph` (BFS up to 3-hop neighborhood) + `help_open` (peer navigation)
- [x] 30 AI-specific tools total

### M4: Local Graph Navigation ✅
- [x] Help buffer neighborhood footer: outgoing + incoming links with titles, missing targets flagged
- [x] Tab cycles through unified list of outgoing + incoming links
- [x] `kb_graph` AI tool returns `{root, depth, nodes, edges}` JSON
- [x] `help_open` AI tool + system prompt guidance so the agent steers the user into help pages

### M5: Performance Quick Wins ✅
- [x] Pre-lowercased title/body/tags cached at insert time (search scales to 2k nodes in <50ms)
- [x] Perf regression test guarding against O(n²) regressions

---

## Phase 5: Knowledge Base (persistent, org-roam style) ✅

Build on the in-memory KB from Phase 4d. SQLite-backed graph store,
org-mode parser, user-authored notes.

### M1: Storage ✅
- [x] SQLite + FTS5 via `rusqlite` (bundled)
- [x] Schema: `nodes`, `links`, `nodes_fts` virtual table (porter + unicode61)
- [x] `save_to_sqlite` / `load_from_sqlite` — atomic transactions, idempotent
- [x] `fts_search(path, query, limit)` — BM25-ranked, prefix queries (`word*`)
- [x] `probe_sqlite` for schema version detection
- [x] `:kb-save <path>` and `:kb-load <path>` commands

### M2: Org-Mode Parser + Watcher ✅
- [x] Hand-rolled org-roam parser — `:PROPERTIES: :ID:`, `#+title:`, `#+filetags:`, `[[id:UUID][display]]` rewriting
- [x] `parse_org_multi` supports file-level AND per-heading `:ID:` drawers (multi-node files)
- [x] Inline heading tags merged with file-level tags
- [x] External `[[url][display]]` links flattened to `display (url)` to avoid scanner collisions
- [x] `ingest_org_dir` walks recursively via `walkdir`, returns `IngestReport`
- [x] `OrgDirWatcher` (notify-based) emits `OrgChange::Upserted(path)` / `Removed(ids)` events
- [x] `:kb-ingest <dir>` command

### M3: Editor Integration ✅
- [x] `:kb-save`, `:kb-load`, `:kb-ingest` commands
- [x] In-memory KB continues to serve `:help` and `kb_*` AI tools — SQLite is the persistence + FTS layer, not a query rewrite
- [ ] Backlink buffer (show what links to current file) — deferred
- [ ] User-authored note workflow (`:kb-new`, `:kb-link`) — deferred
- [ ] Scheme functions: `(kb-search)`, `(kb-insert-link)` — deferred

### M4: GUI Graph View (blocked on GUI backend)
- [ ] Org-roam-ui style force-directed graph of KB nodes and links
- [ ] Pan/zoom, click-to-navigate to help/note buffer
- [ ] Filter by namespace (show only `cmd:*`, only user notes, etc.)
- [ ] Terminal fallback stays as neighborhood adjacency list from 4d M4
- Blocked on: GUI renderer (wgpu or similar); terminal backend can't render graphs well

---

## Phase 6: Embedded Shell

The editor should be the user's primary interface to their shell — not a
terminal multiplexer wrapper, but a first-class shell buffer where the AI
agent can observe, suggest, and execute commands alongside the user.

### M1: Shell Buffer — COMPLETE
- [x] PTY-backed `*Terminal*` buffer via `alacritty_terminal` (full VT100/VT500)
- [x] ShellInsert mode with raw-mode passthrough (keyboard → PTY escape sequences)
- [x] Full grid rendering: colors, attributes (bold/italic/dim/underline/strikeout)
- [x] `:terminal` command; `SPC o t` binding
- [x] `Ctrl-\ Ctrl-n` to exit ShellInsert → Normal mode (Neovim convention)
- [x] `i`/`a`/`A` on a shell buffer re-enters ShellInsert mode
- [x] Shell process exit detection → auto mode switch + buffer cleanup
- [x] `terminal-reset` / `terminal-close` commands (`SPC o r` / `SPC o c`)
- [x] 30fps render tick for smooth terminal output
- [x] Window resize propagation to PTY
- [x] Wide char / spacer handling, cursor positioning

### M1b: Scheme Hooks & set-option! — COMPLETE
- [x] HookRegistry with 7 hook points: before-save, after-save, buffer-open, buffer-close, mode-change, command-pre, command-post
- [x] `(add-hook! HOOK-NAME FN-NAME)` / `(remove-hook! HOOK-NAME FN-NAME)` Scheme bindings
- [x] `(set-option! KEY VALUE)` for line-numbers, relative-line-numbers, word-wrap, break-indent, show-break, theme
- [x] Hook eval drain in main loop (same intent pattern as LSP/DAP)
- [x] Mode-change hooks fire on every mode transition

### M2: AI & Scheme Shell Tools — COMPLETE
- [x] AI tool: `shell_list` — list active shell buffers (ReadOnly tier)
- [x] AI tool: `shell_read_output` — read terminal grid content via cached viewports
- [x] AI tool: `shell_send_input` — send text to PTY (Shell tier)
- [x] Scheme: `(shell-send-input IDX TEXT)` — queued via intent pattern
- [x] Viewport caching: main loop snapshots shell grids for AI/Scheme access
- [x] Intent pattern: `pending_shell_inputs` drained by binary alongside LSP/DAP intents

### M3: Scheme Shell Read Functions — COMPLETE
- [x] `(shell-cwd BUF-IDX)` — CWD of shell process (via /proc/PID/cwd)
- [x] `(shell-read-output BUF-IDX MAX-LINES)` — read viewport snapshot
- [x] `*shell-buffers*` — list of shell buffer indices
- [x] `child_pid()` + `cwd()` on `ShellTerminal` struct
- [x] `shell_cwds` cache on Editor, populated by main loop
- [ ] Scheme REPL overlay in shell buffer (deferred — Layer 1)
- [ ] Pipe bridge: `shell | scheme-fn` (deferred — Layer 3)

### M4: Send-to-Shell — COMPLETE
- [x] `SPC e s` (`send-to-shell`) — send current line to a terminal buffer
- [x] `SPC e S` (`send-region-to-shell`) — send visual selection to terminal
- [x] `find_shell_target()` — prefers active shell, falls back to most recent
- [x] Multi-line regions joined with `\r` for terminal
- [ ] Shell-aware completion (optional, future)

### M2b: MCP Bridge — COMPLETE
- [x] `mae-mcp` crate: Unix socket JSON-RPC server (tokio)
- [x] MCP protocol types: Initialize, ToolList, ToolCall, JSON-RPC 2.0
- [x] `mae-mcp-shim` binary: stdio ↔ Unix socket bridge
- [x] `MAE_MCP_SOCKET=/tmp/mae-{pid}.sock` injected into PTY env
- [x] Main loop drains MCP tool requests alongside AI tool requests
- [x] Same permission tiers as built-in AI
- [ ] MCP resources/prompts (deferred)

### File Auto-Reload — COMPLETE
- [x] `file_mtime` field on Buffer, set on load/save
- [x] `check_disk_changed()` — compares stored vs current mtime
- [x] `reload_from_disk()` — reload clean buffers automatically
- [x] `check_and_reload_buffer()` — called from `switch_to_buffer()`
- [x] `file-changed-on-disk` hook point
- [x] Dirty buffers show warning, no clobber

### Agent Bootstrap — COMPLETE
- [x] `agents.rs` module: agent registry, shim resolution, `.mcp.json` read-merge-write
- [x] Auto-write `.mcp.json` on `:terminal` spawn (idempotent, preserves existing entries)
- [x] `MAE_MCP_SOCKET` inherited from PTY env — file is static (no PID), reusable across restarts
- [x] `:agent-setup <name>` — manually bootstrap a specific agent
- [x] `:agent-list` — show available agents
- [x] `mae --setup-agents [DIR]` — CLI: write `.mcp.json` without starting editor
- [x] Config opt-out: `[agents] auto_mcp_json = false` / `MAE_AGENTS_AUTO_MCP=0`
- [x] `mae-mcp-shim` installed alongside `mae` by `make install`
- [x] KB help entry: `concept:agent-bootstrap`

### M5: Magit Parity
Full git porcelain in a dedicated buffer — the magit experience. Builds on
M1 PTY shell and the existing `SPC g` stubs.

- [x] `SPC g s` — git status buffer with staged/unstaged/untracked/stash sections
- [x] Stage/unstage: `s` to stage file or hunk, `u` to unstage, `S`/`U` for all
- [x] `c c` — commit, `c a` — amend
- [x] Diff view: Tab on file toggles inline diff with added/removed/context coloring
- [x] Discard: `x` — discard unstaged changes for file at cursor
- [x] `BufferView` enum replaces 6 `Option<T>` fields; `BufferMode` trait encodes mode contract
- [x] Shared `render_common::help` deduplicates ~65 LOC between TUI/GUI help renderers
- [x] `GitLineKind` + `DiffLineType` semantic types for rendering and dispatch
- [ ] Log view: `l l` — commit history with graph, `l b` — branch log
- [ ] Blame: `SPC g b` — line-by-line blame in gutter or dedicated buffer
- [ ] Stash: `z z` — stash, `z p` — pop, `z l` — list stashes
- [ ] Keybindings match magit conventions where possible (s/u/c/l/z prefixes)
- [x] AI tools: `git_status`, `git_diff`, `git_log`, `git_commit`, `git_stage`, `git_push`, `git_pull` — structured JSON for agent use (M5a)
- [ ] Scheme exposure: `(git-status)`, `(git-stage FILE)`, `(git-commit MSG)`

---

## Phase 7: Embedded Documentation System

Users must be able to discover, read, and navigate all editor documentation
from within the editor — and the AI peer must have native access to the same
docs to help users effectively. Builds on the existing KB + help buffer.

### M1: Comprehensive Help Content ✅ (v0.7.0)
- [x] Scheme primitive KB nodes (scheme:*) for ~45 functions + 18 variables
- [x] Progressive getting-started tutorial (3 tracks: vim, beginner, AI)
- [x] `:help` fuzzy completion with expanded namespace fallback

### M2: Contextual Help ✅ (v0.7.0)
- [x] `:help` Tab completion → HelpSearch palette
- [x] WhichKeyEntry doc field populated from CommandRegistry
- [x] `:help` fuzzy completion (FTS5 search as you type)

### M3: Documentation Authoring ✅ (v0.7.0)
- [x] `:help-edit <topic>` — edit a help node inline (user-authored overrides)
- [x] User help nodes persisted to `~/.config/mae/help/` directory
- [x] Org-mode format for user-authored help (parsed by existing org parser)

### M4: Doom-style init.scm — Configuration Framework ✅ (v0.7.0)
Inspired by Doom Emacs's module system: declarative, layered, well-documented.

- [x] Ship `scheme/init.scm` — comprehensive documented default config
  - All keybinding examples with comments
  - Hook usage patterns (before-save, after-save, buffer-open, etc.)
  - Option configuration via `(set-option! ...)` with all 14 options listed
  - Theme selection, font size, clipboard mode, AI provider configuration
  - 8 sections: UI, Theme, Editor Options, Keybindings, AI, Shell, Hooks, Custom Commands
- [x] `--check-config` CLI flag — validate init.scm + config.toml without launching editor
- [x] CI E2E step — builds TUI binary and runs `--check-config` to validate init.scm
- [x] Layered init.scm loading (user → project, error isolation) (v0.7.0)
- [x] `after-load` hook (v0.7.0)
- [x] `--debug-init` CLI flag + verbose init logging (v0.7.0)
- [x] `:describe-configuration` health report command (v0.7.0)
- [x] `audit_configuration` MCP tool (structured JSON) (v0.7.0)
- [x] `--check-config --report` CLI extension (v0.7.0)
- [x] `:reload-config` command — hot-reload init.scm without restart (v0.7.0)
- [ ] Module system: `(mae/module! :editor :ai :lsp :dap :shell :kb)`
  - Each module self-contained, can be enabled/disabled
  - Modules declare dependencies (`:lsp` requires `:editor`)
  - `~/.config/mae/modules/` for user modules
- [ ] `(after! module body...)` — run code after a module loads (Doom pattern)
- [ ] `(map! mode keys command)` — ergonomic keybinding macro
- [ ] Package-like autoloads: deferred Scheme evaluation until first use

---

## Phase 8: GUI Rendering Backend

GUI window via winit + skia-safe. Gives MAE direct OS-level key access
(no host terminal intercepting keybindings), GPU-accelerated rendering,
and the foundation for variable-height lines, inline images, and PDF preview.

### M1: Foundation — COMPLETE
- [x] `Renderer` trait extracted from terminal backend (trait-based HAL)
- [x] `InputEvent` type — backend-agnostic input abstraction in mae-core
- [x] `TerminalRenderer` implements `Renderer` trait (drop-in)
- [x] `mae-gui` crate: winit window + Skia raster surface + monospace text
- [x] winit key → KeyPress translation (`input.rs`)
- [x] Skia canvas: surface management, text drawing, status line, theme colors
- [x] Optional `gui` feature flag in mae binary (`--gui` flag)
- [x] Configurable shell exit sequence (shell-insert keymap, not hardcoded)
- [x] Configurable AI permission tier (config + env var)

### M2: Event Loop & Presentation — COMPLETE
- [x] winit `pump_app_events()` integrated with tokio `current_thread` runtime
- [x] Full keyboard input: all editor modes, shell-insert, modifier tracking
- [x] softbuffer pixel presentation (Skia raster → OS window surface)
- [x] AI/LSP/DAP/MCP channel draining in GUI loop (same as terminal)
- [x] Shell terminal spawn/poll/close in GUI mode
- [x] Window resize handling
- [x] CI fix: `--exclude mae-gui` for workspace builds (skia system deps)
- [x] init.scm fix: inject editor state before Scheme file evaluation
- [x] Self-test infrastructure: `:self-test`, `self_test_suite` MCP tool, `--self-test` CLI flag
- [x] Input lock during AI operations (Esc/Ctrl-C to cancel)
- [x] CWD-based project detection at startup (no file arg needed)
- [x] `close_buffer` force parameter, `ai_save`/`ai_load`/`rename_file` AI tools
- [x] 6 ex-commands registered for AI parity (`nohlsearch`, `kb-save`, `kb-load`, `kb-ingest`, `ai-save`, `ai-load`)
- [x] LSP AI tools: `lsp_definition`, `lsp_references`, `lsp_hover`, `lsp_workspace_symbol`, `lsp_document_symbols`
- [x] Event loop refactor: shared `ai_event_handler` + `shell_lifecycle` modules (eliminates terminal/GUI duplication)

### GUI Feature Status

| Feature | Status | Milestone |
|---------|--------|-----------|
| Window + monospace text | ✅ Done | M1-M2 |
| Keyboard input (all modes) | ✅ Done | M2 |
| Window resize | ✅ Done | M2 |
| Status bar | ✅ Done | M2 |
| AI/LSP/DAP/MCP channels | ✅ Done | M2 |
| Shell terminals | ✅ Done | M2 |
| Cursor rendering | ✅ Done | M3 |
| Line numbers / gutter | ✅ Done | M3 |
| Command line display | ✅ Done | M3 |
| Syntax highlighting colors | ✅ Done | M3 |
| Splash screen | ✅ Done | M3 |
| Mouse (click, scroll) | ✅ Done | M3 |
| Shell scrollback | ✅ Done | M3 |
| Desktop launcher + icon | ✅ Done | M3 |
| Font size config | ✅ Done | M3 |
| FPS overlay | ✅ Done | M3 |
| Event loop refactor (run_app) | ✅ Done | M4 |
| Variable-height lines | ✅ Done | M5 |
| Mixed fonts (headings, prose) | — descoped | M5 |
| Inline images (PNG/JPG/SVG) | ❌ Not yet | M6 |
| Org-mode image preview | ❌ Not yet | M6 |
| PDF preview | ❌ Not yet | M7 |
| Mouse click + scroll | ✅ Done | M3 |
| Mouse click-drag select | ✅ Done | M8 |
| Selection highlighting (visual mode) | ✅ Done | M3 |
| Unicode/glyph fallback (font chain) | ✅ Done | M3 |
| Scrollbar (vertical) | ✅ Done | M8 |

### M3: Visual Polish — COMPLETE
- [x] Cursor rendering in GUI (block/line per mode)
- [x] Status bar + command line rendering
- [x] Shell colors theme-aware
- [x] Splash screen with recent files, config shortcut, version display
- [x] Mouse basics (click to place cursor, wheel scroll)
- [x] Shell scrollback (Shift-PageUp/PageDown)
- [x] Input lock redesign (scoped, shell interaction allowed)
- [x] Desktop launcher + SVG icon
- [x] Font size configuration (config.toml + `:set font_size`)
- [x] FPS overlay toggle (`SPC t F`)
- [x] `:set` ex-command + `:edit-config` (`SPC f c`)
- [x] ZZ/ZQ keybindings
- [x] 30-second health check for zombie shell detection
- [x] Font zoom keybindings: `Ctrl+=` increase, `Ctrl+-` decrease, `Ctrl+0` reset
- [x] BackTab / Shift-Tab passthrough in shell-insert mode
- [x] Unicode/glyph fallback: 7-level font chain (configured → JetBrainsMono Nerd Font → Fira Code → Cascadia Code → monospace)
- [x] Line numbers and gutter in GUI (`gutter.rs`: relative/absolute, breakpoint/diagnostic markers)
- [x] Syntax highlighting colors in GUI (tree-sitter spans → theme keys → per-char style)
- [x] Visual mode selection highlighting in GUI (charwise + linewise, multi-line clipping)
- [x] Bug: vertical line characters render with incorrect colors in insert mode (GUI) — fixed (v0.5.1)

### M4: GUI Event Loop Refactor — `run_app` + `EventLoopProxy` ✅

Replaced the `pump_app_events` polling loop with winit's `run_app` + typed `EventLoopProxy<MaeEvent>`, eliminating the 16ms polling latency and conforming to Wayland's event-driven model. This is the architecture used by Alacritty and other production GPU editors.

- [x] Define `MaeEvent` enum (AiEvent, LspEvent, DapEvent, McpToolRequest, ShellTick, McpIdleTick, HealthCheck)
- [x] Switch from `pump_app_events` to `event_loop.run_app(&mut GuiApp)`
- [x] Tokio runtime on background thread with `bridge_task` reading all async channels
- [x] `ApplicationHandler<MaeEvent>::user_event()` dispatches all async events
- [x] `about_to_wait()` → deferred reply timeout + font hot-reload + shell lifecycle + `request_redraw()`
- [x] Removed 16ms poll timeout — event loop sleeps until OS event or proxy wakeup
- [x] Zero-latency async→render pipeline: async task → `proxy.send_event()` → winit wakes → render
- [x] Shared `AtomicBool` flags gate conditional ticks (shell 33ms, MCP 500ms, health 30s)
- [x] `GuiApp` owns all state (no borrowed `WinitCallback<'a>`)
- [x] `main()` is plain `fn` — tokio runtime built manually, terminal path uses `rt.block_on()`
- [x] Shell background theme fix: `NamedColor::Background`/`Black` fall back to `ui.background` style instead of xterm #000000
- [x] `get_option` AI tool: read current option values (name, value, type, default, doc) — symmetric with `set_option`
- [x] `set_option` auto-generated from `OptionRegistry` — no more hardcoded enum drift (was missing `clipboard`)

### M4.5: Display Optimization — COMPLETE
Emacs-inspired display patterns and CJK correctness.
- [x] Command line layout fix — row calculation prevents partial bottom row clipping
- [x] Input-pending pattern (Emacs `dispnew.c:3254`) — keyboard/scroll bypasses 60fps frame cap for immediate feedback
- [x] `last_render` timing fix — measured after render completes, not before
- [x] CJK/wide-character correctness — `unicode-width` in wrap, buffer_render, status_render (GUI + terminal)
- [x] `draw_styled_at` display-width-aware column tracking for CJK text rendering
- [x] Regression tests: row layout (7 heights x 5 cell sizes), CJK wrap/break/width (6 tests)

### M5: Emacs Display Patterns — COMPLETE
Advanced display optimizations from Emacs `dispnew.c` / `xdisp.c` analysis. (v0.6.0: infrastructure, v0.7.0: mouse focus + idle)
- [x] Glyph matrix hashing — `content_hash: u64` on `LineLayout`, hashed over chars + scale (v0.6.0)
- [x] Line-level dirty tracking — `RedrawLevel` enum (None/CursorOnly/Scroll/PartialLines/Full), `dirty_line_range` on Editor (v0.6.0)
- [x] CursorOnly fast path — skip `compute_visible_syntax_spans()`, reuse cached Arc spans (v0.6.0)
- [x] Idle deferred work — defer syntax highlighting and LSP requests to idle periods; click-to-focus, scroll-under-mouse, focus-follows-mouse (v0.7.0)
- **Descoped: Scroll region blit** — `begin_frame()` does full canvas clear; architectural rework for marginal gain. Not worth pursuing without a compositor-level rewrite.
- **Descoped: Partial render skip** — content_hash infrastructure exists but is pointless without scroll region blit. Would add complexity for no measurable benefit.

### M6: Variable-Height Lines & Mixed Fonts — COMPLETE
- [x] Headings rendered at larger font sizes (v0.5.0: heading_scale)
- [x] Bold/italic/underline/strikethrough font decorations (v0.6.0: strikethrough + italic typeface)
- [x] Line-height varies per line type (v0.5.0: heading scale affects line height)
- [x] Heading top padding — h1: 4px, h2: 3px, h3: 2px (v0.6.0)
- [x] Strikethrough rendering — `~~text~~` detection + draw at 60% ascent (v0.6.0)
- [x] Code block tinted backgrounds — fenced/src block detection + full-width bg rect (v0.6.0)
- [x] Proper italic typeface — system italic font loading with skew fallback (v0.6.0)
- **Descoped: SkParagraph / rich text layout** — not available in skia-safe 0.93; requires upgrade to 0.97+ which is a significant dependency change for minimal gain.
- **Descoped: Proportional fonts (prose in variable-width)** — no mainstream code editor does this. Extreme complexity (cursor positioning, selection, alignment) for negligible benefit in a code-first editor.

### M7: Inline Images
- [ ] PNG/JPG/SVG rendering inline with text lines
- [ ] Org-mode `[[file:image.png]]` auto-preview
- [ ] Image scaling to fit viewport width

### M8: PDF Preview
- [ ] pdfium-render integration for PDF page rendering
- [ ] `:pdf <file>` opens a PDF preview buffer
- [ ] Scroll through pages, zoom in/out

### M9: Mouse & Selection
- [x] Click to place cursor (done in M3)
- [x] Click-drag to select text (mouse press/drag/release → visual selection)
- [x] Scrollbar (vertical) — pixel-precise track+thumb in `scrollbar.rs`, theme-aware colors, allocates 1 col from text area (v0.6.0)
- [x] Mouse wheel scroll (done in M3)
- [x] Selection highlighting (done in M3 — visual mode bg/fg in buffer_render.rs)

---

## Phase 9: Org-Mode Editing

Full org-mode editing support — MAE as a first-class org-mode environment.
Builds on the existing org parser (Phase 5 M2) and KB infrastructure.

### M1: Structural Editing ✅ (v0.6.0)
- [x] Heading promotion/demotion (`M-h`/`M-l`, `M-Left`/`M-Right`)
- [x] Heading folding — three-state TAB cycle (SUBTREE→FOLDED→CHILDREN)
- [x] Move subtree up/down (`M-j`/`M-k`, `M-Up`/`M-Down`) — fold-aware
- [x] Narrow/widen subtree (`SPC m s n`/`SPC m s w`)
- [x] zM/zR fold-all/unfold-all for org headings
- [x] Insert heading (M-Enter respects level) — inserts at same level after subtree, enters insert mode (v0.6.0)

### M2: TODO & Agenda
- [x] TODO state cycling (S-Left/S-Right: TODO → DONE → unmarked) (v0.6.0)
- [ ] Priority cycling ([#A]/[#B]/[#C])
- [ ] Tags on headings (`:tag1:tag2:`)
- [ ] Agenda view: query across org files for TODO items

### M3: Tables & Lists
- [ ] Org table mode (alignment, navigation, structure editing, sorting, formulas)
- [x] Checkbox lists (`- [ ]` / `- [x]`) with toggle (v0.6.0: smart-enter + toggle_checkbox_at_cursor)
- [ ] Ordered/unordered list continuation on Enter

### M4: Rich Rendering ✅ (partial)
- [x] Inline markup rendering: `*bold*`, `/italic/`, `=code=`, `~verbatim~` (v0.6.0: compute_org_style_spans)
- [ ] Org `+strikethrough+` inline markup (markdown `~~text~~` done, org `+text+` missing)
- [x] Link rendering and following in org buffers (`[[target][display]]`) (v0.6.0: display regions)
- [ ] Image preview (terminal: sixel/kitty protocol; future: GUI)
- [x] Source block syntax highlighting (`#+begin_src lang`) (v0.6.0: inject_fenced_code_blocks)

### M5: Export & Babel
- [ ] Export to HTML/Markdown (basic)
- [ ] Babel code block execution (Scheme eval built-in, shell via PTY)
- [ ] Results blocks (`#+RESULTS:`)

---

## Phase 10: Package System Architecture Review

Architecture decision record — not implementation. The editor is accumulating
domain-specific subsystems (git_ops, org-mode, project management, LSP server
configs) that may belong as runtime-loadable packages rather than compiled-in
code. This phase produces a binding decision before Phase 6+ features
calcify the boundary.

### M1: Landscape Survey
- [ ] Review Neovim's lazy.nvim model — Lua-based, lazy-loaded, declarative specs
- [ ] Review Emacs's package.el + MELPA — elisp-only, runtime-installed, advice-friendly
- [ ] Review Helix's no-plugin philosophy — all features compiled-in, no user extensions
- [ ] Review Lapce's WASI plugin system — language-agnostic, sandboxed, capability-based
- [ ] Document tradeoffs: startup time, security, discoverability, API stability surface

### M2: MAE-Specific Analysis
- [ ] Inventory current compiled-in subsystems: git_ops, org parser, LSP configs,
      DAP configs, theme loader, syntax grammars, KB seeded help nodes
- [ ] For each: evaluate whether it should be a Scheme package, a WASI plugin,
      or remain compiled-in (using criteria from M1)
- [ ] Decision point: Scheme-only packages (Emacs model) vs WASI plugins (Lapce
      model) vs hybrid (Scheme for UI/glue, WASI for performance-critical)
- [ ] Assess impact on the "AI as peer" principle — can the AI install, inspect,
      and configure packages the same way a user can?

### M3: Architecture Decision Record
- [ ] Write ADR with decision, rationale, and consequences
- [ ] Define package manifest format (if applicable)
- [ ] Define package API contract (what hooks/events packages can bind to)
- [ ] Identify first candidate packages to extract (likely: themes, LSP server
      configs, org-mode, git porcelain)
- [ ] No implementation — output is the ADR document + updated ROADMAP entries

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
    ├─→ Phase 3f (AI multi-file) ✅ ← needed for self-hosting
    │       │
    │       └─→ Phase 3g (hardening) ✅ → Phase 3g-v2 (v0.4.1 modularization) ✅
    │
    ├─→ Phase 3h (vim/emacs parity) ✅
    │
    ├─→ Phase 4b (syntax highlighting) ✅
    │
    ├─→ Phase 4a (LSP) ✅ M1-M4 ← biggest unlock for self-hosting
    │       │
    │       └─→ Phase 4c (DAP) M1/M2/M3/M4 ✅
    │
    ├─→ Phase 4d + 5 (KB + help + SQLite) ✅
    │
    ├─→ Phase 6 (embedded shell) ✅
    │       │
    │       └─→ Phase 6 M5 (magit parity) ← builds on M1 PTY shell + SPC g stubs
    │
    ├─→ Phase 7 (embedded docs) ✅ M1-M4 COMPLETE ← module system remaining
    │
    ├─→ Phase 8 (GUI backend) ✅ M1-M6 COMPLETE
    │       │
    │       └─→ M7+ (images, PDF)
    │
    ├─→ Phase 9 (org-mode editing) ← builds on Phase 5 org parser
    │
    └─→ Phase 10 (package system ADR) ← before features calcify boundaries
```

**Next priority order:**
1. **Phase 7 M4 remaining** (module system) — `mae/module!` macros, layer system, `after!` hook
2. **Phase 8 M8-M9** (GUI) — PDF preview, advanced image features
3. **Phase 4c M3 remaining** — variable hover, watch expressions
4. **LSP packaging review** — multi-language defaults, user-configurable server selection
5. **Packaging readiness audit** — mae-dap, mae-lsp, mae-kb for crates.io publishability
6. **Phase 10** (Package System ADR) — decide package architecture before more subsystems land
7. **Phase 9** (Org-Mode Editing) — full org-mode environment

---

## Test Targets

| Phase | Tests | Notes |
|-------|-------|-------|
| 3e    | 506 ✅ | search, visual, change, count, scroll, text objects |
| 3f    | 521 ✅ | multi-file AI tools, project search, conversation persistence |
| 3g    | — ✅ | refactor only, preserved existing tests |
| 3h    | 1158 ✅ | registers, surrounds, vim quick wins, Scheme REPL, AI prompt UX |
| 4a    | 67 ✅ | LSP connection, navigation, diagnostics, completion (M1-M4) |
| 4b    | 29 ✅ | tree-sitter + syntax highlighting + structural ops |
| 4c    | 80 ✅ | DAP client, manager, AI debug tools, gutter rendering |
| 4d+5  | 70+ ✅ | KB in-memory + SQLite + org parser + help buffer + AI KB tools |
| 6     | 146 ✅ | shell terminal, hooks, options, MCP bridge, file auto-reload |
| 8 M1  | 26 ✅ | shell-insert keymap, permission config, GUI renderer, input translation |
| 8 M2  | 40 ✅ | self-test suite, input lock, AI tool parity, LSP AI tools, agent bootstrap |
| 8 M3-M4 + v0.3.0 | 141 ✅ | GUI polish, font zoom, BackTab, `:read !cmd`, session, tutor→KB, shell auto-close, debugger powerhouse, Doom init.scm |
| v0.4.1 modularization | 266 ✅ | 6 file splits, 12 code smell fixes, model-agnostic prompts |
| v0.5.0 agent reliability | 51 ✅ | agent reliability, DAP observability, self-test infrastructure |
| v0.5.1 vim parity + fixes | 36 ✅ | ghost cursor, status bar, block visual, ignorecase, :g/:v, C-t/C-d, cached theme resolution |
| v0.6.0 GUI + frameworks | 575 ✅ | display regions, BufferView/BufferMode, keymap overlays, Magit, swap files, variable-height, mouse |
| v0.7.0 docs + agent | 47 ✅ | help content, contextual help, user help nodes, config health, display policy, mouse focus |
| **Total** | **2,299** | All passing, 0 failures |
