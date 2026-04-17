# MAE Roadmap

Current state: Phases 1-5 complete, Phase 6 M1 COMPLETE, M2 COMPLETE (1278+ tests). All Tier 1 self-hosting blockers done.
Terminal editor with vi-like modal editing, Scheme runtime, Claude/OpenAI/Ollama
integration, search, visual mode, text objects, change/repeat/replace, scroll,
indent/dedent, case change, line join, fuzzy file picker, command history, shell
escape, horizontal scroll, multi-file AI tools, embedded terminal emulator
(alacritty_terminal), Scheme hooks, and `set-option!` all working.

Self-hosting goal: use MAE + Claude/Ollama to develop MAE itself.

---

## Comprehensive Feature Checklist

### What We Have (521 tests)

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
| **Files** | :e (tab complete), :w, :w path, :wq, :q, :q!, SPC f f (fuzzy picker) |
| **Commands** | :!cmd (shell escape), command history (up/down), :ai-status |
| **AI** | Claude/OpenAI/Ollama tool-calling, conversation buffer, streaming, elapsed timer, multi-file tools, project search |
| **Scheme** | Steel runtime, init.scm, define-key, eval REPL |
| **Themes** | 7 bundled, TOML-based, hot-switchable |
| **Debug** | Self-debug state inspection, DAP protocol types |
| **Renderer** | Line numbers, status bar, which-key popup, multi-window, search/selection highlights |
| **CI** | GitHub Actions (check/test/clippy/fmt), tag-based release, dependabot, git-cliff changelog |

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
| 6 | Auto-reload on external change | future |
| 7 | `:set` options | future |
| 8 | Mouse support | future |
| 9 | `:read !cmd` | future |
| 10 | Multiple cursors | future |
| 11 | Session persistence | 3f M3 |
| 12 | README badges (CI status, Rust version, license, crate count) | future |
| 13 | File tree sidebar (NERDTree/neotree): persistent project tree pane with expand/collapse, file ops | future |

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
- [ ] `C-o` — execute one normal-mode command then return to insert
       (from *Practical Vim* ch. 15 — "Run a Normal Mode Command Without Leaving Insert Mode")

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

### M3: State Inspection
- [ ] `threads` → populate thread list
- [ ] `stackTrace` → populate stack frames
- [ ] `scopes` + `variables` → populate variable tree
- [ ] Variable hover (show value at cursor)
- [ ] Watch expressions

### M4: AI Debug Tools ✅ (754 tests)
- [x] AI tools: `dap_start`, `dap_set_breakpoint`, `dap_continue`, `dap_step`, `dap_inspect_variable`
- [x] Action-oriented design — read-side view already covered by `debug_state`
- [x] Permission tiers: `dap_start` Privileged, breakpoint/continue/step Write, inspect ReadOnly
- [x] Idempotent breakpoint set; explicit errors (not no-ops) on stale-state calls
- [x] Shared `dap_start_with_adapter` entry point — command & AI tool agree on preconditions
- [x] `StepKind` enum replaces stringly-typed step dispatch
- [x] `DebugState::find_variable` encapsulates scope iteration (no leak to tool layer)
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

### M3: Scheme Exposure
- [ ] `(shell-cwd)` — current working directory of the shell process
- [ ] Scheme REPL overlay in shell buffer (Layer 1)
- [ ] Pipe bridge: `shell | scheme-fn` and `scheme-fn | shell` (Layer 3)

### M4: Send-to-Shell
- [ ] `SPC e s` — eval-line sends current line to shell (like Emacs `C-c C-c`)
- [ ] `SPC e S` — eval-region sends visual selection to shell
- [ ] Shell-aware completion (optional, future)

### M5: Magit Parity
Full git porcelain in a dedicated buffer — the magit experience. Builds on
M1 PTY shell and the existing `SPC g` stubs.

- [ ] `SPC g s` — git status buffer with staged/unstaged/untracked sections
- [ ] Stage/unstage: `s` to stage file or hunk, `u` to unstage, `S`/`U` for all
- [ ] `c c` — commit (inline message editing), `c a` — amend
- [ ] Diff view: per-file and per-hunk diffs with syntax-highlighted context
- [ ] Log view: `l l` — commit history with graph, `l b` — branch log
- [ ] Blame: `SPC g b` — line-by-line blame in gutter or dedicated buffer
- [ ] Stash: `z z` — stash, `z p` — pop, `z l` — list stashes
- [ ] Keybindings match magit conventions where possible (s/u/c/l/z prefixes)
- [ ] AI tools: `git_status`, `git_diff`, `git_log` — structured JSON for agent use
- [ ] Scheme exposure: `(git-status)`, `(git-stage FILE)`, `(git-commit MSG)`

