# ADR-034: Cross-peer sharing of derived intelligence (compute-once)

**Status:** Accepted (design); implemented in Phase G.
**Extends:** ADR-031 (derived intelligence is a local projection), ADR-033 (the coordinator that
computes once), ADR-026 (membership = the trust boundary for sharing).
**Feeds:** the RAG/AI cost model; per-KB collection settings.

## Context

Under ADR-029/031 each daemon maintains its own local cozo projection + embeddings. For peers
working on the **same source CRDT with the same pinned config**, recomputing the same embeddings
and re-running the same AI enrichment on every peer is wasteful — redundant compute and, for hosted
models, redundant paid AI tokens. We want peers to **share the benefits of AI enhancement +
projection optimizations** instead of each recreating them, *without* compromising the CRDT-truth +
local-query model (ADR-029).

The key question is **what can/should be shared vs not**:
- **Reproducible-as-content** (AI-enriched relationships/metadata): can be expressed as text.
- **Expensive + non-textual** (vector embeddings): cannot be expressed as text; cannot be *verified*
  from content (you'd have to recompute).
- **Cheap + deterministic** (structural graph + FTS): trivially recomputed locally.

## Decision

Eliminate redundant derived compute with a three-way split, all gated by membership trust.

1. **Reproducible-as-content ⇒ bake into the CRDT source (free sync).** The coordinator's
   (ADR-033) AI enrichment writes relationships/metadata into node text (ADR-030); it syncs to all
   peers as ordinary CRDT ops and each projects it. **No peer re-runs the enrichment** — redundant
   AI enrichment is eliminated by construction.

2. **Expensive + non-textual (embeddings) ⇒ a content-addressed derived-artifact cache, shared
   peer→peer.** Keyed by `(content_hash, embedding_model_id, chunk_version)` (ADR-031), served over
   the existing transport (hub/p2p): the coordinator computes once; other members **fetch** the
   vectors instead of re-embedding. "Same source + same pinned model ⇒ identical vectors ⇒
   interchangeable" (prior art: content-hash + model-version embedding caches; CAS / build-cache
   compute-once-share — Nix, Bazel, sccache).

3. **Cheap + deterministic (structural graph + FTS) ⇒ stays LOCAL.** The parser is fast and a pure
   function of the CRDT; recomputing per-peer is cheaper than shipping + validating a cozo file (and
   avoids engine/version coupling). Not shared.

### Trust + correctness

- **Membership-gated (ADR-026).** A vector is not verifiable from content (verifying = recomputing,
  which defeats the saving), so sharing is **trust-based**: artifacts are accepted only from KB
  **members**; an artifact offered by a non-member is ignored (or recomputed). The **content-hash
  verifies *which content* the artifact is for** (the key is verifiable); the *value* is trusted
  from a member.
- **Model pinning is the interchangeability precondition.** Artifacts from a *different*
  model/config are not interchangeable — hence the per-KB pinned `embedding_model` + `chunk_version`
  (ADR-031). A peer fetching with a mismatched pin recomputes locally.
- **Opt-in verify** (off by default): a peer may recompute a fetched artifact to verify — defeats
  the saving, so reserved for low-trust or audit scenarios.

### Ecosystem wiring (per-KB collection settings)

In the collection CRDT doc: pinned `embedding_model` + `chunk_version`; `share_derived_artifacts`
on/off; the enrichment/embedding **coordinator** (owner default, or elected per ADR-033). ADR-033
decides *who* computes; ADR-034 decides *how the result is shared*; ADR-031 is the local projection
each peer still runs for the cheap structural parts.

## Consequences

**Positive.** Redundant embedding + AI-enrichment compute/cost is eliminated for peers on the same
source + pinned config. Preserves CRDT-truth + fast local queries. Reuses the existing transport +
membership trust boundary. Degrades gracefully (mismatched pin / non-member ⇒ local recompute).

**Costs.** A content-addressed artifact store + an advertise/request/serve protocol over the
transport. A trust decision recorded explicitly (members are trusted for artifact *values*).
Model-pinning migration (changing the pin ⇒ a coordinator re-embed + cache turnover).

## Alternatives rejected

- **Embeddings as CRDT truth:** unverifiable, model-coupled, bloats the CRDT (ADR-031). Rejected —
  share as a derived cache, not truth.
- **Sharing the whole cozo projection file:** engine/version-coupled + fragile + the structural part
  is cheap to recompute. Rejected — share only the expensive non-deterministic artifacts.
- **Recompute-everything-per-peer (status quo):** the redundancy this ADR removes. Rejected.

## Verification

Two members on the same KB + pinned model: peer B **fetches** the coordinator's vectors and does
**not** re-embed (assert no embedding compute on B for content A already embedded). Enrichment runs
once on the coordinator and appears on all peers via CRDT text. A non-member's offered artifact is
ignored. A peer with a mismatched model pin recomputes locally. Toggling `share_derived_artifacts`
off makes every peer compute locally.
