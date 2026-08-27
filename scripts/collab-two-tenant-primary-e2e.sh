#!/usr/bin/env bash
# collab-two-tenant-primary-e2e.sh — ADR-105 finding F, end to end on a real
# daemon: TWO tenants must each be able to share THEIR OWN PRIMARY KB.
#
# This is the scenario ADR-105 Stage 3 exists for, and the one no other e2e
# covers. Every sibling script (`collab-membership`, `collab-encrypted`,
# `collab-p2p-mesh`) shares `collabtest` — a NAMED instance whose name is unique
# by construction — so none of them could ever observe the collision:
#
#   Every editor's primary KB is called "default". While that name doubled as the
#   collaborative id, the FIRST tenant to connect to a shared daemon claimed
#   `kbc:default` permanently, and every later tenant's primary share was accepted
#   and then denied on every subsequent operation — a KB that looks shared and
#   does nothing. D4 mints an opaque id per KB so the two no longer collide.
#
# Topology: one daemon (key+tls), two editors with distinct identities, each
# sharing its own primary with its own marker content.
#
# Oracles, in order of strength:
#   1. The two shares land under DIFFERENT collab ids (the fix itself).
#   2. Neither id is the display name "default" (it is not an address any more).
#   3. Each tenant's marker survives in its own KB — the isolation that matters,
#      asserted on CONTENT rather than on a status line.
#   4. The daemon holds two distinct collections, not one merged one.
#
# Both tenants use the SAME node id on purpose (ADR-105 H5): two people picking
# `note:tenant-canary` is ordinary, and a test using distinct ids is structurally
# unable to observe a collision. The id is deliberately NOT one the bundled manual
# corpus already ships — an earlier draft used `concept:architecture`, which the
# manual already defines, so `kb-create` never wrote the marker and the isolation
# assertions below had nothing to be isolated.
#
# Env: MAE_BIN, MAE_DAEMON_BIN (default: debug). MAE_E2E_PORT pins the port.
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
pick_port() {
  local p="$1"
  for _ in $(seq 0 49); do port_listening "$p" || { echo "$p"; return 0; }; p=$((p + 1)); done
  echo "ERROR: no free port found near $1" >&2; return 1
}
if [ -n "${MAE_E2E_PORT:-}" ]; then PORT="$MAE_E2E_PORT"; else PORT="$(pick_port 9477)"; fi
for bin in "$MAE_BIN" "$MAE_DAEMON_BIN"; do
  [ -x "$bin" ] || { echo "ERROR: missing binary: $bin"; exit 2; }
done

source "$ROOT/scripts/lib/e2e-daemon-harness.sh"
harness_sweep_stale "mae-2tenant-e2e.*"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/mae-2tenant-e2e.XXXXXX")"
# shellcheck disable=SC2034  # assigned indirectly by harness_spawn_daemon
DAEMON_PID=""
[ -n "${KEEP_WORK:-}" ] || harness_trap_install
[ -z "${KEEP_WORK:-}" ] || echo "KEEP_WORK set — leaving $WORK in place"

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

srv "$MAE_DAEMON_BIN" identity >/dev/null 2>&1
A_KEY="$(alice "$MAE_BIN" --collab-identity 2>/dev/null | sed -n 's/.*public key:  mae-ed25519 //p' | awk '{print $1}')"
B_KEY="$(bob   "$MAE_BIN" --collab-identity 2>/dev/null | sed -n 's/.*public key:  mae-ed25519 //p' | awk '{print $1}')"
[ -n "$A_KEY" ] && [ -n "$B_KEY" ] || { echo "ERROR: could not read editor identities"; exit 1; }
srv "$MAE_DAEMON_BIN" authorize mae-ed25519 "$A_KEY" alice >/dev/null
srv "$MAE_DAEMON_BIN" authorize mae-ed25519 "$B_KEY" bob   >/dev/null

