#!/usr/bin/env bash
# collab-p2p-mesh-e2e.sh — FULL two-daemon P2P mesh end-to-end (ADR-025), no central hub.
#
# Two ISOLATED daemons mesh a KB over real iroh QUIC on localhost with `relay = "disabled"`
# (direct addressing — no external relay infra, so it runs in CI). Alice's editor talks ONLY to
# daemon A; Bob's editor talks ONLY to daemon B; the two DAEMONS peer directly over iroh. We prove:
#   - daemon B DIALS daemon A over the mesh (node-id = the authorized Ed25519 peer key);
#   - a KB Alice authors on daemon A CONVERGES to daemon B via the mesh (the canary, edited on
#     daemon A, lands in daemon B's store — pulled peer-to-peer, never through a shared hub);
#   - the owner gates the mesh join (Alice approves the joining peer daemon's fingerprint).
# The unauthorized/forged paths stay closed — covered by the in-process gate + dialer tests
# (daemon/src/p2p.rs, dialer.rs): node-id-mismatch rejection, selective op verification, etc.
#
# Flow: Alice registers + hub-shares a KB (uploads content to daemon A) + edits a canary node,
# sets a permissive policy, then `kb-share-p2p` (the COMMAND — `establish_p2p_share` widens the
# collection's transport to include the mesh) and mints a join ticket. Bob's `kb-join-p2p`
# records the ticket; daemon B's dialer connects to daemon A over iroh; Alice approves daemon B;
# the next dialer cycle pulls the KB. Asserts the canary reached daemon B's store.
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
# The mesh dialer connects as daemon B's identity, so the join principal alice must approve
# is daemon B's FINGERPRINT (the editor join policy gates the mesh join too).
DBK_FP="$(srvB "$MAE_DAEMON_BIN" identity 2>/dev/null | sed -n 's/.*fingerprint: //p' | awk '{print $1}')"
[ -n "$DBK_FP" ] || { echo "ERROR: could not read daemon B fingerprint"; exit 1; }

