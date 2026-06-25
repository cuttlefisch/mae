# ADR-033: Coordinating KB-wide operations — advisory lease + epoch fencing

**Status:** Accepted (design); implemented in Phase F.
**Extends:** ADR-023 (secure write access — the epoch fence reused here), ADR-024 (notification
attention bus — carries the advisory claim), ADR-026 (signed membership — the lease authority +
tiebreak).
**Feeds:** ADR-034 (the coordinator that the lease elects performs the compute-once enrichment).

## Context

CRDTs deliberately avoid locks — concurrent *edits* merge. But some operations are not ordinary
edits: **KB-wide bulk/expensive operations** — running KB-wide AI enrichment, rebuilding
embeddings, or applying a large sweep of revisions (terminology rename across thousands of nodes,
bulk link weight/confidence changes). For these, peers need to (a) not duplicate expensive work,
and (b) not clobber a sweep mid-flight. We need a way to mediate this across the supported
configurations (leaderless P2P; central hub; both).

**Prior art is decisive** (Kleppmann *How to do distributed locking*; CALM theorem; CAP/FLP;
Chubby/ZooKeeper/etcd/Consul; Redlock critique; RxDB leader election; HAT/Antidote; Project
Cambria):

- **Strict distributed mutual exclusion is impossible in a leaderless mesh** (it requires a
  quorum/consensus — CAP/FLP; split-brain). It is also **unnecessary**.
- **The correctness primitive is a fencing token, not a lock** (Kleppmann): a lease can expire
  during a GC pause / network delay, and the paused holder may wake and write after losing it; the
  fix is a **monotonic token the resource checks at write time, rejecting any write whose token
  went backwards.** Chubby's "sequencer" (lock + generation number) and ZooKeeper's `zxid` are
  exactly this.
- **Lock services (Chubby/ZK/etcd/Consul) all assume a central consensus cluster** — they don't
  drop into a leaderless mesh; Redlock *looks* decentralized but lacks fencing and is judged unsafe
  for correctness.
- **CALM:** coordination is required precisely for the non-monotonic decisions ("run this once",
  uniqueness) that CRDT merge alone cannot make.
- **RxDB leader election** is the canonical "elect one worker for expensive derived work, others
  defer" — but it can transiently double-elect, so it is safe **only if the job is idempotent or
  fenced**.
- **HAT:** serializable cross-object isolation is unavailable while staying partition-tolerant — a
  concurrent conflicting sweep is a residual risk the fence + single-writer lease must close.

## Decision

Coordinate KB-wide operations with an **advisory lease (for efficiency) backed by the epoch fence
(for correctness)** — not a lock.

1. **Reuse the ADR-023 epoch as the fencing token.** A KB-wide operation carries an epoch; the KB
   resource **persists the highest accepted epoch and rejects any bulk-op write whose epoch is
   lower, checked at write time** (not merely at lease acquisition — the threat is the paused/slow
   holder). The ADR-026 owner/signed-membership model is the authority that **bumps the epoch on
   lease grant** and gates **who may claim**.

2. **Advisory lease (efficiency, not safety).** A signed claim `{holder_fp, kb_id, op_kind,
   lease_ttl}`:
   - Default **ephemeral** (Yjs-Awareness-style, lost on disconnect), broadcast on the **ADR-024
     attention bus** so peers see "enrichment running on KB X by peer Y" and defer.
   - **In-band LWW claim** in the collection doc only when the lease must survive disconnect.
   - Concurrent claims resolve by the **ADR-026 deterministic tiebreak** (e.g. highest member
     fingerprint). TTL is an *efficiency* dial — a false steal only wastes work, never corrupts.

3. **Bulk sweep pattern.** Run the sweep under the lease on **one writer**, apply the whole sweep as
   a **single atomic CRDT batch** (`Y.transact` / one update) so no peer observes half-applied
   state, replicate, and let the epoch fence reject a stale second writer. For terminology/schema
   renames, prefer **Cambria-style read-time lenses** over a destructive global rewrite where the
   change is expressible as a translation.

4. **Per configuration.** P2P = advisory + fence (accept best-effort dedup; the fence carries
   safety). Hub = the hub may **additionally arbitrate** the lease for stronger dedup, but the
   epoch fence remains mandatory — the hub is an efficiency arbiter, not the safety mechanism (a
   hub-lock without fencing is still unsafe).

## Consequences

**Positive.** Correct across all configs without a consensus cluster or SPOF (fencing carries
safety; merge handles edits). Natural UX ("operation in progress" on the attention bus). Reuses
existing primitives (epoch ADR-023, attention bus ADR-024, membership ADR-026). Liveness recovers a
crashed holder via lease TTL.

**Costs / honest caveats (recorded).** Fencing makes writes *safe* but does **not** restore mutual
exclusion — two peers can both believe they hold the lease; fine here because edits merge and the
stale bulk write is fenced. Advisory election can transiently double-elect — safe only because the
work is idempotent or fenced. Lease-TTL tuning has a too-short (steal from a slow-but-healthy
holder) / too-long (delay re-attempt) tension — absorbed at the *efficiency* layer (jitter +
TTL/3 renewal). Serializable cross-object isolation is unavailable (HAT) — the single-writer lease +
fence are how we close the concurrent-conflicting-sweep gap.

## Alternatives rejected

- **A naive distributed lock (Redlock-style):** unsafe without fencing (the canonical zombie-holder
  bug). Rejected.
- **A consensus cluster (Raft/ZK) as the arbiter:** adds a quorum dependency + ops burden + SPOF,
  contradicting local-first/network-optional. The hub config already provides an optional arbiter
  without mandating consensus. Rejected as the baseline.
- **Strict mutual exclusion:** impossible in the leaderless config (CAP/FLP). Rejected.

## Verification

A paused/slow lease holder's late bulk write (lower epoch) is **rejected** at the resource. Two
peers claiming concurrently converge to one via the deterministic tiebreak; the loser defers. A
terminology sweep applied as one atomic batch is never observed half-applied by a peer; a concurrent
second sweep is fenced. Crashed holder → lease expires → another peer re-attempts. Hub config: the
hub arbitrates dedup; fence still rejects stale writes.
