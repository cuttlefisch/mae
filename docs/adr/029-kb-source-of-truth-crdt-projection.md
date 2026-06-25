# ADR-029: KB source of truth = CRDT; CozoDB = derived projection

**Status:** Accepted (design); implementation phased (epics A–I, see plan). Foundational ADR
for the KB data-architecture redesign.
**Extends / clarifies:** ADR-005 (KB CRDT — realizes its "Phase C" intent), ADR-011 (CozoDB as
the KB backend — reframes cozo as a *projection*), ADR-006 (collaborative state engine),
principle #11 (CRDT-first sync).
**Feeds:** ADR-030 (in-text relationship grammar / parser-as-projector), ADR-031 (derived
intelligence), ADR-032 (durable CRDT store), ADR-033 (operation coordination), ADR-034
(cross-peer derived-artifact sharing), ADR-035 (editor↔daemon boundary — the daemon is optional).

## Context

The KB must satisfy four goals **simultaneously**: (1) reliable multi-user CRDT editing,
(2) native graph-DB tooling (rapid search/traverse over nodes with semantic relationships),
(3) native AI/RAG optimizations (vector + graph retrieval), and (4) correct, performant
operation across **all** supported sync configurations: **hub-hosted, P2P mesh, or both at once**
(ADR-025).

**Today's reality (the problem).** CozoDB is the de-facto source of truth on *both* the editor
and the daemon, and the two are independent masters reconciled only through the yrs collab
layer. Per-node yrs state is stored as a `crdt_doc` BLOB *inside* each cozo row and reconstructed
on load — so principle #11 ("yrs is the substrate, cozo a rebuilt mirror") and ADR-005's Phase C
are **inverted in practice**. Two concrete consequences:
- **Duplication by default:** editor and daemon each build a full cozo KB; sync is two masters
  merging via CRDT.
- **A real bug:** the daemon **never persists collab edits back into its own cozo store**
  (`collab_handler` `kb/node_update` writes only the yrs `doc_store`), so the daemon's
  queryable/integrity view is stale relative to what it is syncing.

**The decisive constraint is multi-configuration correctness.** A "cozo is master" model is only
correct when a single authority owns the database:

| Config | cozo-as-master | CRDT-truth + cozo projection |
|---|---|---|
| Hub-hosted | works (hub = single authority) | works (hub holds CRDT, projects locally) |
| P2P mesh | **breaks** — N cozo masters cannot cleanly merge | works (CRDT converges; each daemon projects) |
| Both at once | **breaks worse** — two authorities, two transports | works (CRDT is transport-agnostic + idempotent) |

MAE's mesh (ADR-025) removes the single authority, so "reliable across hub, p2p, and both" does
not merely *favor* CRDT-truth — it **requires** it.

