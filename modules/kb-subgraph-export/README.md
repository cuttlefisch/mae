# Module: kb-subgraph-export

Export a KB subgraph to a standalone, self-contained, bilingual (EN/ES)
interactive HTML file — chord-diagram nav, language toggle, theme toggle,
browser-history navigation.

The actual export/render logic is compiled Rust — the standalone
`bilingual-kb-export` project (its own repo, path-dependency of `mae-ai`;
see its `kb/adrs/0002-mae-module-not-scheme-reimplementation.org` for why).
This module is thin wiring on top of it, matching `kb-graph-view`'s shape:
no export logic lives in Scheme.

## Info

| Field | Value |
|-------|-------|
| Category | tools |
| Version | 0.1.0 |
| Dependencies | `kb-graph-view` (shares its `graph` keymap) |

## Keybindings

### Graph view (graph keymap, extends kb-graph-view's own bindings)

| Key | Command | Description |
|-----|---------|--------------|
| `e` | `kb-subgraph-export-current` | Export the currently centered node's subgraph to `kb-export-<id>.html` |

## Scheme API

`(kb-export-subgraph-html ID PATH [DEPTH] [TRANSLATIONS] [TITLE])` — the
primitive itself (registered by `mae-scheme`, not this module — this module
only binds a key to it). Queues the request; applied on the next editor
tick, with a status-line result. Same underlying export the
`kb_export_subgraph_html` MCP tool and `:kb-export-html` colon-command use.

- `ID` (required) — seed/anchor node.
- `PATH` (required) — output file. Relative paths resolve against the open
  project root, else CWD.
- `DEPTH` (optional) — BFS hop radius, default 2, clamped to 4.
- `TRANSLATIONS` (optional) — path to a `{id: {title_es, body_es}}` JSON
  overlay. Omit for an English-only export; nodes with no entry (or an
  entry identical to the English text) render an inline fallback notice
  rather than silently mirroring English (see bilingual-kb-export's
  ADR-0003).
- `TITLE` (optional) — page `<title>`/`<h1>` text, default derived from the
  seed node's own title.

This module has no minibuffer/read-string primitive to build an interactive
ID/PATH prompt on top of (mae's Scheme runtime doesn't have one), so its one
command, `kb-subgraph-export-current`, reads the already-open graph view's
center node via `(kb-graph-view-state)` instead of asking. For an arbitrary
ID/PATH/TRANSLATIONS export, call `(kb-export-subgraph-html ...)` directly
from Scheme config, or use the AI's `kb_export_subgraph_html` MCP tool.
