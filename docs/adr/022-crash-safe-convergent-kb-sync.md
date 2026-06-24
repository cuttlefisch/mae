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

## Implementation (as built)

**Reconcile primitive** — `KnowledgeBase::reconcile_remote_node(node_id, remote_diff, remote_sv)`
(`shared/kb/src/lib.rs`): merges `remote_diff` (creating the node if absent; **never** replaces an
existing one), returns a `ReconcileOutcome { action, content_changed, local_ahead }`. `local_ahead` is the
ops the remote lacks (`encode_diff(remote_sv)`), gated on a format-independent state-vector comparison
(`KbNodeDoc::has_ops_beyond` / `kb::sv_has_ops_beyond`) — **not** `diff.is_empty()`, since a yrs v1 update
against a fully-covering SV is still a non-empty `[0,0]`. Divergence is detected **pre-merge** and
**order-independently**: the node pre-existed, the remote genuinely held ops we lacked, AND the two
lineages share no client (`kb::sv_clients_disjoint`). A healthy collab pair always shares the owner's
lineage client (adopted on first join), so a disjoint client set is the exact B-14 signal — and it does
not depend on which side wins the YMap last-writer-wins. On divergence we leave the node untouched and
report `DivergentLineage`; the caller adopts the owner's full state (its diff against our disjoint SV *is*
its full lineage) to establish a shared lineage.

**Wire / daemon** — `kb_join_request` gains optional `node_svs` (omitted when empty → exact pre-ADR-022
shape). The daemon `kb/join` handler replies per node with an incremental `diff` (`encode_diff_and_sv`)
when the member sent an SV, else full `state`; it always includes the daemon's `sv`.

**Editor** — `kb_join_node_svs` gathers per-node SVs from the durable `crdt_docs` (independent of the
pending queue) and threads them through `CollabIntent::JoinKb → CollabCommand::JoinKb → kb_join_request` at
every join site. `kb_register_joined_instance` reconciles each node and re-queues any recovered local-ahead
through the existing durable pending queue (single emit source); the post-(re)connect drain ships it.

**B-17 (found while building the N-peer harness on the real CRDT path):** `derive_kb_client_id` returned a
full `u64`, but a yrs `ClientID` is **53-bit** — `ClientID::new` debug-panics and, in release, force-sets
then strips the top 11 bits, silently truncating a >53-bit id to its low 53 bits. Two fingerprints
differing only above bit 53 would collide on one yrs lineage (a B-16-class collision, release-only). Fixed
at the source (xor-fold into 53 bits, retaining entropy) and defensively at the boundary
(`mae_sync::text::clamp_client_id_to_yrs` in `new_doc_with_client_id`, exact release-parity so debug ==
release). This is the payoff of driving production code paths in tests rather than parallel stand-ins.

Tests: `crates/core/tests/kb_sync_n_peer_e2e.rs` (N∈{2,3,5}: share/join/bidirectional/concurrent/
offline/restart, and the crash crux as `lost_row_adopt_clobbers_documents_the_bug` vs
`lost_row_reconcile_converges`); `reconcile_remote_node_*` units; the real-daemon
`kb_join_with_svs_returns_reconcile_diff_else_full_state` e2e; `client_id_clamped_to_yrs_53_bits`.