**Prior art** (cited; full report in the initiative's plan agent log). For *decentralized*
systems the durable source of truth is the CRDT/op-log and the queryable DB is a derived,
rebuildable index — Actual Budget (CRDT message-log truth → SQLite materialized view),
Automerge-repo ("build your own secondary index outside the document … use the doc id as a
foreign key"). The DB-as-truth products (Figma, Linear, Notion) rely on a *central server* — the
exact thing the mesh removes. The one real "CRDT-on-relational-DB master" attempt
(ElectricSQL's legacy mode) was abandoned. A unified CRDT-DB (cr-sqlite/Ditto) gives neither
graph traversal nor vector search, so it cannot replace Cozo's Datalog + HNSW. Kleppmann's
"turning the database inside-out": a materialized view is a cached, rebuildable subset of the
log; the same log can feed KV, FTS, *and* a graph index.

## Decision

**The yrs/CRDT layer is the canonical, durable source of truth. CozoDB is a deterministic,
durable, rebuildable PROJECTION** — the graph + FTS + vector index and the home of derived
intelligence — maintained locally by every daemon that holds a CRDT replica.

1. **A node's CRDT truth is its source text.** Each node is a yrs doc whose body is
   org/markup-flavored text (title + body, with todo/priority/tags/properties and **typed links
   + their relationship metadata encoded inline**, org-roam style — see ADR-030). The collection
   doc (`kbc:{kb_id}`) holds the node manifest + membership + policy + signed oplog (already
   CRDT, ADR-026). Cozo holds *no authoritative content* — only the projection + derived state.

2. **Cozo is produced by a deterministic projector** = the org parser we already ship
   (`shared/kb/src/org.rs` `parse_org_multi_result`/`parse_typed_links`), run incrementally. The
   **structural projection (nodes, typed-link graph, FTS) MUST be a pure function of CRDT state**:
   the same converged CRDT yields a byte-identical cozo graph on every peer. This is what makes
   the *graph* converge across peers, not just the text.

3. **One universal projection seam.** Every update — hub (`collab_handler`) and p2p (`dialer`) —
   already lands at `doc_store.apply_update`. A projector subscribed to that change feed covers
   all configurations with one mechanism: CRDT update → `doc_store` (truth, WAL-durable) →
   projector parses the changed node text → updates cozo. No per-transport projection logic.

4. **Cozo is rebuildable + self-healing.** Deleting cozo and replaying the CRDT yields an
   identical structural projection. Checkpoints + incremental projection (ADR-032) keep a
   multi-thousand-node KB off the full-rebuild path except on schema bump / corruption.

5. **Derived intelligence is local, not synced truth** (ADR-031): embeddings/HNSW are computed
   per-daemon from content (content-hash + model-version + chunk-version cache); they converge
   across peers iff the per-KB embedding model is pinned, and may be *shared* as a content-
   addressed cache (ADR-034) — never as CRDT truth.

6. **The daemon owns truth + projection + intelligence; the editor is a client** (Phase D): when
   a daemon is present the editor edits via the CRDT and queries the daemon's cozo via RPC,
   keeping no second durable master. **The daemon is OPTIONAL, not required** — a daemon-less editor
   runs the embedded in-process equivalent as the default *floor* (`daemon_mode=off`), not merely a
   fallback. ADR-035 formalizes the editor↔daemon boundary + the `daemon_mode` behavior-set.

### Per-configuration behavior

- **Hub-hosted:** the hub daemon holds the authoritative CRDT replica + the cozo projection;
  thin-client editors edit via CRDT and query the hub's cozo via RPC (the `LruQueryLayer` cache
  hides latency). One projector on the hub.
- **P2P mesh:** each daemon holds a CRDT replica + a local cozo projection; editors query their
  *local* daemon (lowest latency, local-first). Projections converge because the CRDT converges
  and the projector is deterministic.
- **Both:** CRDT flows over hub *and* mesh; idempotent `apply_update` makes double-delivery safe
  (echo-avoidance is an existing perf optimization in the dialer). Each daemon projects locally.

## Consequences

**Positive.** Correct + self-healing across hub/p2p/both (the only model that is). Fixes the
stale-daemon-store bug structurally. Query performance unchanged (cozo stays the read layer).
Eliminates the two-master duplication. The CRDT schema stays small because content + relationships
ride YText (ADR-030), minimizing schema-evolution surface.

**Costs / obligations.**
- A new **projector** component (incremental, idempotent, checkpointed, with a tested full-rebuild
  path) — the core new work (Phase B).
- **Write-path inversion:** KB mutations write the CRDT first, then project; editor mutations
  become RPC to the daemon (Phase D).
- **Determinism is a hard contract** for the structural projector (test: two daemons, same CRDT
  ⇒ identical cozo graph).
- **Two unavoidable CRDT-truth costs** (prior art): tombstone/metadata growth ⇒ checkpoint +
  compaction (ADR-032/028); CRDT **document schema evolution** ⇒ version yrs schemas with
  **upcast-on-read from day one**.

## Alternatives rejected

- **Cozo-as-master + daemon write-back** (~5× less work): incorrect for p2p/both (diverging
  masters), not self-healing, and the model prior-art shows abandoned. Rejected — it fails the
  multi-config requirement, which is the whole point.
- **Unified CRDT-DB** (cr-sqlite / Ditto): no graph traversal + vector search; would lose Cozo's
  Datalog + HNSW. Rejected.
- **Org dirs as the live store:** import-only (ADR-011); the KB lives in the CRDT + its cozo
  projection, not in `.org` files.

## Verification

Determinism (same CRDT ⇒ identical cozo graph on two daemons; delete cozo → rebuild → identical
queries); hub (thin-client edit reflected in hub cozo + live to a second editor); p2p (ingest on
A → B pulls the same typed-link graph; concurrent edits merge + converge; offline heal); both
(no double-apply divergence). See the plan's cross-config verification matrix.
