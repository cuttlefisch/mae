# Module: marks-jumps

Vi marks (m/'), jump list (C-o/C-i), and change list (g;/g,).

## Info

| Field | Value |
|-------|-------|
| Category | editor |
| Version | 0.1.0 |
| Dependencies | None |

## Keybindings

### Normal mode

| Key | Command | Description |
|-----|---------|-------------|
| `m` | `set-mark-await` | Set mark in register (prompts for letter) |
| `'` | `jump-mark-await` | Jump to mark in register (prompts for letter) |
| `C-o` | `jump-backward` | Jump backward in jump list |
| `C-i` | `jump-forward` | Jump forward in jump list |
| `g ;` | `change-backward` | Jump to previous change location |
| `g ,` | `change-forward` | Jump to next change location |

### Visual mode

| Key | Command | Description |
|-----|---------|-------------|
| `m` | `set-mark-await` | Set mark in register |
| `'` | `jump-mark-await` | Jump to mark in register |

## Configuration

No module-specific options. Mark and jump list state is maintained by the kernel.
