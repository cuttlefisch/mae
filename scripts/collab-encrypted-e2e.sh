#!/usr/bin/env bash
# collab-encrypted-e2e.sh — single-host end-to-end validation of ADR-037 E2E
# content encryption: a keyed owner editor shares a KB, enables E2E, plants a
# per-run CANARY in a node's (sealed) body, and we prove the daemon's on-disk
# store + WAL + logs contain ONLY ciphertext (canary ABSENT) while the editor's
# own KB holds the plaintext (canary PRESENT — the positive control that catches
# a silent content-drop). This is the locally-runnable core of the 3d docker gate.
#
# Usage: scripts/collab-encrypted-e2e.sh
# Env:   MAE_BIN, MAE_DAEMON_BIN, MAE_E2E_PORT (see collab-mtls-e2e.sh)
#        MAE_E2E_NEGATIVE=1 — inject-regression control (skip encryption; the canary MUST leak).
# Exit 0 on success, non-zero otherwise.
#
# >>> STATUS: WIP / BLOCKED — NOT wired into CI or the Makefile yet. <<<
# The oracle DESIGN is sound (the negative control proves teeth), but it is currently
# VACUOUS because the canary node edit does not reach the daemon store:
#   - #165: a node created in a named instance after share doesn't sync (owner=None gate).
#   - #166: editing an IMPORTED+shared node is rejected by the epoch fence (client-1 lineage).
# Resolve #165/#166 (a content path that actually syncs + is fence-clean) BEFORE trusting
# this script's PASS. The `:kb-update` ex command (the scriptable content primitive) is real
# and lands with this branch. See issue #153 for the full 3d plan.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAE_BIN="${MAE_BIN:-$ROOT/target/debug/mae}"
MAE_DAEMON_BIN="${MAE_DAEMON_BIN:-$ROOT/daemon/target/debug/mae-daemon}"

port_listening() {
  if command -v ss >/dev/null 2>&1; then ss -tln 2>/dev/null | grep -q ":$1 "
  elif command -v lsof >/dev/null 2>&1; then lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
  else netstat -an 2>/dev/null | grep -iE "[._:]$1[[:space:]].*listen" >/dev/null 2>&1; fi
}
TIMEOUT_BIN="$(command -v timeout || command -v gtimeout || true)"
port_free() { ! port_listening "$1"; }
pick_port() { local p="$1"; for _ in $(seq 0 49); do port_free "$p" && { echo "$p"; return 0; }; p=$((p + 1)); done; echo "ERR" >&2; return 1; }
if [ -n "${MAE_E2E_PORT:-}" ]; then PORT="$MAE_E2E_PORT"; else PORT="$(pick_port 9486)"; fi

for bin in "$MAE_BIN" "$MAE_DAEMON_BIN"; do
  [ -x "$bin" ] || { echo "ERROR: missing binary: $bin (build first)"; exit 2; }
done

