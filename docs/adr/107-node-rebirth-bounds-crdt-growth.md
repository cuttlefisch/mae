# ADR-107: Node rebirth — bounding CRDT growth with the consensus primitive we already have

**Status:** Proposed. Design only; nothing here is built. **Prerequisite for the KB cutover**
(issue #103) rather than follow-on work — see "Why this blocks" below.
**Extends:** ADR-023 (epoch-fenced write access), ADR-026 (peer-verifiable signed, hash-chained
membership), ADR-029 (CRDT is truth), ADR-032 (durable CRDT store / checkpoints).
**Corrects:** ADR-032 §3's claim that a checkpoint "bounds CRDT tombstone growth".
**Depends on:** ADR-106 (version history is a local audit trail) — that decision is what makes this
one safe, and the two must be read together.
**Tracking:** issue #733.

## Context

Under ADR-029 the CRDT is durable truth. Growth in a yrs document is monotonic in **operations**,
not characters: deletion is a flag on the Item, and GC replaces deleted *content* while the Item
struct and its delete-set entry remain. So a document edited daily for years accrues state that
nothing reclaims.

**There is no safe general compaction, and the field is explicit about why.** Yjs's author, on
flattening a document: it *"would destroy the document integrity. You wouldn't be able to merge a
'flattened' document with a non-flattened. Or all parties would need to agree to flatten the
document at the same time."* On the cleverer variant — merging edits older than N days into one
client id — *"This is a good idea and theoretically possible, but you'd need a consensus algorithm
that defines exactly when these edits are merged."* A request for manual GC was closed **wontfix**
with *"You can't gc like that."* An earlier time-based tombstone GC was **removed** in 2017 for
correctness reasons.

The production consequences are documented, on exactly the `Y.Map`-with-tombstones shape ADR-093
uses: an open blocker reports customer documents of ~10M deleted structs needing **4 GB to load and
15 GB to encode**, with memory unreclaimed after `destroy()`. The reporter's own fix was *"copying
the broken documents and loosing the history."*

**And the most-cited production instance of CRDT-as-truth has no compaction at all.** Actual Budget
has shipped this architecture since 2019. Its remedy for growth is a user-facing button that issues
`DELETE FROM messages_crdt` — destroying the source of truth and promoting the projection to truth.
A ~991 MB budget becomes ~13.3 MB after "reset sync". The epoch cleanup its author described as
necessary in 2019 was never built. Seven years in, the escape hatch is the inversion of the thesis.

MAE is on that trajectory by default. What follows is the argument that it does not have to be.

## The asymmetry MAE has and the rest of the field does not

Every blocker quoted above reduces to the same sentence: *you'd need a consensus algorithm.*

**MAE has one.** ADR-026 gives each shared KB a signed, hash-chained membership op-log, and ADR-023
adds an epoch fence: an op authored under a superseded grant is rejected by peers. An **epoch bump is
already a coordinated, peer-verifiable, totally-ordered event** that every participant either
observes or is fenced out by. That is precisely the primitive Yjs users are told they would need and
do not have.

This ADR proposes spending it.

## Decision

1. **Node rebirth.** At an epoch boundary, the KB owner may re-emit a node as a **fresh
   single-client document** whose content equals the current materialized state, discarding the
   prior operation history.

2. **A rebirth is a signed op in the collection doc**, carrying `node_id`, the epoch it belongs to,
   and the content hash of the reborn state. It is therefore subject to the same verification,
   ordering and fencing as every other membership/policy op — not a side channel.

3. **Old state is discarded only after the manifest hash advances**, so a peer either sees the
   rebirth op and adopts the new document, or is already fenced by the epoch it missed. There is no
   window in which two peers disagree about which document is current without one of them being
   fenced.

4. **Rebirth is owner-only and never automatic.** It destroys history, which is a decision, not an
   optimisation. A scheduled or heuristic trigger would make data loss a background process.

5. **Rebirth does not preserve per-node edit history, and does not need to.** ADR-106 makes
   `node_versions` a local audit trail beside the CRDT rather than inside it — so discarding the
   operation log does not take the user's undo with it. **This is the decision that makes rebirth
   safe**, and the two ADRs must move together.

6. **ADR-032 §3 is corrected.** It claims a checkpoint "bounds CRDT tombstone growth". It does not:
   yrs sets `skip_gc: false`, so GC is on by default and a checkpoint is a full state encode that
   carries the delete set with it. A checkpoint is an excellent *rebuild root* and *rollback
   artifact* (now reachable — #632) and is not a growth bound.

## Consequences

**Positive.** The endgame stops being "reset sync". Because the mechanism is an owner-authored
signed op rather than a coordinated flush, it does not require every peer to be online, which is the
requirement that makes flattening impossible in the general Yjs case.

**Negative, and load-bearing.** Rebirth **destroys operation history for that node**, including
attribution of who wrote which character. Anything depending on the op log for a reborn node —
per-user undo across the boundary, blame, ADR-036 signature verification of historical ops — is gone
by design. That is the trade being made deliberately, and the reason for decision 4.

**A rebirth is observable.** Peers see a node's document identity change. Any client caching state
vectors per document must handle that; ADR-105's per-KB addressing means the doc name itself does not
change, only its content lineage.

## Why this blocks the cutover

The cutover makes the CRDT the only authoritative copy of a user's corpus. Adopting that **without**
a growth bound means adopting Actual Budget's position: the architecture works until it doesn't, and
the recovery is to delete truth. It is materially easier to design rebirth now, while MAE's corpora
are small and few KBs are shared, than after years of accumulated operations across a mesh.

## What has already been done, and what it is not

The hottest avoidable writes are removed:

- **Activity tracking left node content** (#729). It wrote a property on every node *read*, which on
  a `Y.Map` is a retired Item per read, forever.
- **`schema_v` is stamped once rather than per edit** (#744). Measured: 23.03 → 0.41 bytes per edit
  across 100 scalar edits, a 56x reduction, because the constant dominated the actual edit.

**Neither is compaction.** They reduce the rate of accumulation; they do not reclaim anything, and
growth remains monotonic. Do not let them be cited as having addressed this ADR.

## Alternatives rejected

- **Do nothing and rely on checkpoints.** This is the status quo, and it rests on ADR-032 §3's
  incorrect claim. A checkpoint captures state including tombstones.

- **Flatten a document in place, as Yjs users periodically attempt.** Rejected on the upstream
  author's own reasoning: integrity is destroyed, a flattened document cannot merge with a
  non-flattened one, and all parties would have to agree simultaneously. A contributor who
  implemented it reports *"all YJS state is lost when doing the flatten (not just old state)"*.

- **Time-based tombstone GC.** Yjs shipped this and **removed** it in 2017 for correctness reasons.
  Repeating a mistake the upstream project already made and reverted is not a plan.

- **Actual Budget's approach: delete the op log and promote the projection.** Honest, shipped, and
  the thing this ADR exists to avoid. Under ADR-029 the projection is *derived* and lossy relative to
  truth, so promoting it is not a compaction — it is choosing which data to lose.

- **Per-node document size caps with hard failure.** Bounds growth, converts a slow problem into a
  sudden one, and does so at the moment a user is trying to write.

## Verification

Design-only ADR; these are the gates the implementation must pass, stated now so they are not
negotiated later.

1. **Growth is actually bounded.** A node edited N times, reborn, then edited N times again must not
   exceed a fixed multiple of its materialized size. Today no test asserts document growth at all
   beyond the two added with #744.
2. **Convergence across a rebirth.** Three or more peers, one of them offline across the boundary,
   must converge — the offline peer either adopts the reborn document or is fenced, never silently
   diverges.
3. **The adversarial case.** A peer that replays *pre-rebirth* ops after the boundary must be
   rejected by the epoch fence, not merged. This is ADR-023's guarantee applied to a new op type and
   must be tested as an attack, not as a happy path.
4. **Content identity.** The reborn node's materialized content must be byte-identical to the
   pre-rebirth state — verified by content hash, not by inspection.
5. **Measured before and after, on a real corpus**, with the numbers recorded in this ADR. No
   implementation lands on the strength of the argument above alone.
