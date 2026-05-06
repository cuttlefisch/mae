# MAE — Modern AI Editor

[![License: GPL-3.0-or-later](https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/tests-2%2C629%20passing-brightgreen.svg)](#)
[![Lines of Code](https://img.shields.io/badge/lines-~120k-informational.svg)](#)
[![Built with AI](https://img.shields.io/badge/Built%20with-Claude%20+%20Gemini%20+%20DeepSeek-blueviolet.svg)](https://github.com/cuttlefisch/mae)

An AI-native lisp machine editor. The human and the AI are peer actors calling
the same Scheme primitives. Built on a Rust core with an embedded R7RS-small
runtime. 2,629 tests. GPL-3.0-or-later.

<p align="center">
  <img src="assets/mae-screenshot.svg" alt="MAE screenshot" width="700">
</p>

## Features

- **AI as peer actor** — 450+ editor commands exposed as AI tools. The AI calls
  the same `dispatch_builtin()` as your keybindings. No shadow API, no simulated
  keystrokes.
- **Multi-provider** — Claude, OpenAI, Gemini, and DeepSeek. Provider-aware
  prompt tuning. Tiered prompt system (Full/Compact) with per-model guardrails.
- **Full vi modal editing** — Motions, operators, text objects, count prefix,
  dot repeat, macros, registers, marks, surround, visual block mode, multi-cursor.
- **LSP first-class** — Go-to-definition, references, hover, completion, rename,
  format, symbol outline, breadcrumbs, peek references. AI gets structured
  semantic data.
- **DAP first-class** — Multi-language debugging (Python, Rust, C/C++).
  Breakpoints (conditional, logpoint), watch expressions, exception breakpoints.
  AI can set breakpoints and inspect variables.
- **Org-mode babel** — Execute code blocks in 8 languages, noweb expansion,
  `:tangle` directive, `:var` cross-references, safety policies. Export to
  HTML and Markdown with TOC, syntax highlighting, tag filtering.
- **Embedded terminal** — Full VT100/VT500 via `alacritty_terminal`. AI can
  observe output and send input. `Ctrl-\ Ctrl-n` exits to normal mode.
- **Knowledge base** — SQLite + FTS5 graph store. 200+ help nodes, bidirectional
  links, org-mode parser, federated instances. Same docs the AI reads.
- **Runtime redefinability** — Embedded R7RS Scheme (Steel). Redefine any
  function while running. 45+ primitives, 17 hook points, `init.scm` is a
  real program.
- **Tree-sitter** — 13 languages with structural parse trees. AI can query
  syntax trees for code reasoning.
- **GUI + Terminal** — winit + Skia 2D hardware-accelerated GUI, ratatui
  terminal fallback. Inline images (PNG/JPG/SVG), variable-height rendering,
  inertial scrolling. Desktop launcher for freedesktop environments.

## Architecture

```
   Human (keys)      Scheme (eval)      AI / MCP (tool call)
        │                  │                     │
        ▼                  ▼                     ▼
   ┌───────────┐    ┌─────────────┐    ┌────────────────────┐
   │ Keymap    │    │ (run-cmd)   │    │  Tool wrappers     │
   │ Lookup    │    │ (define-    │    │  → same functions  │
   │           │    │  command)   │    │  as keybindings    │
   └─────┬─────┘    └──────┬──────┘    └─────────┬──────────┘
         │                 │                     │
         └────────────┬────┴─────────────────────┘
                      ▼
         ┌───────────────────────────┐
         │     Editor Core API       │
         │  dispatch_builtin()       │
         │  buffer.insert/delete()   │
         │  lsp/dap/kb/shell ops     │
         │                           │
         │  450+ commands · same     │
         │  functions for all actors │
         └─────────────┬─────────────┘
                       ▼
         ┌───────────────────────────┐
         │      Editor State         │
         │  Buffers · LSP · DAP      │
         │  Shell · KB · Themes      │
         └───────────────────────────┘
```

All three actors converge on the same Editor Core API. The AI's tools are thin
wrappers — `buffer_read` calls the same `buffer.line()` the renderer uses;
`lsp_definition` queues the same intent as pressing `gd`. When a package author
writes `(define my/summarize ...)`, it's immediately available to both the
user's keybinding and the AI's tool palette.

### Crate Layout

```
mae (binary)
 ├── mae-core       Buffer (rope), editor state, commands, keymap, syntax, babel, export
 ├── mae-renderer    Terminal rendering (ratatui), status bar, popups, shell viewport
 ├── mae-gui         GUI rendering (winit + Skia 2D), mouse input, font config, inline images
 ├── mae-scheme      Steel Scheme runtime, init.scm loading, hook dispatch
 ├── mae-ai          Claude + OpenAI + Gemini + DeepSeek providers, tool execution, conversation
 ├── mae-lsp         LSP client — connection, navigation, diagnostics, completion, formatting
 ├── mae-dap         DAP client — protocol types, transport, breakpoints, stepping, watches
 ├── mae-shell       Terminal emulator (alacritty_terminal), PTY management
 ├── mae-kb          Knowledge base — graph store, org-mode parser, FTS5 search, federation
 └── mae-mcp         MCP bridge — Unix socket server, JSON-RPC, stdio shim
```

## Getting Started

### Prerequisites

- **Rust stable** (1.75+) via [rustup](https://rustup.rs)
- **GUI deps:** `fontconfig-devel`, `freetype-devel` (Fedora) / `libfontconfig1-dev`, `libfreetype6-dev` (Debian/Ubuntu) / Xcode CLI Tools (macOS)
- **Optional:** `make setup-dev` installs `lldb`, `rust-analyzer`, `debugpy` for full self-test coverage

### Build & Run

```sh
git clone git@github.com:cuttlefisch/mae.git && cd mae
make build                      # GUI build (default)
make install                    # install binary + desktop launcher
mae --gui file.rs               # launch GUI
mae file.rs                     # terminal mode
make build-tui                  # terminal-only (no skia dependency)
```

### AI Setup

> [!CAUTION]
> **MAE is in early Alpha.** AI features and cost guardrails are experimental.
> Always monitor your API usage and costs directly in your provider dashboards.

Set one of these environment variables:

```sh
export ANTHROPIC_API_KEY=sk-ant-...    # Claude (default)
export OPENAI_API_KEY=sk-...           # OpenAI
export GEMINI_API_KEY=...              # Gemini
export DEEPSEEK_API_KEY=...            # DeepSeek
```

Or configure in `~/.config/mae/config.toml`:

```toml
[ai]
provider = "claude"
model = "claude-sonnet-4-20250514"

# Optional: permission tier (readonly, write, shell, privileged)
# Default: shell (AI can read, write, and run commands)
permission_tier = "shell"

# Optional: force prompt tier (full, compact)
# Default: auto-detected from model
# prompt_tier = "full"
```

**Provider-aware prompts:** MAE auto-detects the provider from the model name
and injects provider-specific guidance (e.g., Gemini gets explicit JSON
examples; DeepSeek gets anti-looping guardrails).

### First Steps

1. `:tutor` — interactive tutorial (12 lessons: vim, beginner, AI tracks)
2. `SPC SPC` — command palette (fuzzy search all commands)
3. `SPC f f` — find file in project
4. `SPC h h` — help index (knowledge base)
5. `SPC a a` — launch AI agent in embedded shell
6. `SPC a p` — start an AI conversation
7. `:self-test` — verify AI integration

### Configuration

MAE loads `~/.config/mae/init.scm` on startup. This is a real Scheme program,
not a settings file:

```scheme
;; Example init.scm
(set-option! "theme" "gruvbox-dark")
(set-option! "relative-line-numbers" "true")
(set-option! "word-wrap" "true")

;; Custom keybinding
(define-key "normal" "SPC t t" "cycle-theme")

;; Hook: run on buffer save
(add-hook! "before-save" "my-format-fn")
```

Project-local config: `.mae/init.scm` is loaded after user config.
`:describe-configuration` shows a health report. `--check-config` validates
without launching.

## Vim-Level Editing

Full vi modal editing with 450+ commands:

| Category | Features |
|----------|----------|
| Modes | Normal, Insert, Visual (char/line/block), Command, Search, ShellInsert |
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
| Multi-cursor | `Ctrl-d` add next match, `Ctrl-Alt-d` add all, `mc-align` |
| Scroll | Ctrl-U/D/F/B, zz/zt/zb, inertial (kinetic) scrolling in GUI |
| Leader | `SPC` leader system (Doom Emacs style) with which-key popup |
| Code folding | `za` toggle, `zM` close all, `zR` open all (tree-sitter ranges) |
| File tree | `SPC f t` sidebar with expand/collapse, git markers |
| Git status | `SPC g s` Magit-style: stage/unstage/discard at hunk level |
| Swap files | Crash recovery via non-destructive swap files |

### Key Bindings

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
| `SPC SPC` | Normal | Command palette |
| `SPC f f` | Normal | Fuzzy file picker |
| `SPC a a` | Normal | AI agent (shell) |
| `SPC a p` | Normal | AI conversation |
| `SPC o t` | Normal | Open terminal |
| `SPC d b` | Normal | Toggle breakpoint |
| `SPC h h` | Normal | Help index |
| `Ctrl-\ Ctrl-n` | ShellInsert | Exit terminal → Normal |

### Ex Commands

```
:w              Save
:e path         Open file
:q              Quit
:wq             Save and quit
:s/old/new/g    Substitute (current line)
:%s/old/new/g   Substitute (whole buffer)
:theme name     Switch theme
:eval (expr)    Evaluate Scheme
:help topic     Open help
:terminal       Open terminal
:tutor          Interactive tutorial
:self-test      AI integration test
```

## Stack

| Layer | Technology | Why |
|-------|-----------|-----|
| Core | Rust | Eliminates GC problem, ownership model for concurrency |
| Extensions | Scheme R7RS-small (Steel) | Runtime redefinability, hygienic macros, tail calls |
| Terminal UI | ratatui + crossterm | Platform-specific code in the library, not us |
| GUI | winit + skia-safe | Hardware-accelerated 2D, mouse, fonts, inline images |
| Terminal emulator | alacritty_terminal | Full VT100/VT500, same engine as Alacritty |
| AI | Claude / OpenAI / Gemini / DeepSeek | Tool-calling maps 1:1 to command API |
| Protocols | LSP + DAP | First-class — exposed to Scheme and AI |
| Knowledge base | SQLite + FTS5 | Graph store with full-text search, federation |
| Syntax | tree-sitter | 13 languages, structural parse trees |
| Literate programming | Org-babel | 8 execution languages, tangle, noweb, export |

## Roadmap

See [ROADMAP.md](ROADMAP.md) for detailed milestone tracking.

| Phase | Status | Summary |
|-------|--------|---------|
| 1. Core + Renderer | ✅ Complete | Buffer (rope), event loop, terminal renderer, modal editing |
| 2. Scheme Runtime | ✅ Complete | Steel R7RS-small, config loading, `define-key`, REPL |
| 3. AI Integration | ✅ Complete | Multi-provider tool-calling, conversation, permissions |
| 4. LSP + DAP + Syntax | ✅ Complete | Full LSP client, DAP client, 13-language tree-sitter |
| 5. Knowledge Base | ✅ Complete | SQLite graph, org parser, FTS5, help system, federation |
| 6. Embedded Shell | ✅ Complete | alacritty_terminal, MCP bridge, file auto-reload |
| 7. Documentation | ✅ Complete | Tutor (12 lessons), `:describe-configuration`, `--check-config` |
| 8. GUI Backend | ✅ Complete | winit + Skia, inline images, variable-height, inertial scroll |
| 9. Babel + Export | ✅ Complete | 8-language executor, HTML/Markdown export, KB federation |
| **Next** | 🔧 In progress | PDF preview, module system, semantic code search |

## Design Lineage

MAE is informed by analysis of [35 years of Emacs git
history](https://github.com/emacs-mirror/emacs) — identifying the structural
decisions that led to its current maintenance burden:

- **GC retrofit is intractable** — 23,901 commits across 3 branches, still
  unmerged. MAE uses Rust ownership (no GC needed).
- **`xdisp.c` is 38,605 lines** — monolithic display engine. MAE uses a
  `Renderer` trait with separate terminal and GUI backends.
- **Fix ratio doubled** — from 15% to 32% over 35 years. Rust's type system
  structurally prevents this.
- **Bus factor of ~4** — top 5 contributors = 50.8% of commits. MAE enforces
  module boundaries across 10 crates.

## Self-Hosting

MAE is used (alongside Emacs) to develop itself. The AI agent runs in an
embedded shell, calling the same tools the human uses. The GUI is the primary
development target.

## Contributing

Feature branches + PR workflow. CI runs `cargo fmt/clippy/test` on stable.

```bash
make ci          # Full CI pipeline locally (no GUI)
make verify      # check + test + GUI check with summary
make self-test   # AI-driven end-to-end self-test (headless)
```

See [CLAUDE.md](CLAUDE.md) for architecture principles and development guide.

## License

GPL-3.0-or-later — see [LICENSE](LICENSE) for details.

Contributions are owned by their authors. No copyright assignment required.
