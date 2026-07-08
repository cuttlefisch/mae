#!/usr/bin/env bash
# collab-membership-e2e.sh — two-editor end-to-end test of identity-anchored KB
# access control over mTLS (ADR-018: principal = key fingerprint, roles, policy).
#
# Topology (single host, two isolated editor identities):
#   - daemon: key+tls, authorizes alice + bob (distinct labels → distinct keys).
#   - alice (owner): registers + shares the `collabtest` fixture KB.
#   - bob: authorized to CONNECT, but NOT a KB member.
#
# Flow (default join policy = invite):
#   1. alice shares `collabtest` → owner bound to alice's key fingerprint.
#   2. bob :kb-join collabtest             → PENDING (invite policy).
#   3. alice :kb-approve collabtest <bob-fingerprint> editor.
#   4. bob :kb-join collabtest again       → ALLOWED (now a member).
#
# Membership keys on the cryptographic PRINCIPAL (fingerprint), never the label —
# bob is approved by his fingerprint (captured from `mae-daemon authorized`).
# Oracle = the daemon log: kb/join: pending → kb/approve_member: complete →
# kb/join: complete. Coordination uses a shared /sync dir via file barriers.
#
# Env: MAE_BIN, MAE_DAEMON_BIN (defaults to debug). MAE_E2E_PORT pins the daemon
# port; if unset, the first free port from 9477 is auto-selected (loopback-bound,
# so it never collides with a real daemon on 9473, and auto-skips a busy port).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAE_BIN="${MAE_BIN:-$ROOT/target/debug/mae}"
MAE_DAEMON_BIN="${MAE_DAEMON_BIN:-$ROOT/daemon/target/debug/mae-daemon}"
# Portable TCP-listen probe + timeout. Linux has ss + timeout; macOS has neither
# by default. Prefer ss (Linux/CI behavior unchanged), then lsof, then netstat.
port_listening() {
  if command -v ss >/dev/null 2>&1; then ss -tln 2>/dev/null | grep -q ":$1 "
  elif command -v lsof >/dev/null 2>&1; then lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
  else netstat -an 2>/dev/null | grep -iE "[._:]$1[[:space:]].*listen" >/dev/null 2>&1; fi
}
TIMEOUT_BIN="$(command -v timeout || command -v gtimeout || true)"
port_free() { ! port_listening "$1"; }
pick_port() {
  local p="$1"
  for _ in $(seq 0 49); do port_free "$p" && { echo "$p"; return 0; }; p=$((p + 1)); done
  echo "ERROR: no free port found near $1" >&2; return 1
}
if [ -n "${MAE_E2E_PORT:-}" ]; then PORT="$MAE_E2E_PORT"; else PORT="$(pick_port 9477)"; fi
for bin in "$MAE_BIN" "$MAE_DAEMON_BIN"; do
  [ -x "$bin" ] || { echo "ERROR: missing binary: $bin"; exit 2; }
done

# --- Isolation + reliable cleanup (ADR-044): see scripts/lib/e2e-daemon-harness.sh.
# setsid process-group isolation, EXIT/INT/TERM trap, a kernel-enforced TTL that
# kills the daemon even if this script is SIGKILLed, and a pre-flight sweep that
# reaps orphans left by a past run of any collab-*-e2e.sh script.
source "$ROOT/scripts/lib/e2e-daemon-harness.sh"
harness_sweep_stale "mae-member-e2e.*" "mae-mtls-e2e.*" "mae-enc-e2e.*" "mae-mesh-e2e.*"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/mae-member-e2e.XXXXXX")"
DAEMON_PID=""
harness_trap_install

mkdir -p "$WORK"/{srv/.config/mae,srv/.local/share,sync,scen}
mkdir -p "$WORK/alice/.config/mae" "$WORK/alice/.local/share"
mkdir -p "$WORK/bob/.config/mae" "$WORK/bob/.local/share"

cat > "$WORK/srv/.config/mae/daemon.toml" <<EOF
socket = "$WORK/srv/daemon.sock"
data_dir = "$WORK/srv/data"
[collab]
bind = "127.0.0.1:$PORT"
[collab.auth]
mode = "key"
EOF

