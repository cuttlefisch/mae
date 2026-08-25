# ADR-108: SQLite is the *embedded* KB storage backend

**Status:** Accepted. Finalises a choice that had been effectively made at runtime for some time
while remaining contradictory in declarations, defaults and shipped artifacts — and, after a
dedicated spike (D6), closes the hosted-engine question too.
**Corrects:** ADR-102 D2 and, by implication, ADR-012's citation trail — `storage-rocksdb` **does**
exist in cozo 0.7.6, so Track 1 needs no version upgrade and the "first-class risk with its own
gate" it was given is not real. See D7.
**Relates to:** ADR-102 D4 — this ADR does **not** supersede its evidence gate, but it does
invalidate the *baseline* that gate would be measured against: the benchmark was measuring MAE's own
query defects (#753), not the engine. See D5.
**Relates to:** ADR-004/ADR-012 (CozoDB as the KB backend), ADR-029/ADR-053/ADR-037 (why the embedded
engine can never be demoted to a cache), ADR-035 (the dual-mode tax), ADR-104 (system KBs; its
objection to shipping sled), ADR-106 (`node_versions` durability), issue #717 (sled heap corruption),
issues #753 / #687.

> **Reversal on the record (principle #17).** An earlier draft of D6 called Postgres "a live
> candidate" for the hosted daemon and expected a split — editor embedded, daemon on a server engine.
> The spike reversed that. The reversal is written into D6 rather than edited away, because the
> reasoning that produced it (docker-compose deployment makes a DB container ordinary ops) is still
> sound and will be re-proposed by someone who has not seen the counter-evidence.

## Context

MAE is about to publish into a real multi-user environment. The storage story cannot go into that
release contradicting itself, and today it does — in seven places at once.

**What each part of the tree currently believes:**

| Declaration | Backend |
|---|---|
| `mae-kb` default feature | **sled** |
| `crates/core` | sled + sqlite |
| `crates/mae` (the binary) | **sled only** |
| `crates/ai` | **sled only** |
| `crates/scheme` | **sled only** |
| `daemon` | **sqlite only** — cannot open a sled store at all |
| `CozoKbStore::open()` | **sled** |
| `kb_storage_engine` option | **sqlite** |
| `RELEASE_ASSET_ENGINE` | **sled** |
| ADR-102 D4 | RocksDB, for the hosted daemon, gated on a benchmark that has not run |

Three consequences follow, and none of them is theoretical:

1. **The binary's sqlite support is accidental.** `crates/mae` declares sled only and receives
   sqlite purely through Cargo feature unification from `crates/core`. Its own default option
   (`kb_storage_engine = "sqlite"`) therefore depends on a feature it does not declare. Nothing
   fails today; nothing would warn if `crates/core` stopped requesting it either.

2. **The daemon cannot read a sled store.** It compiles `storage-sqlite` only. So any sled store the
   editor creates is not merely slower, it is *invisible* to the daemon — a silent split-brain
   between the two halves of the same product.

3. **The one path still creating sled stores is the one that corrupted.** Issue #717 is a heap
   corruption abort in `build-manual-kb`, which uses `RELEASE_ASSET_ENGINE = "sled"`.

Meanwhile the *runtime* has already chosen. First-run provisioning builds sqlite explicitly, with a
measured 8–14x speedup over sled recorded at the call site; the editor's option defaults to sqlite;
sled stores auto-migrate on open. `RELEASE_ASSET_ENGINE`'s own comment says it is "kept at sled so
this refactor does not silently change the format of a release artifact; the delivery cutover is a
separate, deliberate change." **This ADR is that deliberate change.**

## Decision

1. **SQLite is the EMBEDDED KB storage backend** — the editor, build tooling and shipped artifacts,
   and the daemon *as it exists today*. One embedded backend, not three.

   Scope, stated precisely because this ADR is easy to over-read: it settles **which embedded engine
   MAE uses and declares**, ending a contradiction that spans seven declarations. It does **not**
   settle what engine a hosted, multi-user daemon should use. See D6.

2. **Every consumer declares the same features.** No crate relies on unification to receive a
   backend it depends on. `mae-kb`'s default feature becomes `storage-sqlite`.

3. **`CozoKbStore::open()` opens sqlite.** A defaulting constructor that disagrees with the
   product's default option is a trap, and it was the reachable path by which new sled stores could
   still appear.

4. **sled is retained for exactly one purpose: reading a legacy store during migration.** It stays
   compiled behind `storage-sled`, which is now documented as migration-only, and no code path may
   *create* a sled store. `RELEASE_ASSET_ENGINE` becomes sqlite, so the ADR-KB artifact ships as a
   single verifiable file rather than a directory.

5. **One backend ships at a time. ADR-102's escalation gate is untouched — but its baseline is not.**

   This ADR decides *which single backend MAE ships and declares consistently*. It deliberately does
   **not** decide that sqlite scales, and must not be cited as having done so. ADR-102 D4's evidence
   gate stands as written; what this ADR adds is that the *outcome* of that evaluation must be a
   deliberate migration to one backend, not a second backend shipped alongside sqlite, which is how
   the present contradiction arose.

   **What has changed since this ADR was drafted: the benchmark that framed the risk was measuring
   MAE's own query bugs, not the engine.** An instrumented decomposition of the single-client p99
   (20,000-node sqlite store, driven through the real daemon over its Unix socket) found:

   | component | ms | % |
   |---|---|---|
   | transport + framing + dispatch + state lock | 0.04 | 0.03% |
   | Cozo FTS index (parse, IDF, range-scan, TF-IDF, sort) | ~9 | 7% |
   | **`fts_search`'s post-verification query** | **~113** | **88%** |

   That query is `?[id, title, body] := *nodes{id, title, body}, is_in(id, $ids)`. Cozo compiles the
   post-filter form to a **full relation scan** (`compile.rs`'s `seen_variables` gate), and `is_in`
   is a linear `Vec` probe run once per scanned row — **1.4M string comparisons to fetch 70 rows
   whose primary keys were already in hand**. `get_node` has the same defect: **71.7 ms for one node
   at N=20,000, and a *missing* id costs the same**, which is only possible if every row is read.
   Sixteen sites share the pattern. Tracked as #753.

   **Cozo's index is not the problem and is measurably fine**: a zero-candidate FTS query costs
   **3.4 ms on a real 3,208-node KB and 3.1 ms on the 20,000-node bench store** — 6x the corpus, no
   change. Sublinear. The linearity appears only once MAE's own verification query runs.

   **And the concurrency curve in ADR-054 is contaminated.** `eu-stack` sampling found
   `run_hygiene_scan` on-CPU in **60 of 60 samples** of the benchmark daemon: `scheduler.rs:97` fires
   it on the first `health_tick`, `tokio::time::interval` ticks at t=0, and `hygiene.rs:117-128` then
   calls `get_node` per id — at 71.7 ms each, ~24 minutes of continuous CPU for one pass, which never
   finishes inside a run. The tell was already visible in the stored samples: **N=1 at 139 ms/request
   but N=4 at 73 ms for four concurrent requests**, which request cost cannot explain.

   **Therefore, in order:** fix the query defects (#753), re-run `kb_dispatch_concurrency` on an idle
   daemon with 20 and 24 added to the sweep, and only then grade ADR-102's gate.

   > **DONE, and the result changes the risk assessment.** #753 merged and the benchmark was re-run
   > against the same 20,000-node sqlite store. Criterion's own comparison against the stored
   > baseline: **−90.4% at N=32, −90.5% at N=64, −94.5% multi-tenant 4×4, −90.7% multi-tenant 8×4**
   > (p = 0.00 throughout).
   >
   > | measurement | before | after |
   > |---|---:|---:|
   > | single-client p99 | ~146 ms | **21.6 ms** |
   > | N=32 concurrent, p99 | 349 ms | **34.2 ms** |
   > | N=64 concurrent, p99 | 627 ms | **58.8 ms** |
   > | multi-tenant 8×4 = 32, p99 | ~580 ms | **57.1 ms** |
   >
   > **The SLO (p99 ≤ 2× single-client baseline) now holds to N=32**, against a stated target of ~20
   > concurrent. The earlier "holds to exactly 20" figure was measured on a daemon whose hygiene scan
   > was consuming a core, so it was never a real reading of the engine.
   >
   > **So the scaling risk this decision was hedged against was ours, not Cozo's.** That does not
   > retire ADR-102's gate — the bar is ~50 concurrent at 100K nodes, and this run is 20K — but it
   > removes the reason to expect a miss. Two things remain genuinely unmeasured and should not be
   > inferred from these numbers: the **collab doc store** has still never been benchmarked for
   > concurrency at all, and there is still **no write-contention benchmark**, which is where the one
   > surviving argument for a different engine lives (D7). Grading it against
   the current baseline would attribute MAE's bug to the engine. The doc store — the path real-time
   collaborative editing actually takes — **has still never been benchmarked for concurrency at all**,
   and that gap is unchanged.

6. **The hosted-daemon engine is NO LONGER open: Cozo-on-SQLite serves both lenses. Decouple
   structurally, but ship one backend.**

   *This reverses an earlier draft of this decision, which called Postgres "a live candidate". A
   dedicated spike was run against both lenses — local editor and hosted server — and the evidence
   went the other way. Recorded as a reversal rather than quietly rewritten (principle #17).*

   **The performance case collapsed.** It rested on the p99 figure now known to be 88% MAE's own
   query (D5). Of what remains, **transport is 0.03% of the budget** — a networked engine *adds*
   there — and Cozo's index accounts for ~7% while behaving correctly.

   **The capability substitutes do not substitute.**
   - **Apache AGE is disqualified by its own maintainers' benchmark.** LDBC IC1 — literally
     `MATCH path = shortestPath((p)-[:KNOWS*1..3]-(friend))`, which is MAE's `neighborhood` /
     `shortest_path` shape, from a selective start, at roughly the target scale — measured at
     **7,117 ms** in AGE's own optimization PR. That is 20–70x the p99 budget, single-user and warm.
     The "AGE is stuck on an old Postgres major" premise was *refuted*; it fails on the thing it is
     supposed to be good at. AGE also gates the Postgres major-version cadence.
   - **SQL/PGQ** is committed to **PG19** but **without variable-length paths or shortest path** —
     i.e. without the feature MAE needs.
   - **PostgreSQL FTS is not BM25 and has no corpus-wide IDF.** Its own docs: *"the ranking functions
     do not use any global information, so it is impossible to produce a fair normalization."* SQLite
     FTS5 ranks by BM25 by default. MAE has a passing regression guard that depends on IDF-shaped
     behaviour — `kb_search_context_hub_node_does_not_outrank_specific_target` (issue #357). It would
     not go red on `ts_rank`; the AI would simply retrieve worse RAG context on the hosted deployment
     than on the laptop.
   - **pgvector's own README** warns *"you will see different results for queries after adding an
     approximate index"*, so Cozo HNSW and pgvector HNSW return **different top-k for identical
     vectors**.

   **A second backend is a permanent tax, not a migration cost.** The end state this ADR's earlier
   draft called "likely" — editor on sqlite, hosted daemon on a server engine — is exactly the
   two-backend configuration Miniflux publicly refuses, and their contributor states the cost
   precisely: *"now we have to consider compatibility for any feature that touches the schema or uses
   a complex query. **Which is going to be most of them.**"* Their SQLite request is the
   most-reacted issue in their history, with three volunteer attempts over two years and **zero
   merged**. Measured proxies elsewhere: **Gitea runs 5 DB test legs per PR and carries 62 PRs
   existing solely to fix MSSQL**; Home Assistant runs 9 extra CI legs to serve 4.67% of installs.

   **And one structural fact forecloses the split anyway.** ADR-053 states that server-side plaintext
   search is *structurally impossible* for E2E KBs, and ADR-037's key-blind daemon means headless
   hosting of an encrypted KB cannot serve content beyond relaying ciphertext
   (`daemon/src/kb_query.rs:280` implements this as a hard refusal). **So the embedded engine can
   never be demoted to a cache**: for any E2E KB, including on the hosted deployment, the client must
   replicate and query locally at full capability. The correct hosted design is therefore *"the same
   capable engine everywhere, the server adds multi-tenancy and durability"*.

   **What "decoupled" means instead — and it is not a second backend.** Today `KbStore` is not a seam:
   **29 of its 44 methods default to `NotSupported("… requires CozoDB backend")`**, so its default
   posture is *"if you are not Cozo, you do not work."* The exit ramp is:
   1. **Decouple reads at `KbQueryLayer` (19 methods), not `KbStore` (44).** That seam already has six
      implementations including a **network-backed** one (`RemoteHubQueryLayer`, blocking HTTP), so it
      is proven to survive a non-Cozo backing.
   2. **Plug the leaks ADR-102's Layer-3 objection named** — `db_path` on the trait, `backup.rs`'s
      `fs::copy`, `data_dir.rs`'s hardcoded filename, `migrate.rs`'s `is_dir()` — plus a **transient**
      `KbStoreError` variant, which does not exist, so a partition is currently indistinguishable from
      corruption.
   3. **Treat Datalog, not the trait, as the real coupling**: ~161 raw-Datalog sites, **8 user-facing
      executable surfaces**, 6 stored Datalog views (`kb_view_query` even un-escapes Cozo's
      `DataValue` `Debug` formatting, coupling to the engine's *result encoding*), and Datalog is
      promised in the seeded guidance corpus every AI session reads.
   4. **Build the cross-backend conformance suite before any second backend**, because it has
      independent value now: MAE already runs **three divergent implementations of "search"** behind
      one seam — `search_ranked_pass`'s hand-rolled weights, Cozo `nodes:fts`, and the daemon's
      unranked `title.contains()` capped at `max_scan_nodes`. `remote_hub.rs` confesses the
      consequence in its own comment: the synthetic scores are *"NOT comparable in magnitude to a real
      BM25-style local score."* ADR-035 already named this: *"that gap **is** the dual-mode tax
      surfacing."* Two of the three implementations fail such a suite today. **That is the point.**

7. **The two open risks, with named triggers — neither is performance.**

   - **Cozo is dormant.** v0.7.6, published **2023-12-11** (105,898 downloads, verified against the
     crates.io API); last commit 2024-12-04; the `cozo-community` fork last pushed 2024-12-12 with no
     releases. **KùzuDB, one of ADR-102 D5's three named Track-2 candidates, is archived.** The
     trigger for revisiting is a *measured* miss on the write-contention benchmark below, or a
     security advisory with no upstream — not a calendar date.
   - **The SQLite backend's in-process write lock.** `cozo-0.7.6/src/storage/sqlite.rs:66-78` takes a
     `ShardedLock` per transaction: `write()` for a write, `read()` for a read. **A write excludes
     every reader on that `Db` instance**, and no journal-mode change fixes it, because it is cozo's
     own lock above SQLite. Scoped honestly: it is *per-`Db`*, and the daemon holds one `Db` per KB,
     so contention is confined to a single hot KB; the shape that hurts is a **long** write (bulk
     ingest, projector rebuild) stalling that KB's readers.

     **This is the one place a different engine can still be argued — and the cheapest answer is
     in-family.** `cozo-0.7.6/src/storage/rocks.rs:132-135` takes **no lock at all**:
     `self.db.transact().set_snapshot(true).start()`, with `_write` ignored — an MVCC snapshot
     transaction. So the writer-excludes-readers ceiling is **SQLite-backend-specific, not a cozo
     property**.

     > **Correction to ADR-102 D2 and ADR-012 (principle #17): `storage-rocksdb` DOES exist in cozo
     > 0.7.6.** Verified in the vendored manifest, not docs.rs — `Cargo.toml:243`,
     > `storage-rocksdb = ["dep:cozorocks"]`, on published `cozorocks 0.1.7`. Those ADRs state it does
     > not, and ADR-102's Track 1 is framed as *"a first-class risk with its own gate"* on the
     > strength of needing an upstream release that will never come. **That gate is not real**, which
     > makes Track 1 cheaper and less risky than recorded. Tracked as #687.

     Three things must be checked before RocksDB can be recommended, none yet done: the C++ RocksDB
     build cost (weight, cross-compilation, Windows — ADR-104's concern); whether a directory-shaped
     store has a real backup story (RocksDB has checkpoint APIs, unlike sled, so probably yes); and
     cozo's **per-relation** `ShardedLock`s at the `Db` layer (`runtime/db.rs:109,796-820`), whose
     acquisition sites all appear to take `.read()` — characterize them before claiming RocksDB
     removes the ceiling rather than relocating it.

     **The benchmark that would settle it is a write-contention benchmark, and it has never been
     run.** ADR-102's gate is a read benchmark.

## Why sqlite specifically

Not merely "sled is bad" — the properties matter to decisions already taken elsewhere:

- **A single file can be checksummed, copied and verified.** ADR-104's objection to shipping sled is
  precisely that it is a directory, rewritten on first open, and therefore never checksum-verifiable.
  This now compounds: DB snapshots are the recovery path for the KB cutover, so a store that cannot
  be verified cannot be a backup.
- **Multi-process safety — but read the next paragraph before relying on it.** sled takes an
  exclusive *directory* lock, so two daemon-less `mae` processes cannot share a sled store at all.
  SQLite can, which is the observed usage.

  **Correction to an earlier draft of this ADR, which claimed "SQLite WAL lets several daemon-less
  `mae` processes share a store".** WAL is **never enabled**. `cozo-0.7.6`'s `new_cozo_sqlite` sets
  neither `journal_mode=WAL` nor `busy_timeout`, so the store runs in rollback-journal mode, where a
  writer's exclusive lock blocks *readers* file-wide. That is what the 45-second busy-retry loop in
  `db.rs` exists to paper over, and it is why an experiment measured **~14% raw write-failure under
  two-writer contention**. Multi-process sharing works; it works by retrying, not by WAL.

  This is fixable and is not fixable the way the code currently says it is. `db.rs:200` concludes
  *"there is no hook this crate could use to set the pragma even if it wanted to"* — the premise
  (cozo never exposes its connection) is right, the conclusion does not follow. Per
  [sqlite.org/wal.html](https://www.sqlite.org/wal.html), **WAL is a property of the file header, not
  of the connection**: *"applications can be converted to using SQLite in WAL mode without making any
  changes to the application itself. One has merely to run `PRAGMA journal_mode=WAL;` on the database
  file(s) … then restart the application."* So MAE can open the file once with the `sqlite` crate —
  already a direct `mae-daemon` dependency — set the pragma, close, and hand the file to cozo.

  Two caveats that make this a detect-and-fallback rather than a blanket enable: WAL requires all
  processes on the same host and a writable `-shm` file, and **does not work on a network
  filesystem** — so an NFS home or a cloud-synced home directory (Dropbox/iCloud/OneDrive) breaks.
- **The daemon already requires it**, so choosing anything else means keeping the editor/daemon split
  above.
- **Measured faster on the path that matters**: 8–14x on first-run provisioning.
- **#717.** An observed heap corruption in the sled path, in a build tool, is a poor foundation for a
  multi-user release.

## Consequences

**Positive.** One backend to test, benchmark, back up and support. The editor/daemon split-brain is
closed by construction. Release artifacts become single files that can be verified before use.

**Costs, honestly.** `storage-sled` remains compiled, so the dependency is not gone and the build is
not smaller yet — that only happens when the migration window closes (see below). Existing sled
stores must still be migrated, and the migration remains the one place a sled store is opened.

**Removal condition, stated so it is not indefinite.** `storage-sled` may be dropped once a release
has shipped in which no supported upgrade path can still encounter a sled store. Until then it is
migration-only, and any new use is a regression.

**Not addressed here.** The *collab* document store is a separate plain SQLite database
(`daemon/src/storage.rs`), not a Cozo store. It is unaffected by this decision and was never part of
the inconsistency.

## Verification

1. A guard test asserts every workspace member requests the same `mae-kb` storage features, so a new
   crate cannot reintroduce the divergence silently. This is the check that would have caught the
   binary's accidental sqlite.
2. `CozoKbStore::open()` creates a single file, not a directory.
3. A sled store still opens and still migrates — the migration path is the one thing that must not
   regress.

4. **WAL is demonstrated, not asserted.** Set the pragma out of band, reopen the store through cozo,
   and read `journal_mode` back. Only then correct `db.rs:200`, `schema.rs:89-93` and ADR-004's
   Tier 1 section — the claim in those places was reached from a sound premise and a wrong
   conclusion, which is exactly what an unverified fix looks like. Include the network-filesystem
   fallback path, and exercise it on macOS as well as Linux (principle #13).
5. **The query defects are fixed and re-measured before ADR-102's gate is graded** (#753). Target:
   single-client p99 ~10-12 ms at 20K nodes and **flat in corpus size** — verify the flatness against
   both the 3,208-node real KB and the 20K bench store, because that is the property that decides
   whether the fix is real rather than merely faster.
6. **The write-contention benchmark exists and runs.** ADR-102's gate is a read benchmark; the one
   surviving argument for a different engine is a write ceiling that no benchmark has ever exercised.
7. **The cross-backend conformance suite exists and fails on two of three implementations** when
   first written. If it passes everywhere on day one it is testing the wrong thing.
