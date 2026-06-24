# Module: debug

Debug panel keybindings and SPC d leader.

## Info

| Field | Value |
|-------|-------|
| Category | tools |
| Version | 0.2.0 |
| Dependencies | None |

## Keybindings

### Debug panel (debug keymap)

| Key | Command | Description |
|-----|---------|-------------|
| `j` | `debug-move-down` | Move selection down |
| `k` | `debug-move-up` | Move selection up |
| `Enter` | `debug-panel-select` | Select item |
| `q` | `close-debug-panel` | Close debug panel |
| `o` | `debug-toggle-output` | Toggle output pane |
| `r` | `dap-refresh` | Refresh DAP state |
| `c` | `debug-continue` | Continue execution |
| `n` | `debug-step-over` | Step over |
| `s` | `debug-step-into` | Step into |
| `S` | `debug-step-out` | Step out |
| `?` | `show-buffer-keys` | Show all keybindings |

### Leader bindings (`SPC d`)

| Key | Command | Description |
|-----|---------|-------------|
| `SPC d d` | `debug-start` | Start debug session |
| `SPC d s` | `debug-self` | Debug the editor itself |
| `SPC d q` | `debug-stop` | Stop debug session |
| `SPC d c` | `debug-continue` | Continue execution |
| `SPC d n` | `debug-step-over` | Step over |
| `SPC d i` | `debug-step-into` | Step into |
| `SPC d o` | `debug-step-out` | Step out |
| `SPC d b` | `debug-toggle-breakpoint` | Toggle breakpoint at line |
| `SPC d v` | `debug-inspect` | Inspect variable at point |
| `SPC d p` | `debug-panel` | Open debug panel |
| `SPC d w` | `debug-add-watch` | Add watch expression |
| `SPC d W` | `debug-remove-watch` | Remove watch expression |
| `SPC d e` | `debug-exceptions` | Configure exception breakpoints |

## Configuration

No module-specific options. DAP adapter paths are configured via environment variables (`MAE_DAP_LLDB`, `MAE_DAP_CODELLDB`, `MAE_DAP_DEBUGPY`).
