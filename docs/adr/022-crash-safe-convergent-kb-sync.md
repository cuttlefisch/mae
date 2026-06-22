# ADR-022: Crash-safe, convergent KB sync (state-vector reconcile, N peers)

**Status:** Accepted (design). Supersedes the "kb/join SV-diff = deferred bandwidth optimization"
framing in ADR-020 §Future Work D2 — it is not a bandwidth nicety, it is the **correctness +
crash-safety** mechanism.
**Extends:** ADR-020 (replicated KB CRDT artifact; B-14 adopt-on-join, B-16 lineage/client_id, the
durable pending queue).

## Context

Live T3/T3b/T3c testing proved bidirectional KB sync works and that a *graceful* offline edit or
editor restart converges. But T3c (non-graceful `kill -9`) exposed that today's crash-safety is
**conditional, not guaranteed**:

- **Recovery depends on the durable pending queue.** An offline edit is replayed on reconnect only if
  its `pending_updates` row survived the crash. The node *content* (`crdt_doc`) and the pending-sync
  queue are persisted **separately** (sled, ~500 ms async flush), so a crash can leave content durable
  but the sync-intent row lost.
- **The adopt-on-join is a blind replace.** `kb/join` sends a **full snapshot**; the member
  `adopt_remote_node` **replaces** its local node. So a local edit whose sync-intent was lost (content
  durable) is **silently clobbered** by the daemon's older snapshot on rejoin = lost work.
- **The no-clobber guarantee that *does* hold today is narrow.** The reconnect drains the pending queue
  *before* the re-join (FIFO command channel, main-thread-driven) — a real guarantee, but only for
  edits *still in the queue*. It cannot protect an edit whose row was lost.

For a production system where machines die mid-edit and **N** peers collaborate, sync must converge
**regardless of when a crash lands**, without relying on a separate intent queue surviving.

## Decision

**1. (Re)join/reconnect uses bidirectional CRDT state-vector reconcile — never blind replace.**
Mirror the text-buffer handshake (`sync/state_vector` → daemon `encode_diff_and_sv`), per shared node:

- Member sends each node's **state vector** (`KbNodeDoc::state_vector()`) to the daemon.
- Daemon replies with **only the ops the member lacks** (`DocStore::encode_diff_and_sv`) — the member
  `apply_update`s it (**merge**, preserving local-ahead ops).
- Member computes **its** local-ahead diff against the daemon's returned SV (`KbNodeDoc::encode_diff`)
  and sends it to the daemon → daemon merges → the member's durable-but-unsynced edits re-sync.
- Both sides converge by CRDT merge. **No clobber; no dependence on the pending queue for recovery.**

The durable **`crdt_doc` (content) is the source of truth**; what needs syncing is *re-derived* from
the SV exchange on every (re)connect — so an edit is recovered iff its **content** is durable,
independent of whether any pending-queue row survived. (B-14 adoption is kept only for a *brand-new*
node the member has never seen; an existing node always reconciles.)

**2. `crdt_doc` write durability — shrink the lossy window.**
Persist node content with flush-on-write (or fsync-batched) semantics so the last edit before a hard
power-loss is durable, not buffered for ~500 ms. (Tune the sled/Cozo flush, or WAL the crdt_doc write.)
This narrows the only remaining loss case — "content itself never flushed" — which §1 cannot recover
because there is nothing durable to reconcile.

**3. The pending queue becomes a live-latency optimization, not the recovery mechanism.**
It still gives low-latency propagation of live edits while connected (and its durability + the
drain-before-adopt ordering remain useful), but **correctness no longer depends on it surviving a
crash** — the SV reconcile is the safety net.

### N peers

Each peer↔daemon SV reconcile is independent; the daemon is the convergence hub holding the
authoritative per-node `crdt_doc`, and broadcasts live updates to all subscribers. Any peer's
(re)connect SV-syncs both directions with the daemon, so **K crashed/returning peers each converge with
the hub** and, transitively, with each other — no pairwise coordination, no full-snapshot stampede.
This is the same hub-and-spoke CRDT model text buffers already scale with.

## Consequences

- **Crash-safe by construction:** any non-graceful shutdown (kill, power loss) at any moment → on
  reconnect, the SV exchange re-derives and merges exactly the missing ops both ways → converge, no
  loss of durable content, no clobber. Independent of the sled flush timing for the *intent*.
- **Bandwidth:** reconcile sends diffs, not full snapshots (the ADR-020 D2 win, now a side effect).
- **Migration:** existing nodes already carry `crdt_doc`; the join path switches from
  snapshot-adopt to SV-reconcile. B-16's stable per-peer client_id is a prerequisite (distinct
  lineages must merge, not collide).
- **Reviewer guardrail:** a KB (re)join that *replaces* an existing local node (instead of
  reconciling) is a crash-safety regression — reject it.

## Verification

Unit: SV reconcile preserves local-ahead ops (construct local-ahead + daemon-ahead divergence, reconcile,
assert both converge with neither side's edits lost) — the two-independent-peers harness extended with a
"local-ahead, no pending row" case (simulates the lost-intent crash). E2E (`daemon/tests/collab_e2e.rs`):
member edits offline, its pending queue is dropped, reconnect → SV reconcile still propagates the edit +
does not clobber. Live: **T3c-stress** — edit-then-instant-`kill -9` (sub-flush-window), relaunch,
assert no loss + no clobber across both peers; repeat for a 3rd peer.
