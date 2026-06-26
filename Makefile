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
#   make setup-hooks  — configure git to use .githooks/ (pre-commit fmt check)
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

.PHONY: all build build-tui dev install install-tui install-all install-upgrade uninstall run test test-tui check fmt fmt-check clippy clean clean-cache ci ci-extended ci-docker-e2e ci-complete audit setup-hooks setup-dev self-test check-config code-map code-map-check gen-fixtures doctor help docker-ci docker-new-user docker-smoke docker-dev docker-clean docs-tangle docs-tangle-check install-daemon install-daemon-service bench bench-save bench-compare manual-kb install-manual

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

## install: build release binary + manual KB, install to PREFIX, register desktop entry
install: build manual-kb
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
	@mkdir -p $(DATADIR)/applications
	@sed 's|Exec=mae|Exec=$(PREFIX)/$(BINARY)|' $(DESKTOP_FILE) > $(DATADIR)/applications/mae.desktop
	@sed 's|Exec=mae |Exec=$(PREFIX)/$(BINARY) |' assets/mae-connect.desktop > $(DATADIR)/applications/mae-connect.desktop
	@echo "Installed desktop entries -> $(DATADIR)/applications/mae*.desktop"
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
	@rm -f $(DATADIR)/applications/mae-connect.desktop
	@rm -f $(DATADIR)/icons/hicolor/scalable/apps/mae.svg
	@echo "Removed $(PREFIX)/$(BINARY)"
	@echo "Removed $(PREFIX)/$(SHIM_BINARY)"
	@echo "Removed $(PREFIX)/mae-daemon"
	@echo "Removed $(DATADIR)/applications/mae*.desktop"
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

## check: fast type-check without producing a binary
check:
	$(CARGO) check $(FEAT_FLAG)

## verify: check + test — single command for development validation
verify:
	@echo "=== Check (workspace + GUI) ==="
	$(CARGO) check $(FEAT_FLAG)
	@echo "=== Test ==="
	$(CARGO) test --workspace 2>&1 | tee /dev/stderr | grep "^test result:" | awk -F'[; ]' 'BEGIN{p=0;f=0} {p+=$$4;f+=$$7} END{printf "\n=== %d passed, %d failed ===\n",p,f}'

## fmt: format all Rust sources in place
fmt:
	$(CARGO) fmt

## fmt-check: check formatting without writing (useful in CI)
fmt-check:
	$(CARGO) fmt -- --check

## clippy: run linter across the whole workspace (matches CI + pre-commit hook)
clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

## clippy-daemon: run linter on daemon workspace
clippy-daemon:
	cd daemon && $(CARGO) clippy --all-targets -- -D warnings

## ci: run the full CI pipeline locally (fmt + clippy + check + test + scheme tests)
ci: fmt-check
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) check --workspace --all-targets
	$(CARGO) test --workspace
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
	git config core.hooksPath .githooks
	@echo "Git hooks configured to use .githooks/"

## setup-dev: install development dependencies for full self-test coverage
setup-dev:
	@scripts/setup-dev.sh

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

## clean-cache: reclaim regenerable compilation caches (both workspaces) WITHOUT
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

## test-collab-mtls-e2e: single-host trusted-peer mTLS e2e (real daemon + editor)
test-collab-mtls-e2e: build-tui build-daemon
	MAE_BIN=$(RELEASE_BIN) MAE_DAEMON_BIN=$(CURDIR)/daemon/target/release/mae-daemon \
		scripts/collab-mtls-e2e.sh

## test-collab-membership-e2e: two-editor per-KB membership enforcement e2e
test-collab-membership-e2e: build-tui build-daemon
	MAE_BIN=$(RELEASE_BIN) MAE_DAEMON_BIN=$(CURDIR)/daemon/target/release/mae-daemon \
		scripts/collab-membership-e2e.sh

## test-collab-e2e-all: all trusted-peer e2e tests (mTLS + membership)
test-collab-e2e-all: test-collab-mtls-e2e test-collab-membership-e2e

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
