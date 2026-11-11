# ADR-116: Node version history is complete, attributed, tamper-evident, retained by policy, and rolled back through the write path

**Status:** Proposed (design). Nothing here is built.
**Amends:** ADR-106. Its D2 (versions are written only when an authored ingest destroys content) is
superseded by D2 here. Its D1 (a local, non-synced audit trail) stands, and D1 here says where that
trail lives for a hosted KB. Its "no pruning" stance gives way to D5's explicit, audited retention
policy, which is the form ADR-106 itself said pruning would have to take.
**Relates to:** ADR-113 (publication review: revisions are frozen copies, kept separate from this
history, and they reference it), ADR-092 (the single node mutator), ADR-093 (what a node's content
is), ADR-107 (rebirth), ADR-110 (native KBs: the store is truth), ADR-105 (addressing), ADR-098 and
ADR-111 (principals).
**Evidence:** `docs/research/116-version-history-prior-art.md`.

## Context

Four properties are required of node history: versions are saved under an applied retention
policy, their content is checksum-verified so tampering is evident, there is a record of who
changed what and when, and a node can be rolled back. Measured against the code at `058b363b`, none
holds fully:

| Required | What exists |
|---|---|
| Versions saved | `node_versions` (Cozo) is written only by org ingest (`insert_node_with_history`). Edits from the editor, MCP, collab and the daemon record nothing. A hosted KB, where the daemon holds the truth (ADR-110), has no history at all. |
| Retention | None. History grows without bound, and ADR-106 deferred any policy. |
| Checksum against tampering | The hash covers title, body, tags, todo state and priority. It omits kind, properties, aliases, links and the header shape. It joins fields with `\|`, so two different contents can share a hash. And anyone who can write the store can recompute it, so it does not show tampering. |
| Who, what, when | `author` is the constant `"local"`. A time is recorded; the principal is not. |
| Rollback | `restore_version` writes the Cozo row directly. For a KB whose truth is the CRDT, peers never see that write and the next projection overwrites it. |

The daemon already knows who edits each document. The editor record (`doc_editors`, ADR-113 A1)
records each principal, how it was attributed, and the WAL range of its edits. History should be
built on that record, not on a second one.

## Decision

### D1. History lives with the writer of record, in one format

- **A hosted KB** (the daemon holds the truth) keeps its history in daemon tables in `state.db`,
  beside the editor record, the seed record and ADR-113's revisions. It is therefore in every
  backup, which closes ADR-106 D5's open requirement for this case.