srv()   { HOME="$WORK/srv"   XDG_CONFIG_HOME="$WORK/srv/.config"   XDG_DATA_HOME="$WORK/srv/.local/share"   "$@"; }
alice() { HOME="$WORK/alice" XDG_CONFIG_HOME="$WORK/alice/.config" XDG_DATA_HOME="$WORK/alice/.local/share" "$@"; }
bob()   { HOME="$WORK/bob"   XDG_CONFIG_HOME="$WORK/bob/.config"   XDG_DATA_HOME="$WORK/bob/.local/share"   "$@"; }

# Generate identities; authorize each with a DISTINCT label (relabel the line).
srv "$MAE_DAEMON_BIN" identity >/dev/null 2>&1
A_KEY="$(alice "$MAE_BIN" --collab-identity 2>/dev/null | sed -n 's/.*public key:  mae-ed25519 //p' | awk '{print $1}')"
B_KEY="$(bob "$MAE_BIN" --collab-identity 2>/dev/null | sed -n 's/.*public key:  mae-ed25519 //p' | awk '{print $1}')"
[ -n "$A_KEY" ] && [ -n "$B_KEY" ] || { echo "ERROR: could not read editor identities"; exit 1; }
srv "$MAE_DAEMON_BIN" authorize mae-ed25519 "$A_KEY" alice >/dev/null
srv "$MAE_DAEMON_BIN" authorize mae-ed25519 "$B_KEY" bob   >/dev/null
# ADR-018: membership keys on the cryptographic PRINCIPAL (key fingerprint), not
# the label. Capture bob's fingerprint from the authorized list for the approve.
BOB_FP="$(srv "$MAE_DAEMON_BIN" authorized 2>/dev/null | awk '/[[:space:]]bob[[:space:]]/{print $2} /^  bob /{print $2}' | grep -m1 '^SHA256:')"
[ -n "$BOB_FP" ] || BOB_FP="$(srv "$MAE_DAEMON_BIN" authorized 2>/dev/null | awk '$1=="bob"{print $2}' | head -1)"
[ -n "$BOB_FP" ] || { echo "ERROR: could not read bob's fingerprint"; srv "$MAE_DAEMON_BIN" authorized; exit 1; }

for who in alice bob; do
  cat > "$WORK/$who/.config/mae/init.scm" <<'EOF'
(set-option! "collab-auth-mode" "key")
(set-option! "collab-host-key-policy" "accept-new")
EOF
done

cp "$ROOT/tests/collab-e2e/lib/test-helpers.scm" "$WORK/scen/helpers.scm"

# alice (owner): share (policy=invite), wait for bob's pending request, approve
# bob by FINGERPRINT, wait for his join.
cat > "$WORK/scen/alice.scm" <<EOF
(load "$WORK/scen/helpers.scm")
(describe-group "alice (owner)"
  (lambda ()
    (it-test "connects" (lambda () (wait-connected 30000)))
    (it-test "registers the collabtest fixture as a named instance"
      (lambda () (execute-ex "kb-register collabtest $ROOT/tests/fixtures/kb/collabtest") (sleep-ms 1000)))
    (it-test "shares the collabtest KB by name (owner bound to alice's key)"
      (lambda () (execute-ex "kb-share collabtest") (sleep-ms 800)))
    (it-test "signals shared" (lambda () (write-file "$WORK/sync/shared" "1")))
    (it-test "waits for bob's pending request" (lambda () (wait-for-file "$WORK/sync/bob-tried" 60000)))
    (it-test "approves bob by fingerprint as editor"
      (lambda () (execute-ex "kb-approve collabtest $BOB_FP editor") (sleep-ms 1000)))
    (it-test "signals approved" (lambda () (write-file "$WORK/sync/added" "1")))
    (it-test "waits for bob's join" (lambda () (wait-for-file "$WORK/sync/bob-joined" 60000)))))
EOF

# bob: request join (invite → pending), wait for approval, join (allowed).
cat > "$WORK/scen/bob.scm" <<EOF
(load "$WORK/scen/helpers.scm")
(describe-group "bob (member candidate)"
  (lambda ()
    (it-test "connects" (lambda () (wait-connected 30000)))
    (it-test "waits for share" (lambda () (wait-for-file "$WORK/sync/shared" 60000)))
    (it-test "requests join (invite policy -> pending)"
      (lambda () (execute-ex "kb-join collabtest") (sleep-ms 1000)))
    (it-test "signals tried" (lambda () (write-file "$WORK/sync/bob-tried" "1")))
    (it-test "waits for approval" (lambda () (wait-for-file "$WORK/sync/added" 60000)))
    (it-test "joins (now a member)"
      (lambda () (execute-ex "kb-join collabtest") (sleep-ms 1000)))
    (it-test "signals joined" (lambda () (write-file "$WORK/sync/bob-joined" "1")))))
