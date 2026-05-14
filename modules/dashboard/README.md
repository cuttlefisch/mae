# Module: dashboard

Splash screen with ASCII art and quick-access menu.

## Info

| Field | Value |
|-------|-------|
| Category | ui |
| Version | 0.1.0 |
| Dependencies | None |

## Keybindings

This module registers the `dashboard` command but does not bind it to a key by default. Use `:dashboard` from the command line or bind it manually in `init.scm`.

| Key | Command | Description |
|-----|---------|-------------|
| — | `dashboard` | Show the dashboard/splash screen |

## Configuration

No module-specific options. The splash screen content is rendered by the kernel (Rust); this module provides the command binding only.