- **An editor-local KB** keeps the existing Cozo `node_versions`, upgraded to the same record format.
- **One shared module** (`mae_sync::kb::history`) defines the record, its canonical encoding, the
  hash chain, verification and the retention evaluation (principle #8). Each store is only an
  adapter.

History never enters the CRDT (ADR-106 D4 stands).

### D2. When a version is cut

A version is a checkpoint of the node's whole materialized content. It is not one entry per CRDT
update. The daemon cuts one when any of these happens:
1. **Handoff:** a different principal starts editing the node.
2. **Idle:** the node's edits pause for `history_idle_secs` (default 300).
3. **Long session:** a continuous edit has run for `history_max_interval_secs` (default 1800).
4. **Before a destructive act:** rollback, ingest, a seed overwrite, rebirth, delete or purge. The
   state is always captured before it is replaced.
5. **On request:** a named version (`kb/history.checkpoint {name}`), or an ADR-113 revision freeze.

Each version records the content AFTER the edits it covers, and the principals who made them. A
checkpoint whose content equals the previous version's is not written, so watcher ticks and no-op
saves add nothing (ADR-106's bound, kept).

### D3. The record: who changed what, and when

Each version carries:
- `kb_id`, `node_id`, and `seq` (per node, gapless, starting at 1);
- `kind`: one of `edit`, `named`, `rollback`, `import`, `seed`, `rebirth`, `delete` or `pruned`
  (D5);
- `created_at`, from the daemon clock, as RFC 3339 UTC;
- `editors`: each principal with its attribution kind (Principal, SignedRelay, MeshPeer,
  Unauthenticated, Internal; ADR-113 A1), taken from the editor record over the WAL range;
- `wal_from` and `wal_to`, the edits the version covers;
- `summary` and `name`, both optional; for a rollback, `restored_from` (a `seq`);
- `content`: the canonical encoding of the whole `MaterializedNode`, including `kind`,
  `properties`, `aliases`, `links` and `org_header`;
- `content_hash`, which is `KbNodeDoc::version()` of that content. It is the node's own complete
  digest: it destructures without `..`, so a new field cannot be left out of it;
- `prev_hash` and `entry_hash`, where
  `entry_hash = SHA-256("maehistory/v1" ‖ prev_hash ‖ canonical(entry without entry_hash))`.
  Every field is length-prefixed, so no two different entries encode alike.

The "what" of a version is a diff between two versions, computed when asked for (`kb/history.diff`),
not stored.

### D4. Tamper evidence: a chain, a signed head, and a head kept elsewhere

- **The chain.** Each node's versions form a hash chain. Verification recomputes every
  `content_hash` from its content, and every `entry_hash` from its fields and its predecessor's
  hash. An edited byte, a reordered entry, a removed entry or a forged content all fail.
- **The signed head.** On each maintenance tick and on every backup or export, the daemon signs the
  KB's history head with its own Ed25519 identity. The head is a digest over every node's
  `(node_id, seq, entry_hash)` in node order, and the signed heads are appended to a head log.
- **Verification** (`mae-daemon kb history verify`, and `kb/history.verify`) checks every chain
  against the latest signed head. It reports the first failing entry by node and `seq`.
- **The honest limit** (CT's lesson). Someone holding both the daemon's key and its database can
  rewrite history and re-sign it. That is detected only by comparison with a head observed
  elsewhere. So the head is included in every backup and export, `verify --against <head-file>`
  checks history against a head kept outside, and the head is printed for an operator to keep. A
  hash the attacker can recompute is never presented as tamper-proof.
- **Rollback re-verifies** its target (D6), and refuses an entry that fails.

### D5. Retention: an explicit, audited policy, overridden by holds and records

Each KB has a policy, set by its owner or the daemon operator, and never by an agent principal
(principle #16). The fields:
- **`min_days`**: content younger than this is never pruned. Floor 30.
- **`keep`**: either `all`, or a rule of `max_versions` per node (floor 100) with an optional
  `max_days`.

**Exempt from pruning, whatever the policy:**
- named versions;
- versions that an ADR-113 revision references;
- versions under a hold. An operator places a hold on a KB or a node, and the hold is recorded;
- the latest version of each node.

**Pruning keeps the record and drops the content.** A pruned version's `content` is deleted, and
the entry keeps its header and hashes, now marked `pruned`. The chain therefore still verifies, and
who changed what and when is never lost. A `pruned` audit entry records the policy, the operator or
job, and the time.

Every policy change and every hold is itself a recorded event. Pruning runs on the maintenance tick
and is bounded per pass.

**Default:** `keep: all`, with `min_days: 30`. Nothing is pruned until a policy is set. A deployment
sets its policy explicitly, and `mae-daemon kb history policy` shows each KB's current policy.

### D6. Rollback goes through the write path, and adds history

`kb/history.rollback {kb_id, node_id, seq}`:
- needs `Edit` on the KB;
- verifies the target entry;
- refuses a pruned target.

It then cuts a version of the current state (D2.4) and applies the target's content to the node
document as an ordinary write. The write is a character-level diff of every field (ADR-092), made
through the daemon's own mutation path, so the epoch fence and every peer see it like any edit. It
is attributed to the requesting principal.

The result is a new version of kind `rollback`, with `restored_from`. Nothing after the target is
deleted or rewritten. A rollback is undone by rolling back again.

An editor-local KB does the same through `kb_update_node_with` (ADR-092's single mutator), not
through the direct store write that `restore_version` makes today.

**Until hub signature verification lands (#727):** the daemon applies the rollback as its own
authored write, signed with its identity and attributed to the requester. This is the same footing
as the seed's writes.

### D7. Surfaces and access

| Method | Needs | What it does |
|---|---|---|
| `kb/history.list` | Read | The versions' metadata, without content |
| `kb/history.get` | Read | One version's content and its verification status |
| `kb/history.diff` | Read | The difference between two versions |
| `kb/history.verify` | Read | Verifies a node or the whole KB |
| `kb/history.checkpoint` | Edit | Cuts a named version |
| `kb/history.rollback` | Edit | Rolls back, per D6 |
| `kb/history.set_policy`, `kb/history.hold` | Owner or operator | Retention, per D5 |

MCP's `kb_history` and `kb_restore`, Scheme, and the ex commands all route to these for a hosted
KB. For an E2E KB the daemon cannot read the content, so these methods refuse (`e2e_unsupported`)
until client-side history is designed.

## Implementation plan, and how each property is proven

- **P1 — the shared module.** Record, canonical encoding, chain, verify, retention evaluation.
  Property tests:
  - flipping any byte of any field of any entry fails verification;
  - reordering or removing an entry fails;
  - pruning content still verifies, and a pruned entry's metadata is intact;
  - two different contents never share an `entry_hash` (length-prefix collision cases).
- **P2 — daemon recording.**
  - The five checkpoint triggers, each with a test driven by real `sync/update` traffic: handoff
    between two principals, idle via the maintenance tick, long session, before each destructive
    act, and named.
  - The editors and WAL range of every version match the editor record.
  - A no-op save writes nothing.
- **P3 — reads and verification.** `list`, `get`, `diff` and `verify`; signed heads and the head log;
  `verify --against`; tampering with the state.db rows directly is detected, and so is a forged head.
- **P4 — rollback.** It lands through the write path (a second, joined session receives it); it is
  attributed; it cuts its two versions; it refuses a pruned or corrupted target; it is fenced by the
  epoch like any write; and it round-trips an `org-literal-v1` node byte for byte.
- **P5 — retention.**
  - The policy floors are enforced.
  - Holds, named versions, revision-referenced versions and the latest version survive pruning.
  - Pruned entries still verify.
  - Policy changes and pruning appear in the log.
  - An agent principal is refused when it tries to set a policy.
- **P6 — editor-local KBs.** `node_versions` moves to the record format: entries written before the
  change are marked `legacy` and verified under their old, weaker hash, with that weakness reported.
  Editor and MCP edits record versions, and restore goes through the mutator.

**Performance, measured rather than assumed** (`#[ignore]` benchmarks, with budgets set from the
first run):
- the cost of a checkpoint at 1 KB, 100 KB and 1 MB of content;
- history growth for 1,000 nodes after 30 simulated days of edits, under `keep: all` and under a
  `max_versions` policy;
- `verify` time for a KB with 10,000 versions;
- the pruning pass's time per thousand entries.

## Consequences

- Every authored change to a hosted KB has a durable, attributed, verifiable record. ADR-113's
  "who approved what" and this "who changed what" share one identity model.
- History costs storage, because checkpoints hold full snapshots. D2's grouping and D5's policy
  bound it, and the benchmarks measure it.
- Tamper evidence is real only to the extent that heads are kept outside the daemon. The ADR says
  so, rather than implying more.

## Rejected

- **A version per CRDT update.** It is unreadable as history and its size is unbounded. The WAL and
  the editor record already hold that granularity.
- **History in the CRDT.** It is lossy under GC and rebirth, and it would replicate to every peer
  (ADR-106).
- **Deleting a pruned version outright.** That breaks the chain and erases who changed what. SharePoint
  audits its deletions for the same reason.
- **Rollback as a direct store write.** Peers never see it, and the next projection silently
  reverses it.
- **A per-row hash alone, as tamper evidence.** Anyone with write access recomputes it (CT).
