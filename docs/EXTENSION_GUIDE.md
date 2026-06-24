# MAE Extension Authoring Guide

> This guide covers creating, testing, and publishing MAE modules. It is the
> human-readable counterpart to the `guide:extension-authoring` KB node.

## Quick Start

```bash
mae pkg create my-module
cd modules/my-module
# edit module.toml, autoloads.scm, init.scm
mae pkg doctor my-module
```

## Module Structure

Every module has two required files (`module.toml` + `autoloads.scm`), with an
optional `init.scm` for lazy initialization — many modules ship without it. Core or
auto-enabled modules set `required = true` in the manifest's `[module]` table.

### `module.toml` — Identity

TOML manifest parsed *before* the Scheme engine initializes (enables `mae pkg list`
without starting the editor).

```toml
[module]
name = "my-module"
version = "0.1.0"
description = "What the module does"
category = "editor"          # editor | lang | tools | ui | ai | completion
mae_version = ">=0.9.0"

[flags]
extra = { doc = "Enable extra feature" }

[dependencies]
# other-module = ">=0.1.0"

[entry]
init = "init.scm"
autoloads = "autoloads.scm"
```

**Critical design decision:** The manifest declares *identity* only — name, version,
deps, flags, entry points. It does NOT declare what the module provides (commands,
options, keybindings). Those are registered exclusively in Scheme code
(`autoloads.scm`), which is the single source of truth.

### `autoloads.scm` — Eager Registration

Runs at startup, before user `config.scm`. Register commands, keybindings, options:

```scheme
;; Commands
(define-command "my-greet" (lambda () (buffer-insert "Hello!")) "Greet the user")

;; Keybindings — SPC leader bindings go in the shared `leader` keymap WITHOUT
;; the SPC prefix; this appears as `SPC x g`.
(define-key "leader" "x g" "my-greet")
(set-group-name "leader" "x" "+mystuff")  ;; which-key group label

;; Options
(define-option! "my_greeting" "string" "Hello!" "The greeting text")

;; Hooks
(add-hook! "after-save" "my-after-save-fn")

;; Conditional on flags
(when-flag "+extra"
  (define-key "leader" "x e" "my-extra-cmd"))
```

**Leader vs. mode keymaps:** The shared `leader` keymap is the single source of
truth for the which-key tree and is the *only* place SPC leader bindings belong —
flavor modules (`keymap-doom`, `keymap-nonmodal`) wire the keypad entry that
dispatches into it. Non-leader, buffer- or mode-local bindings can still target a
mode keymap directly (e.g. `(define-key "normal" "g d" "lsp-goto-definition")`).

### `init.scm` — Lazy Initialization

Loaded on first command use via the autoload mechanism:

```scheme
;; Full feature logic here
(provide-feature "my-module")
```

## Loading Order

```
config.toml → SchemeRuntime::new()
  → init.scm (module declarations)
  → discover_modules() → resolve_deps() → load_module_autoloads()
  → config.scm (user customization — runs AFTER module autoloads)
```

**Key invariant:** Module autoloads run BEFORE `config.scm`. Users can override
any module keybinding or option in their config.

## Scheme API Reference

### Buffer Operations

| Function | Description |
|----------|-------------|
| `(buffer-insert text)` | Insert text at cursor |
| `(buffer-delete-range start end)` | Delete character range |
| `(buffer-replace-range start end text)` | Replace range |
| `(buffer-text-range start end)` | Read range |
| `(create-buffer name)` | Create a new empty buffer |
| `(kill-buffer-by-name name)` | Close a buffer by name |
| `*buffer-name*` | Current buffer name |
| `*buffer-text*` | Full buffer contents |
| `*buffer-char-count*` | Character count |
| `*buffer-list*` | All buffer names |

### Buffer Introspection (callable forms)

| Function | Returns | Description |
|----------|---------|-------------|
| `(current-buffer-name)` | string | Current buffer name |
| `(current-buffer-file)` | string or `#f` | File path or false if unsaved |
| `(current-line-number)` | int | 1-indexed line number |
| `(current-column)` | int | Cursor column |
| `(point)` | int | Char offset of cursor |
| `(point-min)` | int | Always 0 |
| `(point-max)` | int | Total chars in buffer |
| `(line-beginning-position)` | int | Start of current line |
| `(line-end-position)` | int | End of current line |

### Selection / Region

| Function | Returns | Description |
|----------|---------|-------------|
| `(region-active?)` | bool | True in visual mode |
| `(region-beginning)` | int | Start of selection |
| `(region-end)` | int | End of selection |
| `(get-selection)` | string | Selected text |

### Cursor & Navigation

