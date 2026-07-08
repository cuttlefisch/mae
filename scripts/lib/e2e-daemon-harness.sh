#!/usr/bin/env bash
# scripts/lib/e2e-daemon-harness.sh — shared SIGKILL-resilient daemon lifecycle
# harness for the collab-*-e2e.sh scripts. Source, don't execute:
#   source "$ROOT/scripts/lib/e2e-daemon-harness.sh"
#
# SCOPE: e2e-TEST-SPAWNED daemons only. Has nothing to do with, and must never
# be used for, the production on-demand-daemon path in
# crates/mae/src/daemon_supervisor.rs — that daemon is deliberately long-lived
# (outlives the spawning editor by design; see ADR-035) and must NOT get a TTL.
#
# Why this exists: a bash `trap` cannot catch SIGKILL. If the parent script is
# force-killed (IDE stop button, closed terminal/SSH session, a hard-cancelled
# CI job), the trap never runs and a spawned mae-daemon is orphaned indefinitely
# (observed on a real dev machine: leaked for a full day). This library adds two
# backstops that don't depend on any trap/Drop running at all, on top of (not
# instead of) the existing trap-based cleanup, which stays as the fast, quiet
# common-path cleanup:
#   1. harness_spawn_daemon: every daemon launch is wrapped in `timeout -k`, a
#      kernel-enforced dead-man's switch that fires even if this shell process
#      no longer exists.
#   2. harness_sweep_stale: a pre-flight sweep run at the TOP of every e2e
#      script, before it starts anything new, that reaps daemons + workdirs
#      orphaned by a PAST run of ANY of the four scripts.
#
# See docs/adr/044-e2e-daemon-lifecycle-safety.md for the full design writeup
# and the TTL/sweep-cutoff derivation.

set -uo pipefail

: "${MAE_E2E_DAEMON_TTL_SECONDS:=600}"   # dead-man's switch; see ADR-044
: "${MAE_E2E_DAEMON_TTL_GRACE:=10}"      # SIGTERM->SIGKILL grace once TTL fires
: "${MAE_E2E_KEEP:=0}"                   # 1 = keep $WORK for debugging
: "${MAE_E2E_SWEEP:=1}"                  # 0 = disable the pre-flight sweep
: "${MAE_E2E_SWEEP_AGE_SECONDS:=$((2 * MAE_E2E_DAEMON_TTL_SECONDS))}"

HARNESS_PIDS=()
HARNESS_TTL_BIN="$(command -v timeout || command -v gtimeout || true)"

# harness_spawn VAR LOGFILE -- cmd...
# Starts cmd in its OWN session (setsid) so it's its own process-group leader -
# a forked grandchild can't survive a parent-directed kill. Records the pid in
# $VAR, HARNESS_PIDS, and (if $WORK is set) $WORK/pids.
harness_spawn() {
  local __var="$1" __log="$2"; shift 2; [ "${1:-}" = "--" ] && shift
  setsid "$@" >"$__log" 2>&1 &
  local pid=$!
  HARNESS_PIDS+=("$pid")
  [ -n "${WORK:-}" ] && printf '%s\n' "$pid" >>"$WORK/pids"
  printf -v "$__var" '%s' "$pid"
}

# harness_spawn_daemon VAR LOGFILE -- daemon-cmd...
# Same as harness_spawn, but wraps the command in `timeout -k GRACE TTL` - the
# dead-man's switch. This fires from the KERNEL side, independent of this
# script's own process tree, so it survives this shell being SIGKILLed.
harness_spawn_daemon() {
  local __var="$1" __log="$2"; shift 2; [ "${1:-}" = "--" ] && shift
  if [ -n "$HARNESS_TTL_BIN" ]; then
    harness_spawn "$__var" "$__log" -- \
      "$HARNESS_TTL_BIN" -k "${MAE_E2E_DAEMON_TTL_GRACE}s" "${MAE_E2E_DAEMON_TTL_SECONDS}s" "$@"
  else
    echo "WARN: no timeout/gtimeout found - daemon TTL dead-man's-switch DISABLED" >&2
    harness_spawn "$__var" "$__log" -- "$@"
  fi
}

