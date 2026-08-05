# MAE — Modern AI Editor
# GNU Makefile for development and deployment lifecycle.
#
# Usage:
#   make              — release build (same as 'make build')
#   make install      — build release + install binary to PREFIX
#   make dev          — debug build (faster compilation, no optimisations)
#   make run [ARGS=…] — dev build and run (e.g. make run ARGS=src/main.rs)
#   make test         — run the full test suite
#   make check        — fast type-check (cargo check, no codegen)
#   make fmt          — format all Rust sources (cargo fmt)
#   make clippy       — linting (cargo clippy)
#   make clean        — remove build artefacts
#   make uninstall    — remove installed binary
#   make build-tui    — terminal-only build (no skia dependency)
#   make test-tui     — run tests without GUI (no skia dependency)
#   make install-tui  — terminal-only install
#   make setup-hooks  — configure git to use .githooks/ (pre-commit fmt+clippy check)
#   make setup-dev    — install dev deps (rustfmt/clippy/lldb/rust-analyzer/…) + hooks
#   make install-vscode — the "MAE for VS Code" extension now lives in its own repo,
#                         github.com/cuttlefisch/mae-vscode (independent release cadence);
#                         this target just points there
#
# Configuration (override on the command line or in the environment):
#
#   PREFIX   — installation directory  (default: ~/.local/bin)
#   RELEASE  — 1 = build with --release (default: 1 for build/install)
#   CARGO    — cargo binary to use      (default: cargo)
#   FEATURES — cargo features to enable   (default: gui)
#
# Examples:
#   make install PREFIX=/usr/local/bin
#   make install PREFIX=$$HOME/.cargo/bin
#   ANTHROPIC_API_KEY=sk-... make run ARGS=myfile.rs

# @ai-caution: [build-env] Never let a git-supplied environment reach a build.
# Git exports an absolute GIT_DIR into hooks when the command runs from a
# linked worktree, and every child inherits it. A build under that environment
# makes skia's `git-sync-deps` bypass its own `is_git_toplevel()` guard and
# `git remote set-url origin <skia mirror>` against the SHARED main .git/config.
# See the long note in .githooks/pre-commit for the full mechanism.
unexport GIT_DIR
unexport GIT_WORK_TREE
unexport GIT_INDEX_FILE
unexport GIT_OBJECT_DIRECTORY
unexport GIT_NAMESPACE
unexport GIT_COMMON_DIR

PREFIX       ?= $(HOME)/.local/bin
DATADIR      ?= $(HOME)/.local/share
CARGO        ?= cargo
FEATURES     ?= gui
FEAT_FLAG    := $(if $(FEATURES),--features $(FEATURES),)
BINARY       := mae
SHIM_BINARY  := mae-mcp-shim
TARGET_DIR   := target

RELEASE_BIN  := $(TARGET_DIR)/release/$(BINARY)
RELEASE_SHIM := $(TARGET_DIR)/release/$(SHIM_BINARY)
DEBUG_BIN    := $(TARGET_DIR)/debug/$(BINARY)

DESKTOP_FILE := assets/mae.desktop
ICON_FILE    := assets/mae.svg

.PHONY: all build build-tui dev install install-tui install-all install-upgrade uninstall run test test-tui check fmt fmt-check clippy clean clean-cache ci ci-extended ci-docker-e2e ci-complete audit setup-hooks setup-dev self-test check-config code-map code-map-check heavy-e2e-check audit-metrics audit-metrics-check audit-metrics-bless lint-shell lint-yaml lint-deploy lint-all test-deploy gen-fixtures doctor help docker-ci docker-new-user docker-smoke docker-dev docker-clean docs-tangle docs-tangle-check install-daemon install-daemon-service bench bench-save bench-compare manual-kb install-manual practices-kb install-practices adr-kb install-adr devpractices-kb install-devpractices verify-adr-kb-sync install-vscode

# Default target: release build
all: build

## build: compile a release binary (optimised, no debug info)
build:
	$(CARGO) build --release $(FEAT_FLAG)

## build-tui: terminal-only release build (no skia dependency)
build-tui:
	$(CARGO) build --release

## build-daemon: build the daemon binary (CozoDB+SQLite)
build-daemon:
	cd daemon && $(CARGO) build --release

## verify-binary: fail if a RUNNING mae/mae-daemon differs from the fresh build
## (two-machine testing guard — prevents testing a fix against a stale binary).
verify-binary:
	@sh scripts/verify-binary.sh

## dev: compile a debug binary (faster compile, includes debug info)
dev:
	$(CARGO) build $(FEAT_FLAG)

