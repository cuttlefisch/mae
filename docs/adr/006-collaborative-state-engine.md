# ADR-006: Collaborative State Engine

**Status**: Accepted
**Date**: 2026-05-17
**KB Source**: `concept:collaborative-state`

## Context

MAE is evolving from a single-user editor with AI tools into a collaborative
state engine where multiple humans and AI agents interact with shared state
(text buffers, visual documents, KB nodes) in real-time.

Requirements driving this decision:
1. Real-time multi-user collaboration (text AND visual/structured content)
2. AI agents as collaborative peers (sequential tool calls, not keystroke-level)
3. Non-textual documents: scene graphs, component trees, design tokens
4. KB nodes as CRDT documents (offline editing, P2P federation)
5. Sustainable maintenance for a small team
6. Performance at scale: 100+ concurrent clients, 100K+ element documents

## Decision

### Transport: JSON-RPC (extend current MCP protocol)

- Zero new dependencies, proven with 130+ tools and multi-client sessions
- Content-Length framing (LSP-compatible) over Unix sockets now, TCP later
- Upgrade path: Content-Type negotiation for msgpack (2-3x wire reduction)
- tonic (gRPC) evaluated for Phase C external API surface only

### Sync Engine: yrs (Yjs Rust port)

- YATA algorithm: YText for buffers, YMap/YArray for visual documents
- Built-in UndoManager (per-user stacks)
- Awareness protocol for cursor/selection sharing
- Proven at scale: Notion (200M+ users), Excalidraw, TLDraw

### Buffer Architecture: Dual Structure

- yrs `YText` is the source of truth for collaborative state
- ropey remains the rendering engine (efficient line indexing)
- Bridge: ropey rebuilt from YText on remote changes (~1ms for 10K lines)

### Visual Documents

- Scene graphs / component trees represented as YMap/YArray
- Same yrs sync infrastructure as text buffers
- Visual mutations are yrs transactions (attributed, undoable)

## Rationale

### Why yrs over alternatives

| Library | Why not |
|---------|---------|
| automerge-rs | Performance cliff at >100K ops, no built-in undo, 2-3x memory overhead |
| diamond-types | Text-only (cannot represent visual/structured content), bus factor = 1 |
| cola | Text positions only, sole maintainer, immature |
| Custom OT | Transform functions explode combinatorially for visual operations |

### Why JSON-RPC over alternatives

| Transport | Why not |
|-----------|---------|
| tonic (gRPC) | 60-90s compile time penalty, unnecessary for localhost |
| capnproto | Bus factor = 1 (David Renshaw), poor ergonomics |
| tarpc | No streaming support (fatal for pub/sub) |
| Custom msgpack | JSON-RPC handles it; msgpack is an optimization, not a replacement |

### Production validation

| Product | Engine | Content | Scale |
|---------|--------|---------|-------|
| Notion | Yjs (custom) | Blocks | 200M users |
| Excalidraw | Yjs | Vector drawings | 10M+ monthly |
| TLDraw | Yjs | Vector drawings | Open source |
| Figma | Eg-walker (proprietary) | Vector graphics | 5M users |

## Architecture

```
┌─────────────────────────────────────────┐
│           MAE State Server              │
│                                         │
│  ┌─────────┐  ┌────────────┐  ┌──────┐ │
│  │ yrs Doc │  │ Broadcaster│  │ MCP  │ │
│  │ (YText  │◄─│ (per-client│  │ Tool │ │
│  │  YMap   │  │  queues)   │  │Dispatch│ │
│  │  YArray)│  └─────┬──────┘  └───┬──┘ │
│  └────┬────┘        │             │     │
│       ▼             │             │     │
│  ┌─────────┐        │             │     │
│  │ ropey   │ (render mirror)      │     │
│  └─────────┘        │             │     │
└─────────────────────┼─────────────┼─────┘
                      │             │
          JSON-RPC (Content-Length framing)
          Unix socket / TCP
                      │             │
         ┌────────────┼─────────────┼────────┐
         │            ▼             ▼        │
    ┌────┴────┐  ┌─────────┐  ┌─────────┐   │
    │Text CLI │  │GUI Client│  │Visual   │   │
    │(TUI)    │  │(Skia)   │  │Client   │   │
    └─────────┘  └─────────┘  └─────────┘   │
```

## Performance Targets

| Benchmark | Target | Method |
|-----------|--------|--------|
| Single-client edit latency | <1ms | criterion: insert into YText |
| 10-client concurrent convergence | <50ms | Integration: 10 tasks, random edits |
| 100K-line document sync | <100ms | Bench: encode/decode yrs doc |
| KB node CRDT merge | <5ms | Bench: concurrent node edits |
| ropey rebuild from YText | <10ms (10K lines) | Bench: apply update, rebuild rope |
| Event broadcast (100 clients) | <1ms | Bench: broadcast to 100 subscribers |

## Consequences

- **New crate**: `mae-sync` wraps yrs with MAE-specific document schemas
- **Ropey kept**: Dual structure adds ~200 lines of bridge code
- **MCP protocol extended**: New `sync/*` methods for state exchange
- **KB storage evolves**: yrs bytes stored in SQLite alongside existing schema
- **Visual clients possible**: Same sync infrastructure serves any client type
- **AI operations are yrs transactions**: Attributed, undoable, conflict-free

## Irreversibility Assessment

| Decision | Reversible? |
|----------|-------------|
| Transport (JSON-RPC) | YES — wire format swappable |
| Sync engine (yrs) | PARTIALLY — Yjs is an industry standard |
| Dual buffer (yrs + ropey) | YES — can drop ropey later |
| KB nodes as yrs docs | COMMITTED — acceptable (Yjs is de-facto standard) |

## References

- ADR-001: Server-Client Protocol
- ADR-002: Text Sync Model (superseded by this ADR)
- ADR-005: KB as CRDT
- Yjs internals: https://github.com/yjs/yjs/blob/main/INTERNALS.md
- yrs crate: https://docs.rs/yrs
