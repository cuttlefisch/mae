# Module: surround

Vim-surround style commands (ds, cs, ys, yss, S in visual).

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
| `d s` | `delete-surround-await` | Delete surrounding delimiter (prompts for char) |
| `c s` | `change-surround-await` | Change surrounding delimiter (prompts for old then new) |
| `y s` | `operator-surround` | Surround motion/text-object with delimiter |
| `y s s` | `surround-line-await` | Surround current line with delimiter |

### Visual mode

| Key | Command | Description |
|-----|---------|-------------|
| `S` | `surround-visual-await` | Surround visual selection with delimiter |

## Configuration

No module-specific options. Delimiter pairs (e.g. `(`, `[`, `{`, `"`, `'`) are handled by the kernel.
