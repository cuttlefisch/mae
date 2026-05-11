# Contributing to MAE

## Prerequisites

- **Rust stable** (1.75+) via [rustup](https://rustup.rs)
- **GUI deps:** `clang`, `fontconfig-devel`, `freetype-devel` (Fedora) / `clang`, `libfontconfig1-dev`, `libfreetype6-dev` (Debian/Ubuntu) / Xcode CLI Tools (macOS)
- **TUI-only:** skip GUI deps — `make build-tui`
- Run **`make doctor`** first — it checks all prerequisites and prints install commands for anything missing.

Optional (for full self-test coverage):
```sh
make setup-dev    # installs lldb, rust-analyzer, debugpy
```

## Container Development

If you prefer not to install Rust and system dependencies locally, everything
works inside Docker:

```sh
make docker-ci          # equivalent to `make ci` in a clean environment
make docker-dev         # interactive shell — `make ci` works inside
make docker-new-user    # validate first-run flow in pristine container
```

The Dockerfile mirrors GitHub CI exactly — if `make docker-ci` passes, the
PR will pass CI. No toolchain management, no version mismatches.

**When to use native vs container:**
- **Native:** faster iteration, GUI builds, `make self-test`
- **Container:** zero setup, first contribution, pre-PR validation, CI debugging

## Getting Your Bearings

- **`make doctor`** — check build prereqs and runtime dependencies
- **[docs/CODE_MAP.md](docs/CODE_MAP.md)** — crate dependency graph and module sizes
- **[docs/terminology.md](docs/terminology.md)** — MAE-specific vocabulary (buffer, window, display region, etc.)
- **[docs/TOOL_ADDITION_CHECKLIST.md](docs/TOOL_ADDITION_CHECKLIST.md)** — step-by-step guide for adding AI tools
- **[ROADMAP.md](ROADMAP.md)** — milestone status and [known bugs](ROADMAP.md#known-bugs)

Check [Known Bugs in ROADMAP.md](ROADMAP.md#known-bugs) before filing a new issue.

## Architecture Quick Tour

MAE is split into 10 crates (see `README.md` for the full layout):

| Crate | Role |
|-------|------|
| `mae-core` | Buffer (rope), editor state, commands, keymap, syntax, babel, export |
| `mae-renderer` | Terminal rendering (ratatui), status bar, popups |
| `mae-gui` | GUI rendering (winit + Skia 2D), mouse, fonts, inline images |
| `mae-scheme` | Steel Scheme runtime, init.scm loading, hook dispatch |
| `mae-ai` | AI providers (Claude/OpenAI/Gemini/DeepSeek), tool execution |
| `mae-lsp` | LSP client — connection, navigation, diagnostics, completion |
| `mae-dap` | DAP client — breakpoints, stepping, watches |
| `mae-shell` | Terminal emulator (alacritty_terminal), PTY management |
| `mae-kb` | Knowledge base — graph store, org parser, FTS5 |
| `mae-mcp` | MCP bridge — Unix socket, JSON-RPC, stdio shim |

Key files:
- `crates/core/src/editor/mod.rs` — editor state struct
- `crates/core/src/editor/dispatch/` — command dispatch (10 submodules)
- `crates/mae/src/main.rs` — CLI entry point and event loop

Module size constraint: **no file over 3,000 lines**. Split into module directories when approaching the limit.

## Workflow

1. Create a feature branch from `main`
2. Make your changes
3. `make ci` must pass
4. Open a PR — review and merge

**Never push directly to `main`.** All changes go through feature branch + PR + CI.

## Testing

| Command | What it does |
|---------|-------------|
| `make ci` | Full CI pipeline: `cargo fmt --check` + `clippy -D warnings` + `cargo check` + `cargo test` (excludes GUI) |
| `make verify` | `make ci` + GUI check with summary line |
| `make audit` | `cargo-deny` security scanning (licenses, advisories, bans) |
| `cargo test -p mae-core` | Run a single crate's tests |
| `cargo test -p mae-core test_name` | Run a single test by name |
| `make build && mae --self-test` | AI-driven end-to-end self-test (requires API key) |
| `:self-test` | Same, from inside the editor |
| `make docker-ci` | Full CI in container (no local toolchain needed) |
| `make docker-new-user` | Validate first-run flow in pristine container |
| `make docker-smoke` | Quick binary smoke test in container |

## Commit Messages

We use [Conventional Commits](https://www.conventionalcommits.org/) for changelog generation via git-cliff:

```
feat(core): add structural selection via tree-sitter
fix(gui): cursor not visible after AI response
refactor(ai): split executor into submodules
docs: update CONTRIBUTING.md with testing section
ci: add cargo-deny to CI pipeline
test(lsp): add rename integration tests
```

Scope = crate name without `mae-` prefix: `core`, `gui`, `ai`, `lsp`, `dap`, `scheme`, `shell`, `kb`, `mcp`, `renderer`.

## Versioning

Versions are managed automatically. **Do not manually edit `VERSION` or the version in `Cargo.toml`.**

On PR merge, the `version-bump.yml` workflow:
1. Reads the current version from `VERSION`
2. Determines bump type from PR labels (preferred) or commit messages (fallback)
3. Bumps the version, updates `Cargo.toml` + `VERSION`, generates a changelog
4. Commits, tags, and pushes — which triggers the release pipeline

| PR Label | Commit Prefix | Bump |
|----------|---------------|------|
| `release:patch` | `fix(...):`  | patch (0.8.0 -> 0.8.1) |
| `release:minor` | `feat(...):` | minor (0.8.0 -> 0.9.0) |
| `release:major` | `feat(...)!:` | major (0.8.0 -> 1.0.0) |

**Labels take precedence** over commit scanning. If your PR has `feat` commits but should only bump patch (e.g., docs or CI changes), add the `release:patch` label.

**For development:** don't bump the version in your branch to "preview" a version number. The version in `main` always reflects the last release. If you need to test with a specific version string locally, use `cargo build` flags or a local override — don't commit it.

## What Makes a Good PR

- Feature branch from `main`
- `make ci` passes (no warnings, no formatting issues)
- New features have tests
- AI tool additions follow [docs/TOOL_ADDITION_CHECKLIST.md](docs/TOOL_ADDITION_CHECKLIST.md)
- No file over 3,000 lines — split into module directories if needed
- Commit messages follow the conventional format above

## Code Style

- `cargo fmt` — format all code
- `cargo clippy -D warnings` — no warnings allowed
- Follow existing module patterns in the crate you're modifying
- Read **CLAUDE.md** for architecture principles and non-negotiable constraints

## AI Testing

The self-test exercises the AI's tool surface against the live editor:

```sh
make build && mae --self-test              # all categories
mae --self-test introspection,editing      # specific categories
```

Requires an API key set in the environment. See `CLAUDE.md` → Development Dependencies for adapter setup.

## License

GPL-3.0-or-later. Contributions are owned by their authors — no CLA required.
