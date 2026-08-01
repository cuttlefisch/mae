# ADR-083: `kb_agenda` Becomes Federation-Aware (ScopedFederatedScanFilterable)

**Status:** Accepted, implemented.

## Context

`execute_kb_agenda` (`crates/ai/src/tool_impls/kb.rs`) accepted a `scope` argument (`"all"` |
`"local"` | `"remote"` | an instance name) suggesting it could query any registered KB, including
federated instances (e.g. a personal RoamNotes-style org-roam directory registered via
`kb_register`). In reality it only ever read `editor.kb.store` — the PRIMARY KB's own durable Cozo
store — then post-filtered the primary's results by `node_matches_scope`. A federated instance's
nodes were never part of the query's input relation at all: not a tag-matching bug, a missing query
entirely. Every filter type (`todo`/`priority`/`tag`/`stale`/`orphan`/`dead_end`/`missing_role`/
`weakly_linked`/`custom`) was equally affected.

This surfaced concretely while debugging why `kb_agenda(filter=tag, value="terraform-onboarding")`
returned zero results against a RoamNotes KB whose nodes were, in fact, correctly tagged —
`execute_kb_health` (a sibling tool) already does this correctly, iterating
`editor.kb.registry.instances` and querying each loaded `KnowledgeBase` directly, which made the
gap in `execute_kb_agenda` clear by contrast.

## Decision

`execute_kb_agenda` now mirrors `execute_kb_health`'s per-instance registry-iteration pattern:

1. Resolve `include_local` from `scope` (identical logic to `execute_kb_health`'s own
   `include_local`) and, if true, query the primary — via `editor.kb.store.agenda_query` when a
   durable store is configured, falling back to a new `KnowledgeBase::agenda_query_in_memory` when
   it isn't (closing a latent asymmetry: federated instances were about to gain in-memory query
   support, there was no principled reason primary should stay less capable).
2. Iterate `editor.kb.registry.instances`, filtered by the same scope-matching closure
   `execute_kb_health` already uses. For each matching instance: query its `CozoKbStore` handle
   (`editor.kb.instance_stores`) if one is open, else its in-memory `KnowledgeBase`
   (`editor.kb.instances`) via `agenda_query_in_memory`, else skip (registered but not loaded —
   same as `execute_kb_health`'s "not loaded" case).
3. Collect every hit as `(Option<String> instance_name, Node)` and post-filter through
   `mae_core::ai_residency::filter_residency_exempt` — the SAME primitive `kb_search`/
   `kb_search_context`/`kb_vector_search` already use, not a second implementation
   (`filter_residency_exempt_primary`, the old primary-only adapter, is deleted — nothing calls it
   anymore).

`KnowledgeBase::agenda_query_in_memory` (`shared/kb/src/lib.rs`) is the new in-memory equivalent of
`CozoKbStore::agenda_query`, matched field-for-field against the Cozo query text — NOT against
`nodes_by_tag`/`nodes_by_priority`'s existing exact-match secondary-index convention — so a caller
sees IDENTICAL results whether a given instance happens to be Cozo-backed or pure-in-memory:
`Tag` is a substring match against the JSON-encoded tags array (mirroring
`str_includes(tags_json, tag)`), `Priority` is `<=` (mirroring `priority <= min_pri`), and every
arm skips a node with an empty title (mirroring the Cozo query's own `title != ''` guard).
`Stale`/`Custom` have no faithful in-memory equivalent — no per-node last-modified timestamp exists
without a Cozo-backed store (the in-memory `StaleNode`/`detect_stale_nodes` concept is "source file
deleted from disk," a different fact entirely; `Custom` is arbitrary Datalog with no in-memory query
engine to run it against) — both return `Err`, and the affected instance is reported in a new
`skipped_instances` field on the tool's JSON response rather than silently omitted.

Reclassified in `crates/mae/src/ai_residency.rs` from `PrimaryOnlyFilterable` to
`ScopedFederatedScanFilterable` — the shape now genuinely matches (`kb_search`/`kb_search_context`/
`kb_vector_search`'s exact contract: scope resolved first, narrows which KBs even get queried, then
results are post-filtered for the seed-content exemption). `PrimaryOnlyFilterable` had zero real
tools left in it once `kb_agenda` moved, so the variant is deleted entirely rather than kept as a
now-empty shape (CLAUDE.md principle #15 — no speculative/dead code for a hypothetical future
tool of that shape).

## Consequences

**Positive**

- `kb_agenda` now actually does what its own `scope` parameter always implied it could do.
- `Stale`/`Custom` limitations are surfaced explicitly (`skipped_instances`), not silently.
- The `PrimaryOnlyFilterable` removal keeps the residency-classification enum honest — every
  variant in it has at least one real tool, checked by the pre-existing
  `every_kb_tool_and_help_open_is_explicitly_classified` test.
- No gate-level behavior change: `PrimaryOnlyFilterable` and `ScopedFederatedScanFilterable` both
  already mapped to `ResidencyDecision::Allow` unconditionally (enforcement always lived in the
  tool's own post-filter for both shapes) — this is a correctness fix to what gets queried, not a
  change to when the call is gated.

**Negative / Risks**

- `agenda_query_in_memory` duplicates the Cozo query text's semantics by hand (two places a filter
  variant's exact meaning is expressed: the CozoScript string and this Rust match arm) rather than
  one shared definition. Accepted as the pragmatic option — the alternative (an embedded query
  engine, or forcing every federated instance through a Cozo store) is a materially larger change
  for a codebase where CozoDB itself is already the chosen query layer for the primary path; kept
  in sync via the new field-for-field-matched test suite (`shared/kb/src/lib.rs`'s
  `agenda_query_in_memory_*` tests) rather than a single source of truth.

## Alternatives Considered

**Force every federated instance through a Cozo-backed store (no more purely in-memory
instances).** Would eliminate the two-implementation duplication above, but `Editor::kb_reimport`
already falls back to a pure in-memory import when opening/creating a store fails (disk issues,
lock contention) by design — removing that fallback path is a much larger, separate architectural
change and would make a currently-recoverable failure mode (can't open the store right now) into a
hard failure instead.

**Keep `kb_agenda` primary-only, document the limitation instead of fixing it.** Rejected — the
tool's own `scope` parameter and documentation already promised federation-wide behavior; leaving
the implementation silently narrower than its own contract is exactly the kind of drift CLAUDE.md
principle #15 calls out.
