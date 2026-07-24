#!/bin/sh
# Verifier for docker-compose.headless-e2e.yml (ADR-055, Phase J / #385).
# Real assertions, no mocks: a real MCP handshake over the real stable
# socket, and a real check that a second racing instance was correctly
# refused rather than silently sharing/overwriting the first's socket.
set -e

mkdir -p /workspace/.git
cd /workspace

echo "=== Verifying headless MCP round trip ==="
SOCKET_PATH=$(mae --headless --print-socket-path)
echo "resolved stable socket path: $SOCKET_PATH"
if [ ! -S "$SOCKET_PATH" ]; then
  echo "FAIL: socket file not present at $SOCKET_PATH"
  exit 1
fi

MAE_MCP_SOCKET="$SOCKET_PATH" mae-mcp-shim --check
echo "OK: real MCP handshake (initialize -> notifications/initialized -> \$/ping) succeeded"

echo "=== Verifying two-instance collision safety ==="
if [ ! -f /result/race-exit-code ]; then
  echo "FAIL: /result/race-exit-code not written by headless-race"
  exit 1
fi
RACE_EXIT=$(sed -n 's/^exit code: //p' /result/race-exit-code)
echo "second instance's exit code was: ${RACE_EXIT:-<empty>}"
if [ "$RACE_EXIT" != "1" ]; then
  echo "FAIL: expected the second headless instance to exit 1 (AlreadyRunning), got: ${RACE_EXIT:-<empty>}"
  exit 1
fi
echo "OK: second instance correctly refused to share/overwrite the first instance's socket"

echo "=== All headless e2e checks passed ==="