---

## Phase 7: Embedded Documentation System

Users must be able to discover, read, and navigate all editor documentation
from within the editor — and the AI peer must have native access to the same
docs to help users effectively. Builds on the existing KB + help buffer.

### M1: Comprehensive Help Content
- [ ] Auto-generate help pages for ALL registered commands (not just hand-authored)
- [ ] Auto-generate help pages for ALL keybindings (keymap → command → doc)
- [ ] Help pages for all Scheme primitives (`buffer-insert`, `define-key`, etc.)
- [ ] Tutorial/onboarding node: `concept:getting-started`

### M2: Contextual Help
- [ ] Hover-help for keybindings in which-key popup (expand doc inline)
- [ ] `:help` fuzzy completion (FTS5 search as you type)
- [ ] AI proactively references help nodes when answering user questions

### M3: Documentation Authoring
- [ ] `:help-edit <topic>` — edit a help node inline (user-authored overrides)
- [ ] User help nodes persisted to `~/.config/mae/help/` directory
- [ ] Org-mode format for user-authored help (parsed by existing org parser)

---

## Phase 8: Org-Mode Editing

Full org-mode editing support — MAE as a first-class org-mode environment.
Builds on the existing org parser (Phase 5 M2) and KB infrastructure.

### M1: Structural Editing
- [ ] Heading promotion/demotion (M-Left/M-Right or TAB cycling)
- [ ] Heading folding (TAB to cycle visibility: folded → children → subtree)
- [ ] Move subtree up/down (M-Up/M-Down)
- [ ] Insert heading (M-Enter respects level)

### M2: TODO & Agenda
- [ ] TODO state cycling (S-Left/S-Right: TODO → DONE → unmarked)
- [ ] Priority cycling ([#A]/[#B]/[#C])
- [ ] Tags on headings (`:tag1:tag2:`)
- [ ] Agenda view: query across org files for TODO items

### M3: Tables & Lists
- [ ] Org table alignment (Tab to next cell, auto-align)
- [ ] Checkbox lists (`- [ ]` / `- [x]`) with toggle
- [ ] Ordered/unordered list continuation on Enter

### M4: Rich Rendering
- [ ] Inline markup rendering: `*bold*`, `/italic/`, `=code=`, `~verbatim~`, `+strikethrough+`
- [ ] Link rendering and following in org buffers (`[[target][display]]`)
- [ ] Image preview (terminal: sixel/kitty protocol; future: GUI)
- [ ] Source block syntax highlighting (```#+begin_src lang```)

### M5: Export & Babel
- [ ] Export to HTML/Markdown (basic)
- [ ] Babel code block execution (Scheme eval built-in, shell via PTY)
- [ ] Results blocks (`#+RESULTS:`)

---

## Phase 9: Package System Architecture Review

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
    │       └─→ Phase 3g (hardening) ✅ ← before codebase grows further
    │
    ├─→ Phase 3h (vim/emacs parity) ✅
    │
    ├─→ Phase 4b (syntax highlighting) ✅
    │
    ├─→ Phase 4a (LSP) ✅ M1-M4 ← biggest unlock for self-hosting
    │       │
    │       └─→ Phase 4c (DAP) M1/M2/M4 ✅
    │
    ├─→ Phase 4d + 5 (KB + help + SQLite) ✅
    │
    ├─→ Phase 6 (embedded shell) ← next high-value target
    │       │
    │       └─→ Phase 6 M5 (magit parity) ← builds on M1 PTY shell + SPC g stubs
    │
    ├─→ Phase 7 (embedded docs) ← parallel with Phase 6
    │
    ├─→ Phase 8 (org-mode editing) ← builds on Phase 5 org parser
    │
    └─→ Phase 9 (package system ADR) ← before Phase 6+ features calcify boundaries
```

**Next priority order:**
1. **Phase 4c M3** (DAP state inspection UI) — debug panel for live debugging
2. **Phase 6 M1-M2** (Embedded Shell) — highest self-hosting value; makes MAE the user's primary terminal
3. **Phase 7 M1-M2** (Embedded Docs) — AI-native docs make the editor self-teaching
4. **Session & file management** — session save/restore, recent files, file watchers
5. **LSP packaging review** — multi-language defaults, user-configurable server selection
6. **Phase 9** (Package System ADR) — decide package architecture before more subsystems land
7. **Phase 8** (Org-Mode Editing) — full org-mode environment
8. **C-o in insert mode** (M1 remaining item) — quick win

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
| **Total** | **~1,148** | All passing, 0 failures |
