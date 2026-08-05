# ADR-093: The node CRDT carries the whole node

**Status:** Accepted. Schema v2 is implemented in `shared/sync/src/kb/node.rs` with the
bridge in `shared/kb/src/lib.rs`; Gate A's four criteria are green. The one-time backfill
that gives existing nodes a v2 lineage belongs to ADR-094 and has not run.
**Extends:** ADR-029 (CRDT is truth, Cozo is a rebuildable projection — if the CRDT cannot
hold a field, then no amount of projection can preserve it).
**Supersedes:** ADR-092 Decision 4 ("what is editable is bounded by what syncs"), whose bound
existed only because the schema was incomplete. With the schema complete, the bound dissolves
and `kb_edit_scope=full-org` becomes reachable.
**Relates to:** ADR-092 D2 (character-level text updates — the same no-duplication discipline
is applied here to `YMap` and `YArray`), ADR-033 (eager container seeding), ADR-032 (durable
snapshot/backup — a checkpoint is only as complete as the schema it captures).

## Context

`KbNodeDoc` carried `id/title/body/tags/links/meta`. `mae_kb::Node` carries six more fields:
`kind`, `todo_state`, `priority`, `aliases`, `properties`, `source_version`. The bridge
reflected the gap — `Node::from_crdt_doc` reconstructed only `id/title/body/tags`, taking
`kind` and `source` as *caller arguments*, and `apply_crdt_doc` wrote back only title/body/tags.

This was invisible because it was latent. These KBs are unshared, so `kb_update_node_with`
branches on `kb_sync_target`, persists straight to Cozo — which stores every field
(`cozo_store/schema.rs:201-214`) — and leaves `crdt_doc` as `None`. Nothing round-trips through
the CRDT, so nothing is lost.

It stops being latent the moment any node is given a CRDT lineage, which is precisely what
`kb_prepare_share_lineage` (`kb_ops/nodes.rs:400-446`) does, and precisely what a hosted,
CRDT-as-truth deployment requires of every node. For the corpus this project is about to
migrate — 3,004 nodes, 2,457 of them org-roam notes whose `:ID:` and `:ROLE:` live in
`properties` — the operation that enables hosting is the same operation that discards the data.

The existing test was no help: `crdt_bridge_roundtrip_preserves_all_fields` set none of the six
fields, so it could not fail on them, and its one non-text assertion (`source`) checked a value
the test had itself passed in as an argument. It has been renamed to
`..._preserves_text_fields`, which is what it covers.

## Decision

1. **The node document carries every `Node` field.** `kind`, `todo_state`, `priority`,
   `source_version` as scalar `YMap` entries; `aliases` as a `YArray`; `properties` as a nested
   `YMap`. A `schema_v` key records the version; **absent means v1**.

2. **New fields are optional and readers tolerate their absence.** Every v2 accessor returns a
   default when its key is missing, so a v1 document opens under v2 unchanged. This is the
   expand half of expand/contract: add without removing, and do not require what has not been
   backfilled.

3. **There is no upcast-on-read.** Opening a v1 document authors nothing. Fields are written
   only when the application writes the node anyway.

   This is the decision that matters most, and it is not an optimization. Automerge's own
   documentation names concurrent migration as what makes CRDT schema change harder than a
   centralized one: *"it could happen that two users independently perform the same migration.
   In this case, you need to ensure that the two migrations don't clash with each other, which
   is difficult."* Their recommended mitigation is hard-coded changes with deterministic
   actor/timestamp so concurrent peers emit byte-identical ops. **MAE avoids needing the
   mitigation**: if reading never writes, there is no migration op to clash. The one-time
   backfill (ADR-094) is a deliberate single-writer pass, not a read-triggered side effect.

4. **Containers are seeded eagerly at construction, never created lazily on first write.**
   Two peers each inserting a fresh `YMap` at the same key means one wins and the loser's
   entries vanish. `collection_core.rs` already records this for `COLL_LEASE_KEY` under
   ADR-033; this ADR applies the same rule to `properties` and `aliases`. Caught by Gate A.3,
   which failed until the containers were seeded.