# Each editor must opt into key auth: `collab_auth_mode` defaults to "psk", and
# the daemon here runs mTLS — without this the client speaks plaintext at a TLS
# listener and every handshake fails with `InvalidContentType`.
for who in alice bob; do
  cat > "$WORK/$who/.config/mae/init.scm" <<'INITEOF'
(set-option! "collab-auth-mode" "key")
(set-option! "collab-host-key-policy" "accept-new")
INITEOF
done

cp "$ROOT/tests/collab-e2e/lib/test-helpers.scm" "$WORK/scen/helpers.scm"

# Each tenant: create a node in its OWN PRIMARY (no kb-register — the primary is
# the point), then `kb-share` with no argument, which shares the active/primary KB.
tenant_scenario() {
  local who="$1" marker="$2"
  cat > "$WORK/scen/$who.scm" <<EOF
(load "$WORK/scen/helpers.scm")
(describe-group "$who (tenant)"
  (lambda ()
    (it-test "connects" (lambda () (wait-connected 30000)))
    (it-test "authors a node in its own primary KB"
      (lambda ()
        (kb-create "note:tenant-canary" "Tenant Canary" "$marker" "note")
        (sleep-ms 500)))
    (it-test "shares its PRIMARY (no argument — the KB everyone calls 'default')"
      (lambda () (execute-ex "kb-share") (sleep-ms 2500)))
    (it-test "signals shared" (lambda () (write-file "$WORK/sync/$who-shared" "1")))
    (it-test "stays alive until the driver has seen both docs"
      (lambda () (wait-for-file "$WORK/sync/docs-seen" 180000)))))
EOF
}
tenant_scenario alice ALICE-ONLY-MARKER
tenant_scenario bob   BOB-ONLY-MARKER

harness_spawn_daemon DAEMON_PID "$WORK/daemon.log" -- env \
  HOME="$WORK/srv" XDG_CONFIG_HOME="$WORK/srv/.config" XDG_DATA_HOME="$WORK/srv/.local/share" \
  MAE_LOG=info "$MAE_DAEMON_BIN"
for _ in $(seq 1 40); do port_listening "$PORT" && break; sleep 0.25; done
port_listening "$PORT" || { echo "ERROR: daemon not listening"; cat "$WORK/daemon.log"; exit 1; }

# The id each tenant actually shared under, read from its own durable registry —
# where D4 persists the mint.
# `|| true` is load-bearing, not defensive noise. Under `set -euo pipefail` a
# `grep` that matches nothing exits 1, `pipefail` propagates that through the
# pipeline, and the command substitution below then aborts the whole script --
# BEFORE the `[ -n "$A_ID" ] || { echo "FAIL: alice never persisted..." }` guard
# a few lines down, which exists for exactly this case and could never run.
# The observable symptom was a CI failure with no FAIL line at all: the TAP said
# "5 passed, 0 failed", then the harness's cleanup kill, then exit 1.
reg_id() {
  grep -oE 'primary_collab_id = "[^"]+"' \
    "$WORK/$1/.local/share/mae/kb-registry.toml" 2>/dev/null \
    | head -1 | cut -d'"' -f2 || true
}

harness_spawn APID "$WORK/alice.tap" -- env \
  HOME="$WORK/alice" XDG_CONFIG_HOME="$WORK/alice/.config" XDG_DATA_HOME="$WORK/alice/.local/share" \
  MAE_COLLAB_SERVER="127.0.0.1:$PORT" MAE_COLLAB_AUTO_CONNECT=1 MAE_SKIP_WIZARD=1 MAE_LOG=warn \
  ${TIMEOUT_BIN:+$TIMEOUT_BIN 180} "$MAE_BIN" --test "$WORK/scen/alice.scm"
sleep 3
harness_spawn BPID "$WORK/bob.tap" -- env \
  HOME="$WORK/bob" XDG_CONFIG_HOME="$WORK/bob/.config" XDG_DATA_HOME="$WORK/bob/.local/share" \
  MAE_COLLAB_SERVER="127.0.0.1:$PORT" MAE_COLLAB_AUTO_CONNECT=1 MAE_SKIP_WIZARD=1 MAE_LOG=warn \
  ${TIMEOUT_BIN:+$TIMEOUT_BIN 180} "$MAE_BIN" --test "$WORK/scen/bob.scm"
