# MAE — Modern AI Editor

A terminal editor where the human and the AI are peer actors calling the same
Lisp primitives. Built on a Rust core with an embedded Scheme (R7RS-small)
runtime. LSP and DAP are first-class protocols exposed to both the extension
layer and the AI agent's tool-calling interface.

## The Problem

Emacs is the only editor with true runtime redefinability — you can redefine any
function while the editor is running, and that property makes it irreplaceable.
But after 35 years and 180k commits, its architecture has hit structural limits:

- **GC retrofit is intractable.** 23,901 commits across 3 branches trying to add
  concurrent garbage collection. Still unmerged. Real threading remains blocked.
- **The display engine is a monolith.** `xdisp.c` is 38,605 lines with 20k+
  commits per decade. ~10% of all Emacs commits are platform support.
- **The fix ratio doubled.** From 15% in the 1990s to 32% post-2010 — a
  complexity ceiling from C + untyped Lisp. One third of all effort is now fixes.
- **Bus factor of ~4 people.** Top 5 contributors = 50.8% of all commits.
  Critical subsystems have single-person dependencies.

Meanwhile, AI coding assistants are bolted onto editors as plugins — they can't
call the same functions as your keybindings, can't access LSP semantics or debug
state, and can't compose with your extensions.

## The Design

MAE makes the AI a peer, not a plugin:

- **Same API surface.** `(buffer-insert ...)`, `(lsp-references ...)`,
  `(dap-inspect-variable ...)` — human keybindings and AI tool calls resolve to
  the same Scheme functions.
- **Structured knowledge.** The AI gets LSP types and references, DAP call stacks
  and variables — it doesn't need to "read" code to understand it.
- **Composable extensions.** When a package author writes
  `(defun my/summarize-buffer ...)`, it's immediately available to both the
  user's keybinding and the AI's tool palette.

### Stack

| Layer | Technology | Why |
|-------|-----------|-----|
| Core | Rust | Eliminates GC problem, ownership model for concurrency |
| Extensions | Scheme R7RS-small (Steel) | Runtime redefinability, hygienic macros, tail calls |
| Terminal UI | ratatui + crossterm | Platform-specific code lives in the library, not us |
| AI | Claude / OpenAI APIs | Tool-calling maps 1:1 to Scheme API |
| Protocols | LSP + DAP | First-class, not bolted on — exposed to Scheme and AI |

### Architecture

```
┌──────────────────────────────────────┐
│           Scheme Runtime             │  ← Extensions, config, packages
│   (define-key, defadvice, REPL)      │
├──────────────────────────────────────┤
│          AI Tool Interface           │  ← Claude/OpenAI tool-calling
│   (same API as user keybindings)     │
├──────────┬───────────┬───────────────┤
│   LSP    │    DAP    │  Knowledge    │  ← Semantic code intelligence,
│  Client  │  Client   │    Base       │    debug state, graph store
├──────────┴───────────┴───────────────┤
│            Rust Core                 │  ← Buffers (rope), windows,
│   (ropey, crossbeam, tokio)          │    commands, event loop
├──────────────────────────────────────┤
│         Terminal Renderer            │  ← ratatui/crossterm (GPU future)
└──────────────────────────────────────┘
```

## Current Status

**Phase 3h + Phase 4a–4c — Keybinding parity, LSP completion, AI prompt UX** (852 tests passing)

