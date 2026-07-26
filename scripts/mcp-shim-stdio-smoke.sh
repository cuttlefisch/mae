#!/usr/bin/env bash
# Generic-MCP-client smoke test for mae-mcp-shim (Phase B, ADR-050 D1/D3).
#
# Drives the shim exactly as any MCP host (VS Code, Zed, Cursor, a
# hand-rolled client) would over stdio: spawns the binary, exchanges
# newline-delimited JSON-RPC messages (the MCP stdio transport — distinct
# from the Content-Length framing used only on the shim's *socket* side to
# MAE, see shim.rs's own doc comment), and does
# initialize -> notifications/initialized -> tools/list -> tools/call.
# Read-only: never mutates editor state.
#
# This is what "smoke-tested against a generic host" means in this repo's
# context: mae-mcp-shim's stdio surface is host-agnostic by construction
# (ADR-046) -- a specific host's UI/chat behavior is out of scope for an
# automated check, but the wire protocol every host (including VS Code)
# depends on is fully exercised here.
#
# Usage:
#   scripts/mcp-shim-stdio-smoke.sh                    # auto-discover a live mae socket
#   MAE_MCP_SOCKET=/tmp/mae-1234.sock scripts/mcp-shim-stdio-smoke.sh
#   scripts/mcp-shim-stdio-smoke.sh /path/to/mae-mcp-shim
#
# Requires: python3, a running `mae`/`mae --headless` instance to connect to.

set -euo pipefail

SHIM_BIN="${1:-mae-mcp-shim}"
if ! command -v "$SHIM_BIN" >/dev/null 2>&1; then
    if [ -x "./target/release/mae-mcp-shim" ]; then
        SHIM_BIN="./target/release/mae-mcp-shim"
    else
        echo "error: '$SHIM_BIN' not found on PATH and no ./target/release/mae-mcp-shim built" >&2
        echo "  build it: cargo build --release -p mae-mcp --bin mae-mcp-shim" >&2
        exit 1
    fi
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "error: python3 is required to run this smoke test" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$SCRIPT_DIR/mcp-shim-stdio-smoke.py" "$SHIM_BIN"
