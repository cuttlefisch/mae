# Module: format

Code formatting — external formatters and format-on-save.

## Info

| Field | Value |
|-------|-------|
| Category | editor |
| Version | 0.1.0 |
| Dependencies | None |

## Keybindings

| Key | Command | Description |
|-----|---------|-------------|
| `SPC c f` | `format-buffer` | Format the current buffer |

## Configuration

### Module flags

| Flag | Description |
|------|-------------|
| `+onsave` | Format buffer on save via the `before-save` hook |

Enable the flag in `init.scm`:

```scheme
(use-module! "format" :flags '(onsave))
```

When `+onsave` is active, the `format-before-save` hook runs automatically before every save.
