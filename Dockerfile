# MAE — Modern AI Editor
# Multi-stage Dockerfile: base -> builder -> ci -> runtime
#
# Usage:
#   docker compose run --rm --build ci         # full CI pipeline
#   docker compose run --rm --build smoke      # quick binary smoke test
#   docker compose run --rm --build new-user   # clean-room first-run validation
#   docker compose run --rm --build dev        # interactive dev shell

# ---------------------------------------------------------------------------
# Stage: base — shared Rust toolchain + system deps
# ---------------------------------------------------------------------------
ARG RUST_VERSION=1.95
FROM rust:${RUST_VERSION}-slim-bookworm AS base

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config make git clang \
    libfontconfig1-dev libfreetype6-dev \
    ca-certificates \
  && rm -rf /var/lib/apt/lists/* \
  && rustup component add rustfmt clippy

WORKDIR /mae

# --- Dependency cache layer ---
# Copy only manifests + lock, create dummy sources, build deps.
# Source-only changes won't invalidate this ~5 min compile.
COPY Cargo.toml Cargo.lock ./
COPY crates/core/Cargo.toml crates/core/Cargo.toml
COPY crates/renderer/Cargo.toml crates/renderer/Cargo.toml
COPY crates/gui/Cargo.toml crates/gui/Cargo.toml
COPY crates/scheme/Cargo.toml crates/scheme/Cargo.toml
COPY crates/lsp/Cargo.toml crates/lsp/Cargo.toml
COPY crates/dap/Cargo.toml crates/dap/Cargo.toml
COPY crates/ai/Cargo.toml crates/ai/Cargo.toml
COPY crates/kb/Cargo.toml crates/kb/Cargo.toml
COPY crates/mae/Cargo.toml crates/mae/Cargo.toml
COPY crates/shell/Cargo.toml crates/shell/Cargo.toml
COPY crates/mcp/Cargo.toml crates/mcp/Cargo.toml
COPY test_fixtures/Cargo.toml test_fixtures/Cargo.toml

# Create dummy source files so cargo can resolve the dependency graph
RUN mkdir -p crates/core/src && echo "" > crates/core/src/lib.rs && \
    mkdir -p crates/renderer/src && echo "" > crates/renderer/src/lib.rs && \
    mkdir -p crates/gui/src && echo "" > crates/gui/src/lib.rs && \
    mkdir -p crates/scheme/src && echo "" > crates/scheme/src/lib.rs && \
    mkdir -p crates/lsp/src && echo "" > crates/lsp/src/lib.rs && \
    mkdir -p crates/dap/src && echo "" > crates/dap/src/lib.rs && \
    mkdir -p crates/ai/src && echo "" > crates/ai/src/lib.rs && \
    mkdir -p crates/kb/src && echo "" > crates/kb/src/lib.rs && \
    mkdir -p crates/mae/src && echo "fn main() {}" > crates/mae/src/main.rs && \
    mkdir -p crates/shell/src && echo "" > crates/shell/src/lib.rs && \
    mkdir -p crates/mcp/src && echo "" > crates/mcp/src/lib.rs && \
    echo "fn main() {}" > crates/mcp/src/shim.rs && \
    mkdir -p test_fixtures/src && echo "" > test_fixtures/src/lib.rs

# Build dependencies only (will fail on our dummy sources, but deps get cached)
RUN cargo build --release --workspace --exclude mae-gui --exclude mae-test-fixtures 2>/dev/null || true

# ---------------------------------------------------------------------------
# Stage: builder — full source compile
# ---------------------------------------------------------------------------
FROM base AS builder

# Copy real source (overwrites dummy stubs)
COPY . .

# Touch all source files so cargo knows they changed vs the dummy stubs
RUN find crates/ test_fixtures/ -name '*.rs' -exec touch {} +

RUN cargo build --release --workspace --exclude mae-gui --exclude mae-test-fixtures

# ---------------------------------------------------------------------------
# Stage: ci — lint + test (build failure = image build failure)
# ---------------------------------------------------------------------------
FROM builder AS ci

RUN cargo fmt --all --check
RUN cargo clippy --workspace --all-targets --exclude mae-gui --exclude mae-test-fixtures -- -D warnings
RUN cargo test --workspace --exclude mae-gui --exclude mae-test-fixtures

# No CMD — this stage exists only to validate. `docker compose build ci` IS the test.

# ---------------------------------------------------------------------------
# Stage: runtime — minimal image for running mae
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    git ca-certificates \
  && rm -rf /var/lib/apt/lists/*

# Non-root user (UID 1000 matches typical host user for volume mounts)
RUN useradd -m -u 1000 -s /bin/bash mae

# Pre-create XDG dirs
RUN mkdir -p /home/mae/.config/mae /home/mae/.local/share/mae /home/mae/.local/state/mae \
  && chown -R mae:mae /home/mae

COPY --from=builder /mae/target/release/mae /usr/local/bin/mae
COPY --from=builder /mae/target/release/mae-mcp-shim /usr/local/bin/mae-mcp-shim

# OCI labels
LABEL org.opencontainers.image.source="https://github.com/cuttlefisch/mae"
LABEL org.opencontainers.image.licenses="GPL-3.0-or-later"
LABEL org.opencontainers.image.description="MAE — Modern AI Editor"

USER mae
WORKDIR /home/mae

ENTRYPOINT ["mae"]
