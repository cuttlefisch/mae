# ADR-030: In-text typed-link/relationship grammar + parser-as-projector

**Status:** Accepted (design); implemented in Phase C.
**Extends:** ADR-029 (CRDT is truth, cozo is the projection), ADR-005 (KB CRDT).
**Feeds:** ADR-031 (derived intelligence over the projected graph), ADR-034 (AI enrichment
writes relationships into the text).

## Context

ADR-029 makes a node's CRDT truth its **source text**. The pivotal sub-question: where do
**typed links and their relationship metadata** (`rel_type`, `weight`, `confidence`) live? Two
options were considered:

- **(A) A separate nested CRDT structure** on the node/collection doc (links as YArray<YMap>).
- **(B) Inline in the node's source text** (org-roam style), parsed out to the cozo graph.

We already ship the machinery for (B): `shared/kb/src/org.rs` parses `[[id:UUID][display]]` and
**typed** links `[[REL_TYPE:NODE_ID][display]]` via `classify_link`/`parse_typed_links`/
`parse_org_multi_result` (returning `OrgParseResult { typed_links, transclusions }`), and the
renderer already draws clean `markup.link` spans. Today `rel_type` is captured but `weight`/
`confidence` are cozo-only (set by manual editing or AI enrichment) and thus not synced or
survivable across a rebuild.

The relationship graph is what makes MAE a graph KB; its lifecycle must be **reliable**
(expensive to recompose over thousands of nodes) and it must **merge across peers**.

## Decision

**Store typed links and their relationship metadata inline in the node's CRDT source text**
(option B). The text is the authoritative representation; the cozo `links` graph is a derived,
incrementally-maintained projection produced by the parser.

1. **Extend the existing link grammar** to carry relationship metadata while keeping the rendered
   view clean. The base form `[[REL_TYPE:NODE_ID][display]]` is preserved; metadata is added as a
   trailing property form (exact syntax finalized in implementation, e.g. an attribute group
   `[[teaches:concept:buffer][display]]{w=0.8 conf=0.9}` or an org-link-parameter convention).
   Constraints: (a) the **default rendered view shows only the display text** (no metadata clutter
   — `markup.link`), (b) the **raw text remains the canonical source** AI reads when parsing a
   node, (c) backward-compatible with existing `[[…]]` links (absent metadata ⇒ defaults:
   `rel_type=references`, `weight`/`confidence` unset).

2. **The parser is the canonical projector.** `parse_typed_links`/`parse_org_multi_result`
   (extended for the new metadata) is the single deterministic function `text → (nodes, typed
   links)` feeding the cozo `links` relation (`add_typed_link_with_confidence`). It MUST be a pure
   function of the text (ADR-029 determinism contract): same text ⇒ same edges + metadata on every
   peer, in a deterministic order.

3. **Relationships are authored CRDT truth.** Because they live in YText, they sync + merge as
   ordinary text edits, survive any cozo rebuild, and require no separate nested-CRDT schema —
   keeping the CRDT schema small (minimal schema-evolution surface vs option A).

4. **AI enrichment writes relationships into the text** (ADR-034 / principle #3): the AI peer's
   enrichment is a CRDT text edit, so it propagates to all peers and each projects it — no peer
   re-runs the enrichment.

## Consequences

**Positive.** Reuses an existing parser + renderer (low risk). The graph is durable, mergeable,
and rebuildable. AI gets reliable raw relationships when reading a node. Smallest possible CRDT
schema (body is YText). Relationship lifecycle is owned by the projection (incremental, not
recomputed each start).

**Costs.** A grammar extension + parser/renderer update (the grammar becomes a load-bearing,
versioned format — covered by ADR-029's yrs-schema-versioning/upcast-on-read obligation, applied
here as **link-grammar versioning**). Heavy human editing of relationship metadata is a text edit
(line-oriented) — acceptable given relationship metadata changes are infrequent vs prose edits;
char-level interleaving risk on the rare concurrent same-link edit is bounded by the property-form
syntax + the projector tolerating malformed metadata (fall back to defaults, never drop the link).

## Alternatives rejected

- **(A) Links as a nested CRDT structure:** larger CRDT schema + more schema evolution + a parallel
  sync/merge path for link metadata, with no UX or correctness gain over inline text. Rejected.
- **Cozo-only typed links (status quo):** not synced, lost on rebuild, per-peer divergent.
  Rejected (fails ADR-029).

## Verification

Round-trip: author a typed link with metadata → text encodes it → projector yields the cozo edge
with `rel_type`/`weight`/`confidence` → rendered view shows only the display text. Determinism:
same node text ⇒ identical edges on two peers. Merge: concurrent relationship edits on two peers
converge (text CRDT) and both graphs match after sync. Backward-compat: legacy `[[…]]` links
project as `references` edges.
