#!/usr/bin/env bash
# collab-p2p-mesh-e2e.sh — FULL two-daemon P2P mesh end-to-end (ADR-025), no central hub.
#
# STATUS: WIP scaffold — NOT yet wired into CI. VALIDATED so far: two daemons bind their iroh
# mesh endpoints with `relay = "disabled"` (direct localhost, no external infra — the key
# CI-viability question) and daemon B DIALS daemon A over the mesh. REMAINING: a mesh share is
# DAEMON-OWNED (`p2p/share_kb` seeds from the daemon's CozoDB KB store with owner = the daemon's
# identity), whereas this scaffold uses the editor's hub `kb-share` (editor-owned) — the
# ownership/transport mismatch means the join is rejected ("not shared over the P2P mesh"). The
# fix is to put content into the daemon's KB store via the HOSTED-KB model (daemon_mode=shared /
# the editor hosting its primary on the daemon) or a headless daemon ingest, THEN p2p/share_kb.
# Tracked for the next iteration (#200). The `--test` daemon-control wiring this needs IS landed.
#
# Two ISOLATED daemons mesh a KB over real iroh QUIC on localhost (relay disabled — direct,
# CI-friendly, no external infra). Alice's editor talks ONLY to daemon A; Bob's editor talks
# ONLY to daemon B; the two DAEMONS peer over iroh. We prove:
#   - daemon B DIALS daemon A over the mesh (node-id = the authorized Ed25519 peer key);
#   - a KB Alice authors on daemon A CONVERGES to Bob's store via the mesh (the canary lands
#     in Bob's KB store — pulled peer-to-peer, never through a shared hub);
#   - the unauthorized path stays closed (covered by the in-process gate tests).
#
# Usage:  scripts/collab-p2p-mesh-e2e.sh
# Env:    MAE_BIN, MAE_DAEMON_BIN, MAE_E2E_PORT (base; A=port, B=port+1), MAE_E2E_KEEP=1
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
PORTA="${MAE_E2E_PORT:-$(pick_port 9540)}"
PORTB="$(pick_port $((PORTA + 1)))"
EDITOR_TIMEOUT=90

for b in "$MAE_BIN" "$MAE_DAEMON_BIN"; do [ -x "$b" ] || { echo "ERROR: missing binary: $b"; exit 2; }; done

CANARY="MESH-CANARY-$$-$(od -An -N4 -tx4 /dev/urandom 2>/dev/null | tr -d ' ' || echo dead)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/mae-mesh-e2e.XXXXXX")"

# --- Isolation + reliable cleanup (same posture as collab-encrypted-e2e.sh) -------
HARNESS_PIDS=()
spawn() {
  local __var="$1" __log="$2"; shift 2; [ "${1:-}" = "--" ] && shift
  setsid "$@" >"$__log" 2>&1 &
  local pid=$!
  HARNESS_PIDS+=("$pid")
  printf '%s\n' "$pid" >>"$WORK/pids"
  printf -v "$__var" '%s' "$pid"
}
cleanup() {
  local rc=$? pid
  for pid in "${HARNESS_PIDS[@]:-}"; do [ -n "$pid" ] && { kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null; } || true; done
  sleep 0.3
  for pid in "${HARNESS_PIDS[@]:-}"; do [ -n "$pid" ] && { kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null; } || true; done
  if [ "${MAE_E2E_KEEP:-0}" = "1" ]; then echo "[harness] KEPT workdir=$WORK pids=(${HARNESS_PIDS[*]:-none})"; else rm -rf "$WORK"; fi
  return "$rc"
}
trap cleanup EXIT INT TERM
mkdir -p "$WORK"/{srvA/.config/mae,srvA/.local/share,srvA/data,srvB/.config/mae,srvB/.local/share,srvB/data,alice/.config/mae,alice/.local/share,bob/.config/mae,bob/.local/share,sync,scen}

srvA()  { HOME="$WORK/srvA"  XDG_CONFIG_HOME="$WORK/srvA/.config"  XDG_DATA_HOME="$WORK/srvA/.local/share"  "$@"; }
srvB()  { HOME="$WORK/srvB"  XDG_CONFIG_HOME="$WORK/srvB/.config"  XDG_DATA_HOME="$WORK/srvB/.local/share"  "$@"; }
alice() { HOME="$WORK/alice" XDG_CONFIG_HOME="$WORK/alice/.config" XDG_DATA_HOME="$WORK/alice/.local/share" "$@"; }
bob()   { HOME="$WORK/bob"   XDG_CONFIG_HOME="$WORK/bob/.config"   XDG_DATA_HOME="$WORK/bob/.local/share"   "$@"; }

# --- Daemon configs: key-mode auth + P2P mesh, relay DISABLED (direct localhost) ---
for d in A B; do
  eval "DIR=\$WORK/srv$d; PORT=\$PORT$d"
  cat > "$DIR/.config/mae/daemon.toml" <<EOF
