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
#   make setup-hooks  — configure git to use .githooks/ (pre-commit fmt check)
#
# Configuration (override on the command line or in the environment):
#
#   PREFIX   — installation directory  (default: ~/.local/bin)
#   RELEASE  — 1 = build with --release (default: 1 for build/install)
#   CARGO    — cargo binary to use      (default: cargo)
#
# Examples:
#   make install PREFIX=/usr/local/bin
#   make install PREFIX=$$HOME/.cargo/bin
#   ANTHROPIC_API_KEY=sk-... make run ARGS=myfile.rs

PREFIX     ?= $(HOME)/.local/bin
CARGO      ?= cargo
BINARY     := mae
SHIM_BINARY := mae-mcp-shim
TARGET_DIR := target

RELEASE_BIN  := $(TARGET_DIR)/release/$(BINARY)
RELEASE_SHIM := $(TARGET_DIR)/release/$(SHIM_BINARY)
DEBUG_BIN    := $(TARGET_DIR)/debug/$(BINARY)

.PHONY: all build dev install uninstall run test check fmt fmt-check clippy clean ci setup-hooks help

# Default target: release build
all: build

## build: compile a release binary (optimised, no debug info)
build:
	$(CARGO) build --release

## dev: compile a debug binary (faster compile, includes debug info)
dev:
	$(CARGO) build

## install: build release binary and install to PREFIX
install: build
	@mkdir -p $(PREFIX)
	@install -m 755 $(RELEASE_BIN) $(PREFIX)/$(BINARY)
	@install -m 755 $(RELEASE_SHIM) $(PREFIX)/$(SHIM_BINARY)
	@echo "Installed $(BINARY) -> $(PREFIX)/$(BINARY)"
	@echo "Installed $(SHIM_BINARY) -> $(PREFIX)/$(SHIM_BINARY)"

## uninstall: remove the installed binary from PREFIX
uninstall:
	@rm -f $(PREFIX)/$(BINARY)
	@rm -f $(PREFIX)/$(SHIM_BINARY)
	@echo "Removed $(PREFIX)/$(BINARY)"
	@echo "Removed $(PREFIX)/$(SHIM_BINARY)"

## run: dev build and run (pass extra arguments via ARGS=…)
run: dev
	$(CARGO) run -- $(ARGS)

## test: run all workspace tests
test:
	$(CARGO) test

## check: fast type-check without producing a binary
check:
	$(CARGO) check

## fmt: format all Rust sources in place
fmt:
	$(CARGO) fmt

## fmt-check: check formatting without writing (useful in CI)
fmt-check:
	$(CARGO) fmt -- --check

## clippy: run linter across the whole workspace
clippy:
	$(CARGO) clippy -- -D warnings

## ci: run the full CI pipeline locally (fmt + clippy + check + test)
ci: fmt-check clippy check test
	@echo "CI passed ✓"

## setup-hooks: configure git to use version-controlled hooks
setup-hooks:
	git config core.hooksPath .githooks
	@echo "Git hooks configured to use .githooks/"

## clean: remove all build artefacts
clean:
	$(CARGO) clean

## help: print this help
help:
	@echo "MAE build targets:"
	@grep -E '^##' Makefile | sed 's/## /  /'