# A per-run canary that cannot collide with fixture text or appear by chance.
CANARY="CANARY-e2e-$$-$(od -An -N4 -tx4 /dev/urandom 2>/dev/null | tr -d ' ' || echo deadbeef)"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/mae-enc-e2e.XXXXXX")"
DAEMON_PID=""
cleanup() { [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true; rm -rf "$WORK"; }
trap cleanup EXIT
mkdir -p "$WORK"/{srv/.config/mae,srv/.local/share,srv/data,alice/.config/mae,alice/.local/share,scen}
srv()   { HOME="$WORK/srv"   XDG_CONFIG_HOME="$WORK/srv/.config"   XDG_DATA_HOME="$WORK/srv/.local/share"   "$@"; }
alice() { HOME="$WORK/alice" XDG_CONFIG_HOME="$WORK/alice/.config" XDG_DATA_HOME="$WORK/alice/.local/share" "$@"; }

# --- Daemon config: key mode (mTLS — encryption requires authenticated peers) ---
cat > "$WORK/srv/.config/mae/daemon.toml" <<EOF
socket = "$WORK/srv/daemon.sock"
data_dir = "$WORK/srv/data"
[collab]
bind = "127.0.0.1:$PORT"
[collab.auth]
mode = "key"
EOF

# --- Identities: generate the owner's, authorize it on the daemon ---
srv "$MAE_DAEMON_BIN" identity >/dev/null 2>&1
A_KEY="$(alice "$MAE_BIN" --collab-identity 2>/dev/null | sed -n 's/.*public key:  mae-ed25519 //p' | awk '{print $1}')"
[ -n "$A_KEY" ] || { echo "ERROR: could not read owner identity"; exit 1; }
srv "$MAE_DAEMON_BIN" authorize mae-ed25519 "$A_KEY" alice >/dev/null

cat > "$WORK/alice/.config/mae/init.scm" <<'EOF'
(set-option! "collab-auth-mode" "key")
(set-option! "collab-host-key-policy" "accept-new")
EOF

cp "$ROOT/tests/collab-e2e/lib/test-helpers.scm" "$WORK/scen/helpers.scm"

# --- Owner scenario: share → enable E2E → plant the canary in a SEALED node body.
# MAE_E2E_NEGATIVE=1 OMITS the enable step (the inject-regression control): with
# no encryption the same canary MUST leak into the daemon store, proving the
# oracle has teeth (a real regression turns the gate RED, not silently green). ---
if [ "${MAE_E2E_NEGATIVE:-0}" = "1" ]; then
  ENABLE_STEP='(it-test "skips encryption (NEGATIVE control)" (lambda () (sleep-ms 200)))'
else
  ENABLE_STEP='(it-test "enables E2E encryption" (lambda () (kb-set-encryption "collabtest" "e2e") (sleep-ms 1500)))'
fi
cat > "$WORK/scen/alice.scm" <<EOF
(load "$WORK/scen/helpers.scm")
(describe-group "E2E owner seals content"
  (lambda ()
    (it-test "connects" (lambda () (wait-connected 30000)))
    (it-test "registers the collabtest fixture"
      (lambda () (execute-ex "kb-register collabtest $ROOT/tests/fixtures/kb/collabtest") (sleep-ms 1000)))
    (it-test "shares it" (lambda () (execute-ex "kb-share collabtest") (sleep-ms 1000)))
    $ENABLE_STEP
    (it-test "plants the canary in a node body (sealed after enable)"
      (lambda () (execute-ex "kb-update collabtest:alpha $CANARY") (sleep-ms 2500)))
    (it-test "signals done" (lambda () (write-file "$WORK/scen/done" "1")))))
EOF

# --- Start the daemon ---
srv env MAE_LOG="mae_daemon=debug,mae_sync=debug,info" "$MAE_DAEMON_BIN" > "$WORK/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 20); do port_listening "$PORT" && break; sleep 0.25; done
port_listening "$PORT" || { echo "ERROR: daemon never listened on $PORT"; cat "$WORK/daemon.log"; exit 1; }

# --- Run the owner scenario ---
set +e
alice env MAE_COLLAB_SERVER="127.0.0.1:$PORT" MAE_COLLAB_AUTO_CONNECT=1 MAE_SKIP_WIZARD=1 \
  ${TIMEOUT_BIN:+$TIMEOUT_BIN 90} "$MAE_BIN" --test "$WORK/scen/alice.scm" > "$WORK/alice.tap" 2> "$WORK/alice.log"
set -e
sleep 1  # let the final sealed node update flush to the daemon store

echo "--- TAP ---"; grep -E '^(ok|not ok|#)' "$WORK/alice.tap" || true

fail=0
# (0) The scenario itself must have succeeded.
grep -qE '# .*0 failed' "$WORK/alice.tap" || { echo "FAIL: scenario steps did not all pass"; fail=1; }

# (1) THE SECURITY ORACLE: the daemon's data dir (store + WAL) must NOT contain
#     the plaintext canary — only sealed ciphertext. In NEGATIVE mode the verdict
#     inverts: the canary MUST leak (else the oracle is toothless / vacuous).
if grep -rqaF "$CANARY" "$WORK/srv/data" 2>/dev/null; then
  if [ "${MAE_E2E_NEGATIVE:-0}" = "1" ]; then
    echo "PASS (negative control): canary LEAKED into the daemon store without encryption — oracle has teeth"
  else
    echo "FAIL: plaintext canary FOUND in the daemon store/WAL — content NOT sealed!"
    grep -rlaF "$CANARY" "$WORK/srv/data" 2>/dev/null | sed 's/^/  leaked in: /'
    fail=1
  fi
else
  if [ "${MAE_E2E_NEGATIVE:-0}" = "1" ]; then
    echo "FAIL (negative control): canary did NOT leak even without encryption — the oracle is VACUOUS (it would never catch a regression)"
    fail=1
  else
    echo "PASS: canary ABSENT from daemon store/WAL (sealed)"
  fi
fi
if [ "${MAE_E2E_NEGATIVE:-0}" != "1" ]; then
  # (1b) ...and not in the daemon logs either.
  if grep -qaF "$CANARY" "$WORK/daemon.log"; then
    echo "FAIL: plaintext canary FOUND in daemon logs"; fail=1
  else
    echo "PASS: canary ABSENT from daemon logs"
  fi

  # (2) POSITIVE CONTROL: the owner's OWN KB store DOES hold the plaintext canary —
  #     proving the edit really happened (a silent content-drop would fail HERE).
  if grep -rqaF "$CANARY" "$WORK/alice/.local/share" 2>/dev/null; then
    echo "PASS: canary PRESENT in the owner's local KB (edit landed)"
  else
    echo "FAIL: canary ABSENT from the owner's own KB — the edit never happened (silent drop)"
    fail=1
  fi

  # (3) Liveness: the daemon must have actually received a collabtest node update,
  #     else 'absent' is vacuous (nothing was ever sent).
  if grep -qiE "kb/node_update.*collabtest|node_update.*collabtest" "$WORK/daemon.log"; then
    echo "PASS: daemon received a collabtest node update (oracle is non-vacuous)"
  else
    echo "WARN: no explicit collabtest node_update log line — check liveness below"
    grep -iE "node_update|collection_op|kb/" "$WORK/daemon.log" | tail -5 || true
  fi
fi

if [ "$fail" -eq 0 ]; then echo "PASS: E2E content sealing e2e (owner${MAE_E2E_NEGATIVE:+, NEGATIVE control})"; else echo "FAIL: E2E content sealing e2e"; fi
exit $fail
