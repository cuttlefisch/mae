# ADR-109: The hosted deployment has a resource budget, set before launch

**Status:** Accepted. Part I, item D4.
**Relates to:** ADR-108 (SQLite is the embedded KB backend; the per-tenant-process model this
budget prices), ADR-104 (system KBs are built to `$XDG_CACHE_HOME` on first run — the per-tenant
duplication priced in D3 below), ADR-054 (daemon concurrency hardening), ADR-102 (engine maturity;
its 100K-node target is the scale line here), ADR-055 (headless MAE as a service target).
**Measurements:** `shared/kb/src/cozo_store/tests/d4_bench.rs`, runnable.

## Context

MAE is about to be hosted for roughly 20 concurrent and 60 total users, one host holding many KBs,
under docker-compose with a persistent storage volume.

**The reason to set a budget now rather than after launch is that resource floors ratchet and never
come back down.** Sentry's enforced self-hosted minimum went from **2,400 MB to 16 GB** — 6.8x — and
its container count from 24 to 57. No single release did that; each one added a little, and no
release ever gave any back, because by then something depended on it. A budget written before launch
is a number a change has to argue against. A budget written after is a description of whatever
happened.

The second reason is specific to ADR-108's decision: the hosted model runs **one daemon process per
tenant**, so every per-process cost is multiplied by the tenant count. That model was chosen for
isolation (an OS boundary rather than a code review as the backstop for the next missed authz
check). Isolation is bought with duplication, and duplication has to be priced or the model gets
abandoned later for a reason that could have been anticipated now.

## Measured inputs

All measured 2026-08-25 on Linux. Every figure below is produced by an `#[ignore]`d benchmark in
`d4_bench.rs`, so it can be re-derived rather than trusted:

```
cargo test --release -p mae-kb --lib d4_bench -- --ignored --nocapture
```

| quantity | measured | note |
|---|---|---|
| On-disk cost per node | **~10.4 KB** | ~1.2 KB body + 2 tags + 1 link; ~8.6x the raw body (FTS index, row overhead, CRDT doc) |
| ...flat in corpus size? | **yes** | 10,350 B at n=2,000 vs 10,364 B at n=8,000 |
| Marginal RSS per open KB store | **~2.4 MB** | 3.85 MB at 4 stores, 2.44 MB at 16 — fixed cost amortizes |
| Base process RSS | **~10 MB** | before any store is opened |
| Single-client p99, 20K nodes | **21.6 ms** | post-#753; the pre-fix 146 ms was MAE's own query defect, not the engine (ADR-108 D5) |
| Concurrency at SLO | **N=32** | 8 tenants x 4 clients measured at 57 ms |

**Flatness is the property that matters most here.** A per-node figure that drifted with N could not
be multiplied out into a capacity plan at all; because it is flat, everything below is arithmetic
rather than extrapolation.

**Cross-check, deliberately by a second method.** A live `mae-daemon` with ~8 stores open measured
**33 MB RSS**, against ~10 MB base + 8 x 2.4 MB = **~29 MB** predicted from the synthetic benchmark.
Two unrelated methods agreeing is what makes the arithmetic below safe to build on; a single
measurement would not be.

## Decision

### D1 — The budget, as arithmetic anyone can re-run

For **T** tenants, each with **K** open KBs averaging **N** nodes:

```
disk   ~=  T x K x N x 10.4 KB   +  T x 14 MB   (system-KB cache, D3)
memory ~=  T x (10 MB + K x 2.4 MB)
```

Worked at the stated target — 20 concurrent users, say **T = 20** tenant processes, **K = 5** KBs
each, **N = 5,000** nodes:

* **disk:** 20 x 5 x 5,000 x 10.4 KB = **~5.2 GB**, plus 20 x 14 MB = **~280 MB** of duplicated
  system KBs → **~5.5 GB**
* **memory:** 20 x (10 + 5 x 2.4) MB = **~440 MB**

At ADR-102's **100K-node** ceiling for a single KB, that one KB alone is **~1 GB on disk**.

