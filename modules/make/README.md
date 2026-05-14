# Module: make

Build system runner — auto-detect, compile, jump to errors.

## Info

| Field | Value |
|-------|-------|
| Category | tools |
| Version | 0.1.0 |
| Dependencies | None |

## Keybindings

| Key | Command | Description |
|-----|---------|-------------|
| `SPC c b` | `run-build` | Run the project build command |
| `SPC c t` | `run-test` | Run the project test command |
| `SPC c n` | `next-error` | Jump to next build error |
| `SPC c p` | `prev-error` | Jump to previous build error |

## Configuration

No module-specific options. The build and test commands are auto-detected from the project root (Makefile, Cargo.toml, package.json, etc.).
