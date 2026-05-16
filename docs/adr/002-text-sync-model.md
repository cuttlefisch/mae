# ADR-002: Text Synchronization Model

**Status**: Deferred
**Date**: 2026-05-16
**KB Source**: `concept:adr-text-sync-model`

## Context

For true collaborative editing (multiple humans + AI agents editing the same
buffer simultaneously), MAE needs a text synchronization model. The two major
approaches are:

- **Operational Transform (OT)**: Centralized server transforms operations.
  Used by Google Docs, VS Code Live Share.
- **CRDT (Conflict-free Replicated Data Types)**: Decentralized merge.
  Used by Zed, Xi-editor, Yjs-based editors.
- **Hybrid (eg-walker)**: Combines OT and CRDT properties.
  Used by Figma (2024+).

## Options Evaluated

### OT (Operational Transform)

**Pros**: Well-understood, server-authoritative, smaller state.
**Cons**: Central server required, TP2 complexity in P2P, character
interleaving on concurrent same-position inserts.

**Production references**: Google Docs, VS Code Live Share (ShareDB).

**Failure modes**:
- Character interleaving: Alice types "cat", Bob types "dog" at same position
  → "cdaotg" depending on network timing
- TP2 complexity: transform functions grow combinatorially with operation types
- Network partition: OT fails without central server ordering

### CRDT (Conflict-free Replicated Data Types)

**Pros**: Decentralized, automatic merge, no central server needed.
**Cons**: Memory overhead (tombstones, unique IDs per character), complex undo.

**Algorithms evaluated**:
| Algorithm | Space | Production Use | Notes |
|-----------|-------|---------------|-------|
| RGA | O(n^2) worst | Xi-editor | Tombstone bloat (16+ bytes/char) |
| YATA | O(n) | Yjs, Zed | Optimized for sequential typing |
| Fugue | O(n) | Research (2023) | Maximal non-interleaving |
| Eg-walker | O(n) | Figma code (2024) | Hybrid OT/CRDT, minimal overhead |

**Rust ecosystem**:
- `automerge-rs`: General-purpose CRDT, rich text support (2.2+). Not rope-integrated.
- `yrs`: Yjs port to Rust. YATA algorithm. Not rope-integrated.
- `diamond-types`: Claimed fastest text CRDT. Plain text only. Early stage.
- `cola`: Operation-based CRDT. Minimal community.

**Critical limitation**: None of the Rust CRDT libraries integrate with `ropey`.
MAE would need a wrapper layer or dual data structure.

### Hybrid (Figma eg-walker)

**Pros**: OT-like performance, CRDT-like merge semantics.
**Cons**: New algorithm (2024), limited production validation, two code paths.

## Decision

**Deferred** until the RPC layer (ADR-001) is proven and stable.

### Rationale

1. Multi-client MCP is prerequisite infrastructure — without reliable connections,
   sync is meaningless.
2. None of the Rust CRDT libraries integrate with ropey natively, requiring
   significant wrapper work.
3. The current single-writer model (editor thread processes all mutations)
   is correct for the near-term multi-client scenario where AI agents call
   tools sequentially through MCP.

### Next Steps

- Prototype `diamond-types` on a branch (plain text collaborative editing)
- Prototype `automerge-rs` on a branch (rich text with formatting)
- Evaluate ropey wrapper approaches
- Benchmark memory overhead at document scale (10K-100K lines)

## Consequences

- Collaborative editing (multiple cursors in same buffer) is not available
  until this decision is made.
- Multi-client MCP still works — clients read/write buffers through tool
  calls, with the editor thread serializing all mutations.
- File-level contention is handled by ADR-003 (file safety).

## References

- Zed CRDT blog: https://zed.dev/blog/crdts
- Xi-editor CRDT details: https://xi-editor.io/docs/crdt-details.html
- Fugue paper (2023): https://arxiv.org/pdf/2305.00583
- Figma multiplayer: https://figma.com/blog/how-figmas-multiplayer-technology-works/
- Automerge 2.0: https://automerge.org/blog/automerge-2/
