# Module: tables

Table editing — align, insert/delete rows/columns, cell navigation.

## Info

| Field | Value |
|-------|-------|
| Category | editor |
| Version | 0.1.0 |
| Dependencies | org, markdown |

## Keybindings

These bindings are added to both the `org` and `markdown` keymaps under the `SPC m b` (buffer/table) prefix.

| Key | Command | Description |
|-----|---------|-------------|
| `SPC m b a` | `table-align` | Align table columns |
| `SPC m b i r` | `table-insert-row` | Insert row below cursor |
| `SPC m b d r` | `table-delete-row` | Delete row at cursor |
| `SPC m b i c` | `table-insert-column` | Insert column to the right |
| `SPC m b d c` | `table-delete-column` | Delete column at cursor |

The same bindings are available in both Org and Markdown buffers.

## Configuration

No module-specific options.
