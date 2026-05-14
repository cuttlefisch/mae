# Module: search

Search and highlight — /, ?, n, N, *, #, gn/gN.

## Info

| Field | Value |
|-------|-------|
| Category | editor |
| Version | 0.1.0 |
| Dependencies | None |

## Keybindings

### Core search (normal mode)

| Key | Command | Description |
|-----|---------|-------------|
| `/` | `search-forward-start` | Start forward search |
| `?` | `search-backward-start` | Start backward search |
| `n` | `search-next` | Jump to next match |
| `N` | `search-prev` | Jump to previous match |
| `*` | `search-word-under-cursor` | Search forward for word at point |
| `#` | `search-word-under-cursor-backward` | Search backward for word at point |

### Visual select matches (normal mode)

| Key | Command | Description |
|-----|---------|-------------|
| `gn` | `visual-select-next-match` | Visually select next search match |
| `gN` | `visual-select-prev-match` | Visually select previous search match |

### Operator + match combos (normal mode)

| Key | Command | Description |
|-----|---------|-------------|
| `dgn` | `delete-next-match` | Delete next search match |
| `dgN` | `delete-prev-match` | Delete previous search match |
| `cgn` | `change-next-match` | Change next search match |
| `cgN` | `change-prev-match` | Change previous search match |
| `ygn` | `yank-next-match` | Yank next search match |
| `ygN` | `yank-prev-match` | Yank previous search match |

### Leader search group (normal mode)

| Key | Command | Description |
|-----|---------|-------------|
| `SPC s s` | `search-buffer` | Search current buffer |
| `SPC s p` / `SPC /` | `project-search` | Search across project (ripgrep) |
| `SPC s h` | `clear-search-highlight` | Clear search highlight |

## Configuration

No module-specific options.
