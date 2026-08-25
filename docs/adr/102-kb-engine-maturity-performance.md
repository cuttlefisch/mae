# ADR-102: KB engine maturity & performance for the hosted daemon

**Status:** Proposed. **Evidence-gated** — the decision between staying on Cozo and replacing it is
deferred behind a benchmark (Phase 0) rather than argued from taste. Phased; Track 2 opens **only if**
Track 1 misses the bar.

**Extends:** ADR-004 (KB scaling — its Tier-1 "5–10 concurrent editors" figure) and ADR-012
(persistent graph KB — its RocksDB rejection, made on 2024 grounds that this ADR re-examines).
**Supersedes-with-evidence:** ADR-054's benchmarked capacity figure, once re-measured at the new bar
(principle #15 / ADR-054's own precedent of benchmarking rather than asserting).
**Relates to:** ADR-101 (structured edges add per-projection work the benchmark must include), ADR-060
(daemon multi-tenancy — per-tenant stores are the load shape), ADR-103 (the enricher is itself a heavy
concurrent writer this engine must absorb), ADR-011 (the daemon-never-persists-collab bug — a
correctness prerequisite orthogonal to throughput).

**Evidence:** `shared/kb/Cargo.toml`, `shared/kb/src/cozo_store/{schema.rs,db.rs}`,
`shared/kb/src/{migrate.rs,backup.rs,data_dir.rs}`, `daemon/benches/kb_dispatch_concurrency.rs`,
ADR-004 §Correction, ADR-054 §Verification, ADR-012 backend table, commits `9255b23a`, `8f9cbb39`,
`#484`, `#447`, `43acda99`.

## Context

MAE is about to host a multi-tenant daemon for **tens of concurrent humans + AI agents**, and a new
24/7 background enricher (ADR-103) that is *itself* a sustained concurrent writer. The KB storage
engine is the foundation all of that sits on, and the evidence says it is the single largest risk in
the enterprise story. The risk has **two layers** that must be named separately, because a fix at one
does not fix the other.

### Layer 1 — the storage backend under Cozo

Only two Cozo storage backends are compiled in (`shared/kb/Cargo.toml`: `storage-sled`,
`storage-sqlite`; cozo is `default-features = false, features = ["rayon"]`, pinned `"0.7"`).

**Sled is a documented liability, not a hypothesis:**
- Unmaintained since 2021 (stated in ADR-012).
- Takes an **exclusive directory lock** — a second process cannot open the same store. This is *the*
  reason for the sled→sqlite migration (`shared/kb/src/migrate.rs`: *"sled takes an exclusive dir
  lock; sqlite/WAL allows multiple processes"*).
- Has an internal open-panic MAE had to wrap rather than crash on (commit `9255b23a`, *"recover from
  sled's internal open panic instead of crashing"*).
- Once silently blocked multi-frontend sharing because federated instances were hardcoded to it
  (commit `43acda99`).

**cozo-0.7.6's sqlite backend is bottlenecked and held together by application-level workarounds:**
- It **never sets `journal_mode=WAL` or `busy_timeout`** — verified by direct inspection of
  cozo-0.7.6 source, recorded in ADR-004's Correction section (not carried forward unverified).
- `DbInstance::new` **panics** on `SQLITE_BUSY` rather than returning a retryable `Result`, and the
  BUSY is **hidden behind an opaque `Display`** string (`"CozoDB: when executing against relation
  '…'"` — the words "locked"/"busy" never surface; `db.rs`). MAE detects contention by pattern-
  matching that lossy string — fragile to any cozo upgrade.
- So MAE hand-rolls **three** compensations: a wall-clock-bounded busy-retry around every script
  (raised to 45s after CI exhaustions, `#484`, `db.rs`), an open-path panic-retry
  (`retry_on_transient_sqlite_busy`, `schema.rs`), and an advisory lock serializing first-time store
  creation (`#447`, `8f9cbb39`) because concurrent `create table if not exists` `.unwrap()`s inside
  cozo's own bootstrap.
- There is **no WAL-checkpoint hook** — cozo exposes no `Connection`, so the app cannot manage WAL
  growth.

**The measured ceiling** (ADR-054, the only benchmarked multi-session number in the repo): a real
`mae-daemon` against a **20K-node store**, criterion `kb_dispatch_concurrency.rs`, real Unix-socket
`kb/search` clients — **p99 stays within 2× baseline up to ~8 concurrent sessions**, then degrades
smoothly (16→241ms, 64→734ms). Multi-tenant gives no savings (each tenant a dedicated store, no cache
overlap). The benchmark is manually-run and hardware-dependent.

### Layer 2 — Cozo itself

Cozo is a **single-maintainer** project, pinned at 0.7 for ~2 years and never upgraded despite the
bugs above, exposing panics-instead-of-Results, opaque errors, and no pragma hooks. For a hosted
service this is a maturity question independent of which backend sits under it. It is also a *deep*
commitment: every Datalog query, the HNSW vector index, and the CRDT→cozo projector (ADR-029) assume
Cozo. Replacing it is a large migration, not a swap.

### Layer 3 — the abstraction leaks (why "just add a feature flag" is false)

`open_with_engine(path, engine)` (`schema.rs:70`) parametrizes the engine string, but engine-specific
assumptions are scattered: `backup.rs` copies a single `kb.sqlite` **file**; `data_dir.rs` hardcodes
`kb.sqlite`; `migrate.rs` detects sled by `is_dir()`; the busy-retry predicate is sqlite-worded. A
RocksDB store is a **directory**, so backup/migration/layout all need a real backend seam — estimated
~500 lines of refactor + tests, not ~50.

### The bar

"Enterprise-grade" needs a number. **Target: ~50 concurrent users + agents over a 100K-node store**,
holding p99 within a stated SLO — roughly a 6× jump over today's measured ~8 sessions and ~5× the 20K
test store.

## Decision

### D1 — Evidence-gated, two-track evaluation (do not pre-commit to an engine)

The choice between "tune Cozo" and "replace Cozo" is made by benchmark, not argument. **Track 1 runs
first; Track 2 opens only if Track 1 misses the bar.**

### D2 — Track 1: Cozo-on-RocksDB + concurrency architecture (do first)

- Enable cozo `storage-rocksdb` behind a **daemon-only** feature flag and `open_with_engine("rocksdb")`,
  **including the cozo version upgrade RocksDB forces** (`storage-rocksdb` does not exist in 0.7.6 per
  ADR-012) — and prove the upgrade does not regress Datalog, HNSW, or the projector before anything
  else. This upgrade is a first-class risk with its own gate, not an incidental.
- Where RocksDB removes the cause of a sqlite workaround (BUSY panics, WAL absence), **retire that
  workaround** rather than carry both.
- **Concurrency architecture, not just the store.** The ~8-session ceiling is partly per-node query
  cost at scale, so Track 1 pairs the backend with read-path work (caching, the ADR-054 snapshot +
  `spawn_blocking` seam) to actually reach ~50. The ADR must state plainly what the store swap alone
  does *not* buy — a faster backend does not remove per-query CPU cost.

### D3 — Backend-abstraction seam (required by Track 1, reusable by Track 2)

Give `KbStore` a clean backend seam so backup, data-dir layout, migration, and busy-retry stop
assuming sqlite/file. This is a precondition for RocksDB (directory store) and is exactly the seam a
Track-2 engine would plug into, so it is not wasted work if Track 2 opens.

### D4 — Per-deployment choice; no daemon-less regression

Hosted daemon → RocksDB (or the Track-2 engine). **The daemon-less editor stays on sqlite** —
portability, light build (no 35MB C++ dep on a laptop), few processes, and the multi-instance sqlite
work already shipped for exactly that case. **Sled is deprecated for new stores** (existing sled
stores still auto-migrate to sqlite; the migration is not removed).

### D5 — Escalation gate: Track 2 (replace Cozo) opens only on a missed bar

If Track 1 cannot hit ~50 users / 100K nodes within SLO, open a head-to-head evaluation of
client-server / higher-concurrency engines — **Postgres + pgvector + Apache AGE**, **KùzuDB**,
**Neo4j** — scored on: concurrent-write throughput, vector search, graph/Datalog expressiveness vs
MAE's actual query set, embeddability vs operational weight, maturity/backing, and migration cost
(every Datalog query + HNSW + the projector). Track 2 is **named, scoped, and deferred**, not
silently omitted and not pre-decided.

## Consequences

**Positive** — a benchmarked capacity figure at the real bar; the sqlite workaround debt shrinks; a
proper backend seam; the daemon-less path is protected; the Cozo-maturity question is confronted with
evidence instead of avoided.

**Negative / risks** — the cozo upgrade may itself regress or stall (it's why 0.7 was frozen);
RocksDB adds 35MB + C++ build/CI/Windows complexity (server-side only, acceptable); the ~500-line
seam is real work before any throughput gain; if Track 2 opens, it is a major migration. All are named
so none is a surprise.

## Explicitly out of scope
- The enrichment feature (ADR-103) and structured edges (ADR-101), except as benchmark load.
- Distributed/sharded multi-node scale (100s of users / millions of nodes) — a later ADR if the bar
  rises; TiKV is noted as the Cozo path there but not pursued now.
- Fixing ADR-011's daemon-persistence bug — a correctness prerequisite tracked there and cross-linked,
  not owned here.

## Phase 0 benchmark protocol (the gate — this is the deliverable that decides D5)
Extend `daemon/benches/kb_dispatch_concurrency.rs` to a **realistic mixed workload**, not search-only:
- Store: **100K nodes / ~600K links** (5× the current roamnotes-scale test), generated reproducibly.
- Clients: **1 / 8 / 16 / 32 / 50 / 64** concurrent real Unix-socket clients, mix ≈ 70% read
  (search/get/links) + 30% write (node upsert + structured-edge upsert — the ADR-103 enricher shape).
- Engines: **cozo-sqlite (today)** vs **cozo-rocksdb (post-upgrade)**, same hardware, same seed.
- SLO: p99 ≤ 2× single-client baseline at 50 concurrent (the ADR-054 SLO shape, at the new bar).
- Report p50/p99 per engine per concurrency, write-contention failure/retry counts, and store-open
  behavior under concurrent first-create. **Pass = cozo-rocksdb holds SLO at 50; miss = open Track 2.**

## Adversarial tests / measurements (principle #14)
- Concurrent writers on the chosen backend **converge with zero lost writes** at 50-way (not just
  "didn't error").
- Store-open under concurrent first-create does not panic (the `#447` failure mode) on the chosen
  backend.
- Kill-9 mid-write recovers a consistent store on reopen (WAL/crash recovery).
- The cozo upgrade preserves Datalog/HNSW/projector output byte-for-byte on a fixture corpus (upgrade
  regression guard).
- Backend seam: backup + restore round-trips a **directory** RocksDB store, not just a file (the
  `backup.rs` assumption).
- Daemon-less sqlite path shows **no regression** on the existing scale test after the seam refactor.
