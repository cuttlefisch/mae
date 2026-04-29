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

| BufferKind | Renderer | Spans Source |
|---|---|---|
| Normal (code) | `render_buffer_content` | tree-sitter `syntax_spans` map |
| Help | `render_buffer_content` | manual heading + inline + link spans |
| Conversation | `render_buffer_content` | `highlight_spans_with_markup()` |
| Messages | `render_messages_window` | dedicated |
| Shell | `render_shell_window` | dedicated (alacritty_terminal) |
| Debug | `render_debug_window` | dedicated |
| FileTree | `render_file_tree_window` | dedicated |
| GitStatus | `render_buffer_content` | standard pipeline |

## Key Invariants

1. **Span parity**: Layout and renderer must see identical spans. Divergence causes cursor misalignment.
2. **No `markup.heading` in conversation buffers**: Would trigger heading scaling, breaking uniform line heights.
3. **Module boundaries**: No 10k+ line files. Each crate has clear responsibilities.
4. **Runtime redefinability**: Users can redefine any function via Scheme while running.
