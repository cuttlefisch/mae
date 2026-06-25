# ADR-005: Knowledge Base Nodes as CRDT Documents

**Status**: Accepted — **storage model superseded by ADR-011** (CozoDB-first KB storage; SQLite-BLOB
persistence dropped, org files become import/export only). The CRDT-node *concept* carries forward
(ADR-006/020/022/026); the SQLite-as-persistence framing here is historical.
**Date**: 2026-05-17
**KB Source**: `concept:adr-kb-crdt`

## Context

MAE's knowledge base currently uses SQLite as both storage and query engine.
For multi-user collaboration, offline editing, and P2P federation, KB nodes
need conflict-free concurrent editing without a central coordinator.

## Decision

Each KB node becomes a **yrs document** with the following schema:

```
YMap {
  id: String,
  title: YText,
  body: YText,
  tags: YArray<String>,
  links: YArray<YMap { dst: String, display: String }>,
  meta: YMap { kind: String, created_at: i64, updated_at: i64 }
}
```

SQLite remains the **persistence backend** — yrs document bytes are stored as
BLOBs alongside the existing schema. FTS5 indexes materialized text from
`YText::to_string()` on every committed transaction.

## Rationale

1. **Offline editing**: Users can edit KB nodes without connectivity. Changes
   merge automatically when reconnected (CRDT property).
2. **P2P federation**: Two MAE instances can sync KB subsets by exchanging
   yrs state vectors and updates. No central server required.
3. **AI attribution**: Each yrs transaction carries a client ID — AI edits
   are distinguishable from human edits in the operation history.
4. **Per-user undo**: yrs `UndoManager` provides per-user undo stacks
   without custom implementation.
5. **Gradual migration**: Store yrs bytes IN SQLite initially. No big-bang
   migration — existing read paths (FTS5, node queries) continue working.

## Consequences

- **Irreversible**: Once users have KB data stored as yrs documents, the
  format is committed. Mitigated by: yrs IS the Yjs standard (Notion,
  Excalidraw, TLDraw all use it).
- **Storage overhead**: yrs documents are larger than raw text (~24-32 bytes
  per operation in history). GC/compaction available for pruning.
- **FTS rebuild**: Every committed transaction must rebuild the FTS5 entry
  for affected nodes. Same pattern as current `sync_to_sqlite()`.
- **Schema evolution**: yrs handles unknown fields gracefully (CRDT property).
  New fields added to the YMap are automatically available to clients that
  understand them, ignored by others.

## Migration Path

1. **Phase A** (current): SQLite only. No yrs dependency.
2. **Phase B**: Add optional `crdt_doc BLOB` column to `nodes` table.
   New nodes get yrs docs. Existing nodes migrated on first edit.
3. **Phase C**: All nodes have yrs docs. SQLite is read cache + FTS index.
   Sync protocol exchanges yrs updates.

## Performance Targets

| Benchmark | Target |
|-----------|--------|
| KB node CRDT merge | <5ms per node |
| FTS5 rebuild (single node) | <1ms |
| Full KB sync (1000 nodes) | <500ms |
| Offline edit queue flush | <100ms for 100 edits |

## References

- ADR-004: KB Scaling
- Yjs document format: https://github.com/yjs/yjs/blob/main/INTERNALS.md
- yrs crate: https://docs.rs/yrs
