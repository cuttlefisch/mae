# ADR-101: Links as first-class structured CRDT edges

**Status:** Proposed. Phased (0–5), each phase independently shippable behind a test that fails
first. **Phase 3 (projector union) is the load-bearing one** and says so. This ADR is a **prerequisite
for ADR-103** (autonomous link enrichment) — that feature has nowhere durable to write a machine
link until this lands.

**Extends:** ADR-029 (CRDT is truth, Cozo is a rebuildable projection — *if the CRDT cannot hold a
field structurally, no projection can preserve it*; this ADR gives links the structural home the
principle already implies) and ADR-093 (the node CRDT carries the whole node — this ADR is the next
schema step, `schema_v = 3`, and copies its tolerant-reader / no-upcast-on-read / single-writer-
backfill discipline verbatim).

**Amends:** ADR-030 (in-text relationship grammar). ADR-030 chose *option B* — links live inline in
the body text and the cozo graph is a parse-time projection. That choice is correct **for
human-authored links** and stays. This ADR adds a *second, structured* edge source for
**machine-authored** links (enrichment, future importers) that must carry provenance and a review
lifecycle the inline grammar cannot express, and makes the projector consume the **union** of the
two. ADR-030's "the body text is the truth for the links a human writes there" is preserved
unchanged; what changes is that it is no longer the *only* truth.

**Relates to:** ADR-092 (one write path for a KB node — structured edges are written through the same
`kb_update_node_with` chokepoint, never a direct cozo write), ADR-033 (KB-wide op coordination — an
edge status flip is a single-node edit and takes no lease), ADR-011 (its noted live bug — "the daemon
never persists collab edits back into its own cozo store" — must be resolved for the structured-edge
round-trip to survive a daemon restart; cross-linked, not owned here).

**Evidence:** `shared/sync/src/kb/node.rs` (the dead `links` field), `daemon/src/projector.rs`
(`project_node` → `replace_node_links`), `shared/kb/src/cozo_store/links.rs`, `shared/kb/src/org.rs`
(`parse_typed_links`, `ParsedLink`).

## Context

A KB link has exactly one durable representation today: a substring of the node's body text. The node
CRDT (`KbNodeDoc`, `shared/sync/src/kb/node.rs`) is the source of truth (ADR-029), and the cozo
`links` relation is a projection the daemon rebuilds by **parsing that body text**:

```
project_node (daemon/src/projector.rs)
  → KbNodeDoc::from_bytes(state)                  // CRDT truth
  → Node::from_crdt_doc(...)                      // body: String
  → mae_kb::org::parse_typed_links(&body, &id)    // [[dst?rel=X&w=Y&c=Z][disp]]  → edges
  → store.replace_node_links(&id, &links)         // wipe ALL edges of `id`, re-insert parsed set
```

`replace_node_links` (`shared/kb/src/cozo_store/links.rs:88`) is a **clear-then-reinsert**: it
deletes every outgoing edge of the source node and re-inserts exactly the set parsed from body text.
Two consequences are load-bearing for this ADR:

1. **Any edge not in the body text is destroyed on the next projection of its source node.** A link
   written straight to the cozo `links` relation via `add_typed_link_with_confidence`
   (`links.rs:59`) survives only until the source node is next edited, reimported, or re-projected —
   then it is silently gone. (The seeded index→concept edges, `links.rs` `seed_typed_relationships`,
   are re-seeded on every rebuild precisely because they have no body text to derive from — the sole
   exception, and it exists only because it is re-run every time.)
2. **The only durable place to add a link is the body text**, and the current AI path does exactly
   that — `execute_kb_add_link` appends `\n[[dst?rel=X&w=Y][dst]]` to the node body
   (`crates/ai/src/tool_impls/kb.rs`). For a human writing one link, fine. For a background enricher
   proposing thousands of machine links, this is wrong on four counts, each independently
   disqualifying:
   - it **mutates human prose** with machine-generated text, mixing authorship in one `YText`;
   - the inline grammar has **no field for provenance** (which model, when, why) — `c=` is a number,
     not an audit trail;
   - it has **no review lifecycle** — a link is either in the text (live) or not; there is no
     `pending` state a human can accept or reject without text surgery;
   - a machine appending to `body` while a human edits the same `body` is **exactly the concurrent-
     `YText` contention** ADR-092 D2 spent a whole decision fixing (the wholesale-replace bug; see
     `set_body` → `reconcile_text_ref`, `node.rs:231`).

Meanwhile the CRDT schema **already has a `links` field** — `LINKS_KEY` (`node.rs:21`), seeded
eagerly as a `YArray` (`node.rs:92,127`), with `add_link`/`remove_link` methods (`node.rs:~537`). It
is a **dead field**: `write_v2_fields` (`shared/kb/src/lib.rs`) does not write it, `apply_crdt_doc`
does not read it, and `add_link` is never called outside tests. It holds bare target strings and
carries no relationship metadata even if it were used. The structural slot exists; nothing lives in
it, and its current shape (`YArray<String>`) is too thin to hold an edge anyway.

