# What a per-KB change index costs on the write path: ADR-074 Amendment 1, C1

**Status:** Measured 2026-11-12, against `main` at `8cb10688`.
**Question:** C1 writes a change index (bump a per-KB `kb_feed.head` row, upsert one `kb_changes`
row per node) inside every update's `wal_append` transaction. Is "+10% at p99 on `apply_update`"
(Open question 2) a realistic budget at the hosted scale, or is this a scaling problem to design
around now?
**Artifacts (re-runnable, `#[ignore]`d):**
- `daemon/src/storage_change_index_bench_tests.rs`: the prototype and fixture.
- `daemon/src/storage_change_index_bench_run_tests.rs`: the scenarios.
- `daemon/src/storage_change_index_bench_ckpt_tests.rs`: the causal checkpoint test.

```
cd daemon && cargo test --release --lib storage::change_index_bench::run::change_index_bench -- --exact --ignored --nocapture
cd daemon && cargo test --release --lib change_index_checkpoint_cause -- --ignored --nocapture
```

## Bottom line

1. **+10% at p99 is not achievable by any in-transaction index, and it is the wrong bar.** The
   cost is not the statements' CPU. It is the WAL frames they add to every commit: 6.20 → 8.44 per
   commit for C1. That moves the 1000-page auto-checkpoint from every 161 commits to every 118. The
   baseline sits just under the point where the checkpoint (an fsync) shows up in p99, and any
   index pushes it over. Single-writer p99 goes 0.42 → 4.2 ms, a step, not a slope. With
   auto-checkpoint off, the same comparison is 183 → 325 µs. The two cheaper shapes tried
   (`WITHOUT ROWID`, and `seq` = the WAL id with no counter row) take the same step.
2. **At the hosted load it is affordable.** That load is ADR-109's 20 concurrent users, run here as
   32 writers × 5 writes/s = 162 writes/s through `apply_update`. C1 adds about 0.1 ms at p50
   (0.62 → 0.75 ms) and about +12–50% at p99, on a p99 that is already 25 ms on this machine. Index
   size is 76 B per node, 0.7% of ADR-109's 10.4 KB per node, and bounded by the node count.
3. **The first real scaling risk is not C1. It is how the store resolves write-lock collisions,
   and C1 amplifies it.** The pool's 4 shard connections share one SQLite file, so writers on
   different shards collide on its single write lock. SQLite resolves a collision by **sleeping**
   (1, 2, 5, 10 … ms), not queueing, so a longer transaction loses disproportionately:
   - closed-loop throughput drops 1,296 → 857 writes/s (−34%) with 32 writers;
   - at 640 writes/s, p50 goes 0.57 → 2.2 ms.

   **Serializing writers in-process with one FIFO lock removes this.** C1 behind that lock runs at
   the baseline's throughput (1,280 vs 1,296/s), and its p99 with 32 writers (58 ms) beats
   *today's* baseline (392 ms).