### D2 — The launch budget is 8 GB RAM / 100 GB disk, and it is a ceiling, not a reservation

Roughly 8x headroom over the D1 arithmetic on memory and ~18x on disk. The headroom is deliberate
and is not slack to be spent: it absorbs the growth D3–D5 name, plus WAL sidecars, checkpoints,
version history (ADR-106) and backups, none of which are in the per-node figure.

**A change that pushes past this argues its case in a PR** — the Sentry ratchet is precisely what
happens when no number exists to argue against.

### D3 — Per-tenant system-KB duplication is a real cost and is accepted, with a named trigger

ADR-104 builds the manual / MaePractices / DevPractices stores to `$XDG_CACHE_HOME` on first run.
Under one-process-per-tenant, each tenant builds its own: **~14 MB of disk and one first-run build
per tenant** (measured on this machine). At T = 20 that is ~280 MB and 20 redundant builds of
byte-identical, read-only content.

Accepted for now, because the alternative — a shared read-only store mounted into every tenant —
punches a hole through exactly the isolation boundary ADR-108's process model exists to create, and
the content is small.

**Trigger to revisit:** T > 50, or first-run build time becoming visible in tenant provisioning.
Not a calendar date. The shared-mount design is the known answer; it just has to be argued against
the isolation it costs.

> **This is the concrete form of the gap G8 recorded as undesigned.** It does not close G8 — TCP
> port allocation per tenant, tenant discovery, whether the OAuth listener is shared or per-tenant,
> and mDNS/iroh identity per tenant all remain open. It prices one dimension of it.

### D4 — Concurrency is bounded at N=32 per tenant process, and the bound is enforced, not hoped

B5 measured the SLO holding to **N = 32** concurrent clients per process, with 8 tenants x 4 clients
at 57 ms. ADR-054 already adds connection caps to the previously-unbounded KB socket and P2P
listener; this ADR fixes the number those caps are set to.

Beyond 32, the honest answer is another tenant process, not a larger one — cozo's SQLite backend
takes a `ShardedLock` per transaction, so a write excludes every reader **on that `Db` instance**
(ADR-108). Vertical scaling of a single process runs into a lock, not a CPU.

### D5 — What is deliberately NOT in the budget

Naming the exclusions matters more than the inclusions, because an unnamed exclusion is what turns
a budget into a false reassurance:

* **Embeddings.** Zero today (nothing is embedded until a `kb_enrich` sweep runs), but a fully
  enriched corpus at 768-dim f32 adds **~3 KB/node** — a **~30%** increase on the per-node figure.
  Budget it when enrichment is turned on by default, not before. See #777.
* **CRDT growth over time.** The per-node figure is a *freshly written* node. Tombstones accumulate
  with edits and are not compactable today; ADR-107 (node rebirth) is the design that would bound
  it, and it is not built. **This is the largest unpriced risk in this ADR** and it is unbounded
  rather than merely unmeasured.
* **Version history.** ADR-106 keeps `node_versions` per node; growth is proportional to edit count,
  which no measurement here covers.
* **Backups and checkpoints.** Whatever retention is chosen multiplies the disk figure directly.
* **macOS.** Every number here is Linux. Per principle #13 one platform's measurement is not a
  property — but the hosted target is Linux containers, so the gap is recorded rather than closed.

## Consequences

**Positive.** A capacity plan that is arithmetic over two flat, re-derivable constants rather than a
guess. A named ceiling that a growth-causing change has to argue against. The per-tenant isolation
model priced honestly instead of discovered to be expensive later.

**Negative.** The two largest growth terms — CRDT tombstones and version history — are exactly the
ones NOT measured, so the budget is most trustworthy at launch and least trustworthy after a year of
editing. That is an argument for building ADR-107, and this ADR should be revisited when it lands.

**Neutral.** The figures are Linux-only and will drift as the schema changes. That is why they live
in a runnable benchmark and this ADR cites the command rather than freezing the numbers in prose
alone — the same discipline `tools/audit-metrics` exists to enforce for file sizes.