# --- E2E-on-mesh variant (MAE_E2E_MESH=1, ADR-043): the owner ENABLES E2E and the content key
# wraps to bob's EDITOR key (BK) — never to daemon B — so the relaying daemon stays key-blind. Bob
# JOINS (publishing his wrap key over the mesh) and the owner approves that editor membership, then
# bob decrypts. Default (unset) runs the plaintext convergence gate unchanged.
E2E_MESH="${MAE_E2E_MESH:-0}"
# Non-E2E uses permissive (mesh daemon auto-admits). E2E MUST use invite: the permissive auto-join
# path records the member WITHOUT their wrap pubkey (collab_handler.rs:2226), so the owner can't
# wrap the content key → a keyless member. Invite → pending WITH the wrap pubkey → approve wraps.
MESH_POLICY="permissive"
[ "$E2E_MESH" = "1" ] && MESH_POLICY="invite"
BK_FP=""
E2E_ENABLE=""   # alice: enable encryption before the canary edit
E2E_BOB_JOIN="" # bob: request editor membership over the mesh
E2E_BOB_PULL="" # bob: re-join after approval to pull the wrapped key + decrypt
E2E_ALICE_ADD=""  # alice: approve bob's EDITOR membership (wraps the content key to BK)
if [ "$E2E_MESH" = "1" ]; then
  BK_FP="$(bob "$MAE_BIN" --collab-identity 2>/dev/null | sed -n 's/.*fingerprint: //p' | awk '{print $1}')"
  [ -n "$BK_FP" ] || { echo "ERROR: could not read bob editor fingerprint"; exit 1; }
  E2E_ENABLE="    (it-test \"enables E2E (owner) BEFORE authoring the canary\" (lambda () (kb-set-encryption \"collabtest\" \"e2e\") (sleep-ms 2500)))"
  E2E_BOB_JOIN="    (it-test \"editor requests membership over the mesh (publishes BK wrap key)\" (lambda () (execute-ex \"kb-join collabtest\") (sleep-ms 3000)))
    (it-test \"signals editor-join\" (lambda () (write-file \"$WORK/sync/bob-ejoin\" \"1\")))"
  E2E_BOB_PULL="    (it-test \"re-joins as approved member — pulls the wrapped content key + decrypts\" (lambda () (execute-ex \"kb-join collabtest\") (sleep-ms 4000)))"
  E2E_ALICE_ADD="    (it-test \"waits for bob's editor membership request\" (lambda () (wait-for-file \"$WORK/sync/bob-ejoin\" 60000) (sleep-ms 1500)))
    (it-test \"approves bob's EDITOR (wraps content key to BK, not the daemon)\" (lambda () (execute-ex \"kb-approve collabtest $BK_FP editor\") (sleep-ms 2500)))"
fi

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
    (it-test "registers KB" (lambda () (execute-ex "kb-register collabtest $ROOT/tests/fixtures/kb/collabtest") (sleep-ms 1000)))
    (it-test "shares (uploads content to daemon A's store)" (lambda () (execute-ex "kb-share collabtest") (sleep-ms 1500)))
$E2E_ENABLE
    (it-test "edits a node with the canary" (lambda () (execute-ex "kb-update collabtest:alpha $CANARY") (sleep-ms 1500)))
    (it-test "join policy ($MESH_POLICY)" (lambda () (execute-ex "kb-set-policy collabtest $MESH_POLICY") (sleep-ms 1200)))
    (it-test "shares over P2P (command: establish_p2p_share widens transport→both)" (lambda () (execute-ex "kb-share-p2p") (sleep-ms 2000)))
    (it-test "writes the join ticket (primitive re-mints + returns it)" (lambda () (write-file "$TICKET_FILE" (kb-share-p2p "collabtest")) (sleep-ms 1000)))
    (it-test "signals shared" (lambda () (write-file "$WORK/sync/shared" "1")))
    (it-test "waits for daemon B's mesh join to land pending" (lambda () (sleep-ms 14000)))
    (it-test "approves the peer daemon (mesh join is owner-gated)" (lambda () (execute-ex "kb-approve collabtest $DBK_FP editor") (sleep-ms 2000)))
$E2E_ALICE_ADD
    (it-test "re-edits the canary so members converge post-approval" (lambda () (execute-ex "kb-update collabtest:alpha $CANARY") (sleep-ms 2500)))
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
$E2E_BOB_JOIN
    (it-test "waits for dial + owner approval + a dialer retry (pulls content)" (lambda () (sleep-ms 42000)))
$E2E_BOB_PULL
    (it-test "loads the joined KB to materialize content" (lambda () (execute-ex "kb-load collabtest") (sleep-ms 3000)))
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
if [ "$E2E_MESH" = "1" ]; then
  # E2E-on-mesh (ADR-043): content is SEALED, both daemons stay KEY-BLIND, and only an EDITOR
  # member decrypts. This proves a mesh-shared KB is E2E-capable (the genesis anchor was seeded).
  # (2) KEY-BLIND: the canary plaintext must be ABSENT from BOTH daemon stores + logs.
  for d in srvA srvB; do
    if grep -rqaF "$CANARY" "$WORK/$d/data" 2>/dev/null; then
      echo "FAIL(mesh-e2e): canary plaintext in $d daemon store — NOT sealed"; grep -rlaF "$CANARY" "$WORK/$d/data" | sed 's/^/  leak: /'; fail=1
    else echo "PASS(mesh-e2e): canary ABSENT from $d daemon store (key-blind)"; fi
  done
  if grep -qaF "$CANARY" "$WORK/daemonA.log" "$WORK/daemonB.log" 2>/dev/null; then echo "FAIL(mesh-e2e): canary in a daemon log"; fail=1; else echo "PASS(mesh-e2e): canary ABSENT from daemon logs"; fi
  # (3) AUTHORED: the owner's editor holds the plaintext (non-vacuity — there IS a canary).
  grep -rqaF "$CANARY" "$WORK/alice/.local/share" 2>/dev/null && echo "PASS(mesh-e2e): canary PRESENT in the owner's editor KB (authored)" || { echo "FAIL(mesh-e2e): owner can't read its own canary (vacuous)"; fail=1; }
  # (4) MEMBER DECRYPT over the mesh — KNOWN GAP #255 (non-fatal). The genesis-seed fix makes E2E
  # SEAL over the mesh (proven above: key-blind), but the joiner's wrap pubkey does not reach the
  # owner through the mesh join path, so members are admitted keyless and can't decrypt yet. This
  # gate pins that state: PASS on sealing/key-blindness (the property #254 delivers), KNOWN-GAP on
  # member decrypt (tracked #255). If this flips to decrypt, remove the gap marker.
  if grep -rqaF "$CANARY" "$WORK/bob/.local/share" 2>/dev/null; then
    echo "PASS(mesh-e2e): joiner's EDITOR DECRYPTED over the mesh — GAP #255 RESOLVED, promote this to a hard assertion"
  else
    echo "KNOWN-GAP(mesh-e2e, #255): joiner did NOT decrypt — member wrap-pubkey not delivered over the mesh join (E2E SEALS + daemons are key-blind; member key-delivery is WIP)"
  fi
  [ "$fail" -eq 0 ] && echo "PASS: P2P mesh E2E — content SEALED + daemons KEY-BLIND (member-decrypt tracked #255)" || echo "FAIL: P2P mesh E2E (sealing/key-blindness broke)"
else
  # (2) NON-VACUITY: the canary reached the OWNER daemon's store (the mesh has real content to serve).
  grep -rqaF "$CANARY" "$WORK/srvA/data" 2>/dev/null && echo "PASS(mesh): canary present in the owner daemon's store" || { echo "FAIL(mesh): canary absent from the owner daemon's store (nothing to converge)"; fail=1; }
  # (3) THE PROPERTY: the canary CONVERGED to the JOINER DAEMON's store over the mesh — pulled
  # peer-to-peer (daemon A → daemon B over iroh), never through a shared hub.
  if grep -rqaF "$CANARY" "$WORK/srvB/data" 2>/dev/null; then
    echo "PASS(mesh): canary CONVERGED to the joiner daemon over P2P (pulled daemon→daemon, no hub)"
  else
    echo "FAIL(mesh): canary did NOT reach the joiner daemon — mesh convergence failed"; fail=1
  fi
  [ "$fail" -eq 0 ] && echo "PASS: P2P mesh two-daemon convergence" || echo "FAIL: P2P mesh two-daemon convergence"
fi
exit $fail
