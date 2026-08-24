# ADR-106: Node version history is a local audit trail, not projected state

**Status:** Accepted. Implemented alongside the Phase 0 fixes under issue #103.
**Amends:** ADR-029 (KB source of truth = CRDT, CozoDB = derived projection) — specifically its
claim that "Cozo holds *no authoritative content* — only the projection + derived state", which is
false as written.
**Relates to:** ADR-032 (durable CRDT store), issue #733 (CRDT compaction / node rebirth), issue
#731 (which raised this), issue #632 (the backup mechanism is unreachable).

## Context

ADR-029 makes CozoDB a projection: derived, rebuildable, holding nothing that cannot be
re-derived from CRDT truth. One relation has never fitted that description.

`node_versions` — the whole `kb_history` / `kb_restore` surface — exists **only** in Cozo. Verified
at `97b99ef5`: it appears nowhere in `shared/sync/`, nowhere in `daemon/src/projector.rs`, and
nowhere in `daemon/src/checkpoint.rs`. It does not sync, and it is not captured by the checkpoint.
It is authoritative content living in the store ADR-029 says holds none.

**A correction to how this was first reported.** Issue #731 (and the audit behind it) claimed the
projector's repair path *destroys* version history. That is **false**, and the test
`version_history_survives_node_deletion_and_reprojection` now pins it: `delete_node` removes the
node row and its links and does not touch `node_versions`, and nothing anywhere in the store deletes
versions. History outlives both a heal-delete and a re-projection.

The real defect was narrower and worse. `snapshot_version` had **no production caller at all** —
only `restore_version`, snapshotting before its own overwrite. So `kb_history` was always empty and
`kb_restore` could not undo a clobber, which is the single thing it exists to do. The recovery
surface was not being destroyed; it was never being written.

## Decision

1. **`node_versions` is a local, non-synced audit trail, and ADR-029's "no authoritative content"
   claim is amended to carve it out explicitly.** It is Cozo-resident by design, not by oversight.

2. **It is written when an authored write destroys content, and only then.**
   `CozoKbStore::insert_node_with_history` snapshots the existing row if — and only if — it exists
   and its versioned content actually differs. Text→store ingest (`import_org_dir_to_store`) is the
   first caller, because a destructive whole-row `:put` from a `.org` file is exactly the clobber
   `kb_restore` needs to undo.

3. **Derived writes do not snapshot.** The projector also writes through `insert_node`, and
   projection is re-derivation, not authorship. Snapshotting it would append a version per node on
   every full rebuild while recording nothing a user could want restored. This is why the snapshot
   is a *distinct method* rather than folded into `insert_node`.

4. **Version history does not move into the CRDT**, despite the CRDT nominally containing an edit
   log. See the rejected alternative below — that log is not durable, by our own design.

5. **It therefore has its own durability requirement.** Being neither synced nor re-derivable, it is
   lost with the Cozo store and nothing else replaces it. It must be inside whatever backup #632
   restores, and that is now a stated requirement rather than an assumption.

## Consequences

**Positive.** `kb_restore` becomes usable for the case it was built for. The bound is structural
rather than a tuning knob: the dominant case by far is re-ingesting an *unchanged* file — a watcher
tick, a scheduler pass, a startup ingest — and that writes no version at all, because the content
hash is compared first.

**The interesting one: this is what makes compaction safe.** Issue #733 proposes discarding CRDT
history at an epoch boundary (node rebirth) to bound tombstone growth, which reads as being in
direct conflict with "keep history". It is the opposite. Because the restore surface is a separate,
non-CRDT audit trail, **the CRDT's operation log can be discarded without taking the user's undo
with it.** History-in-the-CRDT and compaction genuinely cannot coexist; history-beside-the-CRDT and
compaction compose.

**Costs, honestly.** One `get_node` per ingested node on the authored-write path. `node_versions`
grows monotonically — nothing prunes it, and this ADR deliberately does not introduce pruning, since
an audit trail that silently discards entries is worse than one that grows. If growth becomes a real
problem, the answer is an explicit, user-visible retention policy, not a silent cap. And bulk
import (`bulk_import`) bypasses this path entirely and records no history; that is acceptable while
its only callers are store migration and corpus building, and would need revisiting if it ever
became an authored-write path.

## Alternatives rejected

- **Move version history into the node CRDT.** Superficially attractive — the CRDT already retains
  an operation log, so history looks free. It is not: yrs sets `skip_gc: false`, so garbage
  collection is **on by default** and deleted content is discarded, and #733's node rebirth would
  deliberately discard operation history to bound growth. A history surface built on the CRDT log
  would be silently lossy in exactly the situations a user reaches for restore. It would also
  replicate every historical state to every peer, which is a large amount of sync traffic for data
  that is inherently local and retrospective.

- **Snapshot inside `insert_node`, so no call site can forget.** Tempting given #730 was caused by
  exactly that class of omission. Rejected because it cannot distinguish authorship from
  re-derivation: the projector is a legitimate `insert_node` caller and must not generate versions.
  The mitigation is the `@ai-caution` on the method plus the tests, not a blanket hook.

- **Let ADR-029 stand and treat `node_versions` as expendable.** Honest, and briefly considered.
  Rejected because the store is about to become source of truth for the whole KB; "your undo history
  is expendable" is not a property to adopt at the moment durability starts mattering most.

## Verification

Three tests, each pinning a property rather than a case:

1. `an_overwriting_ingest_is_recoverable_from_history` — the end-to-end claim: an ingest replaces
   content, and `kb_restore` recovers the pre-ingest body. This failed to be *possible* before, since
   nothing wrote a version.
2. `re_ingesting_unchanged_content_records_no_version` — 25 unchanged re-ingests leave history empty,
   and creating a new node records nothing. This is the bound; without it the feature is a leak.
3. `version_history_survives_node_deletion_and_reprojection` — pins the correction above, so the
   "the projector destroys history" claim cannot re-enter the record as folklore.
