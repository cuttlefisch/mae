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

1. **Link grammar — orderless key-value attributes in the target (FINALIZED).** A link is
   `[[TARGET][DESCRIPTION]]` where

       TARGET = NODE_ID [ "#" FRAGMENT ] [ "?" KV ( "&" KV )* ]
       KV     = KEY "=" VALUE

   All relationship metadata lives as **orderless, extensible key-value pairs** in the target's
   query, alongside a **clean NODE_ID** (not a prefix on it):

       [[concept:buffer?rel=teaches&w=0.8&c=0.95][the buffer]]

   - **Recognized keys (today):** `rel` (relationship type, default `references`), `w`/`weight`,
     and `c`/`conf`/`confidence` (each 0–1, clamped; default 1.0).
   - **Unknown keys are parsed into a generic `attrs` map and preserved verbatim** in the source
     text — so a future key (`?rel=cites&since=2026-06&by=ai&strength=hard`) needs **no grammar
     change**: today's parser tolerates + carries it, tomorrow's code reads it from `attrs`. This
     is the URI-query model (orderless, extensible); org-roam/wikilinks have no inline
     edge-attribute equivalent, so we extend past them deliberately.
   - **Graceful parsing:** any key order; unknown/custom keys are kept (ignored by the graph
     today); a malformed/non-finite value falls back to its default; a bad query never drops the
     link.
   - **Clean break (no users yet):** the legacy `REL_TYPE:NODE_ID` prefix form is **removed**, not
     maintained — the shipped manual KB is refactored to the new syntax. This eliminates the
     prefix's rel/node-id namespace ambiguity (`foo:bar` was rel:node *or* a `foo:`-namespaced id).
   - **Rendering:** auto-concealed — the query is inside `[[…]]`, hidden by the existing link
     rendering (only the DESCRIPTION shows); link resolution strips the query + fragment to recover
     NODE_ID. No special-case concealment.
   - **Docs:** the grammar is documented as a first-class manual KB node (`concept:kb-link-syntax`)
     to remove ambiguity for humans + the AI peer.

   Supersedes the C1 first cut (an appended `{…}` slug) and the `REL_TYPE:` prefix form.

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
