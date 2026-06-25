# ADR-028: KB data lifecycle (signed membership checkpoints + compaction, rotation, backup, rollback)

**Status:** Accepted (design). Phased: the **membership signed-checkpoint** is committed by ADR-026's
op-log model (implement alongside Phase 2b/4 when log length warrants); the **content-doc lifecycle**
(compaction caps, retention, backup, rollback, crash-safety) is a follow-on slice.
**Extends:** ADR-019 (durable reconstruction-capable sync), ADR-022 (crash-safe convergent sync),
ADR-026 (signed membership op-log).
**Builds on:** ADR-025 (the mesh whose traveling, storage-constrained peers motivate this).

## Context

The P2P mesh (ADR-025) puts shared KBs on **personal, storage-constrained, mobile** devices — a laptop,
eventually a phone — not a managed server with elastic disk. Two things grow without bound and will, left
alone, exhaust local storage or slow fresh-peer onboarding:

1. **The yrs CRDT history.** Every collaborative doc (`kbc:` collection, each KB node) accumulates
   tombstones + update history. yrs GC reclaims deleted *content* but the op/struct history still grows
   with edit volume; a long-lived, heavily-edited KB's on-disk encoding climbs steadily.
2. **The membership op-log (ADR-026).** Membership is now an **append-only signed op-log**; a fresh peer
   verifies current membership by **replaying the whole log** from the anchored genesis. The log is small
   while membership is stable, but over a KB's lifetime (admits, removes, role changes, re-keys, invite
   churn) it grows monotonically, and full replay on every cold join eventually costs.

A daemon can also simply **run low on disk** (other apps, a big KB, many KBs), and a traveling user wants
**backups** they control and the ability to **roll back** a KB to a known-good point after a bad
import/merge — all without a central server. None of this may **break convergence**: pruning or rolling
back local state must never make this peer diverge from honest peers or resurrect deleted data on the mesh.

ADR-019/022 already give durable, crash-safe, reconstruction-capable persistence (WAL-first SQLite in the
daemon, snapshots, SV-reconcile). This ADR adds the **lifecycle policy on top** — *what* we keep, *for how
long*, *how we shrink it*, and *how we checkpoint* — so the substrate stays bounded and recoverable on a
personal device.

## Decision

Adopt a **four-part data lifecycle**, each part convergence-safe by construction.

### 1 — Signed membership checkpoints (committed by the ADR-026 op-log)

A **checkpoint** is a signed snapshot of *derived* membership state at a known causal frontier:
`{kb_id, frontier: set<chain_hash>, derived_members: map<principal, role+epoch>, governance,
checkpoint_epoch, author, sig}`, signed by an **authorizer** (owner, or a quorum co-signature set under
`quorum` governance — same authority that may mutate membership). It is a **trusted replay root**: a peer
that verifies a checkpoint's signature against the **external anchor** (ADR-026: owned-KB pubkey or
join-ticket node-id) may begin `derive_valid_members` *from the checkpoint's frontier* instead of genesis,
and prune op-log entries causally **behind** a frontier that all current members have acknowledged.

Rules that keep it safe:
- A checkpoint **never** changes the derivation result — it must equal a full replay to that frontier
  (verified in tests). It is an optimization, not a new source of truth.
- Ops **concurrent with or after** the checkpoint frontier are retained and still replayed on top — so a
  removal/grant in flight at checkpoint time is never silently dropped.