socket = "$DIR/daemon.sock"
data_dir = "$DIR/data"
[collab]
bind = "127.0.0.1:$PORT"
[collab.auth]
mode = "key"
[collab.p2p]
enabled = true
relay = "disabled"
connection_gate = "authorized_keys"
EOF
done

# --- Identities: daemon A/B + editor alice/bob; extract every public key ---
DAK="$(srvA "$MAE_DAEMON_BIN" identity 2>/dev/null | sed -n 's/.*public key:  mae-ed25519 //p' | awk '{print $1}')"
DBK="$(srvB "$MAE_DAEMON_BIN" identity 2>/dev/null | sed -n 's/.*public key:  mae-ed25519 //p' | awk '{print $1}')"
AK="$(alice "$MAE_BIN" --collab-identity 2>/dev/null | sed -n 's/.*public key:  mae-ed25519 //p' | awk '{print $1}')"
BK="$(bob   "$MAE_BIN" --collab-identity 2>/dev/null | sed -n 's/.*public key:  mae-ed25519 //p' | awk '{print $1}')"
for k in "$DAK" "$DBK" "$AK" "$BK"; do [ -n "$k" ] || { echo "ERROR: could not read a public key"; exit 1; }; done

# Trust: daemon A trusts its editor (alice, over TCP) + daemon B (the mesh dialer B→A).
srvA "$MAE_DAEMON_BIN" authorize mae-ed25519 "$AK"  alice  >/dev/null
srvA "$MAE_DAEMON_BIN" authorize mae-ed25519 "$DBK" daemonB >/dev/null
# Daemon B trusts its editor (bob) + daemon A (mutual, for owner→joiner live pushes).
srvB "$MAE_DAEMON_BIN" authorize mae-ed25519 "$BK"  bob     >/dev/null
srvB "$MAE_DAEMON_BIN" authorize mae-ed25519 "$DAK" daemonA >/dev/null

# Editors: key-mode + each points its daemon_socket at its OWN daemon's control socket.
printf '(set-option! "collab-auth-mode" "key")\n(set-option! "collab-host-key-policy" "accept-new")\n(set-option! "daemon_socket" "%s")\n' "$WORK/srvA/daemon.sock" > "$WORK/alice/.config/mae/init.scm"
printf '(set-option! "collab-auth-mode" "key")\n(set-option! "collab-host-key-policy" "accept-new")\n(set-option! "daemon_socket" "%s")\n' "$WORK/srvB/daemon.sock" > "$WORK/bob/.config/mae/init.scm"
cp "$ROOT/tests/collab-e2e/lib/test-helpers.scm" "$WORK/scen/helpers.scm"

TICKET_FILE="$WORK/sync/ticket"

# Owner: register a KB on daemon A, push content (hub upload to A's store), set permissive
# so the mesh joiner auto-admits, then share over P2P + mint the join ticket.
cat > "$WORK/scen/alice.scm" <<EOF
(load "$WORK/scen/helpers.scm")
(describe-group "alice (mesh owner, daemon A)"
  (lambda ()
    (it-test "connects to daemon A" (lambda () (wait-connected 30000)))
    (it-test "registers KB" (lambda () (execute-ex "kb-register meshtest $ROOT/tests/fixtures/kb/collabtest") (sleep-ms 1000)))
    (it-test "shares (uploads content to daemon A's store)" (lambda () (execute-ex "kb-share meshtest") (sleep-ms 1500)))
    (it-test "edits a node with the canary" (lambda () (execute-ex "kb-update meshtest:alpha $CANARY") (sleep-ms 1500)))
    (it-test "permissive join policy (mesh joiner auto-admits)" (lambda () (execute-ex "kb-set-policy meshtest permissive") (sleep-ms 1200)))
    (it-test "shares over P2P + writes the join ticket" (lambda () (write-file "$TICKET_FILE" (kb-share-p2p "meshtest")) (sleep-ms 1000)))
    (it-test "signals shared" (lambda () (write-file "$WORK/sync/shared" "1")))
    (it-test "stays alive for the mesh pull + live sync" (lambda () (wait-for-file "$WORK/sync/bob-done" 80000)))))
EOF

# Joiner: on daemon B, join the ticket — daemon B dials daemon A over iroh, pulls, converges.
cat > "$WORK/scen/bob.scm" <<EOF
(load "$WORK/scen/helpers.scm")
(describe-group "bob (mesh joiner, daemon B)"
  (lambda ()
    (it-test "connects to daemon B" (lambda () (wait-connected 30000)))
    (it-test "waits for alice's ticket" (lambda () (wait-for-file "$WORK/sync/shared" 60000) (sleep-ms 300)))
    (it-test "joins via the P2P ticket (daemon B dials daemon A)" (lambda () (execute-ex (string-append "kb-join-p2p " (read-file "$TICKET_FILE"))) (sleep-ms 2000)))
    (it-test "waits for the mesh dial + pull (dialer polls ~10s)" (lambda () (sleep-ms 30000)))
    (it-test "loads the joined KB to materialize content" (lambda () (execute-ex "kb-load meshtest") (sleep-ms 3000)))
    (it-test "signals done" (lambda () (write-file "$WORK/sync/bob-done" "1")))))
