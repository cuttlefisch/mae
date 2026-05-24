# Testing Guide

## Running Tests

### Rust (workspace)
```bash
cargo test --workspace          # All Rust tests (3,639+ tests)
cargo test -p mae-core          # Core editor tests only
cargo test -p mae-dap           # DAP client mock tests
cargo test -p mae-mcp           # MCP server tests
cargo test -p mae-sync          # CRDT sync tests
make verify                     # check + test with summary
```

### Scheme (headless editor)
```bash
mae --test tests/editor/                    # Editor E2E tests (~260 steps)
mae --test tests/crdt/                      # CRDT lifecycle tests (~151 steps)
mae --test tests/editor/test_editing.scm    # Single file
make test-scheme-editor                     # Editor tests (builds first)
make test-scheme-crdt                       # CRDT tests
make test-scheme-all                        # All Scheme tests
```

### Integration / E2E
```bash
MAE_TCP_E2E=1 cargo test -p mae --test collab_tcp_e2e -- --ignored --nocapture  # TCP collab
make docker-ci                  # Full CI in container
make docker-collab-test         # Collab E2E (state-server + clients in Docker)
```

### Tiered CI Targets
```bash
make ci              # Fast: fmt + clippy + check + test + Scheme editor tests + check-config + code-map
make ci-extended     # Thorough: ci + CRDT Scheme tests + docker-smoke + docker-new-user
make ci-docker-e2e   # On-demand: Docker collab E2E (when touching collab/sync code)
make ci-complete     # Everything: mirrors GitHub CI
```

| Target | When to Run | Time |
|--------|-------------|------|
| `make ci` | Before every commit | ~3 min |
| `make ci-extended` | Before opening a PR | ~10 min |
| `make ci-docker-e2e` | When touching collab/sync | ~5 min |
| `make ci-complete` | Full validation | ~15 min |

## Test Architecture (3 layers)

### Layer 1: Rust unit tests (`#[test]` / `#[tokio::test]`)
Pure function tests, mock-based protocol tests, data structure tests. Run in-process, no editor startup.

**Key test modules:**
- `crates/core/src/editor/tests/` — 1,000+ tests: editing, navigation, visual mode, operators, text objects, counts, search, commands, shell, mouse, LSP, tables, options, org
- `crates/core/src/window.rs` — 100+ tests: split, focus, balance, maximize, close, resize, scroll, variable-height
- `crates/dap/src/client.rs` — 18 tests: DuplexStream mock adapter, initialize, breakpoints, evaluate, stack trace, scopes, variables, disconnect, timeout
- `crates/mcp/src/` — 65+ tests: handle_request, protocol framing, broadcast, session, client_mgr, TCP
- `crates/core/src/git_status.rs` — 5 tests: section collapse, line kind, toggle
- `crates/core/src/editor/git_ops.rs` — 6 tests: diff hunk parsing, blame parsing
- `crates/mae/src/config.rs` — 23 tests: TOML parsing, option loading, defaults
- `crates/mae/src/bootstrap.rs` — 11 tests: init.scm loading, error isolation
- `crates/kb/` — 135 tests: CRUD, search, FTS5, links, graph

### Layer 2: Scheme E2E tests (`mae --test`)
Boot a real headless editor, exercise the Scheme API. Each `it-test` is one eval-apply cycle with state sync between steps.

**Test files:**
- `tests/editor/test_editing.scm` — Buffer insert, delete, replace
- `tests/editor/test_dispatch_edit.scm` — Edit commands via run-command
- `tests/editor/test_dispatch_nav.scm` — Navigation commands + cursor position
- `tests/editor/test_undo_redo.scm` — Undo/redo sequences
- `tests/editor/test_undo_complex.scm` — Complex undo scenarios
- `tests/editor/test_visual_mode.scm` — Visual selection, region primitives
- `tests/editor/test_search.scm` — Buffer search forward
- `tests/editor/test_modes.scm` — Mode transitions
- `tests/editor/test_options.scm` — Option get/set round-trip
- `tests/editor/test_multi_buffer.scm` — Buffer creation, switching
- `tests/editor/test_keybindings.scm` — define-key, keybinding system
- `tests/editor/test_file_roundtrip.scm` — File write/read
- `tests/editor/test_hooks.scm` — Hook add/remove
- `tests/editor/test_advice.scm` — Advice add/remove
- `tests/editor/test_kb.scm` — KB operations
- `tests/editor/test_test_library.scm` — Self-tests for assertions
- `tests/editor/test_collab_options.scm` — Collab option get/set round-trip
- `tests/editor/test_collab_join_save.scm` — Join-save model: saveas, pathless buffers, collab options
- `tests/editor/test_kb_search.scm` — KB search sort option round-trip
- `tests/crdt/` — 7 files: sync, convergence, concurrent edits, 3-client, undo, state vector, reconcile

