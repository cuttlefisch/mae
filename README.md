# MAE — Modern AI Editor

A terminal editor where the human and the AI are peer actors calling the same
Lisp primitives. Built on a Rust core with an embedded Scheme (R7RS-small)
runtime. 1,277+ tests. GPL-3.0-or-later.

## Why MAE Exists

Emacs is the only editor with true runtime redefinability — you can redefine any
function while the editor is running. But after 35 years and 180k commits, its
architecture has hit structural limits:

- **GC retrofit is intractable.** 23,901 commits across 3 branches trying to add
  concurrent garbage collection. Still unmerged. Real threading remains blocked.
- **The display engine is a monolith.** `xdisp.c` is 38,605 lines. ~10% of all
  Emacs commits are platform support.
- **The fix ratio doubled.** From 15% in the 1990s to 32% post-2010. One third
  of all effort is now fixes.
- **Bus factor of ~4 people.** Top 5 contributors = 50.8% of all commits.

Meanwhile, AI coding assistants are bolted onto editors as plugins — they can't
call the same functions as your keybindings, can't access LSP semantics or debug
state, and can't compose with your extensions.

MAE is a from-scratch editor that addresses both problems: Emacs's architecture
limits and the AI-as-afterthought pattern.

## The Design

MAE makes the AI a **peer, not a plugin**. A keybinding and an AI tool-call both
resolve to the same command via the same dispatcher. There is no separate "AI
mode", no simulated keystrokes, no shadow API:

```
Human (keyboard) ──→ ┌──────────────────┐
Scheme (eval)    ──→ │   234 Commands   │ ──→ Editor State
AI (tool call)   ──→ │   (dispatch)     │ ──→ Buffers, LSP, DAP, Shell, KB
                     └──────────────────┘
All three actors call the same command dispatch.
The AI also has 39 specialized tools that map to the same primitives.
```

When you type `dd` to delete a line, the AI agent can invoke `delete-line` with
the same effect. When a package author writes `(define my/summarize ...)`, it's
immediately available to both the user's keybinding and the AI's tool palette.

## What Makes MAE Different

### AI as Peer Actor

Not a copilot sidebar. The AI calls the same 234 commands you do. It reads
LSP types, DAP debug state, tree-sitter parse trees, and the knowledge base —
structured data, not just syntax. 39 specialized AI tools plus every editor
command as a tool. Permission tiers (ReadOnly, Write, Shell, Privileged) let
you control how far the agent can act autonomously.

### Built-in Documentation & Knowledge Base

`:help` opens a hyperlinked knowledge base with 185+ nodes — the same docs the
AI reads. Tab cycles links, Enter follows, C-o / C-i for history (browser-like).
Every command is auto-documented at startup. The KB is backed by SQLite with
FTS5 full-text search, bidirectional links, org-mode parser for importing
existing notes, and graph queries from both Scheme and AI.

### Embedded Terminal Emulator

Full VT100/VT500 via `alacritty_terminal` — vim, fzf, htop, tmux all work
correctly. This is not a line-oriented shell. `Ctrl-\ Ctrl-n` exits to normal
mode (Neovim convention), `i` to re-enter. AI can observe terminal output and
send input via tools.

### Built-in Debugger (DAP)

Debug Adapter Protocol — multi-language debugging inside the editor. AI can set
breakpoints, step through code, inspect variables, and read call stacks.
Breakpoints and the execution line render in the gutter alongside diagnostics.

### LSP Integration

Go-to-definition (`gd`), find references (`gr`), hover docs (`K`), diagnostics,
completion popup with fuzzy matching. AI gets structured semantic data — not
just syntax, but types, references, and diagnostics.

### Runtime Redefinability (Scheme)

Embedded R7RS Scheme (Steel) — redefine any function while running. 7 hook
points (`before-save`, `after-save`, `buffer-open`, `buffer-close`,
`mode-change`) for event-driven config. `(set-option! ...)` for programmatic
configuration. `init.scm` is a real program, not a settings file.

### Tree-sitter Syntax Highlighting

13 languages (Rust, Python, JavaScript, TypeScript, TSX, Go, Bash, JSON, TOML,
Markdown, YAML, Scheme, Org) with structural parse trees. AI can query syntax
trees for code reasoning.

## Vim-Level Editing

Full vi modal editing with 234 commands:

| Category | Features |
|----------|----------|
| Modes | Normal, Insert, Visual (char/line), Command, Search, ShellInsert, FileBrowser, CommandPalette |
| Motions | hjkl, w/b/e/W/B/E, f/F/t/T, %, {/}, 0/$, gg/G, H/M/L, ge/gE |
| Operators | d, c, y — compose with any motion or text object |
| Text objects | `iw`, `aw`, `i(`, `a{`, `i"`, `it` (tag), and more |
| Count prefix | 5j, 3dd, 2dw |
| Dot repeat | Full `.` repeat for change/delete/insert sequences |
| Registers | Named (`"a`–`"z`), numbered (`"0`–`"9`), system clipboard (`"+`) |
| Macros | `qa` record, `q` stop, `@a` play, `@@` repeat |
| Marks | `ma` set, `'a` jump, `` `a `` exact position |
| Search | `/pattern`, `?pattern`, `n`/`N`, `*`, `:s///g`, `:%s` |
| Surround | `ys{motion}{char}`, `cs{old}{new}`, `ds{char}` (vim-surround) |
| Visual | `v` (charwise), `V` (linewise) + all operators |
| Scroll | Ctrl-U/D/F/B, zz/zt/zb, H/M/L |
| Leader | 14-group `SPC` leader system (Doom Emacs style) with which-key popup |

## Stack

