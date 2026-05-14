# Module: snippets

Snippet expansion engine — tab-stops, mirrors, field navigation.

## Info

| Field | Value |
|-------|-------|
| Category | editor |
| Version | 0.1.0 |
| Dependencies | None |

## Keybindings

| Key | Command | Description |
|-----|---------|-------------|
| `Tab` (insert) | `snippet-expand-or-next` | Expand snippet trigger or advance to next tab-stop |
| `S-Tab` (insert) | `snippet-prev-field` | Go to previous tab-stop |

Snippet sessions commit automatically on `Esc` (handled by the kernel).

## Configuration

No module-specific options. Snippet files are loaded from `~/.config/mae/snippets/` by convention.
