# Architecture Guide

This document is for AI agents and contributors modifying MAE internals.
For user-facing docs, see README.org. For build instructions, see CLAUDE.md.

## Crate Layout

| Crate | Purpose |
|---|---|
| `mae-core` | Buffer (rope), event loop, editor state, commands, modes |
| `mae-renderer` | TUI rendering via ratatui/crossterm (`Renderer` trait) |
| `mae-gui` | GUI rendering via winit + Skia 2D |
| `mae-scheme` | Embedded Steel Scheme runtime |
| `mae-lsp` | LSP client (tower-lsp, diagnostics, completion) |
| `mae-dap` | DAP client (breakpoints, step, inspect) |
| `mae-ai` | AI agent transport (Claude/OpenAI/Gemini/DeepSeek) |
| `mae-kb` | Knowledge base (SQLite, org parser, bidirectional links) |

## GUI Rendering Pipeline

The GUI renderer uses a three-phase pipeline per window per frame:

1. **`compute_layout()`** — produces a `FrameLayout` (line positions, heights, scales)
2. **`render_buffer_content()`** — draws text using the `FrameLayout`
3. **`render_cursor()`** / `compute_cursor_position()` — positions cursor from `FrameLayout`

All three phases MUST consume the same `HighlightSpan` set. See `crates/gui/src/RENDERING.md`.

## Buffer Types and Rendering

| BufferKind | Renderer | Spans Source | BufferLocalOptions |
|---|---|---|---|
| Normal (code) | `render_buffer_content` | tree-sitter `syntax_spans` map | file-type defaults |
| Help | `render_buffer_content` | `highlight_spans_for_buffer()` | word_wrap=true |
| Conversation | `render_buffer_content` | `highlight_spans_for_buffer()` | word_wrap=true |
| GitStatus | `render_buffer_content` | `highlight_spans_for_buffer()` | — |
| *AI-Diff* | `render_buffer_content` | `highlight_spans_for_buffer()` | — |
| Messages | `render_messages_window` | dedicated | word_wrap=true |
| Shell | `render_shell_window` | dedicated (alacritty_terminal) | — |
| Debug | `render_debug_window` | dedicated | — |
| FileTree | `render_file_tree_window` | dedicated | — |
| Visual | dedicated | dedicated | — |

## BufferMode Trait

The `BufferMode` trait (`buffer_mode.rs`) is the contract every buffer kind implements.
It replaces scattered `match buf.kind` blocks with polymorphic dispatch:

| Method | Purpose |
|---|---|
| `mode_name()` | Display name for the status bar |
| `keymap_name()` | Overlay keymap name (e.g. `"git-status"`, `"help"`) |
| `read_only()` | Whether inserts are blocked |
| `default_word_wrap()` | Whether word-wrap defaults to on |
| `has_gutter()` | Whether line numbers render |
| `status_hint()` | One-line discoverability text on mode entry |
| `mode_theme_key()` | Status-bar mode indicator color |
| `insert_mode()` | Which insert mode to enter (Insert vs ShellInsert) |

`BufferKind` implements `BufferMode`. New buffer types add trait arms, not scattered matches.

## BufferView Enum

`BufferView` (`buffer_view.rs`) stores mode-specific state on `Buffer`:

- `Conversation(Box<Conversation>)`
- `Help(Box<HelpView>)`
- `Debug(Box<DebugView>)`
- `GitStatus(Box<GitStatusView>)`
- `Visual(Box<VisualBuffer>)`
- `FileTree(Box<FileTree>)`
- `None`

Accessor methods: `buf.conversation()`, `buf.help_view()`, `buf.git_status_view()`, etc.
Replaces 6 `Option<T>` fields that were always mutually exclusive.

## Keymap Inheritance

`Keymap` has a `parent: Option<String>` field. Buffer-kind overlay keymaps (git-status, help,
debug, file-tree) declare a parent. Key lookup: overlay keymap → parent → fallback.

`which_key_entries_for_current_keymap()` merges overlay + parent entries for the which-key popup.
`define-keymap` in Scheme: `(define-keymap "name" "parent")` creates keymaps with inheritance.

## Display Regions

`DisplayRegion` on `Buffer` provides Emacs-style `invisible` + `display` text properties for
link concealment. `compute_link_regions()` detects md/org links and builds regions per file
extension. The GUI and TUI renderers apply `display_map` on `LineLayout` to replace rope chars
with display chars (e.g. `[text](url)` → `text` with underline).