So the gap is not "the projection is authoritative" — it correctly is not (ADR-029 holds). The gap is
that **a link's structured truth has no home in the CRDT**; its only home is prose. That is fine for
the human inline case and unacceptable for a machine-authored, provenance-bearing, review-gated edge.

## Decision

### D1 — Structured edges are first-class CRDT content (`schema_v = 3`)

Replace the dead `links: YArray<String>` with a structured edge collection on the node CRDT. Each
edge carries:

| Field | Type | Notes |
|---|---|---|
| `dst` | String | target node id (fragment allowed, e.g. `concept:buffer#rope`) |
| `rel_type` | String | one of the known relationship types (`rel_types` relation; validated at write, tolerated on read) |
| `weight` | Float | 0.0–1.0 |
| `confidence` | Float | 0.0–1.0 — for `human` edges defaults 1.0; for `ai` edges the calibrated score (ADR-103) |
| `provenance` | String | `human` \| `ai` — **the field the inline grammar cannot express** |
| `status` | String | `pending` \| `accepted` \| `rejected` — the review lifecycle |
| `rationale` | String | one-line justification (ai edges); empty for human |
| `model` | String | e.g. `qwen2.5:14b` for ai edges; empty for human |
| `created_at` | Int | epoch millis |

These are the same columns the cozo `links` relation already persists (`display`, `weight`,
`confidence`, `created_at` — `schema.rs:236`) **plus** `provenance`, `status`, `rationale`, `model`,
which the projection must gain (D4).

Readers are **tolerant**: a `schema_v < 3` document has no structured edges and is read as "zero
machine edges, all links come from body text" — identical to today. **No upcast on read** — opening a
v1/v2 node MUST NOT write a v3 edge container; that is a live CRDT hazard (two peers each author the
migration op and clash — the exact reasoning in `node.rs:24-45`'s `@ai-caution`). The one-time
backfill is a deliberate single-writer pass (D5), not a read-side effect.

### D2 — CRDT representation: `YMap<edge_id → YMap>`, not `YArray<YMap>`

Store edges as a `YMap` keyed by a **content-derived edge id** `edge_id = hash(dst | rel_type)`, each
value a nested `YMap` of the D1 fields. Rationale, and the tradeoff named honestly:

- **Dedup is structural.** Two peers proposing the same (dst, rel_type) edge converge on **one** map
  at the same key rather than two array entries the projector must later dedup. A `YArray<YMap>`
  would accumulate duplicates under concurrent append.
- **The known hazard** (`node.rs:94-98`): concurrently *creating* the nested map at the same key can
  drop one peer's map. We accept it here **because** the dedup semantics we want *are* "same key = one
  edge", and we mitigate exactly as the existing code does — the field-level writes inside the nested
  map are last-writer-wins per field (fine: `status`/`confidence` are single scalars where LWW is the
  desired convergence), and we never lazily create the top-level edge `YMap` (seed it eagerly at node
  creation, same pattern as `ALIASES_KEY`/`PROPS_KEY`). A concurrent same-edge proposal that races on
  first-create loses one peer's *rationale/model* metadata but not the edge's existence or its
  eventually-converged status — an acceptable loss for a machine annotation, and one the review UI
  surfaces rather than hides.
- Edge **removal** is a status flip to `rejected` (tombstone), never a `YMap` delete, so a concurrent
  re-propose can't resurrect a rejected edge by racing a delete.

### D3 — Human inline links are unchanged; machine links never touch the body

Human-authored links stay in body text as ADR-030 grammar and remain fully editable there. Structured
edges are `provenance = ai` only. **No code path writes a machine link into `body`**;
`execute_kb_add_link`'s body-append behavior is retained *only* for the explicit human/agent "add this
one link" action and is out of scope to change here (it may later be migrated to write a
`provenance=human, status=accepted` structured edge, but that is not required by this ADR). The
invariant this ADR establishes: **the body YText is written only by humans (and the human's agent
acting as a peer); the structured edge map is written only by the machine-enrichment path.** No
surface writes both.

### D4 — The projector consumes the union (the load-bearing change)

`project_node` (`daemon/src/projector.rs`) and its in-process twin `update_links_for_node`
(`links.rs:18`) compute the cozo `links` relation as:

```
live_edges(node) =
    parse_typed_links(node.body)                       // ADR-030 human inline (status: accepted)
  ∪ { e ∈ node.structured_edges : e.status == accepted }   // machine, promoted
```

