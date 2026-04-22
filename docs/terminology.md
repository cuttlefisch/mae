# MAE Terminology & Data Model

This document is the definitive reference for MAE's vocabulary and internal data
model. If you've used Vim, Emacs, or VSCode you'll find some familiar terms that
mean slightly different things here, and several new concepts that have no
equivalent elsewhere. Read this before diving into the code.

---

## Quick comparison

| Concept | MAE | Vim | Emacs | VSCode |
|---------|-----|-----|-------|--------|
| In-memory content | **Buffer** | buffer | buffer | document model |
| Viewport/pane | **Window** | window | window | editor pane |
| OS window | *(terminal, one)* | tab/frame | frame | window |
| Split layout | **WindowManager** | tabpage + splits | frame + window tree | editor group |
| Extension language | **Scheme** (Steel) | Vimscript / Lua | Emacs Lisp | TypeScript |
| AI conversation | **Conversation buffer** | *(none)* | *(none)* | Chat panel |
| Mode transitions | **Mode** enum | modes | major/minor modes | *(none)* |

---

## Core concepts

### Buffer

A **buffer** is an in-memory representation of content. It may or may not be
backed by a file on disk. There is always at least one buffer.

```
Buffer {
    rope:          Rope        — the actual text (O(log n) insert/delete)
    file_path:     Option<PathBuf>  — None = unsaved / special buffer
    name:          String      — display name (e.g. "main.rs", "[scratch]")
    kind:          BufferKind  — Text | Conversation | Messages
    modified:      bool        — unsaved changes?
    conversation:  Option<Conversation>  — set when kind == Conversation
    undo_stack / redo_stack    — per-buffer edit history
}
```

#### BufferKind

There are three kinds of buffer:

| Kind | Backing store | Purpose |
|------|--------------|---------|
| `Text` | `ropey::Rope` | Normal file editing |
| `Conversation` | `Conversation` struct + `ropey::Rope` | AI interaction history + search/selection |
| `Messages` | `MessageLog` | Live log of editor/tracing output, read-only |

`Text` buffers are backed by a rope data structure — a balanced tree that gives
O(log n) insert and delete at any position. This is the same choice as Helix,
Lapce, and other modern editors.

`Conversation` buffers use a hybrid model. The structured interaction history
lives in a `Conversation` struct, but it is **synced to a rope** whenever it
changes. This enables full peer interactivity: the human can use standard vi
motions, visual selection, and search on the AI's thoughts, while the AI
maintains its structured transactional context.


#### Difference from Emacs

In Emacs, special buffers like `*Messages*` and `*scratch*` are ordinary buffers
that happen to be named with asterisks — they're the same data structure as a
file buffer. In MAE, `Messages` and `Conversation` buffers use entirely different
backing stores and have dedicated rendering paths. The buffer kind is statically
known, which lets the renderer, editor commands, and AI tools all branch on it
cleanly.

---

### Window

A **window** is a view onto a buffer. It owns cursor position and scroll state.
It is **not** an OS window.

```
Window {
    id:           WindowId     — unique u32
    buffer_idx:   usize        — which buffer this window is viewing
    cursor_row:   usize        — row in buffer coordinates (0-based)
    cursor_col:   usize        — grapheme column in buffer coordinates
    scroll_offset: usize       — first visible row (vertical scroll)
    col_offset:   usize        — first visible column (horizontal scroll)
}
```

Key point: **cursor state belongs to the window, not the buffer**. Two windows
can display the same buffer at different cursor positions and scroll offsets —
exactly like Vim `:split` or Emacs `(split-window)`. This was one of Emacs's
correct early decisions and we follow it.

#### Difference from VSCode

VSCode and most GUI editors tie cursor state to the document/tab. If you open the
same file twice you get two independent documents. MAE (like Vim and Emacs) has
one buffer per file and can display it in multiple windows simultaneously with
independent cursors.

#### There is no "frame"

