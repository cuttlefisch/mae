# mae-scheme: R7RS Specification Stances

Where the R7RS-small standard leaves behavior "implementation-defined" or
permits design choices, mae-scheme documents its decisions here. Each stance
includes the R7RS section, the choice made, the rationale, and the effect on
extension authors.

This document is the authoritative reference for mae-scheme's dialect
decisions. It will be incorporated into the mae-scheme manual (KB-generated
from code comments, roadmap item).

---

## 1. Immutable Strings (§6.7)

**R7RS says**: "It is an error to use string-set! on literal strings or on
strings returned by symbol->string." Implementations may extend immutability
to all strings.

**mae-scheme stance**: All strings are immutable. `string-set!`, `string-copy!`,
and `string-fill!` signal errors with helpful alternative suggestions.

**Rationale**:
- Strings are `Rc<str>` — zero-cost sharing, no RefCell overhead
- Buffer mutation in MAE happens at the rope level (`buffer-insert`), not
  via string-level mutation
- Racket, Gauche, Guile, Kawa, and Lua (Neovim) all use immutable strings
- SRFI-140 standardizes this approach
- Emacs's own docs note "very little code would break" with immutable strings

**Effect on extension authors**: Use `string-append`, `substring`,
`list->string`, and `string-copy` (returns new string) for string construction.
For heavy text manipulation, use buffer operations.

**Future**: `(scheme mutable-strings)` library may be added using
copy-on-write semantics if demanded.

---

## 2. Immutable Pairs (§6.4)

**R7RS says**: `set-car!` and `set-cdr!` are part of `(scheme base)` and
modify pairs in place.

**mae-scheme stance**: Pairs are `Rc<(Value, Value)>` — structurally
immutable. `set-car!` and `set-cdr!` are provided but create new cons cells
(not true mutation). `list-set!` signals an error.

**Rationale**:
- `Rc`-based pairs enable safe sharing across closures and continuations
- True mutation would require `Rc<RefCell<(Value, Value)>>` — 8 extra bytes
  per cons cell + runtime borrow checks
- Functional list operations (`cons`, `append`, `map`, `filter`) are the
  norm in Scheme code; destructive update is rare

**Effect on extension authors**: Prefer functional style. Use `cons`, `append`,
and `map` to build new lists. The performance difference is negligible for
editor extension workloads.

---

## 3. Numeric Tower (§6.2)

**R7RS says**: Implementations must support exact integers and inexact reals.
Rationals, bignums, and complex numbers are optional.

**mae-scheme stance**:
- **Exact integers**: `i64` fixnums (range: -2^63 to 2^63-1)
- **Inexact reals**: `f64` IEEE 754 double precision
- **Bignums**: Not yet supported. Overflow wraps (planned: `num-bigint`)
- **Rationals**: Not supported. `(/ 1 3)` returns inexact `0.333...`
- **Complex numbers**: Not supported. `(scheme complex)` library is absent.

**Rationale**:
- `i64` covers all practical editor use cases (line numbers, byte offsets,
  Unicode codepoints, timestamps)
- `f64` provides sufficient precision for floating-point math
- Complex numbers have no editor use case
- Bignums will be added when needed (num-bigint crate)

**Effect on extension authors**: `exact->inexact` and `inexact->exact` work.
Integer division `(/ 6 3)` returns exact `2`. Non-divisible `(/ 1 3)` returns
inexact. `complex?` returns `#t` for all numbers (R7RS §6.2.1 permits this
when there is no separate complex type).

---

## 4. Multiple Values (§6.10)

**R7RS says**: `(values obj ...)` delivers multiple values to its continuation.
`call-with-values` receives them.

**mae-scheme stance**: `(values x)` returns `x` directly. `(values x y z)`
returns the list `(x y z)`. `call-with-values` is a compiler special form
that calls the producer, then applies the consumer to the result (using
`apply` for list results, direct call for single values).

**Rationale**:
- True multi-value return requires a separate values type in the VM, which
  adds complexity for a rarely-used feature
- The list representation works correctly with all R7RS patterns:
  `receive`, `let-values`, `let*-values`, `define-values`
- This is the same approach used by several minimal Scheme implementations

**Effect on extension authors**: `(call-with-values producer consumer)`,
`(receive formals expr body)`, and `(let-values ...)` all work as expected.
The only case that differs from spec is `(values)` (zero values), which
returns `()` (empty list) rather than "zero values".

---

## 5. Eval (§6.12)

**R7RS says**: `(eval expr environment-specifier)` evaluates `expr` in the
specified environment.

**mae-scheme stance**: `eval` is a compiler special form (not a library
function). It accepts 1 or 2 arguments. The environment argument is accepted
but ignored — all evaluation happens in the interaction environment.

