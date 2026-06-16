#!/usr/bin/env bash
# collab-mtls-e2e.sh — single-host end-to-end test of the trusted-peer mTLS
# collab path (ADR-017): a real mae-daemon in `key`+`tls` mode + a real editor
# that connects over mTLS using only its Ed25519 identity, then shares a buffer
# and waits for the daemon to confirm the share.
#
# Proves: editor identity gen → admin authorize → mTLS handshake → TOFU pin →
# strict identity binding (daemon authenticates the peer by cert) → JSON-RPC
# initialize + collab-share over the encrypted channel.
#
# Usage:
#   scripts/collab-mtls-e2e.sh
# Env overrides:
#   MAE_BIN         path to the `mae` binary       (default: target/debug/mae)
#   MAE_DAEMON_BIN  path to the `mae-daemon` binary (default: daemon/target/debug/mae-daemon)
#   MAE_E2E_PORT    TCP port for the daemon (default: first free port from 9476;
#                   loopback-bound, so it never collides with a real daemon on
#                   9473 — but auto-skips any port already in use, e.g. another
#                   test run or a personal daemon on the default)
#
# Exit 0 on success (TAP "0 failed"), non-zero otherwise.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAE_BIN="${MAE_BIN:-$ROOT/target/debug/mae}"
MAE_DAEMON_BIN="${MAE_DAEMON_BIN:-$ROOT/daemon/target/debug/mae-daemon}"

# Portable TCP-listen probe + command timeout. Linux has ss + timeout; macOS has
# neither by default. Prefer ss (so Linux/CI behavior is unchanged), then fall
# back to lsof, then netstat. Timeout falls back to gtimeout, then to no timeout.
port_listening() {
  if command -v ss >/dev/null 2>&1; then ss -tln 2>/dev/null | grep -q ":$1 "
  elif command -v lsof >/dev/null 2>&1; then lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
  else netstat -an 2>/dev/null | grep -iE "[._:]$1[[:space:]].*listen" >/dev/null 2>&1; fi
}
# A timeout binary to prefix the editor run with. Used unquoted as
# ${TIMEOUT_BIN:+$TIMEOUT_BIN <secs>} so it expands to nothing when absent (macOS
# without coreutils) — must be a real binary because it runs through `env`.
TIMEOUT_BIN="$(command -v timeout || command -v gtimeout || true)"

# Pick a free TCP port. Honor an explicit MAE_E2E_PORT verbatim; otherwise scan
# upward from a base until nothing is listening — so a running daemon
# (default 9473) or a concurrent test run never causes "address already in use".
port_free() { ! port_listening "$1"; }
pick_port() {
  local p="$1"
  for _ in $(seq 0 49); do port_free "$p" && { echo "$p"; return 0; }; p=$((p + 1)); done
  echo "ERROR: no free port found near $1" >&2; return 1
}
if [ -n "${MAE_E2E_PORT:-}" ]; then PORT="$MAE_E2E_PORT"; else PORT="$(pick_port 9476)"; fi