| Layer | Technology | Why |
|-------|-----------|-----|
| Core | Rust | Eliminates GC problem, ownership model for concurrency |
| Extensions | Scheme R7RS-small (Steel) | Runtime redefinability, hygienic macros, tail calls |
| Terminal UI | ratatui + crossterm | Platform-specific code lives in the library, not us |
| Terminal emulator | alacritty_terminal | Full VT100/VT500, same engine as Alacritty |
| AI | Claude / OpenAI APIs | Tool-calling maps 1:1 to command API |
| Protocols | LSP + DAP | First-class, not bolted on — exposed to Scheme and AI |
| Knowledge base | SQLite + FTS5 | Graph store with full-text search |
| Syntax | tree-sitter | 13 languages, structural parse trees |

### Crate Architecture

```
mae (binary)
 ├── mae-core       Buffer (rope), editor state, commands, keymap, search, themes, syntax
 ├── mae-renderer    Terminal rendering (ratatui), status bar, popups, shell viewport
 ├── mae-scheme      Steel Scheme runtime, init.scm loading, hook dispatch
 ├── mae-ai          Claude + OpenAI providers, tool execution, conversation
 ├── mae-lsp         LSP client — connection, navigation, diagnostics, completion
 ├── mae-dap         DAP client — protocol types, transport, breakpoints, stepping
 ├── mae-shell       Terminal emulator (alacritty_terminal), PTY management
 └── mae-kb          Knowledge base — graph store, org-mode parser, FTS5 search
```

### Event Loop

The binary's `select!` loop multiplexes:

- **Crossterm key events** → modal dispatch (Normal/Insert/Visual/ShellInsert/...)
- **LSP responses** → diagnostics, completions, navigation results
- **DAP events** → breakpoint hits, variable updates
- **Shell PTY output** → viewport render + exit detection
- **AI stream chunks** → conversation buffer updates + tool execution
- **Scheme eval results** → command execution
- **Render tick** → ratatui frame draw (~30fps when shells active)

## Getting Started

```sh
# Build
cargo build --release

# Open a file
./target/release/mae path/to/file.rs

# Open with AI (requires API key)
ANTHROPIC_API_KEY=sk-... ./target/release/mae file.rs
# or
OPENAI_API_KEY=sk-... ./target/release/mae file.rs
```

### Configuration

MAE loads `~/.config/mae/init.scm` on startup:

```scheme
;; Example init.scm
(set-option! "theme" "gruvbox-dark")
(set-option! "relative-line-numbers" "true")
(set-option! "word-wrap" "true")

;; Custom keybinding
(define-key "normal" "SPC t t" "cycle-theme")

;; Hook: auto-format before save (when LSP available)
(add-hook! "before-save" "my-format-fn")
```

### Key Bindings (summary)

| Key | Mode | Action |
|-----|------|--------|
| `i`, `a`, `A`, `o`, `O` | Normal | Enter insert mode |
| `Esc` | Any | Return to normal mode |
| `v` / `V` | Normal | Visual char / line selection |
| `d`, `c`, `y` | Normal/Visual | Delete, change, yank |
| `.` | Normal | Repeat last edit |
| `/`, `?` | Normal | Search forward / backward |
| `:` | Normal | Command mode |
| `gd` | Normal | Go to definition (LSP) |
| `gr` | Normal | Find references (LSP) |
| `K` | Normal | Hover docs (LSP) |
| `SPC SPC` | Normal | Command palette (fuzzy) |
| `SPC f f` | Normal | Fuzzy file picker |
| `SPC a a` | Normal | Open AI conversation |
| `SPC o t` | Normal | Open terminal |
| `SPC h h` | Normal | Help index |
| `SPC d b` | Normal | Toggle breakpoint |
| `Ctrl-\ Ctrl-n` | ShellInsert | Exit terminal → Normal |

### Commands

```
:w              Save
:w path         Save as
:e path         Open file (Tab to complete)
:q              Quit (fails if unsaved)
:q!             Force quit
:wq             Save and quit
:s/old/new/g    Substitute on current line
:%s/old/new/g   Substitute in entire buffer
:theme name     Switch theme
:eval (expr)    Evaluate Scheme expression
:help           Open help / knowledge base
:terminal       Open embedded terminal
:command-list   Show all commands
```

## Project Structure

```
crates/
  mae/          Binary — event loop, key handling, main()
  core/         Buffer (rope), editor state, commands, keymap, search, themes, syntax
  renderer/     Terminal rendering (ratatui), status bar, popups, shell viewport
  scheme/       Steel Scheme runtime, init.scm loading, hook dispatch
  ai/           Claude + OpenAI providers, tool execution, conversation
  lsp/          LSP client — connection, navigation, diagnostics, completion
  dap/          DAP client — protocol types, transport, breakpoints, stepping
  kb/           Knowledge base — SQLite graph store, org-mode parser, FTS5 search
  shell/        Terminal emulator (alacritty_terminal), PTY management
```

## Self-Hosting Goal

The near-term goal is to use MAE + Claude to develop MAE itself. All Tier 1
blockers are complete: multi-file AI editing, LSP semantic understanding,
tree-sitter syntax highlighting, DAP debugging, and the embedded terminal.
MAE is now used for its own development alongside Emacs.

## Design Lineage

This project is informed by a detailed analysis of [35 years of Emacs git
history](https://github.com/emacs-mirror/emacs) — identifying the structural
decisions that led to its current maintenance burden and designing around them.
The full analysis is in the project's org-roam notes.

Key lessons applied:
- Concurrency from day one (no GC retrofit)
- Modular display layer (no monolithic `xdisp.c`)
- Module boundaries that enable distributed ownership
- Forge-native workflow (no mailing lists, no copyright assignment)

## License

GPL-3.0-or-later — see [LICENSE](LICENSE) for details.

Contributions are owned by their authors. No copyright assignment required.
