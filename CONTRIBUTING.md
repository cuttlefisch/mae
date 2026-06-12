# Contributing to MAE

MAE (Modern AI Editor) is a Rust + Scheme editor built as a successor to GNU Emacs, where the human user and an AI agent are peer actors calling the same Lisp primitives. If you want to help build that, welcome.

The full architecture spec lives in [README.md](README.md). The non-negotiable design constraints are documented in [CLAUDE.md](CLAUDE.md). Read both before starting any substantive work.

---

## Table of Contents

- [Getting Started](#getting-started)
- [Architecture Overview](#architecture-overview)
- [Development Workflow](#development-workflow)
- [Code Standards](#code-standards)
- [Testing Philosophy](#testing-philosophy)
- [Where to Start](#where-to-start)
- [Submitting Changes](#submitting-changes)
- [Communication](#communication)
- [License](#license)

---

## Getting Started

### Prerequisites

- **Rust 1.95+** via [rustup](https://rustup.rs) (MSRV is enforced in CI)
- **make** and **git**
- **GUI deps** (only needed for `make build` — skip for TUI-only):
  - Fedora: `clang fontconfig-devel freetype-devel`
  - Debian/Ubuntu: `clang libfontconfig1-dev libfreetype6-dev`
  - macOS: Xcode Command Line Tools

Run `make doctor` first. It checks all prerequisites and prints install commands for anything missing.

### Clone and Build

```sh
git clone git@github.com:cuttlefisch/mae.git
cd mae

make doctor          # verify prerequisites
make build           # GUI build (default)
make build-tui       # TUI-only build (no Skia deps)
```

### Verify Your Setup

```sh
make ci              # fmt check + clippy + cargo check + tests (excludes GUI)
make build && mae --self-test    # AI-driven E2E self-test (requires API key)
```

`make ci` is the gate for every PR. If it passes locally, it will pass in GitHub CI.

### Development Dependencies (Optional)

Full self-test coverage for LSP and DAP categories requires additional tools:

```sh
make setup-dev       # auto-detects package manager, installs lldb, rust-analyzer, debugpy
```

You can also set paths via environment variables: `MAE_DAP_LLDB`, `MAE_DAP_DEBUGPY`, `MAE_LSP_RUST`, etc.

### Container Workflow (No Local Toolchain Required)

If you prefer not to install Rust and system dependencies locally:

```sh
make docker-ci          # equivalent to `make ci` in a clean environment
make docker-dev         # interactive shell — `make ci` works inside
make docker-new-user    # validate first-run flow in pristine container
make docker-smoke       # quick binary smoke test
```

The Dockerfile mirrors GitHub CI exactly. If `make docker-ci` passes, the PR will pass CI.

**When to use which:**
- **Native:** faster iteration, GUI builds, `make self-test`
- **Container:** zero setup, first contribution, pre-PR validation, CI debugging

---

## Architecture Overview

MAE is organized as a Rust workspace with 20 crates and 19 Scheme modules.

### Crate Map

| Crate | Role |
|-------|------|
| `mae-core` | Buffer (rope), editor state, commands, keymap, syntax, babel, export |
| `mae-renderer` | Terminal rendering (ratatui), status bar, popups |
| `mae-gui` | GUI rendering (winit + Skia 2D), mouse, fonts, inline images |
| `mae-scheme` | R7RS-small Scheme runtime, init.scm loading, hook dispatch |
| `mae-ai` | AI providers (Claude/OpenAI/Gemini/DeepSeek), tool execution |
| `mae-lsp` | LSP client — connection, navigation, diagnostics, completion |
| `mae-dap` | DAP client — breakpoints, stepping, watches |
| `mae-shell` | Terminal emulator (alacritty_terminal), PTY management |
| `mae-kb` | Knowledge base — graph store, org parser, FTS5 |
| `mae-mcp` | MCP bridge — Unix socket, JSON-RPC, stdio shim |
| `mae-babel` | Literate programming — code block execution, tangling |
| `mae-export` | Org export (HTML, Markdown) |
| `mae-snippets` | Snippet expansion |
| `mae-format` | Auto-formatting integration |
| `mae-make` | Build system integration |
| `mae-lookup` | Documentation lookup |
| `mae-spell` | Spell checking |
| `mae-sync` | CRDT sync (yrs/YATA), ropey bridge, collaborative state |
| `mae-daemon` | Background daemon — KB persistence, collab sync, WAL persistence |
| `mae` | Binary crate — CLI entry point, config loading, event loops |

The 19 Scheme modules in `modules/` provide keybinding overlays and optional features (agenda, dashboard, git-status, org, tables, surround, multicursor, etc.). See [docs/CODE_MAP.md](docs/CODE_MAP.md) for the dependency graph and module sizes.

Key files to know:
- `crates/core/src/editor/mod.rs` — editor state struct
- `crates/core/src/editor/dispatch/` — command dispatch (10 submodules)
- `crates/mae/src/main.rs` — CLI entry point and event loop

### Key Principles

These are non-negotiable. Full rationale is in [CLAUDE.md](CLAUDE.md).

1. **Concurrency from day one.** No Global Interpreter Lock, ever. Rust ownership for the core; a concurrent-GC-friendly design for the Scheme runtime.
2. **Modular display layer.** The renderer is a separate crate with a clean trait-based HAL. Platform code lives in backend libraries, not in ours.
3. **The AI is a peer, not a plugin.** The AI agent calls the same Scheme functions as user keybindings. No separate "AI mode," no simulated keystrokes.
4. **Scheme-first configurability.** Every user-visible behavior that could differ between users goes through `OptionRegistry`. No config.toml-only settings, no magic constants in rendering code.
5. **No ad-hoc solutions.** When you find yourself duplicating logic between TUI and GUI, extract it. When you find a magic number, make it an option. Fix things properly for both backends.
6. **No file over 3,000 lines.** Split into module directories when approaching the limit.

### Architecture Decision Records

ADRs live in [docs/adr/](docs/adr/). Read them before proposing changes to sync, KB storage, file safety, or the CRDT model. They explain why the current choices were made and what alternatives were evaluated.

---

## Development Workflow

### Branch Model

- `main` is always the last released version. Never push to it directly.
- All changes go through a feature branch + PR + CI.

Branch naming:
- `feature/description` — new capabilities
- `fix/description` — bug fixes
- `docs/description` — documentation-only changes
- `refactor/description` — restructuring without behavior change

### Day-to-Day Loop

```sh
git checkout -b feature/my-thing main
# ... make changes ...
make ci                   # must pass before opening a PR
cargo test --workspace    # full coverage including GUI tests
git push origin feature/my-thing
# open PR on GitHub
```

`make ci` runs: `cargo fmt --check` + `cargo clippy -D warnings` + `cargo check` + `cargo test` (excludes `mae-gui` due to Skia system deps).

`cargo test --workspace` includes GUI tests. Run this locally before marking a PR ready if your change touches rendering.

### Pre-Commit Hook

A pre-commit hook enforces `cargo clippy -D warnings`. Zero warnings is the bar. If clippy produces a warning, fix it — do not add `#[allow(...)]` without a comment explaining why the suppression is justified.

### Commit Messages

We use [Conventional Commits](https://www.conventionalcommits.org/) for changelog generation via git-cliff:

```
feat(core): add structural selection via tree-sitter
fix(gui): cursor not visible after AI response
refactor(ai): split executor into submodules
docs: update CONTRIBUTING.md with testing section
ci: add cargo-deny to CI pipeline
test(lsp): add rename integration tests
```

Scope = crate name without `mae-` prefix: `core`, `gui`, `ai`, `lsp`, `dap`, `scheme`, `shell`, `kb`, `mcp`, `renderer`, `sync`, `daemon`, `babel`.

### Versioning

**Do not manually edit `VERSION` or the version field in `Cargo.toml`.** The `version-bump.yml` workflow handles versioning on merge:

| PR Label | Commit Prefix | Bump |
|----------|---------------|------|
| `release:patch` | `fix(...):` | patch (0.11.0 -> 0.11.1) |
| `release:minor` | `feat(...):` | minor (0.11.0 -> 0.12.0) |
| `release:major` | `feat(...)!:` | major (0.11.0 -> 1.0.0) |

Labels take precedence over commit scanning. If your PR has `feat` commits but should only bump patch (e.g., a docs change), add the `release:patch` label explicitly.

---

## Code Standards

### Error Handling

- No `unwrap()` on external input — user input, file I/O, network, or anything that can fail in production.
- Use `?` propagation and typed errors. `anyhow` is acceptable at binary boundaries; typed errors (`thiserror`) are preferred inside library crates.
- Panics must be reserved for invariant violations that indicate a programming error, not runtime conditions.

### Scheme-First Configurability

If a behavior is user-visible and could reasonably differ between users, it must go through `OptionRegistry`:

1. Register in the relevant `options.rs` with a `config_key` (enables `:set-save` persistence).
2. It becomes automatically accessible via `(set-option!)` / `(get-option)` in Scheme.
3. It becomes automatically accessible via `:set` at runtime in the editor.
4. Default values live in the option definition, never as constants in rendering code.

Constants that are truly fixed (buffer sizes, protocol limits) belong in the relevant module with a comment explaining why they are fixed.

### No Magic Constants

Named constants over bare literals. If the same value appears in two places, it belongs in one definition. If a value controls behavior the user might want to change, it belongs in `OptionRegistry`.

### Shared Computation

All layout math, content formatting, span computation, and data preparation lives in `mae-core` (specifically `render_common/` and `text_utils`). Backend crates (`mae-renderer`, `mae-gui`) contain only the code that touches platform APIs — ratatui widgets or Skia paint calls. If two renderers compute the same thing, extract it.

### Structured Logging

Use the `tracing` crate with structured fields. Prefer:

```rust
tracing::debug!(buffer_id = %id, line_count = lines, "rendering buffer");
```

over format strings that bury structured data in a string. Per-module filters (`RUST_LOG=mae_lsp=debug`) only work when fields are properly structured.

### Naming Conventions

| Context | Name | Example |
|---------|------|---------|
| Editor instance | `editor` | `let mut editor = Editor::new();` |
| Buffer reference | `buf` | `let buf = &editor.buffers[idx];` |
| Window reference | `win` | `let win = editor.window_mgr.focused_window_mut();` |
| Buffer index | `idx` | `let idx = editor.active_buffer_idx();` |
| Second editor | `editor2` | `let mut editor2 = Editor::new();` |

Never abbreviate `editor` to `ed`, `e`, or single letters.

Function naming patterns:
- Command dispatch: `dispatch_<category>` (e.g., `dispatch_nav`, `dispatch_edit`)
- Input handlers: `handle_<mode>` (e.g., `handle_normal_mode`)
- AI tool implementations: `execute_<tool>` (e.g., `execute_buffer_read`)

---

## Testing Philosophy

### Real Editor, Not Mocks

Tests boot a real editor instance (no mocks) and exercise the same Scheme API surface available to users. This is intentional — a test that mocks the buffer management layer is not testing MAE, it is testing the mock.

### One Pending Op Per Test Step

In Scheme tests, each `it-test` is one eval-apply cycle. Putting `buffer-insert` and `goto-char` in the same step can execute in unexpected order. Split into separate steps.

### Tests Must Work in CI

No network access, no display server, no API keys assumed present. Tests that require external services must be gated (e.g., `#[ignore]` with an activation env var like `MAE_TCP_E2E=1`).

### Test Naming

Use descriptive `snake_case` names following the `module_feature_behavior` pattern:

```rust
#[test]
fn buffer_insert_appends_to_empty_buffer() { ... }

#[test]
fn lsp_diagnostics_clears_on_file_close() { ... }
```

### Test Helpers

Defined in `crates/core/src/editor/tests/mod.rs`:

| Helper | Method | Use When |
|--------|--------|----------|
| `editor_with_text(s)` | Char-by-char (input mode) | Testing input processing, mode transitions |
| `editor_with_bulk_text(s)` | `insert_text_at()` (bulk) | Multi-line content without input side effects |
| `editor_with_rust(s)` | Char-by-char + `.rs` path | Syntax highlighting, LSP features |

New helpers follow the `editor_with_*` pattern with a doc-comment.

### Scheme Test Commands

```sh
mae --test tests/crdt/              # CRDT sync tests
mae --test tests/editor/            # Editor feature tests
mae --test tests/collab-e2e/test_smoke.scm  # Single file

make test-scheme-crdt               # CRDT tests (builds first)
make test-scheme-editor             # Editor tests
make test-scheme-all                # All local Scheme tests
```

### Full Test Matrix

| Command | What it does |
|---------|-------------|
| `make ci` | fmt + clippy + check + test (excludes GUI) |
| `make verify` | `make ci` + GUI check with summary line |
| `cargo test --workspace` | All tests including GUI |
| `cargo test -p mae-core` | Single crate |
| `cargo test -p mae-core test_name` | Single test by name |
| `make audit` | `cargo-deny` security scan |
| `make docker-ci` | Full CI in container |
| `make build && mae --self-test` | AI-driven E2E self-test (requires API key) |

---

## Where to Start

### Getting Your Bearings

- **`make doctor`** — check build prereqs and runtime dependencies
- **[docs/CODE_MAP.md](docs/CODE_MAP.md)** — crate dependency graph and module sizes
- **[docs/terminology.md](docs/terminology.md)** — MAE-specific vocabulary
- **[docs/TOOL_ADDITION_CHECKLIST.md](docs/TOOL_ADDITION_CHECKLIST.md)** — step-by-step guide for adding AI tools
- **[ROADMAP.md](ROADMAP.md)** — milestone status and known bugs
- **[CLAUDE.md](CLAUDE.md)** — architecture principles and non-negotiable constraints

Check [Known Bugs in ROADMAP.md](ROADMAP.md#known-bugs) before filing a new issue — it may already be tracked.

### Good First Contributions

- **Documentation:** adding or improving KB nodes in `scheme/help/`, correcting doc-comments, clarifying error messages
- **Scheme modules:** extending an existing module (e.g., agenda, tables, surround) with a missing command
- **Test coverage:** adding tests for documented but uncovered behaviors, especially in `mae-core`
- **New language grammars:** adding tree-sitter grammars for languages not yet covered (the 13 currently supported are listed in `ROADMAP.md`)

### Areas Needing Help

- **Windows support:** the codebase builds on Linux and macOS; Windows has known gaps in PTY handling and path conventions
- **UI polish:** the GUI backend (winit + Skia) has rough edges in font fallback, scroll physics, and high-DPI handling
- **Accessibility:** the TUI renderer has no screen reader support; this is an open design problem

### Feature Priorities

See [ROADMAP.md](ROADMAP.md) for the current milestone breakdown. The next planned phases are RAG pipeline integration, AI harness (model profiles and prompt templates), and keymap architecture migration (trimming kernel bindings, moving the SPC leader tree fully into flavor modules).

---

## Submitting Changes

### PR Template

The [pull request template](.github/pull_request_template.md) asks for:

- **Summary:** 1-3 bullets describing what the PR does
- **Test plan:** a checklist confirming `make ci` passes, no new clippy warnings, new features have tests, no file exceeds 3,000 lines
- **Version label:** `release:patch`, `release:minor`, or `release:major` (or confirm commit prefixes are correct for auto-detection)

Fill these out. PRs without a test plan take longer to review.

### AI Tool Additions

Adding a new MCP tool involves several touch points across crates. Follow [docs/TOOL_ADDITION_CHECKLIST.md](docs/TOOL_ADDITION_CHECKLIST.md) exactly — it is easy to miss the Scheme binding, the help node, or the permission tier wiring.

### Review Process

- One approval is required to merge.
- Respond to review comments or mark them resolved before requesting re-review.
- Do not force-push after review starts. If you need to rebase, coordinate with the reviewer.
- CI must be green at merge time.

### After Merge

The `version-bump.yml` workflow runs automatically. It bumps the version based on your PR label or commit messages, generates a changelog entry, commits, tags, and triggers the release pipeline. You do not need to do anything.

---

## Communication

**GitHub Issues** — bugs, feature requests, and questions about specific behavior. Search before filing; duplicates slow everything down.

**GitHub Discussions** — design questions, architecture proposals, and anything that needs back-and-forth before it becomes an issue or PR. If you are uncertain whether a proposed change fits the project direction, open a Discussion first.

There is no chat channel yet. Discussions are the right place for open-ended questions.

---

## License

GPL-3.0-or-later. Contributions are owned by their authors — no CLA required.
