# Contributing to MAE

## Prerequisites

- **Rust stable** (1.75+) via [rustup](https://rustup.rs)
- `make setup-hooks` — configure git to use version-controlled hooks
- `make setup-dev` — (optional) install `lldb`, `rust-analyzer`, `debugpy` for full self-test coverage

## Workflow

1. Create a feature branch from `main`
2. Make your changes
3. Open a PR — CI must pass
4. Review → merge to `main`

**Never push directly to `main`.** All changes go through feature branch + PR + CI.

## Running Tests

| Command | What it does |
|---------|-------------|
| `make ci` | Full CI pipeline (fmt + clippy + check + test), excludes GUI |
| `make test` | All tests including GUI |
| `make check` | Type-check only (fast) |
| `make build && mae --self-test` | AI-driven end-to-end self-test (requires API key) |
| `:self-test` | Same, from inside the editor |

## Code Style

- `cargo fmt` — format all code
- `cargo clippy -D warnings` — no warnings allowed
- No files over 1000 lines — split into module directories
- Follow existing module patterns in the crate you're modifying

## Architecture

- Read **CLAUDE.md** for design principles and non-negotiable constraints
- Read **ROADMAP.md** for priorities and current state
- Read **README.org** (symlinked from org-roam) for the full architecture spec

## AI Testing

The self-test exercises the AI's tool surface against the live editor:

```sh
make build && mae --self-test              # all categories
mae --self-test introspection,editing      # specific categories
```

Requires an API key set in the environment. See `CLAUDE.md` → Development Dependencies for adapter setup.

## License

GPL-3.0-or-later. Contributions are owned by their authors — no CLA required.
