# Module: agenda

Org agenda buffer keybindings and SPC o a launcher.

## Info

| Field | Value |
|-------|-------|
| Category | app |
| Version | 0.1.0 |
| Dependencies | org |

## Keybindings

### Agenda buffer (agenda keymap)

| Key | Command | Description |
|-----|---------|-------------|
| `Enter` | `agenda-goto` | Go to item at point |
| `q` | `kill-buffer` | Close agenda buffer |
| `r` | `agenda-refresh` | Refresh agenda view |
| `t` | `agenda-filter-todo` | Filter by TODO state |
| `p` | `agenda-filter-priority` | Filter by priority |
| `?` | `show-buffer-keys` | Show all keybindings |

### Normal mode (launcher)

| Key | Command | Description |
|-----|---------|-------------|
| `SPC o a` | `open-agenda` | Open agenda buffer |
| `SPC o A` | `agenda-add` | Add item to agenda |

## Configuration

No module-specific options.