**Rationale**:
- `eval` requires VM access, which foreign functions don't have
- Separate environments (immutable R7RS base, null-environment) would
  require environment objects — significant complexity for rare use
- The interaction environment is what users expect 99% of the time

**Effect on extension authors**: `(eval '(+ 1 2))` works. The environment
argument is a no-op: `(eval '(+ 1 2) (scheme-report-environment 7))` also
works but uses the interaction environment.

---

## 6. Tail Call Optimization (§3.5)

**R7RS says**: "Implementations of Scheme are required to be properly
tail-recursive."

**mae-scheme stance**: Full proper tail calls via `TAIL_CALL` bytecode opcode.
Tail position is recognized in: `if`, `cond`, `case`, `and`, `or`, `when`,
`unless`, `let`, `let*`, `letrec`, `letrec*`, `begin`, `do`, `guard`,
named `let`, `parameterize`, and lambda body.

**Rationale**: This is a hard requirement, not optional. The compiler
identifies tail position and emits `TAIL_CALL` instead of `CALL` + `RETURN`.

**Effect on extension authors**: Recursive algorithms can use unbounded
recursion in tail position. `(letrec ((loop (lambda (n) (if (= n 0) 'done
(loop (- n 1)))))) (loop 1000000))` completes without stack overflow.

---

## 7. Continuations (§6.10)

**R7RS says**: `call-with-current-continuation` captures the current
continuation.

**mae-scheme stance**: Full `call/cc` with heap-allocated frames. Continuations
capture the entire VM state (stack + frames). Both one-shot and multi-shot
invocation are supported.

**Rationale**: Heap-allocated frames (required for proper tail calls anyway)
make continuation capture straightforward — just clone the state.

**Effect on extension authors**: `call/cc`, `dynamic-wind`, and exception
handling all work. Continuations are first-class values that can be stored,
passed, and invoked multiple times.

---

## 8. with-input-from-file / with-output-to-file (§6.13.1)

**R7RS says**: These should make the opened port the "default port" for the
dynamic extent of the thunk.

**mae-scheme stance**: Currently simplified — these open the file and pass the
port to the thunk. They do NOT redirect `current-input-port`/`current-output-port`
because our port parameters are not yet dynamically parameterizable.

**Rationale**: The common usage pattern (opening a file and operating on it)
works. Full port redirection requires `make-parameter`/`parameterize`
integration with the port system (planned).

**Effect on extension authors**: Use `call-with-input-file`/`call-with-output-file`
which explicitly pass the port. These work correctly.

---

## 9. Hygienic Macros (§4.3)

**R7RS says**: `syntax-rules` provides hygienic macro expansion.

**mae-scheme stance**: Full `syntax-rules` with pattern matching, ellipsis
(`...`), literal identifiers, and hygiene via gensym renaming. `let-syntax`
and `letrec-syntax` are supported. `syntax-case` is not provided.

**Effect on extension authors**: Standard `define-syntax` / `syntax-rules`
patterns work. For non-hygienic macros, `define-macro` is available as an
extension (planned).

---

## 10. Error Objects (§6.11)

**R7RS says**: Errors raised by `error` procedure create error objects
inspectable via `error-object-message`, `error-object-irritants`, and
`error-object-type`.

**mae-scheme stance**: Error objects are structured values (tagged vectors)
with message, irritants, and type fields. `guard` and `with-exception-handler`
work with these. `file-error?` and `read-error?` check the error type field.

---

## 11. Library System (§5.6)

**R7RS says**: `define-library` / `import` / `export` provide a module system.

**mae-scheme stance**: Full R7RS library system with `define-library`, `import`
(with `only`, `except`, `prefix`, `rename` modifiers), and `export`. Libraries
use `.sld` extension. The 13 R7RS standard libraries are recognized by
`cond-expand` `(library ...)` but their functions are globally available
(not isolated to library scopes) — standard library import is a no-op since
all stdlib functions are pre-registered.

**Effect on extension authors**: User-defined libraries work with full
isolation. `(import (scheme base))` is accepted but does nothing (functions
already available). This matches how most R7RS implementations handle the
built-in libraries.

---

## Roadmap: mae-scheme Manual

The code comments in `stdlib/*.rs`, `compiler.rs`, `vm.rs`, and `reader.rs`
are structured to be extractable into a reference manual, inspired by the
GNU Emacs Lisp Reference Manual. This is a roadmap item for Phase 13g
(Introspection + Observability):

- **Source**: Module-level `//!` doc comments define spec stances and design
  rationale. Function-level docstrings (the `doc` parameter to `register_fn`)
  describe individual functions.
- **Format**: KB nodes generated from the function registry at startup.
  `scheme:*` namespace for all R7RS functions, `mae:*` for extensions.
- **Navigation**: `:describe-function`, `:apropos`, and `:help scheme:*`
  commands provide runtime access.