EOF

# --- Spawn both daemons; wait for their TCP control listeners ---
spawn DPA "$WORK/daemonA.log" -- env HOME="$WORK/srvA" XDG_CONFIG_HOME="$WORK/srvA/.config" XDG_DATA_HOME="$WORK/srvA/.local/share" \
  MAE_LOG="${MAE_E2E_DAEMON_LOG:-mae_daemon=info,info}" "$MAE_DAEMON_BIN"
spawn DPB "$WORK/daemonB.log" -- env HOME="$WORK/srvB" XDG_CONFIG_HOME="$WORK/srvB/.config" XDG_DATA_HOME="$WORK/srvB/.local/share" \
  MAE_LOG="${MAE_E2E_DAEMON_LOG:-mae_daemon=info,info}" "$MAE_DAEMON_BIN"
echo "[harness] daemonA pgid=$DPA port=$PORTA | daemonB pgid=$DPB port=$PORTB | workdir=$WORK"
for _ in $(seq 1 40); do port_listening "$PORTA" && port_listening "$PORTB" && break; sleep 0.25; done
port_listening "$PORTA" || { echo "ERROR: daemon A never listened"; cat "$WORK/daemonA.log"; exit 1; }
port_listening "$PORTB" || { echo "ERROR: daemon B never listened"; cat "$WORK/daemonB.log"; exit 1; }

# --- Editors: each connects to its OWN daemon over TCP ---
spawn APID "$WORK/alice.tap" -- env HOME="$WORK/alice" XDG_CONFIG_HOME="$WORK/alice/.config" XDG_DATA_HOME="$WORK/alice/.local/share" \
  MAE_COLLAB_SERVER="127.0.0.1:$PORTA" MAE_COLLAB_AUTO_CONNECT=1 MAE_SKIP_WIZARD=1 MAE_LOG="${MAE_E2E_ALICE_LOG:-mae_mcp=warn,info}" \
  ${TIMEOUT_BIN:+$TIMEOUT_BIN $EDITOR_TIMEOUT} "$MAE_BIN" --test "$WORK/scen/alice.scm"
spawn BPID "$WORK/bob.tap" -- env HOME="$WORK/bob" XDG_CONFIG_HOME="$WORK/bob/.config" XDG_DATA_HOME="$WORK/bob/.local/share" \
  MAE_COLLAB_SERVER="127.0.0.1:$PORTB" MAE_COLLAB_AUTO_CONNECT=1 MAE_SKIP_WIZARD=1 MAE_LOG="${MAE_E2E_BOB_LOG:-mae_mcp=warn,info}" \
  ${TIMEOUT_BIN:+$TIMEOUT_BIN $EDITOR_TIMEOUT} "$MAE_BIN" --test "$WORK/scen/bob.scm"
echo "[harness] alice pgid=$APID bob pgid=$BPID"
wait "$APID" 2>/dev/null || true
wait "$BPID" 2>/dev/null || true

echo "--- alice TAP ---"; grep -E '^(ok|not ok|#)' "$WORK/alice.tap" | tail -8 || true
echo "--- bob TAP ---";   grep -E '^(ok|not ok|#)' "$WORK/bob.tap"   | tail -8 || true

fail=0
# (1) The mesh actually connected: daemon B dialed daemon A (node-id verified).
if grep -qaiE "dial|connected to peer|peer verified|pulling|anchored" "$WORK/daemonB.log" 2>/dev/null; then
  echo "PASS(mesh): daemon B engaged the mesh dialer toward daemon A"
else
  echo "FAIL(mesh): daemon B never dialed a peer — the mesh connection didn't establish"; fail=1
fi
# (2) NON-VACUITY: the owner authored the canary into its OWN store.
grep -rqaF "$CANARY" "$WORK/alice/.local/share" 2>/dev/null && echo "PASS(mesh): owner authored the canary" || { echo "FAIL(mesh): canary absent from owner's store"; fail=1; }
# (3) THE PROPERTY: the canary CONVERGED to Bob's store over the mesh (no hub between them).
if grep -rqaF "$CANARY" "$WORK/bob/.local/share" 2>/dev/null; then
  echo "PASS(mesh): canary CONVERGED to the joiner over P2P (pulled daemon→daemon, no hub)"
else
  echo "FAIL(mesh): canary did NOT reach the joiner — mesh convergence failed"; fail=1
fi

[ "$fail" -eq 0 ] && echo "PASS: P2P mesh two-daemon convergence" || echo "FAIL: P2P mesh two-daemon convergence"
exit $fail
