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

.PHONY: all build build-tui dev install install-tui uninstall run test check fmt fmt-check clippy clean ci setup-hooks self-test check-config help

# Default target: release build
all: build

## build: compile a release binary (optimised, no debug info)
build:
	$(CARGO) build --release $(FEAT_FLAG)

## build-tui: terminal-only release build (no skia dependency)
build-tui:
	$(CARGO) build --release

## dev: compile a debug binary (faster compile, includes debug info)
dev:
	$(CARGO) build $(FEAT_FLAG)

## install: build release binary, install to PREFIX, register desktop entry
install: build
	@mkdir -p $(PREFIX)
	@install -m 755 $(RELEASE_BIN) $(PREFIX)/$(BINARY)
	@install -m 755 $(RELEASE_SHIM) $(PREFIX)/$(SHIM_BINARY)
	@echo "Installed $(BINARY) -> $(PREFIX)/$(BINARY)"
	@echo "Installed $(SHIM_BINARY) -> $(PREFIX)/$(SHIM_BINARY)"
	@mkdir -p $(DATADIR)/applications
	@sed 's|Exec=mae|Exec=$(PREFIX)/$(BINARY)|' $(DESKTOP_FILE) > $(DATADIR)/applications/mae.desktop
	@echo "Installed desktop entry -> $(DATADIR)/applications/mae.desktop"
	@mkdir -p $(DATADIR)/icons/hicolor/scalable/apps
	@install -m 644 $(ICON_FILE) $(DATADIR)/icons/hicolor/scalable/apps/mae.svg
	@echo "Installed icon -> $(DATADIR)/icons/hicolor/scalable/apps/mae.svg"
	@if command -v update-desktop-database >/dev/null 2>&1; then \
		update-desktop-database $(DATADIR)/applications 2>/dev/null || true; \
	fi
	@if command -v gtk-update-icon-cache >/dev/null 2>&1; then \
		gtk-update-icon-cache -f -t $(DATADIR)/icons/hicolor 2>/dev/null || true; \
	fi

## install-tui: terminal-only install (no skia dependency)
install-tui: build-tui
	@mkdir -p $(PREFIX)
	@install -m 755 $(RELEASE_BIN) $(PREFIX)/$(BINARY)
	@install -m 755 $(RELEASE_SHIM) $(PREFIX)/$(SHIM_BINARY)
	@echo "Installed $(BINARY) -> $(PREFIX)/$(BINARY) (terminal-only)"
	@echo "Installed $(SHIM_BINARY) -> $(PREFIX)/$(SHIM_BINARY)"

## uninstall: remove installed binary, desktop entry, and icon
uninstall:
	@rm -f $(PREFIX)/$(BINARY)
	@rm -f $(PREFIX)/$(SHIM_BINARY)
	@rm -f $(DATADIR)/applications/mae.desktop
	@rm -f $(DATADIR)/icons/hicolor/scalable/apps/mae.svg
	@echo "Removed $(PREFIX)/$(BINARY)"
	@echo "Removed $(PREFIX)/$(SHIM_BINARY)"
	@echo "Removed $(DATADIR)/applications/mae.desktop"
	@echo "Removed $(DATADIR)/icons/hicolor/scalable/apps/mae.svg"
	@if command -v update-desktop-database >/dev/null 2>&1; then \
		update-desktop-database $(DATADIR)/applications 2>/dev/null || true; \
	fi

## run: dev build and run (pass extra arguments via ARGS=…)
run:
	$(CARGO) run $(FEAT_FLAG) -- $(ARGS)

## test: run all workspace tests
test:
	$(CARGO) test --workspace --exclude mae-gui
	$(CARGO) test -p mae $(FEAT_FLAG)

## check: fast type-check without producing a binary
check:
	$(CARGO) check $(FEAT_FLAG)

## fmt: format all Rust sources in place
fmt:
	$(CARGO) fmt

## fmt-check: check formatting without writing (useful in CI)
fmt-check:
	$(CARGO) fmt -- --check

## clippy: run linter across the whole workspace
clippy:
	$(CARGO) clippy $(FEAT_FLAG) -- -D warnings

## ci: run the full CI pipeline locally (fmt + clippy + check + test, excludes mae-gui)
ci: fmt-check
	$(CARGO) clippy --workspace --all-targets --exclude mae-gui -- -D warnings
	$(CARGO) check --workspace --all-targets --exclude mae-gui
	$(CARGO) test --workspace --exclude mae-gui
	@echo "CI passed ✓"

## setup-hooks: configure git to use version-controlled hooks
setup-hooks:
	git config core.hooksPath .githooks
	@echo "Git hooks configured to use .githooks/"

## check-config: validate init.scm + config.toml without launching the editor
check-config: build-tui
	$(RELEASE_BIN) --check-config

## self-test: run AI-driven e2e self-test headless (requires AI provider)
self-test: build
	$(RELEASE_BIN) --self-test $(CATS)

## clean: remove all build artefacts
clean:
	$(CARGO) clean

## help: print this help
help:
	@echo "MAE build targets:"
	@grep -E '^##' Makefile | sed 's/## /  /'
