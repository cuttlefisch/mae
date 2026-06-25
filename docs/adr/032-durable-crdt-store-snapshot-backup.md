# ADR-032: Durable CRDT store — pinning, snapshot, backup/restore

**Status:** Accepted (design); implemented in Phase A (foundation, before the projector).
**Extends:** ADR-019 (durable reconstruction-capable KB sync), ADR-020 (replicated KB CRDT
artifact), ADR-028 (KB data lifecycle), ADR-029 (CRDT is the truth ⇒ its store must be durable).
**Feeds:** ADR-026 (signed checkpoints can reuse the KB-checkpoint), the projector's rebuild root.

## Context

ADR-029 makes the yrs/CRDT layer the canonical source of truth. It physically lives in the
daemon's `doc_store` (`daemon/src/doc_store.rs`) over a sharded SQLite backend
(`daemon/src/storage.rs`): **WAL-first + threshold compaction**. Each yrs doc is an append-only
WAL of updates plus a periodic full-state snapshot; `apply_update` appends to the WAL durably
(`journal_mode=WAL`, `synchronous=NORMAL`) before the in-memory apply; compaction
(`BEGIN IMMEDIATE` → write snapshot + trim WAL, atomic) fires at `compact_threshold` (500) /
`max_wal_entries` (5000); recovery replays the WAL tail onto the snapshot. Per-doc `Mutex` +
monotonic `wal_seq` serialize hub+p2p applies; yrs idempotency makes re-delivery safe; content-hash
save-intent guards. Each KB = one `kbc:{kb_id}` collection doc + N `kb:{node_id}` node docs, each an
independent WAL+snapshot.

**This is a solid CRDT journal — but it was built for *ephemeral collab sessions*, not for *hosting*
a KB's source of truth.** The gaps are reliability-critical now that the store IS the truth:

1. **Idle eviction DELETES from storage** (`doc_store.rs` ~528): a doc with `connected_clients == 0`
   past the idle timeout is dropped from memory **and deleted from disk**. A hosted KB with no
   editor connected would have its nodes destroyed.
2. **`max_documents` = 1000 is a HARD in-memory cap** (`get_or_create` errors at it): a 5,000-node
   KB cannot load.
3. **No atomic KB-wide checkpoint:** collection + node docs compact independently; there is no
   consistent point-in-time snapshot of a whole KB.
4. **No backup/restore/export API.**
5. **No snapshot integrity commitment** (snapshots stored as-is).

## Decision

Harden the `doc_store` into a durable, snapshotable HOME for KB CRDT truth.

1. **Pin durable/owned KB docs.** Docs belonging to a hosted/owned KB are marked durable: idle
   eviction may drop them from **memory only**, **never from disk**; they lazy-reload on access.
   (Truly ephemeral session docs keep the evict-and-delete behavior.) This is the #1 reliability
   fix.
2. **Memory is an LRU bound over an unbounded durable set.** `max_documents` governs the in-memory
   working set (LRU); the durable set on disk is unbounded. Lazy-load on access; memory-evict the
   coldest; disk keeps everything.
3. **Atomic KB checkpoint.** A checkpoint captures the collection doc + all its node docs at a
   **consistent state-vector** (briefly gating writes or using SQLite snapshot isolation),
   producing a **content-hashed** artifact. This is the projector's trusted rebuild root and can
   back ADR-026 signed checkpoints / ADR-028 compaction; it also bounds CRDT **tombstone growth**.
4. **Backup / restore / export.** Export a KB's full CRDT state (collection + nodes, as a portable
   artifact) and import it on another daemon — for migration (the RoamNotes onboarding), disaster
   recovery, and KB hand-off. Restore is a checkpoint replay.
5. **Snapshot integrity.** Content-hash each snapshot; verify on load; a failed verify falls back to
   WAL replay or the rebuildable cozo projection (ADR-029 self-heal).

## Consequences

**Positive.** The CRDT truth is safely hostable (no data loss for an idle hosted KB), scales beyond
the in-memory cap, and is snapshotable + portable. Checkpoints give a fast rebuild root + tombstone
compaction. Pairs with the rebuildable cozo projection for end-to-end self-healing.

**Costs.** Eviction policy gains a durable/ephemeral distinction (must be correct — over-eager
deletion is the failure mode). Atomic KB checkpoint needs a consistent-SV capture across many docs
(brief write-gate or snapshot isolation). Backup/restore is new surface (CLI + control method +
parity).

## Alternatives rejected

- **Keep ephemeral-session semantics** (status quo): destroys hosted KB data on idle. Rejected.
- **One yrs doc per whole KB** (instead of per-node): simplifies atomic checkpoint but defeats
  per-node incremental sync/projection + per-node SV-reconcile (ADR-022) and bloats every edit's
  replicated unit. Rejected — keep per-node docs; solve checkpoint at the store layer.

## Verification

Hosted KB with no editor connected survives idle eviction + daemon restart (docs reload, counts
intact). A 5,000-node KB loads (LRU memory, full disk). Atomic checkpoint → restore on a second
daemon → byte-identical CRDT + identical projected graph. Snapshot corruption is detected on load
and self-heals via WAL/rebuild. `make ci-all` + daemon clippy `-D`.
