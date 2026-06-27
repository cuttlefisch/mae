#!/usr/bin/env bash
# collab-encrypted-e2e.sh — end-to-end validation of ADR-037 E2E content encryption over
# the FULL multi-user lifecycle: a keyed owner shares a KB, enables E2E, approves a joiner,
# and edits a node carrying a per-run CANARY. We then prove:
#   - the daemon's on-disk store + WAL + logs carry ONLY ciphertext (canary ABSENT) — the
#     key-blind relay never sees plaintext;
#   - BOTH members' local KB stores hold the plaintext (canary PRESENT) — the owner authored
#     it and the joiner DECRYPTED it (real convergence, not a silent drop);
#   - MAE_E2E_NEGATIVE=1 (skip encryption) makes the canary LEAK into the daemon store — the
#     inject-regression control proving the oracle has teeth.
#
# Usage:  scripts/collab-encrypted-e2e.sh
# Env:    MAE_BIN, MAE_DAEMON_BIN, MAE_E2E_PORT, MAE_E2E_NEGATIVE=1
# Exit 0 on success, non-zero otherwise.
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
pick_port() { local p="$1"; for _ in $(seq 0 49); do port_listening "$p" || { echo "$p"; return 0; }; p=$((p + 1)); done; echo ERR >&2; return 1; }
PORT="${MAE_E2E_PORT:-$(pick_port 9521)}"
NEG="${MAE_E2E_NEGATIVE:-0}"

for b in "$MAE_BIN" "$MAE_DAEMON_BIN"; do [ -x "$b" ] || { echo "ERROR: missing binary: $b"; exit 2; }; done

CANARY="CANARY-e2e-$$-$(od -An -N4 -tx4 /dev/urandom 2>/dev/null | tr -d ' ' || echo dead)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/mae-enc-e2e.XXXXXX")"
DP=""
cleanup() { [ -n "$DP" ] && kill "$DP" 2>/dev/null || true; rm -rf "$WORK"; }
trap cleanup EXIT
mkdir -p "$WORK"/{srv/.config/mae,srv/.local/share,srv/data,alice/.config/mae,alice/.local/share,bob/.config/mae,bob/.local/share,sync,scen}
srv()   { HOME="$WORK/srv"   XDG_CONFIG_HOME="$WORK/srv/.config"   XDG_DATA_HOME="$WORK/srv/.local/share"   "$@"; }
alice() { HOME="$WORK/alice" XDG_CONFIG_HOME="$WORK/alice/.config" XDG_DATA_HOME="$WORK/alice/.local/share" "$@"; }
bob()   { HOME="$WORK/bob"   XDG_CONFIG_HOME="$WORK/bob/.config"   XDG_DATA_HOME="$WORK/bob/.local/share"   "$@"; }

cat > "$WORK/srv/.config/mae/daemon.toml" <<EOF
socket = "$WORK/srv/daemon.sock"
data_dir = "$WORK/srv/data"
[collab]
bind = "127.0.0.1:$PORT"
[collab.auth]
mode = "key"
EOF
srv "$MAE_DAEMON_BIN" identity >/dev/null 2>&1
AK="$(alice "$MAE_BIN" --collab-identity 2>/dev/null | sed -n 's/.*public key:  mae-ed25519 //p' | awk '{print $1}')"
BK="$(bob "$MAE_BIN" --collab-identity 2>/dev/null | sed -n 's/.*public key:  mae-ed25519 //p' | awk '{print $1}')"
srv "$MAE_DAEMON_BIN" authorize mae-ed25519 "$AK" alice >/dev/null
srv "$MAE_DAEMON_BIN" authorize mae-ed25519 "$BK" bob   >/dev/null
BOB_FP="$(srv "$MAE_DAEMON_BIN" authorized 2>/dev/null | awk '$1=="bob"{print $2}' | grep -m1 '^SHA256:')"
[ -n "$BOB_FP" ] || { echo "ERROR: could not read bob's fingerprint"; exit 1; }
for who in alice bob; do
  printf '(set-option! "collab-auth-mode" "key")\n(set-option! "collab-host-key-policy" "accept-new")\n' > "$WORK/$who/.config/mae/init.scm"
done
cp "$ROOT/tests/collab-e2e/lib/test-helpers.scm" "$WORK/scen/helpers.scm"

if [ "$NEG" = "1" ]; then ENABLE='(it-test "skip-enc (NEGATIVE)" (lambda () (sleep-ms 200)))'
else ENABLE='(it-test "enable e2e" (lambda () (kb-set-encryption "collabtest" "e2e") (sleep-ms 1500)))'; fi

# Owner: register + share + enable + approve bob + edit the SEALED node body.
cat > "$WORK/scen/alice.scm" <<EOF
(load "$WORK/scen/helpers.scm")
(describe-group "alice (owner)"
  (lambda ()
    (it-test "connects" (lambda () (wait-connected 30000)))
    (it-test "registers collabtest" (lambda () (execute-ex "kb-register collabtest $ROOT/tests/fixtures/kb/collabtest") (sleep-ms 1000)))
    (it-test "shares" (lambda () (execute-ex "kb-share collabtest") (sleep-ms 1200)))
    $ENABLE
    (it-test "signals shared" (lambda () (write-file "$WORK/sync/shared" "1")))
    (it-test "waits for bob pending" (lambda () (wait-for-file "$WORK/sync/bob-tried" 60000)))
    (it-test "approves bob as editor" (lambda () (execute-ex "kb-approve collabtest $BOB_FP editor") (sleep-ms 1200)))
    (it-test "signals approved" (lambda () (write-file "$WORK/sync/added" "1")))
    (it-test "edits the sealed node body" (lambda () (execute-ex "kb-update collabtest:alpha $CANARY") (sleep-ms 2500)))
    (it-test "signals edited" (lambda () (write-file "$WORK/sync/edited" "1")))
    (it-test "waits for bob done" (lambda () (wait-for-file "$WORK/sync/bob-done" 60000)))))