EOF

# --- Start daemon --- (setsid'd + TTL-wrapped via harness_spawn_daemon, ADR-044;
# flattened to an inline `env` argv since setsid execs its argument and `srv` is a
# shell function, not something on PATH it could exec directly)
harness_spawn_daemon DAEMON_PID "$WORK/daemon.log" -- env \
  HOME="$WORK/srv" XDG_CONFIG_HOME="$WORK/srv/.config" XDG_DATA_HOME="$WORK/srv/.local/share" \
  MAE_LOG=info "$MAE_DAEMON_BIN"
for _ in $(seq 1 20); do port_listening "$PORT" && break; sleep 0.25; done
port_listening "$PORT" || { echo "ERROR: daemon not listening"; cat "$WORK/daemon.log"; exit 1; }

# alice first (creates + shares), then bob. Same inline-env flattening as the daemon
# above, each in its own session (group-killable) via harness_spawn.
harness_spawn APID "$WORK/alice.tap" -- env \
  HOME="$WORK/alice" XDG_CONFIG_HOME="$WORK/alice/.config" XDG_DATA_HOME="$WORK/alice/.local/share" \
  MAE_COLLAB_SERVER="127.0.0.1:$PORT" MAE_COLLAB_AUTO_CONNECT=1 MAE_SKIP_WIZARD=1 MAE_LOG="warn" \
  ${TIMEOUT_BIN:+$TIMEOUT_BIN 120} "$MAE_BIN" --test "$WORK/scen/alice.scm"
sleep 3
harness_spawn BPID "$WORK/bob.tap" -- env \
  HOME="$WORK/bob" XDG_CONFIG_HOME="$WORK/bob/.config" XDG_DATA_HOME="$WORK/bob/.local/share" \
  MAE_COLLAB_SERVER="127.0.0.1:$PORT" MAE_COLLAB_AUTO_CONNECT=1 MAE_SKIP_WIZARD=1 MAE_LOG="warn" \
  ${TIMEOUT_BIN:+$TIMEOUT_BIN 120} "$MAE_BIN" --test "$WORK/scen/bob.scm"
wait "$APID" 2>/dev/null || true
wait "$BPID" 2>/dev/null || true

echo "--- alice TAP ---"; grep -E '^(ok|not ok|#)' "$WORK/alice.tap" || true
echo "--- bob TAP ---";   grep -E '^(ok|not ok|#)' "$WORK/bob.tap" || true
echo "--- daemon membership events ---"
grep -iE 'kb/join: pending|kb/approve_member: complete|kb/join: complete' "$WORK/daemon.log" || true

# --- Verdict (strip ANSI from the daemon log first) ---
LOG="$WORK/daemon.clean.log"
sed 's/\x1b\[[0-9;]*m//g' "$WORK/daemon.log" > "$LOG"
# ADR-018 invite flow, keyed on daemon acceptance lines for `collabtest`:
#   bob's join → pending; owner approves; bob's next join → complete (member).
pending=$(grep -cE 'kb/join: pending.*collabtest' "$LOG" || true)
approved=$(grep -cE 'kb/approve_member: complete.*collabtest' "$LOG" || true)
joined_after_approve=$(awk '/kb\/approve_member: complete.*collabtest/{seen=1} seen && /kb\/join: complete.*collabtest/{c++} END{print c+0}' "$LOG")
fail=0
[ "$pending" -ge 1 ] || { echo "FAIL: bob's invite join was not recorded pending (got $pending)"; fail=1; }
[ "$approved" -ge 1 ] || { echo "FAIL: owner approval not seen (got $approved)"; fail=1; }
[ "$joined_after_approve" -ge 1 ] || { echo "FAIL: bob's join after approval was not allowed"; fail=1; }
grep 'authenticated' "$LOG" | grep -q 'bob' || { echo "FAIL: bob never authenticated over mTLS"; fail=1; }
if grep -qE '^not ok' "$WORK/alice.tap" "$WORK/bob.tap"; then echo "FAIL: a scenario step failed"; fail=1; fi
[ "$fail" -eq 0 ] && echo "PASS: ADR-018 invite flow (join → pending → owner approve → join allowed)" || exit 1
