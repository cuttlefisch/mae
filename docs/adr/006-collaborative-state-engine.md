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
│             MAE Daemon                  │
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

## Implementation Notes (v0.11.0)

### Document Addressing

Documents are identified by a `DocAddress` enum (`crates/sync/src/lib.rs`):

```rust
pub enum DocAddress {
    File { project_hash: String, rel_path: String },  // file:{hash}/{path}
    KbNode { node_id: String },                        // kb:{id}
    Shared { name: String },                           // shared:{name}
}
```

### SQLite Connection Pool (fixes B1 bottleneck)

`SqlitePool` (`daemon/src/storage.rs`) uses FNV-1a hash sharding
across N connections (default 4). All shards open the same WAL-mode database.
Reduces p99 write latency from ~50ms to ~12ms at 10 concurrent clients.

### CRDT-Safe Reconciliation (fixes B2)

`TextSync::reconcile_to()` (`crates/sync/src/text.rs`) computes a character-level
LCS diff (via `similar` crate) between current yrs content and a target string,
then applies insert/delete operations as yrs transactions. Preserves CRDT vector
clocks and tombstones — safe for multi-client undo.

### Event Sequence Tracking (fixes B3)

`EditorEvent::SyncUpdate` carries a `wal_seq: u64` field for gap detection.
Server handler `sync/resync` method returns diff from a given WAL sequence point.
Clients detect gaps via monotonic sequence and auto-trigger resync.

### Save Protocol

Content-hash verification (SHA-256) via `docs/save_intent` + `docs/save_committed`.
`DocStore::check_save_intent()` returns `SaveOk` or `SaveConflict` based on
whether the document has pending changes since the client's last known state.
DocAddress variants determine save policies: `File` (LocalFirst — each client
writes own copy), `KbNode` (ServerAuthoritative — CRDT materialized to SQLite),
`Shared` (Ephemeral — `:w` prompts for path). `save_intent` now returns
`save_epoch` (monotonic per-doc). `docs/save_committed` broadcasts to peers
and records metadata (saved_by, content_hash). See ADR-007 for full protocol.

### Background Compaction + Idle Eviction (fixes B4)

Tokio background task runs every `compaction_interval_secs` (default 60s):
- Compacts all in-memory documents (WAL → snapshot)
- Evicts docs idle for `idle_eviction_secs` (default 300s)

### Editor UX

- Disconnect lifecycle: server tracks per-session docs, broadcasts `peer_left` on disconnect, `peer_joined` on connect. `connected_clients` counter wired.
- 7 commands under `SPC C` prefix (doom keymap): start, connect, disconnect, status, share, sync, doctor
- Status bar segment (priority 4): connection state with peer count
- 4 AI tools: `collab_status`, `collab_connect`, `collab_share`, `collab_doctor`
- 5 options: `collab_server_address`, `collab_auto_connect`, `collab_auto_share`, `collab_reconnect_interval`, `collab_user_name`
- Scheme API: `(collab-status)`, `(collab-synced-buffers)`
- `$/debug` method: server internals (uptime, connections, per-doc stats)

## References

- ADR-001: Server-Client Protocol
- ADR-002: Text Sync Model (superseded by this ADR)
- ADR-005: KB as CRDT
- Yjs internals: https://github.com/yjs/yjs/blob/main/INTERNALS.md
- yrs crate: https://docs.rs/yrs