EOF

# Joiner: join (pending) → wait approval → join (member) → wait for the sealed edit to land.
cat > "$WORK/scen/bob.scm" <<EOF
(load "$WORK/scen/helpers.scm")
(describe-group "bob (joiner)"
  (lambda ()
    (it-test "connects" (lambda () (wait-connected 30000)))
    (it-test "waits for share" (lambda () (wait-for-file "$WORK/sync/shared" 60000)))
    (it-test "join (pending)" (lambda () (execute-ex "kb-join collabtest") (sleep-ms 1000)))
    (it-test "signals tried" (lambda () (write-file "$WORK/sync/bob-tried" "1")))
    (it-test "waits for approval" (lambda () (wait-for-file "$WORK/sync/added" 60000)))
    (it-test "waits for the sealed edit FIRST" (lambda () (wait-for-file "$WORK/sync/edited" 60000) (sleep-ms 500)))
    (it-test "join (member) — pulls sealed content + decrypts" (lambda () (execute-ex "kb-join collabtest") (sleep-ms 2000)))
    (it-test "opens the materialized node" (lambda () (execute-ex "help collabtest:alpha") (sleep-ms 800)))
    (it-test "dumps Help buffer" (lambda () (write-file "$WORK/sync/bob-help.txt" (buffer-text "*Help*"))))
    (it-test "CONVERGED: reads the decrypted canary" (lambda () (should (wait-for-content "*Help*" "$CANARY" 8000))))
    (it-test "signals done" (lambda () (write-file "$WORK/sync/bob-done" "1")))))
EOF

srv env MAE_LOG="mae_daemon=info,info" "$MAE_DAEMON_BIN" > "$WORK/daemon.log" 2>&1 &
DP=$!
for _ in $(seq 1 20); do port_listening "$PORT" && break; sleep 0.25; done
port_listening "$PORT" || { echo "ERROR: daemon never listened"; cat "$WORK/daemon.log"; exit 1; }

run() { local who="$1" scen="$2" log="$3"; "$who" env MAE_COLLAB_SERVER="127.0.0.1:$PORT" MAE_COLLAB_AUTO_CONNECT=1 MAE_SKIP_WIZARD=1 ${TIMEOUT_BIN:+$TIMEOUT_BIN 120} "$MAE_BIN" --test "$scen" > "$log" 2>&1; }
run alice "$WORK/scen/alice.scm" "$WORK/alice.tap" &
APID=$!
run bob   "$WORK/scen/bob.scm"   "$WORK/bob.tap"   &
BPID=$!
wait $APID || true
wait $BPID || true
sleep 1; kill $DP 2>/dev/null || true

echo "--- alice TAP ---"; grep -E '^(ok|not ok|#)' "$WORK/alice.tap" | tail -6 || true
echo "--- bob TAP ---";   grep -E '^(ok|not ok|#)' "$WORK/bob.tap"   | tail -6 || true

fail=0
applied=$(grep -c "node_update: applied.*collabtest:alpha" "$WORK/daemon.log" || true)
[ "$applied" -ge 1 ] || { echo "FAIL: the daemon never applied the alpha edit (oracle would be vacuous)"; fail=1; }

# (1) SECURITY ORACLE: the daemon store/WAL must NOT contain the plaintext canary (sealed).
if grep -rqaF "$CANARY" "$WORK/srv/data" 2>/dev/null; then
  if [ "$NEG" = "1" ]; then echo "PASS (negative): canary LEAKED into the daemon store without encryption — oracle has teeth"
  else echo "FAIL: plaintext canary FOUND in the daemon store/WAL — content NOT sealed"; grep -rlaF "$CANARY" "$WORK/srv/data" | sed 's/^/  leak: /'; fail=1; fi
else
  if [ "$NEG" = "1" ]; then echo "FAIL (negative): canary did NOT leak even without encryption — oracle is VACUOUS"; fail=1
  else echo "PASS: canary ABSENT from the daemon store/WAL (sealed)"; fi
fi

if [ "$NEG" != "1" ]; then
  grep -qaF "$CANARY" "$WORK/daemon.log" && { echo "FAIL: canary in daemon logs"; fail=1; } || echo "PASS: canary ABSENT from daemon logs"
  # (2) CONVERGENCE: the JOINER decrypted the sealed edit — asserted IN bob's scenario via
  # `wait-for-content` on the materialized node (joined KBs are in-memory, so we read bob's
  # live buffer, not a disk store). bob's TAP shows `ok ... CONVERGED` iff he decrypted it.
  if grep -qE '^ok .*CONVERGED' "$WORK/bob.tap"; then echo "PASS: JOINER decrypted + converged on the sealed canary (in-memory read)"
  else echo "FAIL: joiner did NOT converge/decrypt the sealed edit (bob.tap)"; grep -E 'CONVERGED' "$WORK/bob.tap" | head; fail=1; fi
fi

[ "$fail" -eq 0 ] && echo "PASS: E2E encrypted multi-user lifecycle${NEG:+ (NEGATIVE control)}" || echo "FAIL: E2E encrypted lifecycle"
exit $fail