| Feature | Status |
|---------|--------|
| Vi modal editing (Normal/Insert/Visual/Command) | ✓ |
| Motions: hjkl, w/b/e/W/B/E, f/F/t/T, %, {/}, 0/$ | ✓ |
| Operators: delete, change, yank, replace, dot repeat | ✓ |
| Count prefix (5j, 3dd, 2dw), marks, macros | ✓ |
| Search: /pattern, ?pattern, n/N, *, :s///g, :%s | ✓ |
| Visual mode: v (charwise), V (linewise) + operators | ✓ |
| Scroll: Ctrl-U/D/F/B, zz/zt/zb, H/M/L | ✓ |
| File management: :e, :w, :wq, :q, :q! + tab completion | ✓ |
| Fuzzy file picker (SPC f f) | ✓ |
| AI integration: Claude + OpenAI tool-calling, streaming | ✓ |
| AI conversation prompt: full readline + scroll + cursor | ✓ |
| Scheme runtime: Steel, init.scm, define-key, eval REPL | ✓ |
| 7 bundled themes (TOML-based, hot-switchable) | ✓ |
| Window splits (vertical/horizontal, binary tree layout) | ✓ |
| Readline keybinds in Insert and Command modes | ✓ |
| LSP completion popup (textDocument/completion) | ✓ |
| LSP go-to-definition, references, hover, diagnostics | ✓ |
| DAP client (protocol types + transport done) | ✓ |
| Tree-sitter syntax highlighting | ✓ |
| Full register system, vim-surround | Planned |
| Knowledge base (SQLite graph store + org-mode parser) | Planned |

See [ROADMAP.md](ROADMAP.md) for the full milestone plan.

## Building

```sh
cargo build --release
```

The binary is `target/release/mae`.

## Running

```sh
# Open a file
cargo run -- path/to/file.rs

# Open with AI (requires API key)
ANTHROPIC_API_KEY=sk-... cargo run -- file.rs
# or
OPENAI_API_KEY=sk-... cargo run -- file.rs
```

### Key Bindings

MAE uses vi-like modal editing:

| Key | Mode | Action |
|-----|------|--------|
| `i`, `a`, `A`, `o`, `O` | Normal | Enter insert mode |
| `Esc` | Any | Return to normal mode |
| `v` / `V` | Normal | Visual char / line selection |
| `d`, `c`, `y` | Normal/Visual | Delete, change, yank |
| `.` | Normal | Repeat last edit |
| `/`, `?` | Normal | Search forward / backward |
| `:` | Normal | Command mode |
| `SPC f f` | Normal | Fuzzy file picker |
| `SPC a a` | Normal | Open AI conversation |
| `SPC w v`, `SPC w s` | Normal | Split vertical / horizontal |

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
```

## Terminology

MAE uses some terms that overlap with Vim and Emacs but mean slightly different
things, and introduces a few new concepts. The short version:

- **Buffer** — in-memory content (text, AI conversation, or message log). May or
  may not be backed by a file. One buffer can be viewed in multiple windows.
- **Window** — a viewport onto a buffer, owning cursor and scroll state. Not an
  OS window. Splitting gives you more windows, not more buffers.
- **WindowManager** — owns the binary split tree that maps windows to screen areas.
- **Conversation buffer** — a buffer whose content is a structured AI chat session,
  not a rope. Rendered directly from structured data, not from text.
- **Mode** — Normal / Insert / Visual / Command / ConversationInput / Search / FilePicker.
  All input is routed through the active mode.
- **LspIntent queue** — how the synchronous editor core issues async LSP requests
  without touching async code directly.

See [docs/terminology.md](docs/terminology.md) for the full reference, including
comparisons with Vim, Emacs, and VSCode and a data-model walkthrough.

## Project Structure

```
crates/
  mae/          Binary — event loop, key handling, main()
  core/         Buffer (rope), editor state, commands, keymap, search, themes
  renderer/     Terminal rendering (ratatui), status bar, popups
  scheme/       Steel Scheme runtime, init.scm loading
  ai/           Claude + OpenAI providers, tool execution, conversation
  dap/          Debug Adapter Protocol types and transport
  lsp/          Language Server Protocol (planned)
  kb/           Knowledge base (planned)
```

## Self-Hosting Goal

The near-term goal is to use MAE + Claude to develop MAE itself. This requires
completing the editor essentials (Phase 3e), AI multi-file editing (Phase 3f),
and LSP integration (Phase 4a) so Claude has semantic code understanding.

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
