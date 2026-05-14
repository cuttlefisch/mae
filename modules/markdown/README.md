# Module: markdown

Markdown keybindings — headings, promotion/demotion, narrowing.

## Info

| Field | Value |
|-------|-------|
| Category | lang |
| Version | 0.1.0 |
| Dependencies | None |

## Keybindings

All bindings apply in the `markdown` keymap (active in Markdown buffers).

### Fold / Cycle

| Key | Command | Description |
|-----|---------|-------------|
| `Tab` | `md-cycle` | Cycle visibility of heading at point |
| `S-Tab` | `md-global-cycle` | Cycle visibility of all headings |

### Headings

| Key | Command | Description |
|-----|---------|-------------|
| `M-Left` / `M-h` | `md-promote` | Promote heading one level |
| `M-Right` / `M-l` | `md-demote` | Demote heading one level |
| `M-Up` / `M-k` | `md-move-subtree-up` | Move subtree up |
| `M-Down` / `M-j` | `md-move-subtree-down` | Move subtree down |
| `M-Enter` | `md-insert-heading` | Insert new heading at same level |
| `Enter` | `smart-enter` | Smart newline (continues lists, etc.) |

### Narrowing

| Key | Command | Description |
|-----|---------|-------------|
| `SPC m s n` | `md-narrow-subtree` | Narrow buffer to current subtree |
| `SPC m s N` / `SPC m s w` | `md-widen` | Widen back to full buffer |

### Links

| Key | Command | Description |
|-----|---------|-------------|
| `SPC m l` | `edit-link` | Edit link at point (URL + label dialog) |

## Configuration

No module-specific options.