4. **A build without `HAVE_USLEEP` makes the same risk severe today, with or without C1.** That is
   any build that falls back to `sqlite3-src`'s bundled SQLite, which includes this repo's
   `Dockerfile`. There, every collision sleeps **one second**. Unmodified `main` at the hosted
   rate (162 writes/s offered) delivered **13–25 writes/s**, with p99 of 5–10 s. With 8 writers in
   a closed loop, `apply_update` eventually **returned "database is locked" to a client**. The
   FIFO writer lock restores the full 162/s. See [the bundled build](#the-bundled-sqlite-build).

**Recommendation:**
- **Keep C1's in-transaction design.** Do not fall back to the in-memory coalescer it rejects. The
  measured problem is lock contention, which coalescing would not fix.
- **Before or with C1, serialize writes in-process.** Use one writer lock, or one writer
  connection, in `SqlitePool`. This pays for itself on the baseline alone.
- **Take checkpoints off the commit path:** `wal_autocheckpoint=0` plus a background PASSIVE
  checkpoint on a timer or frame count.
- **Restate the bar.** Use added µs at p50 and throughput at saturation, both measured on an idle
  machine. Do not use p99 ±10%: p99 here is checkpoint cadence plus noise, and its run-to-run
  spread was ±30% or more.

## Method

- **Machine:** 16 hardware threads (hybrid P/E-core laptop CPU), Linux, btrfs on NVMe. The SQLite
  is the system library, 3.51.2, except in the bundled section (3.42.0).
- **Not idle.** Other sessions' builds and a VM held load average between **7 and 45** throughout.
  Every result line records `load1`. The defence against that is pairing: within each run, every
  mode of a scenario runs back to back, in an order rotated each run. Ratios are taken per run
  against the base beside it, then the median and range across runs are reported. Absolute
  numbers are inflated by contention. On an idle machine expect lower absolutes and tighter ranges.
- **Store:** file-backed, `SqliteBackend::open` (production 4 shards). Same PRAGMAs as production:
  WAL, `synchronous=NORMAL`, `busy_timeout=5000`, `secure_delete=ON`.
- **Content:** 100,000 KB node documents (ADR-102's ceiling) across 4 KBs, 196 MB. Each node has:
  - a ~1.2 KB-body snapshot (ADR-109's per-node body) with its integrity hash;
  - three WAL-tail updates interleaved across documents, written through the real
    `editor_record::write_attributed`;
  - its editor and origin rows;
  - a full change index (one row per node).
- **Write path:** the real `DocStore::apply_update`, with `main.rs`'s settings: compact threshold
  500, max docs 4096, max WAL entries 5000, and the size limits. Each write is:
  - a 1–8 character yrs insert from a client replica of a node;
  - attributed to a principal;
  - timed from call to return.

  Writers have disjoint 64-node hot sets, round-robin over the KBs, so they spread over all shards.
  Hot docs are loaded before timing. One `wal_append`-only scenario isolates the storage cost.
- **Index prototype:** `IndexedBackend` wraps the real backend.
  - **`base`** calls the real `SqliteBackend::wal_append` untouched.
  - **`c1`** runs the same body (`BEGIN IMMEDIATE`, `write_attributed`, `commit_or_rollback`)
    with C1's two statements before commit:
    - `INSERT … ON CONFLICT DO UPDATE SET head = head + 1 RETURNING head` on `kb_feed`;
    - an upsert into `kb_changes(kb_id, subject) → seq, op`, a rowid table with a `(kb_id, seq)`
      index as the draft states it.

    After commit it signals a per-KB `tokio::sync::watch`.
  - **Variants:**
    - `c1_norowid`: the change table is `WITHOUT ROWID`.
    - `walseq`: `seq` is the WAL row id, and there is no counter row.
    - `*_serial`: the same, behind one in-process `tokio::sync::Mutex` (FIFO) around `wal_append`.
- **Oracles:**
  - After every scenario the bench asserts that an indexing mode advanced the heads by **exactly**
    one per successful write and that its change rows agree with the heads. It also asserts that
    the baseline wrote nothing to the index. A no-op prototype would cost nothing, so a cheap
    result could otherwise pass for the wrong reason.
  - The fan-out readers must each end at their KB's head, so no notice was missed.
- **Load shapes:**
  - closed loop: 1, 8 or 32 writers, as fast as possible;
  - open loop: 32 writers × 20/s (640/s) and × 5/s (162/s);
  - fan-out: 64 or 256 SSE-like streams, each waking on the watch and reading `kb_changes`
    from its cursor through the store's own connections, uncoalesced or with 250 ms coalescing.
- **Runs:** 5 runs for each scenario, except the bundled build (2 runs).

## Results

Medians over 5 runs, with [min–max]. The `vs base` columns are per-run ratios to the paired base
run.

### Single writer: the step at p99, and its cause

| | p50 µs | p99 µs | p50 vs base | p99 vs base |
|---|---|---|---|---|
| `wal_append`, base | 117 [68–191] | 522 [459–5354] | | |
| `wal_append`, c1 | 176 [154–215] | 4577 [3598–7361] | 152% [113–254] | 776% [137–998] |
| `wal_append`, c1_norowid | 200 [177–282] | 5344 [4276–7292] | 193% | 820% |
| `wal_append`, walseq | 189 [122–227] | 4330 [2905–6258] | 180% | 627% |
| `apply_update`, base | 232 [181–801] | 2921 [684–4967] | | |
| `apply_update`, c1 | 385 [294–1017] | 5307 [4089–8873] | 163% [113–170] | 179% [159–598] |

The causal test (`change_index_checkpoint_cause`, quieter machine, 5 runs) measured WAL frames per
commit and compared auto-checkpoint on and off:

| mode | WAL frames / commit | commits per auto-checkpoint |
|---|---|---|
| base | 6.20 | 161 |
| c1 | 8.44 | 118 |
| c1_norowid | 8.06 | 124 |
| walseq | 7.14 | 140 |

| `wal_append`, 1 writer | p50 µs | p99 µs | p99 vs base |
|---|---|---|---|
| base, auto-checkpoint 1000 | 81 [72–83] | 418 [325–467] | |
| c1, auto-checkpoint 1000 | 130 [124–135] | 4169 [3547–5849] | 998% [960–1252] |
| base, auto-checkpoint off | 72 [69–83] | 183 [158–283] | |
| c1, auto-checkpoint off | 132 [118–137] | 325 [282–365] | 154% [129–205] |

**Reading.**
- The p50 cost is real and stable: about **+50–60 µs** per write, **+65–77%** of a ~75 µs
  storage-only append. Most of it is the extra 2.2 pages each commit writes to the WAL.
- The p99 step is the checkpoint. Turn it off and the step disappears. What remains (+54%) is the
  same per-page cost seen at p50.
- `WITHOUT ROWID` saves 0.4 frames per commit and 22% of index bytes (59 vs 76 B per row). Dropping
  the counter row (`walseq`) saves 1.3 frames. Neither changes the conclusion, and `walseq`
  reintroduces the cross-KB counting leak C1 rejects.

### Contention: where it scales, and the fix

| scenario | mode | p50 ms | p99 ms | writes/s |
|---|---|---|---|---|
| 8 writers, closed | base | 2.5 | 34 [33–77] | 1279 |
| | c1 | 3.9 | 56 [40–76] | 890 |
| | base_serial | 2.8 | 41 [27–103] | 1732 |
| | c1_serial | 3.8 | 34 [30–74] | 1323 |
| 32 writers, closed | base | 6.4 | 392 [241–421] | 1296 |
| | c1 | 9.4 | 267 [197–661] | 857 |
| | base_serial | 11.7 | 83 [41–136] | 1889 |
| | c1_serial | 17.1 | 58 [53–177] | 1280 |
| 32 × 20/s (640/s) | base | 0.57 | 34 [23–36] | 647 |
| | c1 | 2.2 | 55 [41–80] | 645 |
| | base_serial | 0.49 | 23 [21–65] | 645 |
| | c1_serial | 0.86 | 48 [28–132] | 644 |
| **32 × 5/s (162/s)** | **base** | **0.62** | **25 [18–34]** | 162 |
| | **c1** | **0.75** | **38 [20–40]** | 162 |
| | base_serial | 0.48 | 26 [15–52] | 162 |
| | c1_serial | 0.79 | 29 [25–126] | 162 |

**Reading.**
- On today's design the store saturates at about 1,300 writes/s on this machine. C1 lowers that to
  about 860, because each collision between shard connections costs a busy-handler sleep, and
  longer transactions collide more.
- One FIFO writer lock turns collisions into queueing:
  - base throughput rises 46% at 32 writers;
  - p99 falls 4–5×;
  - C1 behind the lock matches the unserialized baseline's throughput.
- Closed-loop p50 rises under the lock because writers wait in line instead of failing fast into a
  sleep. Tail and throughput both improve.
- At the hosted rate (162/s) the store is about 12% utilized. C1's cost there is about 0.1 ms at p50.

### Fan-out reads (C1, uncoalesced unless noted)

| writers | streams | write p99 ms | reads/s | read p50 µs | read p99 ms |
|---|---|---|---|---|---|
| 32 × 20/s | 64 | 63 [54–130] | 975–6,705 | 8–38 | 21–81 |
| 32 × 20/s | 256 | 74 [42–104] | 2,125–21,654 | 12–24 | 11–47 |
| 32 × 20/s | 256, 250 ms coalescing | 78 [71–167] | 599–1,154 | 30–158 | 31–87 |
| 8, closed | 256 | 80 [73–93] | 8,234–9,641 | 10–26 | 25–30 |

**Reading.**
- The cursor query itself is cheap: tens of µs, bounded by the 1000-row page.
- Streams did not measurably slow writes: write p99 with readers sits within the range of C1 alone,
  and the offered 645 writes/s was delivered.
- A read's p99 is time spent waiting for a shard connection that a writer holds, possibly asleep
  in the busy handler. A dedicated read connection, or one query per KB fanned out in memory,
  would decouple it.
- 250 ms coalescing caps reads at streams × 4/s, as designed.
- Fan-out is not the first constraint at tens of streams.

### Index growth and the hot counter row

- **Growth.** One row per node: **76 B** (rowid) or **59 B** (`WITHOUT ROWID`) per node, 7.6 MB for
  100k nodes. It is bounded by the node count, not the edit rate. It is 0.7% of ADR-109's per-node
  figure and does not move the budget.
- **The hot counter row.** The per-KB counter adds one page per commit (c1 vs walseq, 1.3 frames).
  It adds no contention of its own, because SQLite already serializes every writer on the file.
  With 32 writers, 8 share each KB's row, and `c1` against `walseq` showed no extra tail.

## The bundled-SQLite build

`sqlite3-src` (via `sqlite3-sys`) links the system `libsqlite3` when pkg-config finds it. Otherwise
it compiles its bundled 3.42.0 with `cc` and **no defines**. Two things follow:
- **The fallback is what this repo's `Dockerfile` produces.** The builder installs `pkg-config` but
  no `libsqlite3-dev`, and `bookworm-slim` has no system SQLite.
- **That fallback leaves `HAVE_USLEEP` undefined.** `sqliteDefaultBusyCallback` then takes its
  `#else` branch: on every busy retry it sleeps **a whole second**
  (`sqlite3OsSleep(db->pVfs, 1000000)`) until `busy_timeout` (5 s) runs out.

Measured by building the same benchmark with `SQLITE3_NO_PKG_CONFIG=1`. The `FIXTURE` line reports
`sqlite=3.42.0`.

- **8 writers, closed loop, base mode, i.e. unmodified `main`:** the scenario ran for about 11
  minutes and ended with `apply_update` returning `database is locked (code 5)`. A client update
  was refused after five 1-second retries.
- **32 × 5/s (162/s offered, the hosted rate), 2 runs, load 35–56:**

  | mode | delivered writes/s | p50 ms | p99 ms | max s |
  |---|---|---|---|---|
  | base (`main`) | **13–25** | 0.2–2.2 | **5,065–10,002** | 35–62 |
  | c1 | 11–13 | 2.5–4.0 | 10,023–13,141 | 25–70 |
  | base_serial | **161–162** | 0.9–2.6 | **42–61** | 0.11–0.14 |
  | c1_serial | 160–161 | 8.7–16 | 103–163 | 0.15–0.20 |

  Unserialized, the store cannot carry the hosted rate at all. It delivers a tenth of what is
  offered, because every collision between shard connections costs whole seconds and blocks
  that shard's mutex, and with it a runtime worker thread, meanwhile. One in-process FIFO writer
  lock restores the full rate, with or without C1. The high load during this run inflates the
  serial modes' absolutes. The gap of two orders of magnitude does not depend on it.

**Whether a deployed image is built this way has to be checked against that image itself.** This
measurement establishes what the repo's build configuration produces, not what a given host runs.
If it is built this way, the fix is independent of C1:
- link a system SQLite, or compile the bundled one with `-DHAVE_USLEEP=1`;
- serialize writers in-process.

Either removes the 1-second sleeps; the serialization also removes collisions altogether.

## What was not measured

- **Collection-document writes.** C1 diffs the manifest under the document lock. That is CPU in
  `apply_update`, not storage, and is proportional to the manifest size.
- **The replace path, mesh relay and seed apply.** They take the same `wal_append`.
- **Compaction.** It runs every 500 updates per document, in its own transaction; under 0.2% of
  writes, so outside p99.
- **macOS, and an idle machine.** Per principle #13 these figures are one platform's. The paired
  ratios are the robust output; the absolutes are not.
