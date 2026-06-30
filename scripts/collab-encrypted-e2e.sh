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
# MAE_E2E_REMOVAL=1 appends the ADR-037 §D3 removal+rotation phase to the same run (so the
# history-retention claim is grounded in content bob legitimately read first). After bob reads
# CANARY1 as a member, the owner REMOVES bob — which rotates the content key (fresh key wrapped
# only to remaining members) — and authors CANARY2 under the new key. bob stays connected, so
# the daemon (a generic broadcaster that does NOT re-filter subscribers by membership) still
# relays him the CANARY2 CIPHERTEXT; holding only the stranded old key, he cannot decrypt it.
# This proves §D3 end-to-end:
#   - FORWARD SECRECY (the attacker's test): CANARY2 plaintext NEVER materializes in bob's store
#     even though he receives the ciphertext — the rotated key strands him;
#   - HISTORY RETENTION (§D3's distinguishing property): bob KEEPS CANARY1 plaintext (removal
#     doesn't wipe what he legitimately decrypted with the old key);
#   - KEY-BLIND across rotation: CANARY2 plaintext is ABSENT from the daemon store/WAL/logs;
#   - NON-VACUITY: the daemon applied >=2 alpha edits and the owner CAN read CANARY2 — the
#     post-rotation content really flowed and is sealed, so bob's blindness is a real outcome.
# The crypto backstop itself (old key opens nothing post-rotation) is also unit-tested:
# op_set/kb `rotate_on_remove_rekeys_remaining_members_and_strands_the_removed_one`.
#
# Usage:  scripts/collab-encrypted-e2e.sh
# Env:    MAE_BIN, MAE_DAEMON_BIN, MAE_E2E_PORT, MAE_E2E_NEGATIVE=1, MAE_E2E_REMOVAL=1
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
REMOVAL="${MAE_E2E_REMOVAL:-0}"
ROTATE="${MAE_E2E_ROTATE:-0}"
# §D3 removal / ADR-040 rotation extend the run; give each editor more headroom.
EDITOR_TIMEOUT=35
{ [ "$REMOVAL" = "1" ] || [ "$ROTATE" = "1" ]; } && EDITOR_TIMEOUT=60
# These phases only make sense with encryption ON; refuse the nonsensical combos loudly.
if [ "$NEG" = "1" ] && { [ "$REMOVAL" = "1" ] || [ "$ROTATE" = "1" ]; }; then
  echo "ERROR: MAE_E2E_NEGATIVE=1 is incompatible with the removal/rotation phases (they need encryption)"; exit 2
fi
if [ "$REMOVAL" = "1" ] && [ "$ROTATE" = "1" ]; then
  echo "ERROR: run MAE_E2E_REMOVAL and MAE_E2E_ROTATE separately (they both extend the alpha node)"; exit 2
fi

for b in "$MAE_BIN" "$MAE_DAEMON_BIN"; do [ -x "$b" ] || { echo "ERROR: missing binary: $b"; exit 2; }; done

CANARY="CANARY-e2e-$$-$(od -An -N4 -tx4 /dev/urandom 2>/dev/null | tr -d ' ' || echo dead)"
# §D3: a DISTINCT post-rotation canary so the two phases can never alias each other.
CANARY2="CANARY2-postrot-$$-$(od -An -N4 -tx4 /dev/urandom 2>/dev/null | tr -d ' ' || echo dead)"
# ADR-040: a DISTINCT canary authored AFTER an owner identity rotation, by the NEW key.
CANARY3="CANARY3-postrotid-$$-$(od -An -N4 -tx4 /dev/urandom 2>/dev/null | tr -d ' ' || echo dead)"
# #171 purge: a DISTINCT canary written as PLAINTEXT *before* enable. The natural
# share→enable flow ships it to the key-blind daemon in the clear; reseal-on-enable must
# PURGE it (share_doc replace, not merge) so it does not survive at rest after encryption.
PRECANARY="PRECANARY-preenable-$$-$(od -An -N4 -tx4 /dev/urandom 2>/dev/null | tr -d ' ' || echo dead)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/mae-enc-e2e.XXXXXX")"