`pending` and `rejected` structured edges are **not** projected as live cozo edges — they exist in the
CRDT and surface only through the review layer (ADR-103). `replace_node_links` stays the wipe-and-
rebuild primitive; it is simply fed the union rather than the parse-only set. **Determinism is
preserved** (ADR-029 D2): the same CRDT state → the same cozo graph, now over a union that is a pure
function of CRDT content. The cozo `links` schema gains `provenance` so a projected machine edge is
distinguishable from a human one at query time (needed by the review surface and by
`kb_links_from`, which today drops even `confidence` on read — a companion fix, ADR-103 PREREQ-C).

Collision rule when a human inline link and a machine edge name the same (dst, rel_type): the **human
edge wins** (provenance=human, confidence=1.0), and the machine edge is auto-`rejected` as redundant
(recorded, not silently dropped) so the enricher does not re-propose it every sweep.

### D5 — Backfill is a single-writer pass (follows ADR-094's shape)

Existing v1/v2 nodes gain no structured edges until touched. A one-time, single-writer backfill
(daemon-side, the same deliberate-pass shape ADR-093 defers to ADR-094) may materialize existing
`provenance=human` inline links into structured edges **only if** we later want inline links
represented structurally; **this ADR does not require it** — the union in D4 means inline links keep
working with zero migration. The backfill is named here so it is a conscious *option*, not an implied
obligation.

## Consequences

**Positive**
- Machine links get a durable, syncable home that survives projection (fixes the destroy-on-reproject
  trap) without polluting human prose.
- Provenance, confidence, rationale, model, and a `pending/accepted/rejected` lifecycle become
  first-class — the substrate ADR-103's review queue and threshold gating need.
- CRDT stays the single source of truth; the cozo graph stays a deterministic projection — ADR-029
  strengthened, not bent.
- The human inline path is untouched; zero migration required for existing KBs.

**Negative / risks**
- A third schema version to keep tolerant-reader-correct; the `@ai-caution` discipline is now load-
  bearing across three versions.
- The nested-`YMap`-first-create race (D2) is a real, accepted small loss of machine metadata under
  exact concurrent same-edge proposal; mitigated, surfaced, not eliminated.
- The projector union adds work per projection proportional to structured-edge count; bounded (edges
  per node are few) but must be benchmarked with PREREQ-B's suite, not assumed.
- ADR-011's daemon-persistence bug must be fixed for a structured edge to round-trip a daemon
  restart; this ADR depends on that and says so rather than papering over it.

## Explicitly out of scope (named, not silently absent)
- Migrating `execute_kb_add_link` / the human "add link" surface onto structured edges (D3 keeps it
  on body text).
- The enrichment logic that *produces* machine edges (candidate discovery, model judgment, confidence
  calibration, threshold gating, the review buffer) — all ADR-103.
- Any change to the inline grammar itself (ADR-030 stands).
- Distributed/hosted DB backend concerns (ADR-102).

## Phased implementation (each phase fails a test first)
- **Phase 0** — `schema_v = 3` constant + tolerant readers + eager container seed; a v2 doc opens
  unchanged (test: v2 bytes → zero structured edges, body links intact).
- **Phase 1** — structured-edge CRUD on `KbNodeDoc` (`upsert_edge`, `set_edge_status`, `edges()`),
  returning `#[must_use]` update bytes like the existing setters (test: N-way convergent edge upsert;
  concurrent same-edge → one edge; status flip converges LWW).
- **Phase 2** — `Node`/`MaterializedNode` carry structured edges; `write_v2_fields`/`apply_crdt_doc`
  round-trip them (test: encode→decode identity incl. provenance/status).
- **Phase 3 (load-bearing)** — projector union in `project_node` + `update_links_for_node`; cozo
  `links` gains `provenance`; human-vs-machine collision → human wins, machine auto-rejected (test:
  deterministic projection over union; pending/rejected NOT live; reproject does not destroy a
  machine `accepted` edge — the regression the whole ADR exists to prevent).
- **Phase 4** — surface `provenance`+`confidence` on `kb_links_from`/`links_to` read (fixes the
  drop-on-read gap; test: query returns them).
- **Phase 5** — optional single-writer backfill (D5), behind a flag, idempotent (test: idempotent, no
  read-side upcast).

## Adversarial tests (principle #14 — the failure modes, not the happy path)
- A machine `accepted` edge **survives** a source-node body edit + reproject (the core regression).
- Two daemons proposing the same edge converge to one edge with a single converged status (≥3-peer).
- A `rejected` edge is **not** resurrected by a concurrent re-propose racing a delete.
- Opening a v1 and a v2 doc writes **no** migration op (no upcast-on-read; assert state-vector
  unchanged).
- `pending`/`rejected` edges never appear in the projected cozo graph or in `kb_links_from` live
  results.
- Human inline link and machine edge on the same (dst, rel_type): human wins, machine recorded as
  rejected, enricher does not re-propose next sweep.
