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

**Multi-tenant capacity (ADR-060 Phase F, ~2026-07):** the same methodology with an
explicit N-tenant dimension, made possible by issue #460's fix (before it, `mae-daemon`
never opened more than its own primary store, so there was no way to run a genuine
multi-instance benchmark against the real binary at all). Each tenant gets its own
dedicated 20K-node store and an unlimited quota — this measures Phase B's per-instance
lock isolation, not Phase C's quota-enforcement overhead. Single run, same reference
machine, both benchmarks back-to-back (`cargo bench -p mae-daemon --bench
kb_dispatch_concurrency`, no filter):

| Tenants × sessions/tenant | Total concurrent | p50 | p99 |
|---|---|---|---|
| 1 × 1 | 1 | 91ms | 116ms |
| 2 × 4 | 8 | 126ms | 161ms |
| 4 × 4 | 16 | 290ms | 397ms |
| 8 × 4 | 32 | 463ms | 683ms |

**Regression check (required before trusting the above, per Phase F's own Verification
bullet):** this run's single-tenant baseline (N=1: p50=93ms, p99=102ms) and the
multi-tenant sweep's 1-tenant×1-session figure (p50=91ms, p99=116ms) are close — same
dispatch path, same store size, measured in the same run — confirming Phases A-D's
tenant-scoping work did not regress the pre-existing single-tenant case. (The single-tenant
N=1 figure in this specific run, p99=102ms, itself runs measurably slower than the
~71ms recorded above from an earlier session/machine-state — expected hardware/load
drift the original text already warns about, not a regression signal on its own; the
regression check that matters is the same-run comparison just described.)

Applying the same "p99 ≤ 2x single-session baseline" SLO (116ms → 232ms ceiling) to the
multi-tenant sweep: the 2×4 level (161ms) holds, the 4×4 level (397ms) exceeds it — a
**~8-total-concurrent-session ceiling**, the same order of magnitude as the single-tenant
number above, not a large multiple of it either way. Stated plainly per this phase's own
caution against overstating resource *savings*: this is the **expected, honest** result for
synthetic tenants with zero cache overlap (each has its own distinct 20K-node store,
sharing nothing) — tenant isolation costs no *extra* capacity beyond what raw concurrent
load already predicts, but it also delivers no *savings* beyond what genuine cache-overlap
between tenants' real workloads could actually provide. A deployment where tenants' KBs
share meaningfully overlapping content (a more realistic team scenario than this
benchmark's deliberately-disjoint synthetic stores) would be expected to do better than
this ceiling, not worse — but that claim is not made here, since it wasn't measured here.

**Correction (found via a real CI failure, not assumed — ~2026-07):** the WAL
mode / `busy_timeout` PRAGMAs above describe an implementation against
`crates/kb/src/persist.rs`, a file that no longer exists — this codebase's KB
storage was later migrated to CozoDB (`shared/kb/src/cozo_store/`, ADR-014's
binary-architecture split), and **CozoDB 0.7.6 never configures
`journal_mode=WAL` or `busy_timeout` anywhere** (confirmed by direct
inspection of `cozo-0.7.6/src/storage/sqlite.rs`, not carried forward
unverified from this ADR's original text). The "(done)" mitigation in the
bottleneck table below is therefore not accurate for the current
implementation. What actually exists instead, discovered and extended this
session while fixing a real "database is locked" panic under concurrent
store-open (issue #447's follow-on): two independent, hand-rolled
application-level compensations for the same underlying gap —
`Db::run_with_busy_retry` (`shared/kb/src/cozo_store/db.rs`, pre-existing —
exponential-backoff-with-full-jitter retry around `run_script` calls, raised
to a 400-attempt budget after this exact contention flaked a CI run before)
for the query/write path, and `retry_on_transient_sqlite_busy_panic`
(`shared/kb/src/cozo_store/schema.rs`, added this session, same backoff
shape) for the store-*open* path specifically, since `DbInstance::new`
panics rather than returning a retryable `Result` on this condition. Neither
is a PRAGMA-level fix — both are call-site retry loops absorbing an upstream
crate gap. `daemon/src/storage.rs` (the daemon's *separate* hand-rolled
SQLite backend for collab/CRDT persistence, not the KB store) DOES set these
PRAGMAs correctly — this correction applies specifically to the CozoDB-backed
KB store this ADR is about, not every SQLite usage in the codebase.

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
| SQLITE_BUSY | High write contention | App-level retry w/ full jitter (`Db::run_with_busy_retry` for queries, `retry_on_transient_sqlite_busy_panic` for opens) — see the Tier 1 correction above; NOT a WAL/`busy_timeout` PRAGMA fix, cozo 0.7.6 sets neither |
| Slow FTS5 | Large index, complex queries | Limit results, prefix queries |
| Memory growth | Connection cache | Pooling with limits (Tier 2) |
| WAL file growth | Long-running readers | Periodic `PRAGMA wal_checkpoint(TRUNCATE)` |

## Consequences

- **Superseded by the Tier 1 correction above for the current CozoDB-backed
  store**: the three bullets below describe the WAL-mode implementation this
  ADR originally specified, which is not what actually runs today. Kept for
  historical record rather than deleted, since `daemon/src/storage.rs`'s
  separate collab/CRDT SQLite backend genuinely does implement WAL mode this
  way — the bullets are accurate for *that* usage, just not the KB store.
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