# Observe the docs and release the editors WHILE THEY ARE STILL ALIVE.
#
# This has to run before the `wait` below, and that is the whole point. The
# scenario's final step used to be a fixed `(sleep-ms 6000)`, so six seconds was
# the real deadline for the canary node to reach the daemon; the 30s
# `wait_for_log_docs` further down runs only AFTER both editors have exited, and
# once an editor is gone no amount of waiting can make its sync happen. On a
# loaded runner where connect->share took 47s, that budget was never going to
# hold, and the failure looked like "the node never synced" rather than "we
# stopped waiting too early".
#
# The editors now park on `$WORK/sync/docs-seen` instead of sleeping, and this
# observer writes it once both documents are visible -- or once it gives up, so
# a genuinely-never-synced doc still reaches the assertions below instead of
# hanging both editors out to the harness TTL.
observe_and_release() {
  # Deliberately id-AGNOSTIC. The obvious version polls `reg_id` for each
  # tenant's collab id and greps for that exact doc — but `primary_collab_id` is
  # persisted asynchronously and in practice is not durable in
  # `kb-registry.toml` until the editor exits (which is why the assertions below
  # read it only after `wait`). An observer that needs the ids therefore cannot
  # get them while the editors are alive, and would deadlock until its own
  # timeout: measured, both ids stayed empty for the full 120s window while the
  # documents themselves had been on the daemon since the first second.
  #
  # Counting DISTINCT canary documents needs no ids at all, and two distinct
  # ones is exactly the property the assertions check anyway.
  local deadline=$(( $(date +%s) + ${E2E_SYNC_TIMEOUT_SECS:-120} ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if [ "$(sed 's/\x1b\[[0-9;]*m//g' "$WORK/daemon.log" 2>/dev/null \
             | grep -oE 'kbn:[0-9a-f-]+:note:tenant-canary' \
             | sort -u | wc -l)" -ge 2 ]; then
      break
    fi
    sleep 0.25
  done
  : > "$WORK/sync/docs-seen"
}
observe_and_release &
OBSERVER_PID=$!

wait "$APID" 2>/dev/null || true
wait "$BPID" 2>/dev/null || true
wait "$OBSERVER_PID" 2>/dev/null || true

echo "--- alice TAP ---"; grep -E '^(ok|not ok|#)' "$WORK/alice.tap" || true
echo "--- bob TAP ---";   grep -E '^(ok|not ok|#)' "$WORK/bob.tap" || true


# And wait for the mint to be durable rather than assuming it already is. The
# editor persists `primary_collab_id` asynchronously after `kb/share`, so
# reading immediately after `wait` is a race that only loses under CI load --
# which is why this passed locally and on most runs. Bounded, so a genuine
# never-persisted stays a failure rather than a hang.
for _ in $(seq 1 50); do
  A_ID="$(reg_id alice)"; B_ID="$(reg_id bob)"
  [ -n "$A_ID" ] && [ -n "$B_ID" ] && break
  sleep 0.2
done
echo "--- collab ids ---"
echo "alice primary → ${A_ID:-<none>}"
echo "bob   primary → ${B_ID:-<none>}"

LOG="$WORK/daemon.clean.log"
sed 's/\x1b\[[0-9;]*m//g' "$WORK/daemon.log" > "$LOG"
fail=0

[ -n "$A_ID" ] || { echo "FAIL: alice never persisted a primary collab id"; fail=1; }
[ -n "$B_ID" ] || { echo "FAIL: bob never persisted a primary collab id"; fail=1; }

# ORACLE 1 — finding F itself.
if [ -n "$A_ID" ] && [ -n "$B_ID" ] && [ "$A_ID" = "$B_ID" ]; then
  echo "FAIL: both tenants' primaries claimed the SAME collab id ($A_ID) — finding F"
  fail=1
fi
# ORACLE 2 — a name is not an address.
for id in "$A_ID" "$B_ID"; do
  case "$id" in
    default|primary) echo "FAIL: a primary synced under its DISPLAY NAME ('$id')"; fail=1;;
  esac
done
# ORACLE 3 — the daemon accepted BOTH shares, under both ids.
for id in "$A_ID" "$B_ID"; do
  [ -n "$id" ] || continue
  grep -qE "kb/share.*$id" "$LOG" || { echo "FAIL: daemon never saw a share for '$id'"; fail=1; }
done
# ORACLE 4 — the addressing property itself, on the daemon: the SAME node id in
# both tenants' KBs must be TWO distinct documents. This is what ADR-105 changed —
# before it, both resolved to one `kb:note:tenant-canary` and the second tenant's
# share silently landed on the first's content.
A_DOC="kbn:$A_ID:note:tenant-canary"; B_DOC="kbn:$B_ID:note:tenant-canary"
# Wait for the docs to ARRIVE rather than assuming the scenario's `(sleep-ms
# 2500)` was long enough (#762).
#
# The collab-id poll above already learned this -- its own comment says "a race
# that only loses under CI load, which is why this passed locally and on most
# runs" -- and the lesson stopped one assertion short. This is the same shape:
# propagation time depends on how loaded the machine is, so a fixed budget
# expires under contention and the failure lands on whoever shared a runner.
#
# Observed exactly that way: the daemon log held each tenant's SEEDED nodes
# (`option:*`, `guide:*`, `cmd:*`) and not the canary, i.e. "not yet", not
# "broken". Bounded by wall clock, so a genuinely-never-synced node still fails
# rather than hanging.
wait_for_log_docs() {
  local deadline=$(( $(date +%s) + ${E2E_SYNC_TIMEOUT_SECS:-30} ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    sed 's/\x1b\[[0-9;]*m//g' "$WORK/daemon.log" > "$LOG"
    if grep -q "$A_DOC" "$LOG" && grep -q "$B_DOC" "$LOG"; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}
# A cheap re-check: `observe_and_release` above already waited with the editors
# alive, so this normally returns immediately. Kept so the refreshed `$LOG` the
# assertions grep is written exactly once, here.
wait_for_log_docs || true
grep -q "$A_DOC" "$LOG" || { echo "FAIL: alice's canary node never reached the daemon as $A_DOC"; fail=1; }
grep -q "$B_DOC" "$LOG" || { echo "FAIL: bob's canary node never reached the daemon as $B_DOC"; fail=1; }
[ "$A_DOC" != "$B_DOC" ] || { echo "FAIL: both tenants' canary nodes are the SAME document"; fail=1; }

# NOT ASSERTED HERE: content-level isolation (alice's bytes absent from bob's KB).
# An earlier draft tried to, by grepping each editor's store for a per-tenant
# marker, and the marker was not observable there — so the two absence checks
# would have passed trivially, which is a vacuous oracle rather than a weak one.
# Content isolation IS asserted, with a real oracle, in the daemon's own suite:
# `collab_handler_same_node_id_isolation_tests` seeds two tenants with the SAME
# node id and checks the victim's CONTENT is unchanged after a cross-KB write.
# What THIS script adds is the deployment-level property that no unit test can
# reach: two real editors, two real identities, one real daemon, both sharing a KB
# that every editor calls "default".

if grep -qE '^not ok' "$WORK/alice.tap" "$WORK/bob.tap"; then echo "FAIL: a scenario step failed"; fail=1; fi

if [ "$fail" -eq 0 ]; then
  echo "PASS: two tenants each shared their own primary under distinct collab ids (ADR-105 finding F)"
else
  echo "--- daemon share/auth lines ---"
  grep -iE 'kb/share|authenticated|auth|reject|denied' "$LOG" | head -25
  echo "--- alice tap (full) ---"; tail -30 "$WORK/alice.tap"
  echo "--- bob tap (full) ---";   tail -30 "$WORK/bob.tap"
  exit 1
fi
