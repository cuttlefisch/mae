# ADR-010: CRDT Algorithm Assessment (eg-walker vs yrs/YATA)

**Status**: Accepted (no-go on eg-walker migration)
**Date**: 2026-05-27
**Related**: ADR-002 (text sync), ADR-006 (collaborative state engine), ADR-008 (CRDT target metrics)

## Context

Real-world smoke testing with two MAE editors connected via the daemon
revealed transport-layer bugs: cancel-safety in `read_message` within
`tokio::select!`, framing protocol mismatches in tests, and WAL sequence gaps.
This prompted an assessment of whether the CRDT algorithm itself (yrs/YATA)
should be replaced with eg-walker (Kleppmann et al., 2024), a hybrid OT/CRDT
algorithm that claims superior memory characteristics.

## Assessment

| Criterion | yrs/YATA | eg-walker | Weight |
|-----------|----------|-----------|--------|
| Rust impl exists | Yes (yrs, mature) | No (TypeScript ref only) | Critical |
| Memory for long docs | O(edits) growth | O(doc) + O(edits on merge) | High |
| Rich text/structured | YMap, YArray, YXml | Text only | High |
| Undo/redo | Built-in UndoManager | Must be built | Medium |
| P2P capability | Via sync protocol | Native | Low (for now) |
| Production track record | Notion 200M+ users | Figma code 2025 | High |

## Key Findings

1. **Are our problems algorithmic?** No. All identified bugs are transport-layer
   (cancel-safety of `read_message` in `tokio::select!`, framing protocol
   mismatch in tests, test fidelity gaps). YATA merges correctly when updates
   arrive. Switching algorithms fixes none of the identified bugs.

2. **Port cost?** Extremely high. No Rust implementation exists. Would need to
   port ~3k lines TypeScript, build undo/redo, awareness protocol, ropey
   bridge, WAL persistence. Estimated 3-6 months full-time.

3. **Is yrs hobbling us?** No. yrs is stable, supports the full MAE feature
   set (text via YText, KB nodes via YMap, per-user undo via UndoManager,
   awareness protocol). UTF-16 offset encoding is handled via ropey's
   `char_to_utf16_cu()`. Problems are entirely in the transport/integration
   layer.

4. **eg-walker advantages are theoretical for MAE.** The O(doc) memory
   advantage matters for documents with millions of edits. MAE's use case
   (editor sessions, not persistent collaboration databases) rarely accumulates
   >100k edits per document.

5. **P2P readiness.** MAE's transport is already generic over
   `AsyncBufRead + AsyncWrite`. P2P for LAN needs: mDNS discovery,
   bidirectional sync (currently unidirectional client-to-server), per-peer
   state reconciliation. Feasible in 2-4 weeks after transport bugs are fixed.
   Daemon remains valuable as persistence/relay node. Neither algorithm
   choice gates P2P -- the transport layer is the bottleneck.

## Decision

**Retain yrs/YATA. Do not migrate to eg-walker.**

The bugs motivating this assessment are transport-layer problems, not
algorithmic ones. A CRDT migration would cost 3-6 months, lose critical
features (structured data types, built-in undo), and fix zero identified bugs.

## Revisit Criteria

Revisit this decision in 12 months if any of the following occur:

- A production-quality Rust implementation of eg-walker appears
- Sessions routinely exceed 100k edits per document
- yrs maintenance stalls (currently active: last release within 6 months)

## Consequences

- Continue investing in transport-layer fixes (dedicated reader tasks,
  Content-Length framing, cancel-safe I/O)
- yrs remains the single CRDT substrate for text, KB, and visual documents
- No API changes needed -- TextSync, awareness, undo/redo remain stable
- eg-walker remains on the radar as a future option if the Rust ecosystem
  matures

## References

- [eg-walker paper](https://arxiv.org/abs/2409.14252) (Kleppmann et al., 2024)
- [yrs documentation](https://docs.rs/yrs)
- ADR-002 (text sync -- accepted: yrs)
- ADR-006 (collaborative state engine)
- ADR-008 (CRDT target metrics)
