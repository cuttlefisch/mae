# ADR-014: Binary Architecture Split

**Status:** Accepted
**Date:** 2026-06-04
**Supersedes:** None

## Context

MAE was a monolithic binary (`crates/mae/src/main.rs`) that linked every crate
in the workspace. This caused three concrete problems:

1. **Linker conflict blocks CozoDB SQLite**: `rusqlite` (in the daemon workspace) and
   `cozo/storage-sqlite-src` both declare `links = "sqlite3"` in their Cargo
   manifests. Same workspace = duplicate symbols. CozoDB was forced to use
   `storage-sled` (unmaintained since 2021, SIGABRT on nightly Rust).

2. **No background KB maintenance**: File watchers, ingestion, hygiene,
   embeddings, and activity decay all ran in the editor's idle tick. Editor
   closes = all background work stops.

3. **KB memory scales linearly**: Sled loads all keys into RAM at open time.
   No bounded memory regardless of KB size.

## Decision

Split into two workspaces with separate `Cargo.lock` files:

### Repository Layout

```
mae/                              (repo root)
├── Cargo.toml                    (editor workspace — cozo+sled, rusqlite OK)
├── Cargo.lock                    (editor lock)
├── crates/                       (17 editor-only crates)
├── daemon/                       (daemon workspace — cozo+sqlite, no rusqlite)
│   ├── Cargo.toml
│   ├── Cargo.lock                (independent resolution)
│   └── src/
└── shared/                       (crates used by both workspaces)
    ├── kb/                       (mae-kb: CozoDB store, org parser, federation)
    ├── sync/                     (mae-sync: yrs CRDT, ropey bridge)
    └── mcp/                      (mae-mcp: JSON-RPC protocol, DaemonClient)
```

### Editor Process (mae)

Owns all interactive state: buffers, modes, keybindings, rendering, undo/redo,
AI conversation, LSP/DAP. The **local sled-backed embedded CozoDB is the floor**
— a daemon-less editor runs the full KB (read/edit, agenda, search, projection)
in-process, standalone (ADR-035, `daemon_mode=off`). When a daemon is connected,
the editor *optimizes* KB access through a **bounded LRU cache** (200 nodes ≈
400KB) backed by daemon RPC instead of the local store — an optimization, not a
requirement.

### Daemon Process (mae-daemon)

Owns KB persistence (CozoDB+SQLite), background maintenance, file watching,
ingestion pipeline. Communicates via JSON-RPC over Unix socket using the same
Content-Length framing as the MCP server.

The daemon is **optional** — the editor works standalone with the local sled KB
as its floor (ADR-035). The daemon earns its place only by an objective value
category — SHARED across frontends, OUTLIVES editor sessions, COORDINATES peers,
or DURABILITY — never as a hard dependency for single-user KB/AI/IDE features.

### LRU Cache + Daemon RPC

```
Editor                                  Daemon
┌─────────────────────┐                 ┌──────────────────┐
│ LruQueryLayer       │ ── JSON-RPC ──→ │ CozoKbStore      │
│  cache: LRU<200>    │                 │ (SQLite backend)  │
│  ~400KB bounded     │                 │ FederatedQuery    │
└─────────────────────┘                 └──────────────────┘
```

- `LruQueryLayer` implements `KbQueryLayer` with bounded LRU cache
- Cache hits: <1μs (in-process HashMap lookup)
- Cache misses: 5-15ms (JSON-RPC round-trip to daemon)
- `DaemonClient`: synchronous blocking I/O (KbQueryLayer trait is sync)
- `KbContext::query_layer()` returns daemon query or local query transparently

### Configuration

Three new options in OptionRegistry (`daemon_enabled`, `daemon_socket`,
`daemon_cache_size`) with config.toml `[daemon]` section and Scheme API.

## Alternatives Considered

### Shared SQLite file (rejected)

CozoDB caches relation metadata in-memory at open time. Two CozoDB processes
on the same SQLite file creates a stale catalog problem — daemon writes aren't
visible to editor's CozoDB handle without re-opening.

### Emacs-style monolithic daemon (rejected)

Emacs daemon owns ALL state; clients are pure display. Creates stale state
issues, display/terminal mismatches, and no resilience — daemon crash = total
data loss. MAE's daemon is independently restartable.

### VS Code three-process model (rejected)

Over-engineered for MAE. We don't need Main/Renderer/ExtHost separation — our
TUI/GUI renderers are compile-time backends, not separate processes.

## Consequences

- CozoDB can use SQLite backend in daemon (linker conflict resolved)
- Editor memory is bounded regardless of KB size
- Background KB maintenance survives editor close
- Two `Cargo.lock` files to maintain
- CI builds both workspaces (`make ci-all`)
- Release artifacts include `mae-daemon` binary + systemd unit