# --- Isolation + reliable cleanup -------------------------------------------------
# Every process we start runs in its OWN session (setsid → it is its own process-group
# leader), so cleanup can `kill -- -PGID` the whole group — a forked daemon child can't
# survive a parent kill. We track only OUR pids (in HARNESS_PIDS + $WORK/pids); we NEVER
# signal anything we didn't spawn (your `~/.local/bin/mae` + daemon are untouched). The
# trap fires on normal exit, `set -e` failure, and INT/TERM (incl. when this script is
# stopped as a background task). MAE_E2E_KEEP=1 preserves the workdir for debugging.
HARNESS_PIDS=()
# spawn VAR LOGFILE -- <cmd...> : start <cmd> in a new session, record its pgid in VAR.
spawn() {
  local __var="$1" __log="$2"; shift 2; [ "${1:-}" = "--" ] && shift
  setsid "$@" >"$__log" 2>&1 &
  local pid=$!
  HARNESS_PIDS+=("$pid")
  printf '%s\n' "$pid" >>"$WORK/pids"
  printf -v "$__var" '%s' "$pid"
}
cleanup() {
  local rc=$?
  local pid
  for pid in "${HARNESS_PIDS[@]:-}"; do [ -n "$pid" ] && { kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null; } || true; done
  sleep 0.3
  for pid in "${HARNESS_PIDS[@]:-}"; do [ -n "$pid" ] && { kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null; } || true; done
  if [ "${MAE_E2E_KEEP:-0}" = "1" ]; then
    echo "[harness] KEPT workdir=$WORK  pids=(${HARNESS_PIDS[*]:-none})"
  else
    rm -rf "$WORK"
  fi
  return "$rc"
}
trap cleanup EXIT INT TERM
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
# ADR-040 rotation: rotating the owner identity changes its per-node op-set client_id, so the
# first post-rotation edit to an existing node trips the ADR-023 epoch fence once ("rebase
# required") and must auto-re-author under the new client_id. Enable auto fence-resolution for
# the owner so a planned rotation can keep authoring without an interactive prompt.
if [ "$ROTATE" = "1" ]; then
  printf '(set-option! "collab-fence-resolution" "auto")\n' >> "$WORK/alice/.config/mae/init.scm"
fi
cp "$ROOT/tests/collab-e2e/lib/test-helpers.scm" "$WORK/scen/helpers.scm"

if [ "$NEG" = "1" ]; then ENABLE='(it-test "skip-enc (NEGATIVE)" (lambda () (sleep-ms 200)))'
else ENABLE='(it-test "enable e2e" (lambda () (kb-set-encryption "collabtest" "e2e") (sleep-ms 1500)))'; fi