5. **`properties` merges per key; `aliases` diffs by value.** Neither is a clear-and-refill.
   Two peers editing *different* properties must both survive — a wholesale replace would
   converge while silently dropping one peer's unrelated key, which is ADR-092 D2's defect in
   `YMap`/`YArray` form. The schema-version stamp is likewise written only on a real change, so
   an unchanged save still authors no ops.

6. **`from_crdt_doc`'s `kind`/`source` arguments become fallbacks**, used only when the document
   does not carry them. A v2 document's own values win. Otherwise the function echoes its
   caller's arguments back and "round-trip" means nothing — which is exactly what the old test
   was asserting.

## Consequences

**Positive.** A CRDT-born node is now a complete node, so the migration ADR-094 describes can be
lossless rather than lossy — which is the difference between it being possible and not. ADR-092's
editing bound lifts, so the properties drawer becomes safely editable and `kb_edit_scope=full-org`
is reachable. A checkpoint (ADR-032) now captures a whole node rather than a fragment. And the
projector, when wired, can rebuild a full Cozo row from CRDT truth instead of a partial one.

**Costs, honestly.** The document is larger and every node write now touches more keys, though
the no-op guards mean an unchanged field still authors nothing. Scalars are last-write-wins per
key, so two peers setting *the same* scalar concurrently resolve arbitrarily-but-deterministically
rather than merging — acceptable for `kind`/`todo_state`/`priority`, which are enumerated values,
and explicitly not the model for text. Existing v1 documents remain v1 until something writes
them; until the ADR-094 backfill runs, a KB is a mix of v1 and v2 nodes, and that is intended
rather than a transitional embarrassment.

**Not addressed here.** `Node` still has no timestamps. If the hosted UI needs created/modified,
they belong in this schema and should be added before the backfill rather than retrofitted after.

## Alternatives rejected

- **Upcast v1 documents on read, with deterministic actor ids** (Automerge's own recommendation).
  Rejected because not writing is strictly safer than writing identically: it removes the failure
  mode instead of making the two writes agree, and it needs no coordination on what "identical"
  means across versions. The mitigation would be the right answer if reading *had* to upgrade;
  here it does not.
- **Keep ADR-092 D4's bound and leave the fields out of the CRDT.** Rejected: the bound was a
  consequence of the gap, not a design goal, and it makes a lossless migration impossible. Keeping
  it would mean either migrating lossily or not migrating.
- **Store the extra fields as serialized JSON in the existing `meta` map.** Rejected — a JSON blob
  is a single opaque value, so two peers editing different properties conflict on the whole blob.
  That reintroduces exactly the clobbering Decision 5 exists to prevent.
- **Fold the front matter into the body text instead of adding fields** (ADR-029 §1's model, and
  ADR-092's deferred "Phase 7"). Not rejected on merit — it remains the more faithful long-term
  shape — but it makes every property edit a text edit subject to text merge semantics, and it
  requires a body-format migration across the whole corpus *before* the corpus can be migrated at
  all. Typed fields are reachable now and are what unblocks ADR-094.

## Verification

Gate A, all four green, each stated as an observation rather than a review:

1. **`crdt_roundtrip_preserves_every_node_field`** — `Node → KbNodeDoc → Node` is field-wise
   identical, with `from_crdt_doc` called using deliberately *wrong* `kind`/`source` arguments so
   a pass proves the document carried them. Failed before this change, on `kind`.
2. **`a_v1_document_opens_under_v2_without_loss_or_spurious_ops`** — every v2 accessor tolerates
   the absent key, **and the encoded length is unchanged after reading**. The second half is the
   real assertion: it proves reading authors nothing, which is what Decision 3 claims.
3. **`two_peers_adding_v2_metadata_to_a_v1_node_converge_without_duplication`** — two peers apply
   the *same* migration plus their own distinct property; each value appears once and both
   properties survive. This is the Automerge hazard made falsifiable, and it failed until
   Decision 4 was implemented.
4. **`concurrent_property_edits_on_different_keys_do_not_clobber`**,
   **`concurrent_alias_edits_do_not_duplicate_shared_aliases`**,
   **`setting_v2_fields_to_their_current_values_produces_no_ops`** — the ADR-092 D2 oracles
   restated for `YMap` and `YArray`. Peer-equality alone is free from a CRDT and would pass on a
   clear-and-refill; the oracle is what happened to the key the other peer wrote.
