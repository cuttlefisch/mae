# Module: org

Org-mode keybindings — headings, TODOs, babel, export, narrowing.

## Info

| Field | Value |
|-------|-------|
| Category | lang |
| Version | 0.1.0 |
| Dependencies | None |

## Keybindings

All bindings apply in the `org` keymap (active in Org-mode buffers).

### Fold / Cycle

| Key | Command | Description |
|-----|---------|-------------|
| `Tab` | `org-cycle` | Cycle visibility of heading at point |
| `S-Tab` | `org-global-cycle` | Cycle visibility of all headings |

### TODO / Priority

| Key | Command | Description |
|-----|---------|-------------|
| `S-Left` | `org-todo-prev` | Cycle TODO state backward |
| `S-Right` | `org-todo-next` | Cycle TODO state forward |
| `S-Up` | `org-priority-up` | Increase priority |
| `S-Down` | `org-priority-down` | Decrease priority |

### Headings

| Key | Command | Description |
|-----|---------|-------------|
| `M-Left` / `M-h` | `org-promote` | Promote heading one level |
| `M-Right` / `M-l` | `org-demote` | Demote heading one level |
| `M-Up` / `M-k` | `org-move-subtree-up` | Move subtree up |
| `M-Down` / `M-j` | `org-move-subtree-down` | Move subtree down |
| `M-Enter` | `org-insert-heading` | Insert new heading at same level |
| `Enter` | `smart-enter` | Smart newline (continues lists, checkboxes) |

### Narrowing

| Key | Command | Description |
|-----|---------|-------------|
| `SPC m s n` | `org-narrow-subtree` | Narrow buffer to current subtree |
| `SPC m s N` / `SPC m s w` | `org-widen` | Widen back to full buffer |

### Links and Tags

| Key | Command | Description |
|-----|---------|-------------|
| `SPC m l` | `edit-link` | Edit link at point (URL + label dialog) |
| `SPC m t` | `org-set-tags` | Set tags on heading |

### Babel (requires `+babel` flag)

| Key | Command | Description |
|-----|---------|-------------|
| `SPC m x` / `C-c C-c` | `babel-execute` | Execute code block at point |
| `SPC m X` | `babel-execute-all` | Execute all code blocks in buffer |
| `SPC m T` | `babel-tangle` | Tangle code blocks to source files |
| `SPC m '` | `babel-edit-special` | Edit code block in dedicated buffer |

### Export (requires `+export` flag)

| Key | Command | Description |
|-----|---------|-------------|
| `SPC m e h` | `org-export-html` | Export buffer to HTML |
| `SPC m e m` | `org-export-markdown` | Export buffer to Markdown |
| `SPC m e s` | `org-export-subtree` | Export subtree |

## Configuration

### Module flags

| Flag | Description |
|------|-------------|
| `+babel` | Enable babel code block execution bindings |
| `+export` | Enable org export bindings (HTML, Markdown) |

Enable flags in `init.scm`:

```scheme
(use-module! "org" :flags '(babel export))
```