# §D3 removal+rotation segments — empty unless MAE_E2E_REMOVAL=1, so the default run stays
# byte-identical to the lifecycle-only gate. $WORK/$BOB_FP/$CANARY2 expand HERE (build time),
# so the injected text carries concrete values and won't re-expand inside the scenario heredocs.
ALICE_REMOVAL=''
BOB_REMOVAL=''
if [ "$REMOVAL" = "1" ]; then
  ALICE_REMOVAL="    (it-test \"waits for bob to read CANARY1\" (lambda () (wait-for-file \"$WORK/sync/bob-got1\" 60000)))
    (it-test \"removes bob — rotates the content key (ADR-037 §D3)\" (lambda () (execute-ex \"kb-member-remove collabtest $BOB_FP\") (sleep-ms 2000)))
    (it-test \"edits the node under the ROTATED key\" (lambda () (execute-ex \"kb-update collabtest:alpha $CANARY2\") (sleep-ms 2500)))
    (it-test \"signals rotated+edited\" (lambda () (write-file \"$WORK/sync/rotated\" \"1\")))"
  BOB_REMOVAL="    (it-test \"signals read CANARY1\" (lambda () (write-file \"$WORK/sync/bob-got1\" \"1\")))
    (it-test \"waits for the post-rotation edit\" (lambda () (wait-for-file \"$WORK/sync/rotated\" 60000)))
    (it-test \"stays subscribed — absorbs the post-rotation broadcast it CANNOT decrypt\" (lambda () (sleep-ms 4000)))"
fi

# ADR-040 owner identity-rotation segments — empty unless MAE_E2E_ROTATE=1. After bob reads
# CANARY1 as a member, the owner ROTATES its identity key (collab-rotate-identity → Rebind +
# E2e content-key re-wrap to the NEW key, shipped owner-gated), then authors CANARY3 under the
# NEW key. bob (still a member) must converge on CANARY3 — proving the new key is a valid owner
# whose content the daemon accepts and a member decrypts, while the relay stays key-blind.
ALICE_ROTATE=''
BOB_ROTATE=''
if [ "$ROTATE" = "1" ]; then
  ALICE_ROTATE="    (it-test \"waits for bob to read CANARY1\" (lambda () (wait-for-file \"$WORK/sync/bob-got1\" 60000)))
    (it-test \"rotates the owner identity key (ADR-040)\" (lambda () (execute-ex \"collab-rotate-identity\") (sleep-ms 3500)))
    (it-test \"edits the node UNDER THE ROTATED key\" (lambda () (execute-ex \"kb-update collabtest:alpha $CANARY3\") (sleep-ms 3000)))
    (it-test \"signals rotated-id\" (lambda () (write-file \"$WORK/sync/rotid\" \"1\")))"
  BOB_ROTATE="    (it-test \"signals read CANARY1\" (lambda () (write-file \"$WORK/sync/bob-got1\" \"1\")))
    (it-test \"waits for the post-rotation edit\" (lambda () (wait-for-file \"$WORK/sync/rotid\" 60000)))
    (it-test \"re-joins to pull post-rotation content under the rotated owner (snapshot, like CANARY1)\" (lambda () (execute-ex \"kb-join collabtest\") (sleep-ms 3500)))"
fi

# Owner: register + share + enable + approve bob + edit the SEALED node body.
cat > "$WORK/scen/alice.scm" <<EOF
(load "$WORK/scen/helpers.scm")
(describe-group "alice (owner)"
  (lambda ()
    (it-test "connects" (lambda () (wait-connected 30000)))
    (it-test "registers collabtest" (lambda () (execute-ex "kb-register collabtest $ROOT/tests/fixtures/kb/collabtest") (sleep-ms 1000)))
    (it-test "shares" (lambda () (execute-ex "kb-share collabtest") (sleep-ms 1200)))
    (it-test "pre-enable PLAINTEXT edit (residual #171 fixture)" (lambda () (execute-ex "kb-update collabtest:alpha $PRECANARY") (sleep-ms 1500)))
    $ENABLE
    (it-test "signals shared" (lambda () (write-file "$WORK/sync/shared" "1")))
    (it-test "waits for bob pending" (lambda () (wait-for-file "$WORK/sync/bob-tried" 60000)))
    (it-test "approves bob as editor" (lambda () (execute-ex "kb-approve collabtest $BOB_FP editor") (sleep-ms 1200)))
    (it-test "signals approved" (lambda () (write-file "$WORK/sync/added" "1")))
    (it-test "edits the sealed node body" (lambda () (execute-ex "kb-update collabtest:alpha $CANARY") (sleep-ms 2500)))
    (it-test "signals edited" (lambda () (write-file "$WORK/sync/edited" "1")))
$ALICE_REMOVAL$ALICE_ROTATE
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
    (it-test "join (member) — pulls sealed content + decrypts to disk" (lambda () (execute-ex "kb-join collabtest") (sleep-ms 3000)))
$BOB_REMOVAL$BOB_ROTATE
    (it-test "signals done" (lambda () (write-file "$WORK/sync/bob-done" "1")))))
EOF

spawn DP "$WORK/daemon.log" -- env \
  HOME="$WORK/srv" XDG_CONFIG_HOME="$WORK/srv/.config" XDG_DATA_HOME="$WORK/srv/.local/share" \
  MAE_LOG="${MAE_E2E_DAEMON_LOG:-mae_daemon=info,info}" "$MAE_DAEMON_BIN"
echo "[harness] daemon pgid=$DP port=$PORT workdir=$WORK"
for _ in $(seq 1 40); do port_listening "$PORT" && break; sleep 0.25; done
port_listening "$PORT" || { echo "ERROR: daemon never listened"; cat "$WORK/daemon.log"; exit 1; }

# Editors: each in its own session (group-killable), TAP+logs merged to its file.
spawn APID "$WORK/alice.tap" -- env \
  HOME="$WORK/alice" XDG_CONFIG_HOME="$WORK/alice/.config" XDG_DATA_HOME="$WORK/alice/.local/share" \
  MAE_COLLAB_SERVER="127.0.0.1:$PORT" MAE_COLLAB_AUTO_CONNECT=1 MAE_SKIP_WIZARD=1 \
  MAE_LOG="${MAE_E2E_ALICE_LOG:-mae_mcp=warn,info}" \
  ${TIMEOUT_BIN:+$TIMEOUT_BIN $EDITOR_TIMEOUT} "$MAE_BIN" --test "$WORK/scen/alice.scm"
spawn BPID "$WORK/bob.tap" -- env \
  HOME="$WORK/bob" XDG_CONFIG_HOME="$WORK/bob/.config" XDG_DATA_HOME="$WORK/bob/.local/share" \
  MAE_COLLAB_SERVER="127.0.0.1:$PORT" MAE_COLLAB_AUTO_CONNECT=1 MAE_SKIP_WIZARD=1 \
  MAE_LOG="${MAE_E2E_BOB_LOG:-mae_mcp=warn,info}" \
  ${TIMEOUT_BIN:+$TIMEOUT_BIN $EDITOR_TIMEOUT} "$MAE_BIN" --test "$WORK/scen/bob.scm"
echo "[harness] alice pgid=$APID bob pgid=$BPID"
wait "$APID" 2>/dev/null || true
wait "$BPID" 2>/dev/null || true

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
  # (2) CONVERGENCE: the JOINER DECRYPTED the sealed snapshot on join. Joined KBs get a
  # durable CozoKbStore under the joiner's data dir, so the materialized PLAINTEXT lands on
  # disk — the canary is PRESENT in bob's KB store iff he decrypted it (was ciphertext-only
  # before the join-decrypt fix). Owner authored it (also present).
  grep -rqaF "$CANARY" "$WORK/alice/.local/share" 2>/dev/null && echo "PASS: canary PRESENT in OWNER's KB (authored)" || { echo "FAIL: canary absent from owner's KB"; fail=1; }
  if grep -rqaF "$CANARY" "$WORK/bob/.local/share" 2>/dev/null; then echo "PASS: canary PRESENT in JOINER's KB store (decrypted on join + converged)"
  else echo "FAIL: joiner did NOT decrypt the sealed snapshot — only ciphertext in bob's store"; fail=1; fi
  # (3) RESIDUAL #171 PURGE (the attacker's test): the PRE-enable plaintext canary was
  # shipped to the daemon in the clear, then enable RE-SEALED the node. With reseal-as-
  # REPLACE (share_doc + secure_delete) the daemon must NOT retain that plaintext at rest.
  # On the OLD merge path this FAILS (plaintext stacks under the op-set); the fix PURGES it.
  if grep -rqaF "$PRECANARY" "$WORK/srv/data" 2>/dev/null; then
    echo "FAIL(#171): PRE-enable plaintext canary STILL in the daemon store after enable — not purged"; grep -rlaF "$PRECANARY" "$WORK/srv/data" | sed 's/^/  residual: /'; fail=1
  else echo "PASS(#171): PRE-enable plaintext canary PURGED from the daemon store on enable"; fi
fi

# --- (3) ADR-037 §D3: removal rotates the key — removed member can't read NEW content,
# keeps history, relay stays key-blind. Only runs when MAE_E2E_REMOVAL=1 (⇒ encryption on).
if [ "$REMOVAL" = "1" ]; then
  echo "--- §D3 removal+rotation oracle ---"
  # NON-VACUITY: the post-rotation edit must have actually flowed (>=2 alpha applies: C1 + C2).
  applied2=$(grep -c "node_update: applied.*collabtest:alpha" "$WORK/daemon.log" || true)
  [ "$applied2" -ge 2 ] || { echo "FAIL(§D3): daemon applied <2 alpha edits ($applied2) — the post-rotation write never landed, oracle vacuous"; fail=1; }
  # ROTATION ACTUALLY RAN: the owner's bridge must have logged the §D3 re-key (not a silent skip).
  grep -qaF "§D3: removed member + rotated content key" "$WORK/alice.tap" && echo "PASS(§D3): owner rotated the content key on removal" || { echo "FAIL(§D3): owner never logged the §D3 rotation — removal didn't re-key"; fail=1; }
  # KEY-BLIND across rotation: CANARY2 plaintext must be sealed everywhere on the relay.
  if grep -rqaF "$CANARY2" "$WORK/srv/data" 2>/dev/null; then
    echo "FAIL(§D3): post-rotation canary FOUND in the daemon store/WAL — NOT sealed"; grep -rlaF "$CANARY2" "$WORK/srv/data" | sed 's/^/  leak: /'; fail=1
  else echo "PASS(§D3): post-rotation canary ABSENT from the daemon store/WAL (sealed)"; fi
  grep -qaF "$CANARY2" "$WORK/daemon.log" && { echo "FAIL(§D3): post-rotation canary in daemon logs"; fail=1; } || echo "PASS(§D3): post-rotation canary ABSENT from daemon logs"
  # OWNER reads post-rotation content (proves the new content is real + the new key works).
  grep -rqaF "$CANARY2" "$WORK/alice/.local/share" 2>/dev/null && echo "PASS(§D3): post-rotation canary PRESENT in OWNER's KB (authored under the new key)" || { echo "FAIL(§D3): owner can't read its own post-rotation content"; fail=1; }
  # FORWARD SECRECY (the attacker's test): bob received the ciphertext but, stranded on the old
  # key, must NEVER materialize CANARY2 plaintext.
  if grep -rqaF "$CANARY2" "$WORK/bob/.local/share" 2>/dev/null; then
    echo "FAIL(§D3): REMOVED member DECRYPTED post-rotation content — rotation did not strand the old key"; grep -rlaF "$CANARY2" "$WORK/bob/.local/share" | sed 's/^/  leak: /'; fail=1
  else echo "PASS(§D3): removed member could NOT read post-rotation content (forward secrecy)"; fi
  # HISTORY RETENTION (§D3's distinguishing property): bob KEEPS the pre-removal plaintext.
  grep -rqaF "$CANARY" "$WORK/bob/.local/share" 2>/dev/null && echo "PASS(§D3): removed member RETAINS pre-removal history (CANARY1)" || { echo "FAIL(§D3): removal wiped the member's legitimately-read history"; fail=1; }
fi

# --- ADR-040: owner identity rotation. The owner rotates its key, then authors CANARY3 under
# the NEW key; a still-member peer must converge on it, and the relay stays key-blind. Only
# runs when MAE_E2E_ROTATE=1 (⇒ encryption on).
if [ "$ROTATE" = "1" ]; then
  echo "--- ADR-040 owner identity-rotation oracle ---"
  # ROTATION ACTUALLY RAN: the owner's bridge logged the rotation (not a silent skip).
  # (PR2c-2 reworded this to "rotation shipped" — it now covers owner + member KBs.)
  grep -qaF "rotate-identity: rotation shipped" "$WORK/alice.tap" \
    && echo "PASS(rotid): owner shipped an identity rotation" \
    || { echo "FAIL(rotid): owner never shipped a rotation (the handler didn't run)"; fail=1; }
  # NON-VACUITY + NEW KEY IS A VALID AUTHOR: the post-rotation edit, signed by the NEW key,
  # landed on the daemon (>=2 alpha applies: CANARY1 under the old key + CANARY3 under the new).
  applied3=$(grep -c "node_update: applied.*collabtest:alpha" "$WORK/daemon.log" || true)
  [ "$applied3" -ge 2 ] || { echo "FAIL(rotid): <2 alpha edits ($applied3) — the post-rotation (new-key) write never landed; the daemon rejected the rotated owner's op or the oracle is vacuous"; fail=1; }
  # KEY-BLIND across the rotation: CANARY3 plaintext must be sealed everywhere on the relay.
  if grep -rqaF "$CANARY3" "$WORK/srv/data" 2>/dev/null; then
    echo "FAIL(rotid): post-rotation canary FOUND in the daemon store/WAL — NOT sealed"; grep -rlaF "$CANARY3" "$WORK/srv/data" | sed 's/^/  leak: /'; fail=1
  else echo "PASS(rotid): post-rotation canary ABSENT from the daemon store/WAL (sealed under the new key)"; fi
  grep -qaF "$CANARY3" "$WORK/daemon.log" && { echo "FAIL(rotid): post-rotation canary in daemon logs"; fail=1; } || echo "PASS(rotid): post-rotation canary ABSENT from daemon logs"
  # NEW KEY IS A VALID OWNER: the owner reads content it authored under the rotated key.
  grep -rqaF "$CANARY3" "$WORK/alice/.local/share" 2>/dev/null \
    && echo "PASS(rotid): owner reads content it authored under the ROTATED key" \
    || { echo "FAIL(rotid): owner can't read content authored under the rotated key"; fail=1; }
  # CONVERGENCE UNDER ROTATION (the real property): the still-member peer DECRYPTS the
  # post-rotation content — so the Rebind transferred ownership and the member tracked it.
  grep -rqaF "$CANARY3" "$WORK/bob/.local/share" 2>/dev/null \
    && echo "PASS(rotid): member CONVERGED on post-rotation content under the rotated owner" \
    || { echo "FAIL(rotid): member did NOT converge on the rotated owner's content"; fail=1; }
fi

suffix=""
[ "$NEG" = "1" ] && suffix=" (NEGATIVE control)"
[ "$REMOVAL" = "1" ] && suffix="$suffix + §D3 removal/rotation"
[ "$ROTATE" = "1" ] && suffix="$suffix + ADR-040 identity rotation"
[ "$fail" -eq 0 ] && echo "PASS: E2E encrypted multi-user lifecycle$suffix" || echo "FAIL: E2E encrypted lifecycle$suffix"
exit $fail