# harness_cleanup — SIGTERM the group, brief grace, SIGKILL the group. This is
# still useful (fast, quiet cleanup on the common path); it is the FIRST line
# of defense, not the only one — the TTL above and the sweep below are what
# cover the case where this never runs at all.
harness_cleanup() {
  local rc=$? pid
  for pid in "${HARNESS_PIDS[@]:-}"; do
    [ -n "$pid" ] && { kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null; } || true
  done
  sleep 0.3
  for pid in "${HARNESS_PIDS[@]:-}"; do
    [ -n "$pid" ] && { kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null; } || true
  done
  if [ -n "${WORK:-}" ]; then
    if [ "${MAE_E2E_KEEP:-0}" = "1" ]; then
      echo "[harness] KEPT workdir=$WORK pids=(${HARNESS_PIDS[*]:-none})"
    else
      rm -rf "$WORK"
    fi
  fi
  return "$rc"
}
harness_trap_install() { trap harness_cleanup EXIT INT TERM; }

# harness_sweep_stale GLOB...
# Pre-flight defense-in-depth, run BEFORE this script creates its own $WORK.
# Two independent passes, matching the existing e2e workdir naming convention
# (mae-member-e2e.*, mae-mtls-e2e.*, mae-enc-e2e.*, mae-mesh-e2e.*):
#   (a) process pass: reap any process whose /proc/<pid>/environ HOME matches
#       one of GLOB under $TMPDIR — i.e. an editor/daemon launched by a PAST
#       run of ANY of the four scripts — but ONLY if that process has been
#       running longer than MAE_E2E_SWEEP_AGE_SECONDS (default 2x the TTL,
#       i.e. 1200s/20min). That age gate is the safety margin: if the TTL
#       mechanism is doing its job, nothing legitimate is still alive by then;
#       it also means this can NEVER race-kill a concurrently-running sibling
#       e2e invocation on the same dev machine, since no script's real runtime
#       comes anywhere close to 20 minutes (see ADR-044's derivation table).
#   (b) directory pass: rm -rf any matching workdir older than the same age
#       cutoff, regardless of whether a live process was found for it (covers
#       the case where the daemon already died but the dir was never removed).
# Uses the same /proc/<pid>/environ marker-matching technique already
# validated in this codebase by DaemonTestEnv::reap
# (crates/mae/src/daemon_supervisor.rs) for the analogous Rust-test problem.
harness_sweep_stale() {
  [ "${MAE_E2E_SWEEP:-1}" = "1" ] || return 0
  local tmproot="${TMPDIR:-/tmp}" pat dir pid age
  for pat in "$@"; do
    for dir in "$tmproot"/$pat; do
      [ -d "$dir" ] || continue
      for pid in $(pgrep -f "mae-daemon|mae --test" 2>/dev/null); do
        [ -r "/proc/$pid/environ" ] || continue
        tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | grep -qF "HOME=$dir" || continue
        age="$(ps -o etimes= -p "$pid" 2>/dev/null | tr -d ' ')"
        [ -n "$age" ] && [ "$age" -gt "$MAE_E2E_SWEEP_AGE_SECONDS" ] || continue
        echo "[harness] sweep: reaping orphaned pid=$pid (age=${age}s) from stale run $dir" >&2
        kill -TERM "$pid" 2>/dev/null || true; sleep 0.2; kill -KILL "$pid" 2>/dev/null || true
      done
      local dir_mtime dir_age
      dir_mtime="$(stat -c %Y "$dir" 2>/dev/null || stat -f %m "$dir" 2>/dev/null || echo 0)"
      dir_age=$(( $(date +%s) - dir_mtime ))
      if [ "$dir_mtime" -gt 0 ] && [ "$dir_age" -gt "$MAE_E2E_SWEEP_AGE_SECONDS" ]; then
        echo "[harness] sweep: removing stale workdir $dir (age=${dir_age}s)" >&2
        rm -rf "$dir"
      fi
    done
  done
  return 0
}
