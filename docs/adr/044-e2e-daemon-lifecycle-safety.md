# ADR-044: E2E-test daemon lifecycle safety (SIGKILL-resilient cleanup)

**Status:** Accepted (2026-07-08). Prompted by an orphaned debug-build `mae-daemon` found
running on a dev machine a full day after its spawning e2e run finished, interfering with
unrelated live `mae --gui` sessions on the same machine.
**Relates:** ADR-035 (editor/daemon boundary — the production on-demand-daemon path this
ADR explicitly does **not** touch).
**Scope:** the four `scripts/collab-*-e2e.sh` end-to-end harnesses only.

## Context

`scripts/collab-membership-e2e.sh` and `scripts/collab-mtls-e2e.sh` tracked their spawned
`mae-daemon` in a plain shell variable and relied on `trap cleanup EXIT` to `kill` it on the
way out. **A bash `trap` cannot catch `SIGKILL`.** If the script itself is force-killed (an
IDE stop action, a closed terminal/SSH session, a hard-cancelled CI job), the trap never
runs and the daemon — plus its temp workdir — is orphaned indefinitely. This is exactly what
was observed: a debug-build daemon left running for a full day.

Two sibling scripts, `scripts/collab-encrypted-e2e.sh` and `scripts/collab-p2p-mesh-e2e.sh`,
were hardened for this five days earlier — `setsid` process-group isolation, `trap ... EXIT
INT TERM`, SIGTERM→0.3s grace→SIGKILL escalation targeting the process group — but the fix
was never backported to the other two, and the hardened pair independently duplicated the
same ~30-line block between themselves rather than sharing it. Even the hardened pattern
shares the same fundamental gap: nothing survives an actual `SIGKILL` of the parent script.

A straightforward backport of the hardened trap pattern into the two unfixed scripts was
considered and rejected: it would be a third and fourth copy of a still-`SIGKILL`-vulnerable
pattern, and copy-paste drift between independently-maintained copies is exactly how the two
unfixed scripts ended up here in the first place.

## Decision

Two backstops, additive to the existing trap-based cleanup (kept as the fast, quiet
common-path), **neither dependent on any trap or `Drop` running at all**, added via one
shared library (`scripts/lib/e2e-daemon-harness.sh`) sourced by all four scripts instead of
copy-pasted:

1. **Kernel-enforced TTL / dead-man's-switch.** `harness_spawn_daemon` wraps every e2e daemon
   launch in `timeout -k <grace> <ttl>` (default `ttl=600s`, `grace=10s`). This is enforced by
   the `timeout` process itself, independent of whether the parent script's process tree still
   exists — it fires across a `SIGKILL` of the parent. `ttl=600s` is derived from the longest
   internal timeout across all four scripts today (~120–140s worst case, e.g.
   `collab-membership-e2e.sh`'s `run_editor` `timeout 120`, `collab-mtls-e2e.sh`'s `timeout
   90`+`timeout 20`) — a ~4–5x margin, generous enough to never false-fire on a slow CI/dev
   box, while bounding the worst case from "a full day" down to "10 minutes, guaranteed."
2. **Pre-flight stale-orphan sweep.** `harness_sweep_stale`, run at the top of all four scripts
   before creating their own workdir, scans for daemons/workdirs from a *past* run of any of
   the four (matching their existing `/tmp/mae-{member,mtls,enc,mesh}-e2e.*` naming) older than
   `2×ttl` (20 min — always looser than the TTL, so it can never race a legitimately-running
   concurrent invocation), and reaps them. The matching technique — a process's
   `/proc/<pid>/environ` against a known workdir marker — mirrors `DaemonTestEnv::reap`
   (`crates/mae/src/daemon_supervisor.rs`), which already solves the identical
   untrackable-child problem for a single Rust test; this ports that validated technique to a
   shared bash helper rather than inventing a new one.

`scripts/lib/e2e-daemon-harness.sh` exposes `harness_spawn`, `harness_spawn_daemon`,
`harness_cleanup`/`harness_trap_install`, and `harness_sweep_stale`. All four scripts source
it; the two previously-unfixed scripts also had their daemon launch flattened from a shell-
function call (`srv env ... &`) to an inline `env` argv, since `setsid` execs its argument via
`execvp` and a bash function isn't on `PATH`.

## Explicit scope exclusion

`crates/mae/src/daemon_supervisor.rs`'s `spawn_daemon_process` intentionally spawns
`mae-daemon` **detached** for the production on-demand-daemon feature (ADR-035) — that daemon
is *meant* to outlive the spawning editor process; this is correct, desired behavior, not a
bug. **No TTL, no sweep, and no change belongs there.** This ADR and its implementation are
scoped strictly to the four e2e-test-spawned daemons, which are never meant to outlive their
test run. `DaemonTestEnv` in that same file remains as the correct, already-existing,
Rust-test-local solution to the same class of problem for that one test.

## Verification (the falsifier)

- All four `collab-*-e2e.sh` scripts produce identical PASS/TAP output before and after
  migration.
- `MAE_E2E_DAEMON_TTL_SECONDS=2 MAE_E2E_DAEMON_TTL_GRACE=1` on any script: the daemon dies on
  schedule.
- `kill -9` a running script's shell from another terminal mid-run: the orphaned `mae-daemon`
  is gone within `ttl+grace` seconds, zero manual intervention — the specific failure this ADR
  fixes.
- Run one script, hard-kill it before its own cleanup runs, then run a *different* one of the
  four: `harness_sweep_stale` logs and reaps the first script's orphan.