## install: build release binary + manual KB + practices KB, install to PREFIX, register desktop entry
install: build manual-kb practices-kb devpractices-kb adr-kb
	@mkdir -p $(PREFIX)
	@install -m 755 $(RELEASE_BIN) $(PREFIX)/$(BINARY)
	@install -m 755 $(RELEASE_SHIM) $(PREFIX)/$(SHIM_BINARY)
	@echo "Installed $(BINARY) -> $(PREFIX)/$(BINARY)"
	@echo "Installed $(SHIM_BINARY) -> $(PREFIX)/$(SHIM_BINARY)"
	@mkdir -p $(DATADIR)/mae
	@rm -rf $(DATADIR)/mae/mae-manual.cozo
	@cp -r assets/mae-manual.cozo $(DATADIR)/mae/mae-manual.cozo
	@cp assets/mae-manual.cozo.sha256 $(DATADIR)/mae/mae-manual.cozo.sha256
	@echo "Installed manual KB -> $(DATADIR)/mae/mae-manual.cozo"
	@rm -rf $(DATADIR)/mae/mae-practices.cozo
	@cp -r assets/mae-practices.cozo $(DATADIR)/mae/mae-practices.cozo
	@cp assets/mae-practices.cozo.sha256 $(DATADIR)/mae/mae-practices.cozo.sha256
	@echo "Installed practices KB -> $(DATADIR)/mae/mae-practices.cozo"
	@rm -rf $(DATADIR)/mae/mae-devpractices.cozo
	@cp -r assets/mae-devpractices.cozo $(DATADIR)/mae/mae-devpractices.cozo
	@cp assets/mae-devpractices.cozo.sha256 $(DATADIR)/mae/mae-devpractices.cozo.sha256
	@echo "Installed DevPractices KB -> $(DATADIR)/mae/mae-devpractices.cozo"
	@rm -rf $(DATADIR)/mae/mae-adr.cozo
	@cp -r assets/mae-adr.cozo $(DATADIR)/mae/mae-adr.cozo
	@cp assets/mae-adr.cozo.sha256 $(DATADIR)/mae/mae-adr.cozo.sha256
	@echo "Installed ADR KB -> $(DATADIR)/mae/mae-adr.cozo"
	@mkdir -p $(DATADIR)/applications
	@sed 's|Exec=mae|Exec=$(PREFIX)/$(BINARY)|' $(DESKTOP_FILE) > $(DATADIR)/applications/mae.desktop
	@echo "Installed desktop entry -> $(DATADIR)/applications/mae.desktop"
	@mkdir -p $(DATADIR)/icons/hicolor/scalable/apps
	@install -m 644 $(ICON_FILE) $(DATADIR)/icons/hicolor/scalable/apps/mae.svg
	@echo "Installed icon -> $(DATADIR)/icons/hicolor/scalable/apps/mae.svg"
	@mkdir -p $(DATADIR)/mae/modules
	@if [ -d modules ]; then \
		cp -r modules/* $(DATADIR)/mae/modules/; \
		echo "Installed modules -> $(DATADIR)/mae/modules/"; \
	fi
	@if command -v update-desktop-database >/dev/null 2>&1; then \
		update-desktop-database $(DATADIR)/applications 2>/dev/null || true; \
	fi
	@if command -v gtk-update-icon-cache >/dev/null 2>&1; then \
		gtk-update-icon-cache -f -t $(DATADIR)/icons/hicolor 2>/dev/null || true; \
	fi
	@echo ""
	@echo "Next steps:"
	@echo "  mae --init-config    # generate config + init.scm + run first-time wizard"
	@echo "  mae file.rs          # launch with GUI (default)"
	@echo "  mae -nw file.rs      # launch in terminal"
	@case ":$$PATH:" in *":$(PREFIX):"*) ;; *) \
		echo ""; \
		echo "  Warning: $(PREFIX) is not on your PATH. Add to your shell profile:"; \
		echo "    export PATH=\"$(PREFIX):\$$PATH\""; \
	esac

## install-tui: terminal-only install (no skia dependency)
install-tui: build-tui
	@mkdir -p $(PREFIX)
	@install -m 755 $(RELEASE_BIN) $(PREFIX)/$(BINARY)
	@install -m 755 $(RELEASE_SHIM) $(PREFIX)/$(SHIM_BINARY)
	@echo "Installed $(BINARY) -> $(PREFIX)/$(BINARY) (terminal-only)"
	@echo "Installed $(SHIM_BINARY) -> $(PREFIX)/$(SHIM_BINARY)"

## install-upgrade: rebuild all components, stop services, replace binaries, restart
install-upgrade:
	@set -e; \
	OLD_V=$$($(PREFIX)/$(BINARY) --version 2>/dev/null || echo "(not installed)"); \
	echo "=== MAE Upgrade ==="; \
	echo "Current: $$OLD_V"; \
	echo ""; \
	RESTART_DAEMON=0; \
	if systemctl --user is-active mae-daemon >/dev/null 2>&1; then \
		echo "Stopping mae-daemon..."; \
		systemctl --user stop mae-daemon; \
		RESTART_DAEMON=1; \
	fi; \
	if [ -f $(PREFIX)/$(BINARY) ]; then \
		cp $(PREFIX)/$(BINARY) $(PREFIX)/$(BINARY).bak; \
		echo "Backed up $(BINARY) -> $(BINARY).bak"; \
	fi; \
	if [ -f $(PREFIX)/mae-daemon ]; then \
		cp $(PREFIX)/mae-daemon $(PREFIX)/mae-daemon.bak; \
		echo "Backed up mae-daemon -> mae-daemon.bak"; \
	fi; \
	echo ""; \
	echo "Building..."; \
	$(MAKE) build build-daemon; \
	echo ""; \
	echo "Installing..."; \
	$(MAKE) install install-daemon-service; \
	NEW_V=$$($(PREFIX)/$(BINARY) --version 2>/dev/null || echo "unknown"); \
	OLD_MAJOR=$$(echo "$$OLD_V" | sed 's/mae //' | cut -d. -f1); \
	NEW_MAJOR=$$(echo "$$NEW_V" | sed 's/mae //' | cut -d. -f1); \
	if [ -n "$$OLD_MAJOR" ] && [ -n "$$NEW_MAJOR" ] && [ "$$OLD_MAJOR" != "$$NEW_MAJOR" ] 2>/dev/null; then \
		echo ""; \
		echo "WARNING: MAJOR VERSION CHANGE ($$OLD_MAJOR -> $$NEW_MAJOR)"; \
		echo "  Config or protocol changes may require manual migration."; \
		echo "  Check CHANGELOG.md for breaking changes."; \
	fi; \
	if [ "$$RESTART_DAEMON" = "1" ]; then \
		echo "Restarting mae-daemon..."; \
		systemctl --user start mae-daemon || \
			echo "WARNING: Failed to restart mae-daemon"; \
	fi; \
	echo ""; \
	echo "=== Upgrade Complete ==="; \
	echo "  $$OLD_V -> $$NEW_V"

## install-all: install editor + daemon + systemd services
install-all: install install-daemon-service
	@echo ""
	@echo "Full install complete."
	@echo "  mae                      — launch editor"
	@echo "  systemctl --user enable --now mae-daemon"

## uninstall: remove installed binaries, desktop entries, icon, and services
uninstall:
	@rm -f $(PREFIX)/$(BINARY)
	@rm -f $(PREFIX)/$(SHIM_BINARY)
	@rm -f $(PREFIX)/mae-daemon
	@rm -f $(DATADIR)/applications/mae.desktop
	@rm -f $(DATADIR)/icons/hicolor/scalable/apps/mae.svg
	@echo "Removed $(PREFIX)/$(BINARY)"
	@echo "Removed $(PREFIX)/$(SHIM_BINARY)"
	@echo "Removed $(PREFIX)/mae-daemon"
	@echo "Removed $(DATADIR)/applications/mae.desktop"
	@echo "Removed $(DATADIR)/icons/hicolor/scalable/apps/mae.svg"
	@rm -rf $(DATADIR)/mae/modules
	@echo "Removed $(DATADIR)/mae/modules/"
	@if [ "$$(uname -s)" = "Darwin" ]; then \
		launchctl bootout gui/$$(id -u)/com.cuttlefisch.mae-daemon 2>/dev/null || true; \
		rm -f $(HOME)/Library/LaunchAgents/com.cuttlefisch.mae-daemon.plist; \
		echo "Removed launchd agent"; \
		rm -rf $(HOME)/Applications/MAE.app; \
		echo "Removed ~/Applications/MAE.app"; \
	else \
		systemctl --user disable --now mae-daemon 2>/dev/null || true; \
		rm -f $(HOME)/.config/systemd/user/mae-daemon.service; \
		systemctl --user daemon-reload 2>/dev/null || true; \
		echo "Removed systemd services"; \
	fi
	@if command -v update-desktop-database >/dev/null 2>&1; then \
		update-desktop-database $(DATADIR)/applications 2>/dev/null || true; \
	fi

## run: dev build and run (pass extra arguments via ARGS=…)
run:
	$(CARGO) run $(FEAT_FLAG) -- $(ARGS)

## test: run all workspace tests (including GUI)
test:
	$(CARGO) test --workspace

## test-daemon: run daemon workspace tests
test-daemon:
	cd daemon && $(CARGO) test

## test-tui: run workspace tests without GUI (no skia deps required)
test-tui:
	$(CARGO) test --workspace --exclude mae-gui

## test-nextest: run all workspace tests via cargo-nextest (issue #470) --
## opt-in, not the default `test` target. nextest's process-per-test global
## scheduling is what makes CI's `stable / test` leg much faster, and CI
## uses --release there (proven clean on a full run); a plain DEBUG build
## under nextest's default full-machine concurrency was empirically observed
## to be more prone to transient resource contention for the handful of
## tests that spawn a real `mae --headless` subprocess and assert on its
## resource usage (see .config/nextest.toml's `heavy-subprocess-e2e` group
## for the details/mitigation) -- every one of those tests passes cleanly in
## isolation or under --release, so this is a real machine-load tradeoff,
## not a logic bug. Requires `cargo install cargo-nextest`. Prefer
## `test-nextest-release` if you hit contention locally.
test-nextest:
	$(CARGO) nextest run --workspace

## test-nextest-release: as test-nextest, but --release --features gui --
## the exact configuration CI's `stable / test` leg uses, and the one
## actually validated clean on a full run.
test-nextest-release:
	$(CARGO) nextest run --workspace --release --features gui

## test-daemon-nextest: run daemon workspace tests via cargo-nextest --
## opt-in; the daemon workspace's real socket tests already use ephemeral
## ports and showed no contention issues locally, unlike the editor
## workspace's headless-subprocess e2e tests.
test-daemon-nextest:
	cd daemon && $(CARGO) nextest run

## check: fast type-check without producing a binary
check:
	$(CARGO) check $(FEAT_FLAG)

## verify: check + test — single command for development validation
verify:
	@echo "=== Check (workspace + GUI) ==="
	$(CARGO) check $(FEAT_FLAG)
	@echo "=== Test ==="
	$(CARGO) test --workspace 2>&1 | tee /dev/stderr | grep "^test result:" | awk -F'[; ]' 'BEGIN{p=0;f=0} {p+=$$4;f+=$$7} END{printf "\n=== %d passed, %d failed ===\n",p,f}'

# @ai-caution: [two-workspaces] `daemon/` is a SEPARATE cargo workspace (ADR-014,
# its own Cargo.lock). A bare `cargo fmt` / `cargo clippy` at the repo root does
# NOT see it. Every quality target below must therefore run twice, or it reports
# clean while leaving half the tree unchecked — which is exactly what happened on
# 2026-08-04: `cargo fmt --all --check` at the root said "clean", and CI's
# `daemon / check + test` then failed on formatting. There was no `fmt-daemon`
# target at all, so no local invocation could have caught it.

## fmt: format all Rust sources in place (BOTH workspaces)
fmt:
	$(CARGO) fmt --all
	cd daemon && $(CARGO) fmt --all

## fmt-check: check formatting without writing (BOTH workspaces)
fmt-check:
	$(CARGO) fmt --all -- --check
	cd daemon && $(CARGO) fmt --all -- --check

## fmt-daemon: format the daemon workspace only
fmt-daemon:
	cd daemon && $(CARGO) fmt --all

## clippy: run linter across the editor workspace
clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

## clippy-daemon: run linter on daemon workspace
clippy-daemon:
	cd daemon && $(CARGO) clippy --all-targets -- -D warnings

## pre-commit: the local quality gate, in ONE place.
## `.githooks/pre-commit` calls this rather than restating the command list --
## the hook, the Makefile and CI had drifted into three disagreeing definitions
## of "what must pass", and the daemon workspace fell through the gap between
## them. Anything added here is picked up by the hook automatically.
pre-commit: fmt-check clippy clippy-daemon code-map-check heavy-e2e-check
	@$(MAKE) --no-print-directory verify-adr-kb-sync
	@echo "✅ pre-commit gate passed"

## ci: run the full CI pipeline locally (fmt + clippy + check + test + scheme tests)
ci: fmt-check
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) check --workspace --all-targets
	$(MAKE) test
	@echo "==> Scheme editor tests..."
	./target/debug/mae --test tests/editor/
	@echo "==> Config validation..."
	./target/debug/mae --check-config
	@echo "==> Code-map freshness..."
	cd tools/code-map && $(CARGO) run --release -- --workspace-root ../.. --check
	@echo "CI passed ✓"

## ci-all: editor + daemon CI (both workspaces)
ci-all: ci test-daemon clippy-daemon
	@echo "CI all (editor + daemon) passed ✓"

## ci-extended: thorough CI — run before opening a PR (ci + CRDT tests + docker smoke)
ci-extended: ci
	@echo "==> Scheme CRDT tests..."
	./target/debug/mae --test tests/crdt/
	@echo "==> Docker smoke test..."
	$(MAKE) docker-smoke
	@echo "==> Docker new-user test..."
	$(MAKE) docker-new-user
	@echo "CI extended passed ✓"

## ci-docker-e2e: on-demand collab E2E in Docker (when touching collab/sync code)
## DISABLED: Docker E2E requires proper Scheme async/yield support for
## reliable cross-container coordination. Protocol correctness is covered by:
##   - collab_e2e.rs (23 server protocol tests)
##   - tests/crdt/ (142 CRDT Scheme tests)
##   - tests/collab-local/ (85 local collab Scheme tests)
## Re-enable when Scheme runtime supports blocking wait primitives.
ci-docker-e2e:
	@echo "==> Docker collab E2E (SKIPPED — see Makefile comment)..."
	@echo "Docker collab E2E skipped ✓"

## ci-complete: everything — mirrors GitHub CI
ci-complete: ci-extended ci-docker-e2e
	@echo "CI complete passed ✓"

## audit: run cargo-deny security + license scanning
audit:
	cargo deny check

## setup-hooks: configure git to use version-controlled hooks
setup-hooks:
# Only claim core.hooksPath if it is unset or already ours. A contributor may
# point it at a machine-local hook directory that chains to .githooks (e.g. a
# confidentiality/secret-scanning pre-commit that must run FIRST, then delegate).
# Overwriting that silently disables their guard while still reporting success —
# the failure mode is invisible, which is exactly why this is a check, not a set.
	@current="$$(git config --get core.hooksPath || true)"; \
	if [ -z "$$current" ] || [ "$$current" = ".githooks" ]; then \
		git config core.hooksPath .githooks; \
		echo "Git hooks configured to use .githooks/"; \
	else \
		echo "core.hooksPath is already set to '$$current' — leaving it alone."; \
		echo "  Ensure that hook chains to .githooks/pre-commit, or unset it and re-run."; \
	fi
# Defence in depth for the skia `git-sync-deps` remote clobber (root cause and
# the real fix are documented in .githooks/pre-commit; the leak happens when a
# build inherits a git-supplied GIT_DIR, NOT via upward directory discovery —
# skia's sources live in the cargo registry, where no enclosing repo exists).
#
# These two settings are per-clone (.git/config cannot be committed) and are a
# backstop for the case where a build is launched outside the scrubbed paths:
#
#   sync-deps.disable — skia's own opt-out. Under a leaked GIT_DIR its lookup
#     resolves to THIS repo's config, so setting it here makes the script
#     return before it reaches `git remote set-url`.
#   remote.origin.pushurl — `set-url` writes only remote.origin.url, while push
#     prefers pushurl. If the URL is ever rewritten again, push still goes to
#     the right place instead of prompting for Google credentials.
	git config sync-deps.disable true
	@url="$$(git config --get remote.origin.url || true)"; \
	if [ -n "$$url" ] && [ -z "$$(git config --get remote.origin.pushurl || true)" ]; then \
		git config remote.origin.pushurl "$$url"; \
		echo "remote.origin.pushurl pinned to $$url"; \
	fi
	@echo "skia git-sync-deps backstops set (protects this repo's git remote)"

## setup-dev: install dev dependencies (rustfmt/clippy + DAP/LSP tools) + git hooks
setup-dev:
	@scripts/setup-dev.sh
	@$(MAKE) setup-hooks

## check-config: validate init.scm + config.toml without launching the editor
check-config: build-tui
	$(RELEASE_BIN) --check-config

## self-test: run AI-driven e2e self-test headless (requires AI provider)
self-test: build
	$(RELEASE_BIN) --self-test $(CATS)

## code-map: generate docs/CODE_MAP.md and docs/CODE_MAP.json
code-map:
	cd tools/code-map && $(CARGO) run --release -- --workspace-root ../..

## code-map-check: verify code map is up to date (for CI)
code-map-check:
	cd tools/code-map && $(CARGO) run --release -- --workspace-root ../.. --check

## heavy-e2e-check: every test binary that spawns a real `mae --headless` must be
## listed in all THREE heavy-subprocess filters (.config/nextest.toml, ci.yml,
## badges.yml). Omitting one does not fail loudly -- it surfaces as an
## intermittent 30s socket-bind timeout, usually in a DIFFERENT test.
heavy-e2e-check:
	./scripts/check-heavy-e2e-lists.sh

## audit-metrics: regenerate docs/AUDIT_METRICS.json (structural metrics + marker cross-refs)
audit-metrics:
	cd tools/audit-metrics && $(CARGO) run --release -- --workspace-root ../..

## audit-metrics-check: fail on NEW or growing ceiling violations (for CI).
## Ratchets against docs/AUDIT_BASELINE.json -- pre-existing debt passes at its
## accepted size, a file that grows past tolerance fails, a file that shrinks
## never fails.
audit-metrics-check:
	cd tools/audit-metrics && $(CARGO) run --release -- --workspace-root ../.. --check

## audit-metrics-bless: re-accept the CURRENT set of ceiling violations as the
## baseline. Run this ONLY when deliberately taking on new debt -- and pair it
## with an `@ai-caution: [architecture-debt]` marker + a ROADMAP.md cross-link,
## per CLAUDE.md's tagging convention.
audit-metrics-bless:
	cd tools/audit-metrics && $(CARGO) run --release -- --workspace-root ../.. --bless

## gen-fixtures: generate large test fixtures for perf benchmarking
gen-fixtures:
	bash assets/gen-large-org.sh
	bash assets/gen-long-lines.sh

## doctor: check build prerequisites and report status
doctor:
	@OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; WARN="\033[33m!\033[0m"; \
	printf "MAE Build Prerequisites\n=======================\n\n"; \
	if command -v rustc >/dev/null 2>&1; then \
		V=$$(rustc --version | awk '{print $$2}'); \
		printf "  $$OK rustc $$V\n"; \
	else printf "  $$FAIL rustc not found — install via https://rustup.rs\n"; fi; \
	if command -v cargo >/dev/null 2>&1; then \
		printf "  $$OK cargo\n"; \
	else printf "  $$FAIL cargo not found\n"; fi; \
	if command -v clang >/dev/null 2>&1; then \
		printf "  $$OK clang (GUI build)\n"; \
	else printf "  $$WARN clang not found — needed for GUI build (make build-tui works without it)\n"; fi; \
	if command -v pkg-config >/dev/null 2>&1; then \
		printf "  $$OK pkg-config\n"; \
	else printf "  $$WARN pkg-config not found — needed for GUI build\n"; fi; \
	if pkg-config --exists fontconfig 2>/dev/null; then \
		printf "  $$OK fontconfig headers\n"; \
	else printf "  $$WARN fontconfig-devel not found — needed for GUI build\n"; fi; \
	if pkg-config --exists freetype2 2>/dev/null; then \
		printf "  $$OK freetype headers\n"; \
	else printf "  $$WARN freetype-devel not found — needed for GUI build\n"; fi; \
	printf "\n"; \
	case ":$$PATH:" in *":$(HOME)/.local/bin:"*) \
		printf "  $$OK ~/.local/bin is on PATH\n";; *) \
		printf "  $$WARN ~/.local/bin is not on PATH — add to shell profile:\n"; \
		printf "    export PATH=\"$$HOME/.local/bin:\$$PATH\"\n";; esac; \
	printf "\nTUI-only (make build-tui) needs only rustc + cargo.\n"

## clean: remove all build artefacts
## clean: remove ALL build artifacts (both workspaces) — forces a full rebuild
clean:
	$(CARGO) clean
	cd daemon && $(CARGO) clean

## # --- Lint / static analysis for everything that is not Rust -----------------
#
# Rust is covered by `make clippy` + the pre-commit gate. These cover the shell
# scripts, the CI/release workflows, and the deployment role — none of which had
# any linting at all before 2026-08.

lint-shell:  ## shellcheck every tracked shell script (fails at warning+)
	./scripts/lint-shell.sh

lint-yaml:  ## yamllint the workflows and the deployment role
	yamllint -c .yamllint.yml .github/workflows/ deploy/

lint-deploy:  ## ansible-lint the deployment role at the production profile
	cd deploy/ansible && ansible-lint .

lint-all: lint-shell lint-yaml lint-deploy  ## every non-Rust linter

test-deploy:  ## prove the deployment role's safety checks refuse bad configs
	./deploy/ansible/tests/run-tests.sh

clean-cache: reclaim regenerable compilation caches (both workspaces) WITHOUT
## a full rebuild. Cargo never garbage-collects incremental session dirs from past
## code states, so on a heavily-branched workspace they grow without bound (we hit
## ~370 GB). Incremental is now off by default (.cargo/config.toml), but this stays
## as the fast disk-reclaim if any incremental data is produced (e.g. via
## CARGO_INCREMENTAL=1). Safe: pure cache, no final artifacts removed.
clean-cache:
	rm -rf target/*/incremental daemon/target/*/incremental
	@echo "Reclaimed incremental caches (both workspaces)."

## manual-kb: build the pre-built manual KB (CozoDB file + SHA-256 checksum)
manual-kb:
	@mkdir -p assets
	$(CARGO) run --release --bin build-manual-kb -- assets/mae-manual.cozo

## install-manual: install pre-built manual KB to XDG data dir
install-manual: manual-kb
	@mkdir -p $(DATADIR)/mae
	@rm -rf $(DATADIR)/mae/mae-manual.cozo
	@cp -r assets/mae-manual.cozo $(DATADIR)/mae/mae-manual.cozo
	@cp assets/mae-manual.cozo.sha256 $(DATADIR)/mae/mae-manual.cozo.sha256
	@echo "Installed manual KB -> $(DATADIR)/mae/mae-manual.cozo"

## practices-kb: build the pre-built dev-practices KB (issue #370)
practices-kb:
	@mkdir -p assets
	$(CARGO) run --release --bin build-practices-kb -- assets/mae-practices.cozo

## install-practices: install pre-built practices KB to XDG data dir
install-practices: practices-kb
	@mkdir -p $(DATADIR)/mae
	@rm -rf $(DATADIR)/mae/mae-practices.cozo
	@cp -r assets/mae-practices.cozo $(DATADIR)/mae/mae-practices.cozo
	@cp assets/mae-practices.cozo.sha256 $(DATADIR)/mae/mae-practices.cozo.sha256
	@echo "Installed practices KB -> $(DATADIR)/mae/mae-practices.cozo"

## devpractices-kb: build the pre-built generic DevPractices KB (issue #514, ADR-076)
devpractices-kb:
	@mkdir -p assets
	$(CARGO) run --release --bin build-devpractices-kb -- assets/mae-devpractices.cozo

## install-devpractices: install pre-built DevPractices KB to XDG data dir
install-devpractices: devpractices-kb
	@mkdir -p $(DATADIR)/mae
	@rm -rf $(DATADIR)/mae/mae-devpractices.cozo
	@cp -r assets/mae-devpractices.cozo $(DATADIR)/mae/mae-devpractices.cozo
	@cp assets/mae-devpractices.cozo.sha256 $(DATADIR)/mae/mae-devpractices.cozo.sha256
	@echo "Installed DevPractices KB -> $(DATADIR)/mae/mae-devpractices.cozo"

## adr-kb: build the pre-built ADR-as-KB-node KB (ADR-059, molecularly-structured decision records)
adr-kb:
	@mkdir -p assets
	$(CARGO) run --release --bin build-adr-kb -- assets/mae-adr.cozo

## install-adr: install pre-built ADR KB to XDG data dir
install-adr: adr-kb
	@mkdir -p $(DATADIR)/mae
	@rm -rf $(DATADIR)/mae/mae-adr.cozo
	@cp -r assets/mae-adr.cozo $(DATADIR)/mae/mae-adr.cozo
	@cp assets/mae-adr.cozo.sha256 $(DATADIR)/mae/mae-adr.cozo.sha256
	@echo "Installed ADR KB -> $(DATADIR)/mae/mae-adr.cozo"

## verify-adr-kb-sync: CI staleness gate (ADR-059 Phase E) -- fails if a structured ADR
## header field (Status/Extends/Relates to/Depends on/Supersedes) changed relative to
## BASE without assets/mae-adr.cozo(.sha256) being regenerated in the same range.
## BASE defaults to origin/main (a PR's target branch); override for a local check
## against a different ref, e.g. `make verify-adr-kb-sync BASE=HEAD~3`.
BASE ?= origin/main
verify-adr-kb-sync:
	$(CARGO) run --release --bin verify-adr-kb-sync -- --base $(BASE)

## install-vscode: pointer to the extracted "MAE for VS Code" extension repo
install-vscode:
	@echo "The VS Code extension moved to github.com/cuttlefisch/mae-vscode -- see its README for install instructions."
	@exit 1

## install-daemon: build + install mae-daemon to PREFIX
install-daemon: build-daemon
	@mkdir -p $(PREFIX)
	@install -m 755 daemon/$(TARGET_DIR)/release/mae-daemon $(PREFIX)/mae-daemon
	@mkdir -p $(HOME)/.config/mae
	@if [ ! -f $(HOME)/.config/mae/daemon.toml ]; then \
		cp assets/daemon-config.toml $(HOME)/.config/mae/daemon.toml; \
		echo "Installed daemon config -> ~/.config/mae/daemon.toml"; \
	fi
	@echo "Installed mae-daemon -> $(PREFIX)/mae-daemon"

## install-daemon-service: install daemon service (systemd on Linux, launchd on macOS)
install-daemon-service: install-daemon
	@if [ "$$(uname -s)" = "Darwin" ]; then \
		mkdir -p $(HOME)/Library/LaunchAgents; \
		mkdir -p $(HOME)/Library/Logs/mae; \
		sed -e 's|__BINDIR__|$(PREFIX)|g' -e 's|__LOGDIR__|$(HOME)/Library/Logs/mae|g' \
			assets/com.cuttlefisch.mae-daemon.plist \
			> $(HOME)/Library/LaunchAgents/com.cuttlefisch.mae-daemon.plist; \
		echo ""; \
		echo "Installed launchd agent -> ~/Library/LaunchAgents/"; \
		echo "Binary: $(PREFIX)/mae-daemon"; \
		echo ""; \
		echo "Next steps:"; \
		echo "  launchctl load ~/Library/LaunchAgents/com.cuttlefisch.mae-daemon.plist"; \
		echo "  tail -f ~/Library/Logs/mae/mae-daemon.log"; \
	else \
		mkdir -p $(HOME)/.config/systemd/user; \
		install -m 644 assets/mae-daemon.service $(HOME)/.config/systemd/user/mae-daemon.service; \
		systemctl --user daemon-reload 2>/dev/null || true; \
		echo ""; \
		echo "Installed mae-daemon.service -> ~/.config/systemd/user/"; \
		echo "Binary: $(PREFIX)/mae-daemon"; \
		echo ""; \
		echo "Next steps:"; \
		echo "  systemctl --user enable --now mae-daemon   # start + auto-start on login"; \
		echo "  journalctl --user -u mae-daemon -f         # view logs"; \
	fi

## test-scheme: run Scheme test files locally (pass TEST_PATH=path)
test-scheme: build-tui
	$(RELEASE_BIN) --test $(or $(TEST_PATH),tests/collab-e2e/)

## test-scheme-crdt: run CRDT/sync Scheme tests
test-scheme-crdt: build-tui
	$(RELEASE_BIN) --test tests/crdt/

## test-scheme-editor: run editor feature Scheme tests
test-scheme-editor: build-tui
	$(RELEASE_BIN) --test tests/editor/

## test-scheme-collab-local: run collab state transition tests (no server needed)
test-scheme-collab-local: build-tui
	$(RELEASE_BIN) --test tests/collab-local/

# ADR-044: independent, looser backstop for the *script* hanging before it ever
# reaches the daemon-TTL protection inside scripts/lib/e2e-daemon-harness.sh
# (default daemon TTL there is 600s) — this is not the leak fix itself, just a
# belt-and-suspenders cap on a `make test-collab-*-e2e` invocation itself.
E2E_SCRIPT_TIMEOUT := 750

## test-collab-mtls-e2e: single-host trusted-peer mTLS e2e (real daemon + editor)
test-collab-mtls-e2e: build-tui build-daemon
	MAE_BIN=$(RELEASE_BIN) MAE_DAEMON_BIN=$(CURDIR)/daemon/target/release/mae-daemon \
		timeout -k 30 $(E2E_SCRIPT_TIMEOUT) scripts/collab-mtls-e2e.sh

## test-collab-membership-e2e: two-editor per-KB membership enforcement e2e
test-collab-membership-e2e: build-tui build-daemon
	MAE_BIN=$(RELEASE_BIN) MAE_DAEMON_BIN=$(CURDIR)/daemon/target/release/mae-daemon \
		timeout -k 30 $(E2E_SCRIPT_TIMEOUT) scripts/collab-membership-e2e.sh

## test-collab-encrypted-e2e: ADR-037 E2E content-encryption lifecycle e2e
test-collab-encrypted-e2e: build-tui build-daemon
	MAE_BIN=$(RELEASE_BIN) MAE_DAEMON_BIN=$(CURDIR)/daemon/target/release/mae-daemon \
		timeout -k 30 $(E2E_SCRIPT_TIMEOUT) scripts/collab-encrypted-e2e.sh

## test-collab-p2p-mesh-e2e: ADR-025 two-daemon P2P mesh e2e (no central hub)
test-collab-p2p-mesh-e2e: build-tui build-daemon
	MAE_BIN=$(RELEASE_BIN) MAE_DAEMON_BIN=$(CURDIR)/daemon/target/release/mae-daemon \
		timeout -k 30 $(E2E_SCRIPT_TIMEOUT) scripts/collab-p2p-mesh-e2e.sh

## test-collab-e2e-all: all trusted-peer e2e tests (mTLS + membership + encrypted + mesh)
test-collab-e2e-all: test-collab-mtls-e2e test-collab-membership-e2e test-collab-encrypted-e2e test-collab-p2p-mesh-e2e

## test-scheme-all: run all local Scheme tests (crdt + editor + collab-local)
test-scheme-all: build-tui
	$(RELEASE_BIN) --test tests/crdt/
	$(RELEASE_BIN) --test tests/editor/
	$(RELEASE_BIN) --test tests/collab-local/

## test-scheme-ci: same as test-scheme-all (CI entry point)
test-scheme-ci: test-scheme-all

## test-scheme-r7rs: run R7RS compliance + torture + benchmark suites
test-scheme-r7rs:
	cargo test -p mae-scheme --test r7rs_compliance -- --nocapture
	cargo test -p mae-scheme --test scheme_torture -- --nocapture
	cargo test -p mae-scheme --test scheme_benchmarks -- --nocapture

## docker-collab-test: run collab CRDT E2E tests in Docker containers
## Uses `--wait` so compose exits once all client/verifier services complete,
## then inspects the verifier exit code for pass/fail.
docker-collab-test:
	@echo "Running collab E2E tests (docker compose)..."
	@docker compose -f docker-compose.collab-test.yml up --build --wait 2>&1; \
	RC=$$(docker compose -f docker-compose.collab-test.yml ps -a verifier --format '{{.ExitCode}}' 2>/dev/null); \
	echo "--- verifier output ---"; \
	docker compose -f docker-compose.collab-test.yml logs --no-log-prefix verifier; \
	echo "--- verifier exit code: $${RC:-unknown} ---"; \
	docker compose -f docker-compose.collab-test.yml down --volumes --timeout 10; \
	exit $${RC:-1}

## docker-headless-e2e: run headless MAE service-mode E2E in Docker containers
## (ADR-055, Phase J / #385) — real mae --headless, a real two-instance
## collision race, and a real MCP handshake via mae-mcp-shim --check.
## Uses `--wait` so compose exits once all client/verifier services complete,
## then inspects the verifier exit code for pass/fail.
docker-headless-e2e:
	@echo "Running headless MAE E2E tests (docker compose)..."
	@docker compose -f docker-compose.headless-e2e.yml up --build --wait 2>&1; \
	RC=$$(docker compose -f docker-compose.headless-e2e.yml ps -a verifier --format '{{.ExitCode}}' 2>/dev/null); \
	echo "--- verifier output ---"; \
	docker compose -f docker-compose.headless-e2e.yml logs --no-log-prefix verifier; \
	echo "--- verifier exit code: $${RC:-unknown} ---"; \
	docker compose -f docker-compose.headless-e2e.yml down --volumes --timeout 10; \
	exit $${RC:-1}

## docker-ci: run full CI pipeline in a container (no local toolchain needed)
docker-ci:
	docker compose run --rm --build ci

## docker-new-user: validate new-user install flow in a clean container
docker-new-user:
	docker compose run --rm --build new-user

## docker-smoke: quick binary smoke test in container
docker-smoke:
	docker compose run --rm --build smoke

## docker-dev: interactive dev shell with full Rust toolchain
docker-dev:
	docker compose run --rm --build dev

## docker-clean: remove MAE Docker images and build cache
docker-clean:
	docker compose down --rmi local --volumes

## docs-tangle: tangle KB ADR nodes → docs/adr/ markdown (future: automated from KB)
docs-tangle:
	@echo "ADR docs in docs/adr/ — currently maintained manually."
	@echo "Future: automated tangle from KB concept:adr-* nodes."
	@ls docs/adr/*.md 2>/dev/null || echo "No ADR docs found."

## docs-tangle-check: verify docs/adr/ is present and non-empty (CI)
docs-tangle-check:
	@test -d docs/adr && test -n "$$(ls docs/adr/*.md 2>/dev/null)" || (echo "FAIL: docs/adr/ missing or empty" && exit 1)
	@echo "docs-tangle-check passed ✓"

## bench: run criterion benchmarks (buffer ops, CRDT ops)
bench:
	cargo bench --package mae-core --package mae-sync

## bench-save: save benchmark baseline for comparison
bench-save:
	cargo bench --package mae-core --package mae-sync -- --save-baseline main

## bench-compare: compare against saved baseline
bench-compare:
	cargo bench --package mae-core --package mae-sync -- --baseline main

## help: print this help
help:
	@echo "MAE build targets:"
	@grep -E '^##' Makefile | sed 's/## /  /'