| Function | Description |
|----------|-------------|
| `(cursor-goto row col)` | Move cursor to position |
| `*cursor-row*`, `*cursor-col*` | Current cursor position |
| `(open-file path)` | Open a file |
| `(switch-to-buffer name)` | Switch to buffer |

### Commands

| Function | Description |
|----------|-------------|
| `(define-command name fn doc)` | Register a command |
| `(run-command name)` | Execute a command |
| `(command-exists? name)` | Check if registered |
| `(undefine-command! name)` | Remove (for unload) |
| `*command-list*` | All registered commands |

### Keymaps

| Function | Description |
|----------|-------------|
| `(define-key keymap key command)` | Bind key to command |
| `(set-group-name map prefix label)` | Set which-key group label (e.g. `(set-group-name "leader" "x" "+mystuff")`) |
| `(define-keymap name parent)` | Create new keymap with parent |
| `(undefine-key! keymap key)` | Remove binding |
| `*keymap-list*` | All keymap names |
| `(keymap-bindings name)` | All bindings in keymap |

### Options

| Function | Description |
|----------|-------------|
| `(define-option! name type default doc)` | Register option |
| `(set-option! name value)` | Set value |
| `(get-option name)` | Read current value |
| `(undefine-option! name)` | Remove (for unload) |
| `*option-list*` | All options as tuples |

### Hooks & Advice

| Function | Description |
|----------|-------------|
| `(add-hook! hook fn)` | Subscribe to event |
| `(remove-hook! hook fn)` | Unsubscribe |
| `(advice-add! command kind fn)` | Add before/after advice to a command |
| `(advice-remove! command fn)` | Remove advice from a command |
| `*current-command*` | Name of command being dispatched |

Hooks `command-pre` and `command-post` fire for every command dispatch.
Per-command advice wraps specific commands:

```scheme
(define (my-before-save)
  (message (string-append "Saving " (current-buffer-name) "...")))

(advice-add! "save" ":before" "my-before-save")
```

### String Utilities

| Function | Description |
|----------|-------------|
| `(string-split str sep)` | Split string into list |
| `(string-join lst sep)` | Join list with separator |
| `(string-trim str)` | Trim whitespace |
| `(string-contains? str sub)` | Substring check |
| `(string-replace str from to)` | Replace all occurrences |
| `(string-upcase str)` | Uppercase |
| `(string-downcase str)` | Lowercase |

### Process Execution

| Function | Description |
|----------|-------------|
| `(shell-command cmd)` | Run shell command, return stdout (1MB cap) |

### Module Queries

| Function | Description |
|----------|-------------|
| `(module-loaded? name)` | Is module active? |
| `(module-version name)` | Version string or `#f` |
| `(module-list)` | All active module names |
| `(module-flags name)` | Enabled flags for module |
| `(when-module name fn)` | Conditional on module presence |
| `(when-flag flag fn)` | Conditional on flag |

### Deprecation

| Function | Description |
|----------|-------------|
| `(deprecate-function! old new since)` | Register deprecation |
| `(check-deprecated name)` | Check and warn once |

## Flags

Flags are optional sub-features that modules declare:

```toml
[flags]
agenda = { doc = "Task/schedule agenda view" }
babel = { doc = "Code block execution" }
```

Use `when-flag` in autoloads to conditionally load:

```scheme
(when-flag "+agenda"
  (autoload "open-agenda" "org-agenda" "Open agenda view")
  (define-key "leader" "o a" "open-agenda"))
```

## Testing Your Module

1. **Manifest validation:** `mae pkg doctor my-module`
2. **Load validation:** `mae --check-config`
3. **Live development:** `:module-reload my-module` after editing
4. **Disable test:** Remove module from init.scm, verify editor starts
5. **Override test:** Add overrides in config.scm, verify they take effect

## Naming Conventions

Module authors MUST prefix their definitions with the module name:

```scheme
;; Good: namespaced
(define-command "my-module-greet" ...)
(define-option! "my_module_greeting" ...)

;; Bad: global pollution
(define-command "greet" ...)
```

`mae pkg doctor` warns about unprefixed definitions.

## R7RS Libraries

mae-scheme supports R7RS `define-library` for structured code organization.
All editor primitives are available as globals (no import needed), but libraries
provide encapsulation for reusable code:

```scheme
(define-library (my-utils)
  (import (scheme base))
  (export count-words format-size)
  (begin
    (define (count-words str)
      (length (string-split str " ")))
    (define (format-size bytes)
      (cond
        ((> bytes 1048576) (string-append (number->string (/ bytes 1048576)) "MB"))
        ((> bytes 1024) (string-append (number->string (/ bytes 1024)) "KB"))
        (else (string-append (number->string bytes) "B"))))))
```