- Pruning op-log entries behind a checkpoint requires that frontier be **acknowledged by all current
  members** (else a slow/offline peer that hasn't seen the checkpoint can still full-replay from genesis);
  a peer that lacks the checkpoint falls back to genesis replay — correctness never depends on having it.
- This is the Keybase-sigchain **merkle-checkpoint** / git **shallow-clone** pattern, grounded in the same
  prior art ADR-026 cites.

### 2 — Content-doc compaction (bounded yrs history)

Per-KB, configurable caps drive periodic compaction of the yrs docs (the daemon's existing background
compaction, ADR-022, extended with policy):
- `kb.history.max_versions` / `kb.history.max_age` — node version-history retention (the `kb_history`
  snapshots) trimmed beyond the cap, **oldest first**, never the current head.
- `kb.compact.target_bytes` — when a doc's encoded size exceeds the target, re-encode via yrs GC +
  snapshot to a fresh state vector. **Convergence guard:** compaction only reclaims state **causally
  dominated by the acknowledged frontier of every current member** — never drops an update a member hasn't
  seen (that would resurrect-on-reconcile or diverge). On a personal device with one user this is trivially
  the local frontier; on the mesh it is the meet of members' acknowledged SVs.
- Compaction is **crash-safe** (ADR-022): WAL-first, snapshot swapped atomically; a crash mid-compact
  leaves the pre-compact state intact.

### 3 — Backup & rollback (user-owned, no server)

- **Snapshot/backup:** `kb-backup <kb> [path]` writes a **self-contained, verifiable** snapshot — the
  current yrs state + the membership op-log (or a signed checkpoint) + the external anchor — to a
  user-chosen path (local dir, external drive, synced folder). Portable across the user's own machines.
- **Rollback:** `kb-rollback <kb> <snapshot>` restores a KB to a prior snapshot **locally**. Convergence
  rule: rollback is a *local* operation; to avoid resurrecting data the mesh has moved past, a rolled-back
  peer **re-reconciles forward** from current members on next connect (SV-reconcile makes the mesh's
  current state win for anything newer). Rollback recovers *this peer's* working state after a bad
  local import/merge; it does **not** rewrite mesh history (that would need a signed membership-level
  action, out of scope here). The UX warns when a rollback predates acknowledged remote state.
- Reuses ADR-019's reconstruction-capable persistence — a backup *is* a durable reconstruction root.

### 4 — Storage pressure policy (graceful degradation)

When the daemon approaches a configurable `daemon.storage.soft_limit`: (a) trigger compaction (part 2)
early; (b) trim history caps more aggressively; (c) emit an ADR-024 attention-bus notification
(`storage_pressure`) so the human/AI peer can act (move/leave a KB, raise the cap, free disk); (d) at a
`hard_limit`, refuse *new* shares but never corrupt or silently drop an existing member's unsynced data —
back-pressure, not data loss. All limits are OptionRegistry options (Scheme-accessible, principle #7).

## Configuration

All under the daemon config + OptionRegistry (parity: CLI flag, editor option, Scheme, MCP):
`kb.history.max_versions`, `kb.history.max_age`, `kb.compact.target_bytes`,
`kb.checkpoint.every_n_ops` (mint a membership checkpoint after N ops), `daemon.storage.soft_limit`,
`daemon.storage.hard_limit`. Sensible bounded defaults; never an unbounded grow-forever default.

## Adversarial / robustness review

- **Prune resurrects deleted data on reconcile** → prevented: compaction/pruning only reclaims state
  causally dominated by **all current members' acknowledged frontier**; anything a member hasn't seen is
  retained. A peer lacking a checkpoint full-replays from genesis.
- **Forged checkpoint ("everyone trusts me as owner")** → rejected: a checkpoint is signed and verified
  against the **external anchor** (ADR-026), and must equal a full replay to its frontier; a relay cannot
  mint one. Under `quorum`, it needs the co-signature threshold.
- **Rollback as an attack (revert past a revocation to revive a removed peer)** → prevented: rollback is
  *local* and re-reconciles forward on reconnect; membership is derived from the signed op-log, not from
  rolled-back local state, so a removal still applies. Rollback can't rewrite mesh membership history.
- **Storage-exhaustion DoS (a peer floods edits to fill a victim's disk)** → bounded by per-KB caps +
  `hard_limit` back-pressure (refuse new shares, notify) — degrade, never corrupt; combined with ADR-026
  membership (only members can write) it's not an open vector.
- **Crash mid-compact / mid-rollback** → ADR-022 WAL-first + atomic swap leaves the prior state intact;
  no half-written KB.

## Consequences

- Personal/mobile peers stay **bounded** in storage and **fast** to cold-join (checkpoint replay root),
  closing the operational gap ADR-025's mobility opened.
- Users get **portable, verifiable backups** + **local rollback** with no server — reinforcing
  local-first / user-ownership (CLAUDE.md #12).
- Cost: checkpoint signing + the convergence-frontier bookkeeping for safe pruning; both small and
  off the edit hot path (background, like existing compaction).
- Reviewer guardrail: any prune/compact/rollback that reclaims or reverts state **not** causally
  dominated by the relevant acknowledged frontier is a convergence regression — reject it. "Bounded
  storage" must never come at the cost of "data a member hasn't seen."

## Verification

Unit: checkpoint sign/verify (anchor mismatch + tamper rejected); checkpoint-rooted derivation **equals**
full genesis replay to the same frontier; pruning behind an all-acked frontier preserves derivation;
compaction guard refuses to reclaim un-acked state. Convergence E2E on a **real-daemon mesh**: compact one
peer, reconcile with a peer that was offline during the compaction → both converge, no resurrected/lost
data; cold-join from a checkpoint matches cold-join from genesis. Backup→rollback→reconnect: a peer rolled
back past a remote edit re-converges forward; a peer rolled back past a remote **removal** is still treated
as removed. Storage-pressure: soft-limit triggers compaction + a `storage_pressure` notification; hard-limit
refuses new shares without data loss. Crash-injection mid-compact/mid-rollback leaves prior state intact
(ADR-022 parity). Cross-OS (macOS + Linux) for backup paths (XDG-first, #13).
