# ADR-004: Knowledge Base Scaling Architecture

**Status**: Accepted (Tier 1 implemented)
**Date**: 2026-05-16
**KB Source**: `concept:adr-kb-scaling`

## Context

MAE's knowledge base uses SQLite with FTS5 for full-text search. The current
deployment serves a single user with ~500 nodes. As MAE moves toward
multi-client and team environments, the KB needs to scale.

### Current Baseline

- ~500 nodes, <5ms search latency
- Single `Connection::open()` per operation (no pooling)
- No WAL mode (default rollback journal)
- Schema version 5, migration chain v1-v5

## Decision

### Tier 1: Single-Machine (< 20K nodes, 5-10 concurrent editors) — IMPLEMENTED

Enable WAL mode and SQLite pragmas for concurrent access:

```sql
PRAGMA journal_mode = WAL;       -- concurrent readers + single writer
PRAGMA busy_timeout = 5000;      -- 5s retry on SQLITE_BUSY
PRAGMA synchronous = NORMAL;     -- safe with WAL, better performance
```

**Implementation**: Added to `init_schema()` in `crates/kb/src/persist.rs`.

**Performance impact**:
- Read latency: unchanged (<5ms)
- Write latency: slightly improved (WAL batches writes)
- Concurrent reads: now safe during writes
- SQLITE_BUSY failures: reduced (5s retry)

### Tier 2: Multi-Instance (20-100 users, <100K nodes) — PLANNED

- Dedicated `mae-kb-server` microservice (async tokio-based)
- Connection pooling (`deadpool-sqlite` or `r2d2-sqlite`)
- Write-ahead buffer: queue writes to 50ms batches
- Read replicas for search-heavy workloads
- FTS5 performance at scale: ~50ms at 100K nodes (acceptable)

### Tier 3: Enterprise (100+ users, 500K+ nodes) — DEFERRED

- PostgreSQL + pgvector for semantic search
- Write sharding by namespace prefix
- Event sourcing for real-time sync
- Streaming logical replication to read replicas

## Performance Expectations

| Dataset | Index Size | Search Latency | Rebuild Time |
|---------|-----------|---------------|-------------|
| 1K nodes | 2MB | <1ms | 10ms |
| 10K nodes | 20MB | 2-5ms | 50-100ms |
| 100K nodes | 200MB | 10-20ms | 500-800ms |
| 1M nodes | 2GB+ | 50-100ms | 3-5s |

## SQLite Bottlenecks to Monitor

| Symptom | Cause | Mitigation |
|---------|-------|-----------|
| SQLITE_BUSY | High write contention | WAL + busy_timeout (done) |
| Slow FTS5 | Large index, complex queries | Limit results, prefix queries |
| Memory growth | Connection cache | Pooling with limits (Tier 2) |
| WAL file growth | Long-running readers | Periodic `PRAGMA wal_checkpoint(TRUNCATE)` |

## Consequences

- WAL mode creates `kb.db-wal` and `kb.db-shm` files alongside the database.
  These are normal SQLite WAL artifacts.
- `busy_timeout` means KB operations may block for up to 5 seconds under
  contention instead of failing immediately.
- `synchronous = NORMAL` is safe with WAL — data integrity is maintained on
  crash. The tradeoff is that the most recent transaction might be lost on
  power failure (not process crash).

## References

- SQLite WAL documentation: https://sqlite.org/wal.html
- SQLite `busy_timeout`: https://sqlite.org/pragma.html#pragma_busy_timeout