for bin in "$MAE_BIN" "$MAE_DAEMON_BIN"; do
  [ -x "$bin" ] || { echo "ERROR: missing binary: $bin (build first)"; exit 2; }
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/mae-mtls-e2e.XXXXXX")"
DAEMON_PID=""
cleanup() {
  [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

mkdir -p "$WORK"/{srv/.config/mae,srv/.local/share,cli/.config/mae,cli/.local/share,scen,ws}
srv_env() { HOME="$WORK/srv" XDG_CONFIG_HOME="$WORK/srv/.config" XDG_DATA_HOME="$WORK/srv/.local/share" "$@"; }
cli_env() { HOME="$WORK/cli" XDG_CONFIG_HOME="$WORK/cli/.config" XDG_DATA_HOME="$WORK/cli/.local/share" "$@"; }

# --- Daemon config: key mode, mTLS (default) ---
cat > "$WORK/srv/.config/mae/daemon.toml" <<EOF
socket = "$WORK/srv/daemon.sock"
data_dir = "$WORK/srv/data"
[collab]
bind = "127.0.0.1:$PORT"
[collab.auth]
mode = "key"
EOF

# --- Identities: generate the editor's, authorize it on the daemon ---
srv_env "$MAE_DAEMON_BIN" identity >/dev/null 2>&1   # generate daemon identity
CLI_LINE="$(cli_env "$MAE_BIN" --collab-identity 2>/dev/null | sed -n 's/.*public key:  //p')"
[ -n "$CLI_LINE" ] || { echo "ERROR: could not read editor identity"; exit 1; }
srv_env "$MAE_DAEMON_BIN" authorize $CLI_LINE >/dev/null

# --- Editor config: key mode + accept-new TOFU (headless) ---
cat > "$WORK/cli/.config/mae/init.scm" <<'EOF'
(set-option! "collab-auth-mode" "key")
(set-option! "collab-host-key-policy" "accept-new")
EOF

# --- Scenario: connect over mTLS, share a buffer, await daemon confirmation ---
cp "$ROOT/tests/collab-e2e/lib/test-helpers.scm" "$WORK/scen/helpers.scm"
cat > "$WORK/scen/mtls.scm" <<EOF
(load "$WORK/scen/helpers.scm")
(describe-group "trusted-peer mTLS collab"
  (lambda ()
    (it-test "connects to daemon over mTLS"
      (lambda () (wait-connected 30000)))
    (it-test "status is reported"
      (lambda () (should (pair? (collab-status)))))
    (it-test "creates a file"
      (lambda () (write-file "$WORK/ws/notes.txt" "")))
    (it-test "opens it"
      (lambda () (open-file "$WORK/ws/notes.txt")))
    (it-test "edits + saves"
      (lambda ()
        (run-command "enter-insert-mode")
        (buffer-insert "trusted hello\n")
        (run-command "enter-normal-mode")
        (run-command "save")
        (sleep-ms 200)))
    (it-test "shares the buffer over mTLS"
      (lambda () (run-command "collab-share")))
    (it-test "daemon confirms the share"
      (lambda () (wait-synced "notes.txt" 30000)))))
EOF

# --- Start the daemon ---
srv_env env MAE_LOG=info "$MAE_DAEMON_BIN" > "$WORK/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 20); do
  port_listening "$PORT" && break
  sleep 0.25
done
port_listening "$PORT" || { echo "ERROR: daemon failed to listen on $PORT"; cat "$WORK/daemon.log"; exit 1; }
grep -q 'mTLS' "$WORK/daemon.log" || { echo "ERROR: daemon not in mTLS mode"; cat "$WORK/daemon.log"; exit 1; }

# --- Run the editor scenario over mTLS ---
set +e
cli_env env MAE_COLLAB_SERVER="127.0.0.1:$PORT" MAE_COLLAB_AUTO_CONNECT=1 MAE_SKIP_WIZARD=1 \
  ${TIMEOUT_BIN:+$TIMEOUT_BIN 90} "$MAE_BIN" --test "$WORK/scen/mtls.scm" > "$WORK/tap.out" 2> "$WORK/cli.log"
set -e

echo "--- TAP ---"
grep -E '^(ok|not ok|#|1\.\.)' "$WORK/tap.out" || true
echo "--- daemon auth ---"
grep -iE 'mTLS client authenticated|authenticated peer' "$WORK/daemon.log" | tail -2 || true

# --- Verdict: the daemon must have authenticated the peer, and no test failed ---
if ! grep -q 'mTLS client authenticated' "$WORK/daemon.log"; then
  echo "FAIL: daemon never authenticated an mTLS peer"; exit 1
fi
if grep -qE '^not ok' "$WORK/tap.out"; then
  echo "FAIL: a scenario step failed"; exit 1
fi
if ! grep -qE '# .*0 failed' "$WORK/tap.out"; then
  echo "FAIL: did not see a clean TAP summary"; exit 1
fi
echo "PASS: trusted-peer mTLS collab e2e"
