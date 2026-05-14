# Module: file-tree

NERDTree-style file browser sidebar.

## Info

| Field | Value |
|-------|-------|
| Category | ui |
| Version | 0.1.0 |
| Dependencies | None |

## Keybindings

### Normal mode (toggle)

| Key | Command | Description |
|-----|---------|-------------|
| `SPC f t` | `file-tree-toggle` | Toggle file tree sidebar |

### File tree buffer (file-tree keymap)

#### Navigation

| Key | Command | Description |
|-----|---------|-------------|
| `j` | `file-tree-down` | Move cursor down |
| `k` | `file-tree-up` | Move cursor up |
| `gg` | `file-tree-first` | Go to first entry |
| `G` | `file-tree-last` | Go to last entry |
| `C-e` | `file-tree-scroll-down` | Scroll down |
| `C-y` | `file-tree-scroll-up` | Scroll up |
| `C-d` | `file-tree-half-page-down` | Scroll half page down |
| `C-u` | `file-tree-half-page-up` | Scroll half page up |

#### Actions

| Key | Command | Description |
|-----|---------|-------------|
| `Enter` / `o` | `file-tree-open` | Open file or expand directory |
| `s` | `file-tree-open-vsplit` | Open in vertical split |
| `i` | `file-tree-open-hsplit` | Open in horizontal split |
| `Tab` | `file-tree-expand` | Expand directory |
| `S-Tab` | `file-tree-global-cycle` | Cycle all directories open/closed |
| `x` | `file-tree-close-parent` | Close parent directory |
| `u` | `file-tree-parent` | Move to parent directory |
| `C` | `file-tree-cd` | Change root to selected directory |
| `R` | `file-tree-refresh` | Refresh tree |
| `m a` | `file-tree-create` | Create new file or directory |
| `d` | `file-tree-delete` | Delete entry |
| `r` | `file-tree-rename` | Rename entry |
| `q` | `file-tree-toggle` | Close file tree |
| `?` | `show-buffer-keys` | Show all keybindings |

## Configuration

No module-specific options.
