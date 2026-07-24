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

### Tier 1: Single-Machine (< 20K nodes, ~8 concurrent MCP sessions at a p99 ≤ 2x-baseline SLO) — IMPLEMENTED

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

**Measured capacity (ADR-054, ~2026-07):** the "5-10 concurrent editors" figure
above was an unverified estimate — before ADR-054's daemon concurrency
hardening, every KB Unix-socket read/write RPC held a single global
`Arc<Mutex<DaemonState>>` across the entire synchronous CozoDB call
(`daemon/src/handler.rs`), which would have serialized concurrent sessions
regardless of what this section claimed. ADR-054 replaced that with a
snapshot-then-drop + `spawn_blocking` pattern (relying on Cozo's own
in-process `relation_locks`/`running_queries` concurrency control, not a new
app-level lock) and added a `criterion` benchmark
(`daemon/benches/kb_dispatch_concurrency.rs`) that spawns the real
`mae-daemon` binary against a **20,000-node** store (matching this section's
own "< 20K nodes" framing) and drives 1/4/8/16/32/64 concurrent real
Unix-socket `kb/search` clients, measuring p50/p99 latency per level. Result
on the reference dev machine:

| Concurrent sessions | p50 | p99 |
|---|---|---|
| 1  | 53ms  | 71ms  |
| 4  | 62ms  | 99ms  |
| 8  | 73ms  | 95ms  |
| 16 | 143ms | 241ms |
| 32 | 285ms | 393ms |
| 64 | 551ms | 734ms |

Applying an SLO of "p99 stays within 2x the single-client baseline"
(71ms → 142ms ceiling) yields **~8 concurrent MCP sessions** before that SLO
is exceeded — coincidentally close to the old unverified figure, but now for
a verified, different reason: degradation past that point is smooth (roughly
linear with session count out to 64), not a contention cliff, meaning the
remaining bottleneck at higher counts is genuine CPU/query cost per search
against a 20K-node store, not lock serialization. Re-run via
`cargo bench -p mae-daemon --bench kb_dispatch_concurrency`; figures are
hardware-dependent and will drift — re-measure before quoting this table in
anything customer-facing.

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
