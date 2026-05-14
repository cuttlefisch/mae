# Module: spell

Spell checking via aspell/hunspell with inline markers.

## Info

| Field | Value |
|-------|-------|
| Category | checkers |
| Version | 0.1.0 |
| Dependencies | None |

## Keybindings

| Key | Command | Description |
|-----|---------|-------------|
| `z=` | `spell-suggest` | Show correction suggestions for word at point |
| `]s` | `spell-next` | Jump to next misspelled word |
| `[s` | `spell-prev` | Jump to previous misspelled word |
| `SPC t s` | `spell-toggle` | Toggle spell checking in current buffer |

## Configuration

No module-specific options. Requires `aspell` or `hunspell` to be installed on the system.
