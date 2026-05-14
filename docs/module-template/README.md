# Creating a MAE Module

## Quick Start

1. **Copy the template** into `modules/your-module/`:
   ```sh
   cp -r docs/module-template modules/my-module
   ```

2. **Edit `module.toml`** — set `name`, `description`, `category`, and `mae_version`.

3. **Edit `autoloads.scm`** — add keybindings, commands, hooks, and/or AI tools.
   End with `(provide-feature "my-module-autoloads")`.

4. **Enable in `init.scm`**:
   ```scheme
   (use-modules! '((my-module)))
   ```
   With flags:
   ```scheme
   (use-modules! '((my-module +extra)))
   ```

5. **Reload** — `:reload-config` or restart MAE.

## Module Structure

```
modules/my-module/
  module.toml       # Manifest (required)
  autoloads.scm     # Entry point (required)
  lib.scm           # Additional Scheme code (optional)
  help/*.org        # Help nodes (optional)
```

## manifest Fields (`module.toml`)

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique identifier (lowercase, hyphens) |
| `version` | Yes | Semver (e.g. `0.1.0`) |
| `description` | Yes | One-line summary |
| `mae_version` | Yes | Minimum MAE version (e.g. `>=0.9.0`) |
| `category` | Yes | `editor`, `lang`, `tools`, `ui`, `completion`, `app`, `checkers`, or `os` |
| `author` | No | Author name |
| `license` | No | SPDX identifier (default: GPL-3.0-or-later) |
| `homepage` | No | URL for docs/repo |
| `depends` | No | List of required module names |

## Stability Markers

Add `@stability` to your `autoloads.scm` header:

- **stable** — API won't change without deprecation period
- **experimental** — API may change between minor versions

## Scheme API Reference

Key functions available in autoloads:

| Function | Purpose |
|----------|---------|
| `(define-key keymap keys command)` | Bind a key sequence |
| `(define-command name desc handler)` | Register an editor command |
| `(add-hook! hook-name fn-name)` | Register a hook callback |
| `(register-ai-tool! ...)` | Expose a tool to the AI agent |
| `(when-flag module flag body...)` | Conditionally run on +flag |
| `(provide-feature name)` | Declare module loaded |
| `(run-command name)` | Dispatch an editor command |
| `(set-option! name value)` | Set an editor option |

See `:help concept:scheme-api` for the full list.

## Testing

1. `:describe-module my-module` — check module loaded correctly
2. `:self-test` — run the self-test suite
3. Check for keybinding conflicts: `:describe-key SPC m x`