### Layer 3: Docker / TCP E2E
Multi-process collab tests with real TCP connections.

## Test Framework

### Assertions (mae-test.scm)
| Assertion | Purpose |
|-----------|---------|
| `(should val)` | Assert truthy |
| `(should-not val)` | Assert falsy |
| `(should-equal a b)` | Assert equal |
| `(should-contain haystack needle)` | Substring check |
| `(should-error thunk)` | Assert error raised |
| `(should-match haystack pattern)` | Alias for should-contain |
| `(should-mode expected)` | Assert editor mode |
| `(should-greater-than a b)` | Assert a > b |
| `(should-less-than a b)` | Assert a < b |
| `(should-buffer-state text row col)` | Combined buffer + cursor check |

### Test Primitives (SharedState-backed)
| Function | Returns |
|----------|---------|
| `(buffer-string)` | Active buffer text |
| `(buffer-text name)` | Named buffer text |
| `(cursor-row)` | Cursor row (0-indexed) |
| `(cursor-col)` | Cursor column (0-indexed) |
| `(current-mode)` | Mode string |
| `(status-message)` | Last status bar message |
| `(get-option name)` | Option value or #f |
| `(region-active?)` | Visual selection active? |
| `(region-beginning)` | Selection start offset |
| `(region-end)` | Selection end offset |
| `(buffer-search-forward pat)` | Char offset or #f |
| `(get-buffer-by-name name)` | Buffer index or #f |

### Writing Scheme Tests
```scheme
(describe-group "Feature name"
  (lambda ()
    (it-test "setup"
      (lambda ()
        (create-buffer "*test*")))
    (it-test "mutate"
      (lambda ()
        (buffer-insert "hello")))
    (it-test "verify"
      (lambda ()
        (should-equal (buffer-string) "hello")))))
```

**Rules:**
- One pending op per test step (buffer-insert + cursor-goto = 2 steps)
- No `(run-tests)` at end — Rust-side iteration handles execution
- Assertions signal errors caught by the runner
- `run-command` and `execute-ex` dispatch editor commands

## Coverage Map

| Area | Rust Tests | Scheme Steps | Notes |
|------|-----------|-------------|-------|
| Buffer editing | 32+ | 50+ | Insert, delete, replace |
| Cursor/navigation | 55+ | 20+ | All movement commands |
| Modal editing (vi) | 80+ | 14+ | Normal, insert, visual, command |
| Text objects | 15+ | — | Word, paragraph, quotes, brackets |
| Operators | 33+ | — | Delete, change, yank |
| Search | 34+ | 10+ | Forward, backward, word-under-cursor |
| Window management | 100+ | — | Split, focus, balance, maximize, resize |
| Undo/redo | 15+ | 30+ | Basic + complex sequences |
| Options | 40+ | 12+ | Registry, get/set, persistence |
| Commands | 75+ | 22+ | Dispatch, edit, nav commands |
| KB | 135+ | 12+ | CRUD, search, FTS5, links |
| LSP | 50+ | — | Mock protocol, completion |
| DAP | 18+ | — | Mock adapter, all request types |
| MCP | 65+ | — | Protocol, framing, handle_request |
| CRDT sync | 36+ | 151+ | Convergence, concurrent, 3-client, encoding edge cases |
| Collab/state server | 26+ | 12+ | Storage, doc store, handler, limits, options |
| Git ops | 11+ | — | Diff parsing, blame, status |
| Config | 23+ | — | TOML parsing, defaults |
| Shell | 40+ | — | PTY, lifecycle, modes |
| Hooks | 2+ | 7+ | Add, remove |
| Advice | — | 6+ | Before/after, remove |
| Mouse | 46+ | — | Click, scroll, focus |
| Org-mode | 28+ | — | Headings, checkboxes, rendering |
| Tables | 13+ | — | Align, insert, delete |
| Performance | 15+ | — | Large file operations |

## What Cannot Be Tested Headless

| Area | Strategy |
|------|----------|
| Real LSP servers | Rust mock tests / MCP manual |
| Real DAP adapters | Rust DuplexStream mocks |
| GUI rendering | Future: Skia snapshot tests |
| AI round-trip | Rust mock HTTP / MCP manual |
| Real git ops | Rust tempdir tests (parse-only tested) |
| Real TCP collab | `MAE_TCP_E2E=1` / Docker |
| Shell interactive I/O | Rust integration tests |

## Adding Tests

**Scheme test?** When testing user-facing workflows that exercise the Scheme API: command dispatch, buffer operations, mode transitions, option round-trips.

**Rust test?** When testing pure functions, protocol parsing, data structures, internal APIs, or anything requiring mocks (DAP, LSP, MCP).