Use from another file:

```scheme
(import (my-utils))
(message (string-append "Words: " (number->string (count-words (buffer-string)))))
```

### Built-in Libraries

| Library | Purpose |
|---------|---------|
| `(scheme base)` | R7RS base language |
| `(scheme write)` | `display`, `write`, `newline` |
| `(scheme char)` | Character predicates and case conversion |
| `(scheme cxr)` | `caaar` through `cddddr` |
| `(mae async)` | Yield-based async: `sleep-ms`, `wait-for-file`, `wait-until` |

## Async / Yield

mae-scheme uses cooperative yielding for blocking operations. When a function
yields, control returns to the editor event loop (UI stays responsive), and
execution resumes when the condition is met:

```scheme
;; Sleep without blocking the event loop
(sleep-ms 1000)

;; Wait for a file to appear (with timeout)
(wait-for-file "/tmp/output.json" 5000)

;; Wait until a condition is true
(wait-until (lambda () (file-exists? "/tmp/ready")) 3000)
```

These functions are available as globals. For explicit library import:
`(import (mae async))`.

## Introspection

Inspect the runtime from Scheme:

```scheme
;; Procedure metadata
(procedure-arity car)           ; => "1"
(procedure-documentation car)   ; => "Return the first element of a pair"
(procedure-name car)            ; => "car"

;; GC / runtime stats (alist)
(gc-stats)      ; => ((eval-count . 42) (collections . 3) ...)
(gc-collect!)   ; Force a GC cycle

;; Docstrings on user functions (first string in body)
(define (greet name)
  "Greet a user by name."
  (string-append "Hello, " name "!"))

(procedure-documentation greet)  ; => "Greet a user by name."
```

## Debugging Scheme Code

MAE includes a built-in DAP adapter for Scheme. Set breakpoints and step
through `.scm` files:

```
:debug-start scheme path/to/file.scm
```

Features:
- **Breakpoints** at source lines (set via debug panel or `:debug-toggle-breakpoint`)
- **Step modes**: step-in, step-over, step-out
- **Frame inspection**: locals, upvalues, call stack
- **Eval in context**: evaluate expressions at a breakpoint

The Scheme LSP provides IDE support for `.scm` files:
- **Completion**: R7RS keywords + all registered functions + user globals
- **Hover**: docstrings with arity display
- **Diagnostics**: syntax and compilation errors with source locations
- **Go-to-definition**: jump to user-defined function source
- **Signature help**: parameter names and arity

## Design Philosophy

1. **Composition over inheritance** — register commands, not subclasses
2. **Single source of truth** — Scheme code is both declaration and implementation
3. **Stable API contract** — semver, deprecation cycles, `mae_version` constraints
4. **No framework** — modules call registered Scheme functions, no imports needed

## Kernel Boundary

The line: **if it needs `tokio`, PTY, or FFI, it's kernel. If it's commands +
keybindings + hooks + options, it's a module.**

Modules are Scheme-only packages. They call Rust functions already in the kernel
(exposed via `register_fn`). This is exactly how Emacs works — `org-mode.el` calls
C functions for buffer operations.

## Reference Implementations

- **Simplest:** `modules/dashboard/` — 1 command, splash screen rendering
- **Typical:** `modules/search/` — keybindings across normal/visual modes
- **Complex:** `modules/file-tree/` — module-owned keymap via `define-keymap`

## CLI Reference

| Command | Description |
|---------|-------------|
| `mae pkg list` | Show all discovered modules |
| `mae pkg doctor [NAME]` | Validate manifests and entry points |
| `mae pkg info <NAME>` | Detailed module information |
| `mae pkg create <NAME>` | Scaffold a new module |
| `mae pkg help` | Usage information |

## Introspection Commands

| Command | Binding | Description |
|---------|---------|-------------|
| `:describe-module <name>` | — | Manifest, commands, options, status |
| `:describe-bindings` | `SPC h B` | Full keymap table for current mode |
| `:describe-mode` | `SPC h m` | Current buffer's mode and keymap |
| `:describe-key` | `SPC h k` | What command a key is bound to |
| `:describe-command` | `SPC h c` | Command documentation |
| `:describe-option` | `SPC h o` | All option values |
| `:describe-configuration` | — | Health report: validates init.scm (primary config) plus the legacy config.toml bootstrap |

## Related KB Nodes

- `concept:modules` — module system architecture
- `concept:flags` — flag syntax and validation
- `concept:design-philosophy` — principles behind the system
- `concept:package-system` — underlying require/provide primitives
- `concept:scheme-api` — full Scheme function reference
- `concept:hooks` — hook points for extension
- `concept:options` — option registry and types
