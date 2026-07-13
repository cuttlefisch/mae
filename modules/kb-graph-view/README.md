# Module: kb-graph-view

Native KB graph view keybindings and `SPC h g` leader entry.

## Info

| Field | Value |
|-------|-------|
| Category | tools |
| Version | 0.1.0 |
| Dependencies | None |

## Keybindings

### Graph view (graph keymap)

| Key | Command | Description |
|-----|---------|--------------|
| `h`/`j`/`k`/`l` | `kb-graph-view-navigate-{left,down,up,right}` | Move node selection |
| `Enter` | `kb-graph-view-select-current` | Navigate the companion window to the selected node |
| `g r` | `kb-graph-view-refresh` | Refresh in place |
| `q` / `Escape` | `kb-graph-view-close` | Close the graph view |
| `?` | `show-buffer-keys` | Show all keybindings |

### Leader

| Key | Command | Description |
|-----|---------|--------------|
| `SPC h g` | `kb-graph-view-open-default` | Open the native KB graph view |

## Scheme API

`(kb-graph-view-open [id] [depth])`, `(kb-graph-view-close)`,
`(kb-graph-view-refresh)`, `(kb-graph-view-set-depth n)`,
`(kb-graph-view-navigate dir)`, `(kb-graph-view-select-current)` — the same
primitives the `kb_graph_view_*` MCP tools call.
