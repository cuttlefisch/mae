# ADR-002: Text Synchronization Model

**Status**: Accepted (yrs/YATA)
**Date**: 2026-05-17 (updated)
**Superseded by**: ADR-006 (Collaborative State Engine)
**KB Source**: `concept:adr-text-sync`

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

**Accepted: yrs (Yjs Rust port, YATA algorithm)** with dual-structure
(yrs YText + ropey mirror for rendering).

### Rationale

1. Multi-client MCP is now proven and stable (ADR-001 complete).
2. MAE's vision extends beyond text to visual documents (scene graphs,
   component trees) — this eliminates text-only CRDTs (diamond-types, cola)
   and makes custom OT intractable (combinatorial transform explosion).
3. yrs provides YText, YMap, YArray — handles text AND structured content
   in a single sync framework.
4. Built-in `UndoManager` with per-user stacks eliminates custom undo work.
5. Yjs ecosystem is the de-facto standard for collaborative apps (Notion,
   Excalidraw, TLDraw, Huly — 200M+ users combined).
6. Dual structure (yrs + ropey) preserves rendering performance while
   adding CRDT sync. Bridge overhead is ~1ms per remote edit.

### Why NOT the other options

| Library | Eliminated because |
|---------|-------------------|
| automerge-rs | Performance cliff at >100K ops, no built-in undo, 2-3x memory |
| diamond-types | Text-only, cannot represent visual content, bus factor = 1 |
| cola | Text positions only, sole maintainer, immature |
| Custom OT | Transform functions explode for visual operations |

### Implementation Plan

- **Phase A**: `mae-sync` crate with yrs dependency, document schemas
- **Phase B**: Buffer integration (dual yrs YText + ropey)
- **Phase C**: MCP protocol extended with `sync/*` methods
- **Phase D**: KB nodes as yrs documents (see ADR-005)

## Consequences

- Collaborative editing becomes possible once `mae-sync` crate is implemented.
- Dual structure adds ~200 lines of bridge code (yrs YText → ropey rebuild).
- KB nodes gain offline editing and P2P federation (see ADR-005).
- Visual documents (scene graphs, design components) use the same sync
  infrastructure as text buffers.
- yrs document format is a long-term commitment — acceptable because Yjs
  is the most widely deployed CRDT format in production.
- File-level contention (ADR-003) remains relevant for non-collaborative
  single-writer scenarios.

## References

- Zed CRDT blog: https://zed.dev/blog/crdts
- Xi-editor CRDT details: https://xi-editor.io/docs/crdt-details.html
- Fugue paper (2023): https://arxiv.org/pdf/2305.00583
- Figma multiplayer: https://figma.com/blog/how-figmas-multiplayer-technology-works/
- Automerge 2.0: https://automerge.org/blog/automerge-2/
