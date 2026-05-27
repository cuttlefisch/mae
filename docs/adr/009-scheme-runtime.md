# ADR-009: Scheme Runtime — mae-scheme Replaces Steel

**Status**: Accepted
**Date**: 2026-05-26
**KB Source**: `concept:adr-scheme-runtime`

## Context

MAE's extension language was originally Steel, a Scheme-like language embedded
via `steel-core`. Steel had several limitations that became blocking:

1. **Foreign function signature** — `register_fn` required typed parameters
   (e.g., `fn(String, i64) -> bool`), one registration per arity variant.
   MAE needed `fn(&[Value]) -> Result<Value>` for uniform dispatch.

2. **Binding shadowing** — `register_value` couldn't update existing globals.
   Required `set!` workarounds in test runner to refresh editor state variables.

3. **Security advisory** — RUSTSEC-2025-0141 (bincode dependency) with no
   upstream fix path.

4. **Macro limitations** — Steel's macro system didn't implement R7RS
   `syntax-rules` with full ellipsis handling.

5. **No yield/suspend** — Steel had no mechanism for cooperative multitasking.
   `sleep-ms` blocked the entire event loop.

## Decision

Replace Steel with a purpose-built R7RS-small Scheme runtime (`mae-scheme`).

## Architecture

### Core Components (~13,700 LOC Rust)

| Component | LOC | Purpose |
|-----------|-----|---------|
| `compiler.rs` | ~3,020 | S-expression → bytecode (41 special forms) |
| `vm.rs` | ~3,600 | Stack-based bytecode VM (23 opcodes) |
| `stdlib.rs` | ~2,500 | 261 R7RS-small standard library functions |
| `reader.rs` | ~800 | S-expression parser with source locations |
| `value.rs` | ~1,150 | Tagged value type (Rc-based GC) |
| `macros.rs` | ~700 | Hygienic `syntax-rules` with ellipsis |
| `library.rs` | ~500 | R7RS `define-library` / `import` / `export` |
| `lsp.rs` | ~350 | In-process Scheme LSP (Swank-style) |
| `introspect.rs` | ~540 | FunctionDoc registry, apropos, gc-stats |
| `error.rs` | ~300 | Structured errors with source locations |

### Key Design Choices

**ForeignFn signature**: `Fn(&[Value]) -> Result<Value, LispError>` — uniform
dispatch, arity checked by VM before call. Eliminates Steel limitation #1.

**Immutable values**: Strings are `Rc<str>`, pairs are `Rc<(Value, Value)>`.
No `RefCell` on data types. Mutation via `set!` on bindings, not values.

**GC strategy**: Rc (Stage 1). `Trace` trait implemented for future
gc-arena migration. Cyclic structures are rare in Scheme (no mutable pairs).

**Yield/suspend**: `Op::Yield` + `EvalResult::Yield(request, vm_state)`.
Foreign functions yield via `LispError::Yield(reason)`. The event loop
resumes with `Vm::resume(state, value)`. Same mechanism for `sleep-ms`,
breakpoints, and `wait-for-file`.

**Source maps**: Compiler tracks `current_loc: Option<SourceLocation>`.
Every `emit()` call passes the location. No source-location-on-values
(unlike Chez annotations or Racket syntax objects).

**In-process LSP**: Queries live VM globals, code pool, and library registry
directly. No JSON-RPC subprocess. Microsecond completion, not millisecond.

**Yield-based DAP**: Breakpoints are `YieldRequest::Breakpoint(info)`.
Step modes (StepIn, StepOver, StepOut) are ephemeral — auto-reset to Run
after triggering (Guile trap model). No separate debugger process.

### R7RS Compliance

1,732 tests: 1,115 R7RS compliance, 310 unit, 117 torture, 25 benchmarks,
110 IO, 55 misc. Spec stances documented in `crates/scheme/SPEC_STANCES.md`.

Key stances: immutable strings (SRFI-140), immutable pairs, i64+f64 numeric
tower, multiple values as lists, eval as compiler special form.

### Integration Points

- `SchemeRuntime` wraps `Vm` with `SharedState` for editor↔scheme data flow
- `inject_editor_state()` updates both VM globals and SharedState
- `apply_to_editor()` processes pending mutations (buffer edits, commands, etc.)
- 177 editor functions registered as `ForeignFn` values
- `(mae async)` library for yield-based async primitives
- `scheme_lsp_bridge.rs` intercepts LSP intents for `.scm` files
- `scheme_dap_bridge.rs` intercepts DAP intents for `mae-scheme` adapter

## Consequences

### Positive

- **Zero external Scheme dependencies** — no `steel-core`, no bincode advisory
- **R7RS-small compliance** — portable Scheme code, not a Steel dialect
- **Yield infrastructure** — `sleep-ms` no longer blocks the event loop
- **Hygienic macros** — full `syntax-rules` with ellipsis patterns
- **Source-level debugging** — breakpoints, stepping, frame inspection for Scheme code
- **IDE support** — completion, hover, diagnostics, go-to-definition for `.scm` files
- **Introspection** — `procedure-arity`, `procedure-documentation`, `gc-stats`
- **5,470 total workspace tests** — comprehensive regression coverage

### Negative

- **No Steel compatibility** — existing Steel extensions must be rewritten
  (mitigated: the R7RS API is close enough that most ports are mechanical)
- **Rc GC** — no cyclic structure collection until gc-arena migration (Stage 2)
- **Single-threaded VM** — no parallel Scheme evaluation (matches Emacs model)

### Neutral

- **Same `SchemeRuntime` public API** — all callers (bootstrap, test_runner,
  event loops) unchanged
- **Same `SharedState` pattern** — battle-tested during Steel era

## Prior Art

Surveyed 8 Scheme implementations before design:

| Implementation | Key Lesson |
|----------------|-----------|
| Chibi-Scheme | R7RS reference, test suite baseline |
| Guile | In-process model (Swank), GC stats alist, trap-based debugging |
| Chez Scheme | Richest GC API, annotation objects for source tracking |
| Racket | check-syntax for diagnostics, syntax objects |
| Chicken | Compilation strategy (not applicable to embedded use) |
| Gambit | Benchmark suite |
| S7 | Minimal embedding (no macros — too limited) |
| Steel | What not to do (typed FFI, no yield, binding shadowing) |

Full research in RoamNotes: `mae_scheme_prior_art.org`, `mae_scheme_gc_strategy.org`,
`mae_scheme_async_yield.org`, `scheme_introspection_prior_art.org`.

## References

- R7RS-small: https://small.r7rs.org/
- `crates/scheme/SPEC_STANCES.md` — 12 explicit specification stances
- `crates/scheme/src/` — implementation source
