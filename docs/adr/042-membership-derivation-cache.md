# ADR-042: Membership-derivation performance & deterministic replay

**Status:** Accepted (2026-07-01). Pre-dogfood deep dive, Workstream B (issue #247, umbrella #251).
**Extends:** ADR-018 (membership/roles), ADR-023 (epoch fence), ADR-026 (peer-verifiable signed op-log —
the derive's input), ADR-039 (local blocklist — a derive input the cache must honor).
**Relates:** ADR-040 (rotation — the derive resolves owner successors), ADR-037 (E2E — key-derivation walks
the same op-log).
**Prior art:** CRDT deterministic replay (Yjs/YATA); topological sort (Kahn 1962); memoization keyed on a
monotonic version stamp (yrs state vectors).

## Context

Every daemon access check for an **anchored / E2E** KB re-derives the whole membership set from the signed
op-log: `oplog_ops()` (full decode) → `derive_governance` → `derive_valid_members_governed`
(`daemon/src/collab_handler.rs::kb_access` and 4 sibling gates). Two costs surface at dogfood scale
(enterprise KB shared over P2P + encryption):

1. **`causal_order` was O(n²).** It emitted one causal generation per pass, each pass scanning all *n* ops.
   Membership ops form a **near-linear chain** (each op's `prev_hash` = the current head), so depth ≈ *n* ⇒
   *n* passes × *n* scan.
2. **The full derive ran on every access.** Cost scales with membership **churn** (members × rotations ×
   role-changes × recovery-key registers), not node count — and it ran per read/edit/manage gate, i.e. many
   times per second on a busy mesh.

The op-log is append-only and unbounded (see the `// PERF/DOGFOOD` flag on `append_signed_op`); pruning is a
separate, larger decision deferred to v0.16 pending dogfood numbers.

## Decision

**1. `causal_order` → O(n log n).** Rebuilt as a BFS from the anchored genesis over a children-adjacency map
(Kahn's algorithm), emitting each causal generation in ascending `chain_hash` order. The **emit order is
byte-identical** to the prior implementation — a hard requirement, since every honest peer must replay the
op-log in the same order or membership diverges. Guarded by a property test
(`causal_order_matches_the_reference_impl_on_random_trees`) that runs both the new and a verbatim copy of the
old impl over 64 random op-trees (linear chains, wide fan-out, orphans) and asserts identical output.

**2. Memoized derivation (`DocStore::derived_membership`).** A per-KB cache returns a shared
`Arc<DerivedMembership>` (`{governance, members}`) without re-decoding or re-deriving when nothing that feeds
the derive has changed. The five gate sites route through it.

**Cache key = every input to the derive.** A hit requires ALL of:
- the collection **op-log state-vector** matches (any membership op advances the `kbc:` doc's yrs SV ⇒
  mismatch ⇒ recompute) — the monotonic version stamp;
- the **trust anchor** matches (defensive; the genesis owner pubkey is stable per KB);
- `now < valid_until`, where `valid_until` = the earliest **future** member timebox (`expires_at`) — the one
  input that flips the set *without* an op-log change; absent ⇒ `u64::MAX`.

The **local blocklist** (ADR-039) is per-peer and not in the SV, so `add_kb_block` / `remove_kb_block`
explicitly invalidate the KB's entry.

## Correctness invariant (the security property)

The cache must **never serve a membership a fresh derive would not** — in particular it must never keep
admitting a removed member, miss a rotation, honor an expired timebox, or serve a locally-blocked principal
as authoritative. This holds because the key covers every derive input: op-log (SV), anchor, blocklist
(explicit invalidation), and time (`valid_until`). Falsified by
`derive_cache_hits_but_never_serves_stale_membership`: it asserts a cache hit on an unchanged op-log, then
that an op-log advance, a **block of the owner** (same SV — must drop the owner's genesis-rooted authority),
and an unblock each invalidate and reflect the change.

## Consequences

- Repeated access on an unchanged op-log is O(1) (Arc clone); the expensive derive runs once per op-log
  advance / blocklist change / timebox boundary. This is the design the dogfood measures against.
- Intra-derive re-decodes (`oplog_head` double-decode, `membership_at` / `effective_removal` re-scans in the
  validity fixpoint) still occur **on a miss** — flagged `// PERF`, folded into a future pass only if dogfood
  shows misses dominate. The cache makes misses rare, so this is lower priority.
- **Deferred (v0.16):** op-log pruning/compaction (unbounded growth); `owner_principal_chain` predecessor
  retirement (governance tightening). Both are tracked, not silent (docs/CODE_REVIEW_FLAGS.md, #237/#247).