## Highlight Span Dedup

`render_common::spans::highlight_spans_for_buffer()` centralizes span selection for buffer kinds
that use the standard text pipeline (Conversation, Help, GitStatus, *AI-Diff*). Both GUI and TUI
renderers call this in their `_` arm — if `Some`, use shared spans; if `None`, use syntax spans.
Specialized renderers (Shell, Debug, Messages, Visual, FileTree) keep dedicated arms.

## Line Counting Rules

Ropey adds a phantom empty line after trailing `\n`. Two functions exist:

| Function | Includes phantom? | Use for |
|---|---|---|
| `line_count()` | Yes | `clamp_cursor()`, rope index lookups, search iteration |
| `display_line_count()` | No | **Navigation cursor positioning**, scroll bounds, gutter width |

**Rule**: If you're setting `cursor_row` during navigation (jumps, marks, goto, LSP, diagnostics),
use `display_line_count()`. Using `line_count()` allows the cursor to land on an invisible phantom
line (ghost line bug). `clamp_cursor()` is the exception — insert mode needs the phantom line
after pressing Enter at EOF.

See `crates/gui/src/RENDERING.md` for the full decision table and pixel budget rules.

## Option Registry

`OptionRegistry` (`options.rs`) is the single source of truth for all editor settings.
Each `OptionDef` has: name, aliases, kind (Bool/String/Float/Int/Theme), default, config_key, doc, valid_values.

Flow: `:set foo bar` → `Editor::set_option("foo", "bar")` → validates kind + range → sets field on `Editor`.
`get_option(name)` reads the field back. `OptionKind::Int` added in v0.6.0 for scroll_speed, popup sizing, etc.

Range clamping prevents rendering corruption (e.g. heading_scale ≤0 → infinite loop, scrollbar_width > cell_width).

## Scheme API Surface

The Scheme runtime exposes ~40 functions to extension code:

| Category | Functions |
|---|---|
| Buffer editing | `buffer-insert`, `buffer-delete-range`, `buffer-replace-range`, `buffer-undo`, `buffer-redo` |
| Buffer read | `*buffer-name*`, `*buffer-text*`, `*buffer-char-count*`, `buffer-text-range`, `*buffer-list*`, `get-buffer-by-name` |
| Cursor/nav | `cursor-goto`, `*cursor-row*`, `*cursor-col*`, `open-file`, `switch-to-buffer` |
| Windows | `*window-count*`, `*window-list*` |
| Options | `set-option!`, `set-local-option!`, `get-option`, `*option-list*` |
| Commands | `define-command`, `run-command`, `command-exists?`, `*command-list*` |
| Keymaps | `define-key`, `define-keymap`, `undefine-key!`, `*keymap-list*`, `keymap-bindings` |
| Hooks | `add-hook!`, `remove-hook!` |
| File I/O | `read-file`, `file-exists?`, `list-directory` |
| Packages | `require`, `provide`, `featurep`, `load-path`, `add-to-load-path!` |
| UI | `set-status!`, `set-theme!`, `message` |

Write-side: `SharedState` (Arc<Mutex>) accumulates `pending_*` fields during eval.
Read-side: `inject_editor_state()` snapshots editor state as global variables before each eval.
Apply: `apply_to_editor()` drains pending changes after eval completes.

## Package System

`require`/`provide` implement Emacs-style feature loading:

1. `(require "feature")` searches `load-path` for `feature.scm`
2. If found, evaluates the file
3. File must call `(provide "feature")` to mark itself as loaded
4. Subsequent `(require "feature")` returns immediately (no-op)

`load-path` defaults: `~/.config/mae/packages/`, `~/.config/mae/lisp/`.
`(add-to-load-path! DIR)` prepends to search path.

`CommandSource::Autoload { feature }` enables deferred loading: command dispatch triggers
`(require feature)` on first invocation, then re-dispatches the command.

## Key Invariants

1. **Span parity**: Layout and renderer must see identical spans. Divergence causes cursor misalignment.
2. **No `markup.heading` in conversation buffers**: Would trigger heading scaling, breaking uniform line heights.
3. **Line counting**: Navigation uses `display_line_count()`; rope indexing uses `line_count()`. See above.
4. **Layout pixel tolerance**: Overflow checks in `compute_layout()` use 0.5px FP tolerance. Do not remove.
5. **Module boundaries**: No 10k+ line files. Each crate has clear responsibilities.
6. **Runtime redefinability**: Users can redefine any function via Scheme while running.
