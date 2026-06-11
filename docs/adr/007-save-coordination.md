# ADR-007: Save Coordination Protocol

**Status**: Accepted
**Date**: 2026-05-19
**KB Source**: `concept:save-coordination`

## Context

MAE's collaborative editing (ADR-006) synchronizes CRDT state between clients
via the daemon, but has no protocol for coordinating file saves. Multiple
clients editing the same document need clear answers to:

1. Who writes to disk? When?
2. What happens when a joiner has no local copy of the file?
3. How do peers know a save occurred?
4. What prevents overwriting concurrent edits?

Without save coordination, clients save blindly, joiners can't `:w`, and
disconnect behavior is undefined.

## Decision

### Document Types and Save Policies

Every collaborative document has a **type** derived from `DocAddress`
(`crates/sync/src/lib.rs`):

| DocAddress variant | Save Policy | `:w` behavior | Source of truth |
|-------------------|-------------|--------------|-----------------|
| `File { project_hash, rel_path }` | **LocalFirst** | Each client writes to their own `{project_root}/{rel_path}`. File created on first save if it doesn't exist. | CRDT (real-time), local filesystem (durable) |
| `KbNode { node_id }` | **ServerAuthoritative** | KB owner client persists CRDT to SQLite. Other clients see "KB node saved" status. | CRDT (real-time), SQLite (durable) |
| `Shared { name }` | **Ephemeral** | `:w` prompts for file path (like scratch buffer). | CRDT (real-time), server WAL (crash recovery only) |

### Save Protocol: `save_intent` / `save_committed`

The protocol uses two JSON-RPC methods for coordination:

**`docs/save_intent`** (client -> server):
```json
{ "doc": "daily.org", "expected_hash": "<sha256>" }
```
Server compares `expected_hash` against CRDT content hash. Returns:
- `{ "status": "ok", "server_hash": "<sha256>", "save_epoch": N }` — safe to save
- `{ "status": "conflict", "server_hash": "<sha256>" }` — client must sync first

**`docs/save_committed`** (client -> server):
```json
{ "doc": "daily.org", "save_epoch": N, "content_hash": "<sha256>", "saved_by": "alice" }
```
Server records metadata and broadcasts `save_committed` notification to peers.
Peers mark their buffer as clean (content matches what was saved).

### Save Epoch Tracking

The server maintains a monotonically increasing `save_epoch` per document.
Each successful `save_intent` increments the epoch. The epoch prevents
stale `save_committed` from a slow client from being associated with the
wrong save_intent.

### Disconnect Lifecycle

| Scenario | Behavior | Rationale |
|----------|----------|-----------|
| Graceful quit (`:q`) | TCP close -> server detects EOF -> broadcasts `peer_left` -> doc persists | Standard TCP lifecycle |
| Client crash | TCP keepalive timeout -> same as graceful | OS cleans up socket |
| Network drop | Write timeout (5s) -> server drops client -> broadcasts `peer_left` | Bounded queues prevent blocking |
| Last client leaves | Doc stays in memory + WAL. Idle timer starts. Compacted + evicted after `idle_eviction_secs`. | Prevents data loss from temporary disconnects |
| Client reconnects | Gets latest CRDT state via `sync/diff`. Status bar shows "remote changes available". | Client decides when to write locally |

### File Path Resolution (Joiners)

When a client joins a `File`-type document:
1. Extract `rel_path` from `DocAddress`
2. Try `{project_root}/{rel_path}` (if project context exists)
3. Try `{CWD}/{rel_path}` as fallback
4. **Always set file_path** — the file may not exist yet (created on `:w`)
5. On save, create parent directories if needed (`create_dir_all`)

### Concurrent Save Behavior

Both clients can `:w` independently. `save_intent` checks the hash against
the CRDT content, not against another client's file. No lock contention.
CRDT is the single source of truth for content correctness.

### Git Workflow

CRDT sync and git are complementary, not competing:
- CRDT handles real-time collaboration
- Git handles version history
- Each client commits to their own worktree
- `DocAddress::File` uses `project_hash` for worktree disambiguation
- `git push/pull` reconciles between machines

## Consequences

- **Each client writes their own copy** (LocalFirst for files)
- **Server is coordination point**, not file server — never touches the filesystem
- **Joiners get content via CRDT**, write to their own `{project_root}/{rel_path}`
- **`save_committed` enables peer notification** without requiring file sharing
- **Save epoch prevents stale-commit confusion**
- **No ownership concept** — doc outlives its creator (Google Docs model)

## Irreversibility Assessment

| Decision | Reversible? |
|----------|-------------|
| LocalFirst save policy | YES — can add server-side save later |
| save_intent/save_committed protocol | YES — additive protocol extension |
| Per-client file writes | YES — can add shared filesystem later |
| Save epoch tracking | YES — monotonic counter, trivial to extend |

## References

- ADR-003: File Safety
- ADR-006: Collaborative State Engine
- Google Docs save model (doc outlives creator)
- VS Code Live Share (each client has own workspace)
