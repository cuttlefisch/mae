#!/usr/bin/env bash
# collab-membership-e2e.sh — two-editor end-to-end test of per-KB membership
# enforcement among trusted peers over mTLS (ADR-017 phase 4).
#
# Topology (single host, two isolated editor identities):
#   - daemon: key+tls, authorizes alice + bob (distinct labels).
#   - alice (owner): shares the KB, later adds bob as a member.
#   - bob: authorized to CONNECT, but NOT a KB member at first.
#
# Flow:
#   1. alice connects + shares KB 'default'  (members = [alice]).
#   2. bob connects + kb-joins              → DENIED (not a member).
#   3. alice :kb-member-add default bob.
#   4. bob kb-joins again                   → ALLOWED.
#
# Oracle = the daemon log: it must show a denied join for a non-member, a
# membership change, and no denial after bob is added. Coordination uses a
# shared /sync dir (single host) via the scheme test framework's file barriers.
#
# Env: MAE_BIN, MAE_DAEMON_BIN (defaults to debug), MAE_E2E_PORT (default 9477).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAE_BIN="${MAE_BIN:-$ROOT/target/debug/mae}"
MAE_DAEMON_BIN="${MAE_DAEMON_BIN:-$ROOT/daemon/target/debug/mae-daemon}"
PORT="${MAE_E2E_PORT:-9477}"
for bin in "$MAE_BIN" "$MAE_DAEMON_BIN"; do
  [ -x "$bin" ] || { echo "ERROR: missing binary: $bin"; exit 2; }
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/mae-member-e2e.XXXXXX")"
DAEMON_PID=""
PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null || true; done
  [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

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

for who in alice bob; do
  cat > "$WORK/$who/.config/mae/init.scm" <<'EOF'
(set-option! "collab-auth-mode" "key")
(set-option! "collab-host-key-policy" "accept-new")
EOF
done

cp "$ROOT/tests/collab-e2e/lib/test-helpers.scm" "$WORK/scen/helpers.scm"

# alice (owner): share, wait for bob's denied attempt, add bob, wait for bob's join.
cat > "$WORK/scen/alice.scm" <<EOF
(load "$WORK/scen/helpers.scm")
(describe-group "alice (owner)"
  (lambda ()
    (it-test "connects" (lambda () (wait-connected 30000)))
    (it-test "shares KB" (lambda () (run-command "kb-share") (sleep-ms 800)))
    (it-test "signals shared" (lambda () (write-file "$WORK/sync/shared" "1")))
    (it-test "waits for bob's denied join" (lambda () (wait-for-file "$WORK/sync/bob-tried" 60000)))
    (it-test "adds bob as member"
      (lambda () (execute-ex "kb-member-add default bob") (sleep-ms 800)))
    (it-test "signals added" (lambda () (write-file "$WORK/sync/added" "1")))
    (it-test "waits for bob's join" (lambda () (wait-for-file "$WORK/sync/bob-joined" 60000)))))
EOF

# bob: wait for share, attempt join (denied), wait for add, attempt join (allowed).
cat > "$WORK/scen/bob.scm" <<EOF
(load "$WORK/scen/helpers.scm")
(describe-group "bob (member candidate)"
  (lambda ()
    (it-test "connects" (lambda () (wait-connected 30000)))
    (it-test "waits for share" (lambda () (wait-for-file "$WORK/sync/shared" 60000)))
    (it-test "attempts join (expect denied)"
      (lambda () (execute-ex "kb-join default") (sleep-ms 800)))
    (it-test "signals tried" (lambda () (write-file "$WORK/sync/bob-tried" "1")))
    (it-test "waits for membership" (lambda () (wait-for-file "$WORK/sync/added" 60000)))
    (it-test "attempts join (expect allowed)"
      (lambda () (execute-ex "kb-join default") (sleep-ms 800)))
    (it-test "signals joined" (lambda () (write-file "$WORK/sync/bob-joined" "1")))))
EOF

# --- Start daemon ---
srv env MAE_LOG=info "$MAE_DAEMON_BIN" > "$WORK/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 20); do ss -tlnp 2>/dev/null | grep -q ":$PORT " && break; sleep 0.25; done
ss -tlnp 2>/dev/null | grep -q ":$PORT " || { echo "ERROR: daemon not listening"; cat "$WORK/daemon.log"; exit 1; }

run_editor() {
  local who="$1" scen="$2" log="$3"
  "$who" env MAE_COLLAB_SERVER="127.0.0.1:$PORT" MAE_COLLAB_AUTO_CONNECT=1 MAE_SKIP_WIZARD=1 \
    MAE_LOG="warn" timeout 120 "$MAE_BIN" --test "$scen" > "$log" 2>&1
}

# alice first (creates + shares), then bob.
run_editor alice "$WORK/scen/alice.scm" "$WORK/alice.tap" &
PIDS+=($!)
sleep 3
run_editor bob "$WORK/scen/bob.scm" "$WORK/bob.tap" &
PIDS+=($!)
wait "${PIDS[@]}" 2>/dev/null || true

echo "--- alice TAP ---"; grep -E '^(ok|not ok|#)' "$WORK/alice.tap" || true
echo "--- bob TAP ---";   grep -E '^(ok|not ok|#)' "$WORK/bob.tap" || true
echo "--- daemon membership events ---"
grep -iE 'kb/join denied|kb membership change|not a member' "$WORK/daemon.log" || true

# --- Verdict (strip ANSI from the daemon log first) ---
LOG="$WORK/daemon.clean.log"
sed 's/\x1b\[[0-9;]*m//g' "$WORK/daemon.log" > "$LOG"
denied=$(grep -c 'kb/join denied' "$LOG" || true)
changed=$(grep -c 'kb membership change' "$LOG" || true)
joined_after_add=$(awk '/kb membership change.*member=bob.*add=true/{seen=1} seen && /kb\/join/ && !/denied/{c++} END{print c+0}' "$LOG")
fail=0
[ "$denied" -ge 1 ] || { echo "FAIL: expected a denied join for the non-member (got $denied)"; fail=1; }
[ "$changed" -ge 1 ] || { echo "FAIL: expected a membership change (got $changed)"; fail=1; }
[ "$joined_after_add" -ge 1 ] || { echo "FAIL: bob's join after being added was not allowed"; fail=1; }
grep 'authenticated' "$LOG" | grep -q 'bob' || { echo "FAIL: bob never authenticated over mTLS"; fail=1; }
if grep -qE '^not ok' "$WORK/alice.tap" "$WORK/bob.tap"; then echo "FAIL: a scenario step failed"; fail=1; fi
[ "$fail" -eq 0 ] && echo "PASS: per-KB membership enforced (non-member denied → owner add → allowed)" || exit 1