Emacs distinguishes:
- **window** — a viewport inside the Emacs process (what you'd call a "pane")
- **frame** — an OS-level window (what everyone else calls a "window")

MAE runs in a terminal. There is exactly one terminal window. The concept of an
Emacs "frame" does not exist in MAE. When GUI rendering is added (wgpu, future
work), the concept will be introduced then.

---

### WindowManager

The **WindowManager** owns all windows and their layout. It maintains a binary
split tree that maps windows to screen rectangles.

```
WindowManager {
    windows:    HashMap<WindowId, Window>
    layout:     LayoutNode   — binary split tree
    focused_id: WindowId     — which window receives input
}
```

The layout is a binary tree of splits:
```
LayoutNode::Leaf(window_id)
LayoutNode::VSplit { ratio, left, right }   — vertical split (side by side)
LayoutNode::HSplit { ratio, top, bottom }   — horizontal split (stacked)
```

At render time the renderer walks the tree, carves up the terminal area according
to the split ratios, and assigns a screen rectangle to each leaf window.

---

### Editor

The **Editor** struct is the single top-level state machine. It contains:
- the `Vec<Buffer>` (all open buffers)
- the `WindowManager` (all windows and their layout)
- the current `Mode`
- all transient state: command line, search input, completion popup, etc.

There is exactly one `Editor` per process. It has no I/O dependencies — file
reads and writes happen through `Buffer::from_file` and `Buffer::save`. The AI
agent, the Scheme runtime, and human keybindings all drive it through the same
method API (`editor.dispatch_builtin(name)` or direct method calls). This is the
central architectural commitment: **one state machine, one API surface, shared by
human and AI**.

---

### Mode

MAE is a modal editor. The active mode determines how keystrokes are interpreted.

| Mode | Description |
|------|-------------|
| `Normal` | Navigation and operator entry. The "home" mode. |
| `Insert` | Character-by-character text editing. |
| `Visual(Char)` | Character-wise visual selection. |
| `Visual(Line)` | Line-wise visual selection. |
| `Command` | Ex command line (`:` prompt). Readline-style editing. |
| `ConversationInput` | Typing into the AI conversation prompt. Full readline bindings. |
| `Search` | Incremental search (`/` or `?` prompt). |
| `FilePicker` | Fuzzy file picker overlay. |

#### Difference from Emacs

Emacs doesn't have modal editing — it uses key chords (`C-x C-s`, `M-x`) rather
than modes. MAE's modal model follows Vim/Helix. However, the extension language
(Scheme) is Lisp, and the configuration model (`define-key`, `defadvice`) is
closer to Emacs than to Vim's scripting.

---

## AI-specific concepts

### Conversation

A **Conversation** is the structured state for an AI interaction session. It
lives inside a `Conversation`-kind buffer.

```
Conversation {
    entries:        Vec<ConversationEntry>   — message history
    input_line:     String                   — current draft
    input_cursor:   usize                    — byte offset of editing cursor
    scroll:         usize                    — lines from bottom (0 = auto-follow)
    streaming:      bool                     — response in flight?
    streaming_start: Option<Instant>         — elapsed time display
}
```

Each entry has a `ConversationRole`:

| Role | Meaning |
|------|---------|
| `User` | Text the user typed and submitted |
| `Assistant` | Text the AI produced (may arrive via streaming) |
| `ToolCall { name }` | The AI called a tool (collapsed by default) |
| `ToolResult { success }` | The tool's return value (collapsed by default) |
| `System` | Informational messages (errors, cancellation notices) |

The `scroll` field controls history browsing: `0` means auto-follow the bottom
(new content scrolls into view); any positive value is a line offset upward from
the auto-scroll position. This lets users page through a long conversation while
typing continues correctly.

### AI as peer

The central design principle: **the AI calls the same functions as the user**.

When a user presses `dd`, the keymap dispatches `delete-line`. When the AI wants
to delete a line, it calls `(delete-line)` in Scheme — the same function. Both
paths go through `Editor::dispatch_builtin`. There is no separate "AI mode" and
no simulated keystrokes.

Practically this means:
- Every editor command the user can invoke is available to the AI as a tool.
- The AI cannot do anything the user cannot do (and vice versa).
- Permission tiers (ReadOnly / Write / Shell / Privileged) gate what the AI is
  allowed to call, not what exists.

### LspIntent queue

The `Editor` core is synchronous. LSP communication is async (over stdio to a
language server process). The bridge is a **intent queue**:

1. An editor command (triggered by keypress or AI tool call) pushes an
   `LspIntent` onto `editor.pending_lsp_requests`.
2. The binary's event loop drains the queue each tick, converts intents to async
   `LspCommand` messages, and sends them to the LSP task.
3. The LSP task sends results back via a channel; the binary applies them to the
   editor state.

This pattern keeps the `Editor` state machine free of async concerns. The same
pattern is used for DAP (`DapIntent` → `DapCommand`).

---

## Extension concepts

### Scheme runtime

MAE embeds [Steel](https://github.com/mattwparas/steel), an R7RS-small Scheme
implementation. The runtime serves the same role as Emacs Lisp in Emacs:
configuration, key binding, custom commands, packages.

Key points:
- `init.scm` is loaded at startup (equivalent to `.emacs` / `init.el`)
- `(define-key mode key command)` binds keys
- `(defun ...)` / `(define ...)` creates new editor commands
- `:eval (expr)` is the in-editor REPL
- All buffer operations (`buffer-insert`, `buffer-delete`, etc.) are exposed as
  Scheme functions — the same ones the AI uses as tools

There is no distinction between "built-in command" and "Scheme command" at the
user level. Both appear in the command palette, both can be bound to keys.

### Command

A **command** is a named, invocable unit of editor behavior:

```
Command {
    name:        String         — e.g. "delete-line", "cursor-down"
    description: String         — shown in command palette
    source:      CommandSource  — Builtin | Scheme(fn_name)
}
```

`Builtin` commands are Rust functions dispatched through `Editor::dispatch_builtin`.
`Scheme` commands call a named Scheme function with the editor state injected.

Commands are registered in the `CommandRegistry` at startup (builtins) or when
Scheme code is evaluated (`define-key` with a lambda creates a Scheme command on
the fly).

### Keymap

A **keymap** is a trie from key sequences to command names. There is one keymap
per mode (`"normal"`, `"insert"`, `"visual"`, `"command"`).

Key sequences can be chords of any length. A partial match (e.g. pressing `SPC`
then waiting) triggers the **which-key** popup, which shows all bindings that
complete the current prefix.

---

## What MAE intentionally omits (and why)

| Concept | Why absent |
|---------|-----------|
| **Frame** (OS window) | Terminal-only for now; will be introduced with GPU renderer |
| **Tab** (buffer list view) | Splits + file picker cover the use case without UI complexity |
| **Major/minor modes** (Emacs) | Scheme functions compose freely; there's no need for a mode layer |
| **Global Interpreter Lock** | Rust ownership + concurrent Scheme GC from day one |
| **Gap buffer** | Replaced by `ropey` rope for O(log n) edits anywhere |
| **xdisp.c equivalent** | Renderer is a separate crate; platform code lives in ratatui/wgpu |
