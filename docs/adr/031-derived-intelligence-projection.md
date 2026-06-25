# ADR-031: Derived-intelligence projection (FTS, embeddings, hygiene)

**Status:** Accepted (design); implemented in Phase E.
**Extends:** ADR-029 (cozo = projection), ADR-030 (the structural projector).
**Feeds:** ADR-034 (cross-peer sharing of the expensive derived artifacts).

## Context

Beyond the structural graph (nodes + typed links), cozo holds **derived intelligence**: the FTS
index, the HNSW vector embeddings (384-dim, `embeddings` relation + `embeddings:semantic` HNSW),
and hygiene/health suggestions. Under ADR-029 these are *not* CRDT truth; they are projections.
But they differ in cost and determinism, so they need distinct treatment:

- **FTS** is cheap + a deterministic function of node content.
- **Embeddings** are expensive (compute / paid API tokens) and depend on the model + chunking, not
  just content.
- **Hygiene/AI suggestions** are expensive and may be non-deterministic.

The prior-art consensus for derived indexes over a source of truth: **fully rebuildable, but
maintained incrementally and cached by content so a rebuild never re-does unchanged work**
(Kleppmann "inside-out"; LlamaIndex/LangChain embedding caches keyed by content hash; checkpointed
idempotent projectors).

## Decision

Each daemon maintains its derived intelligence as a **local projection** of its CRDT replica,
incremental and content-addressed. (Sharing these artifacts across peers to avoid redundant
compute is ADR-034; this ADR defines the local mechanism each peer runs.)

1. **FTS** is rebuilt incrementally from changed node text by the structural projector (ADR-030) —
   re-tokenize only changed nodes; full rebuild only on schema bump / corruption.

2. **Embeddings are content-addressed + cached.** The cache key is
   `(content_hash, embedding_model_id, chunk_version)` — **not** content alone, because a model or
   chunking change must invalidate. Unchanged content is never re-embedded. A node content change
   schedules a (re-)embed for its changed chunks only.

3. **Per-KB model pinning.** The embedding `model_id` + `chunk_version` are **per-KB collection
   settings** (in the collection CRDT doc). This makes vectors *interchangeable across peers* on
   the same KB (the precondition for ADR-034 sharing) and makes vector-search results converge.

4. **Determinism boundary.** The *structural* projection (graph/FTS) must be deterministic
   (ADR-029). Embeddings need only converge **given a pinned model**; vector-search results may
   differ slightly across peers running different models — acceptable for RAG, and the reason
   embeddings are explicitly *not* CRDT truth.

5. **Enrichment runs on a coordinator** (ADR-033 lease): KB-wide enrichment/embedding is executed
   by a single coordinator (owner default / elected) rather than every daemon, and its results are
   shared (relationships as CRDT content per ADR-030; vectors via ADR-034). This is what prevents
   N peers from re-spending compute/AI tokens.

6. **Hygiene/health** scans run against the cozo projection (already on the daemon scheduler);
   suggestions that are worth sharing are written into the source text as authored content
   (ADR-030) and thus sync; ephemeral suggestions stay local + re-runnable.

## Consequences

**Positive.** Query/RAG performance unchanged (cozo + HNSW). No wasted recompute (content-hash
cache). Model pinning makes derived artifacts interchangeable, enabling ADR-034 sharing.
Rebuildable (self-heal) without recomputing unchanged embeddings.

**Costs.** A content-hash/model/chunk-keyed embedding cache + invalidation discipline. A per-KB
model-pinning setting + the migration story when the pinned model changes (a deliberate,
coordinator-driven re-embed). Hygiene results that are baked into text become authored history.

## Alternatives rejected

- **Embeddings as CRDT truth** (sync the vectors as canonical state): bloats the CRDT with derived
  data, couples it to a model, and is unverifiable from content. Rejected — vectors are derived;
  sharing is an optimization (ADR-034), not truth.
- **Content-only embedding cache key:** silently serves stale vectors after a model/chunking
  upgrade (a known footgun). Rejected.

## Verification

Edit a node → only its embeddings recompute (others cache-hit). Change the pinned model → a
coordinator re-embed is triggered; results converge across peers. Delete cozo → structural
projection + FTS rebuild deterministically; embeddings repopulate from cache where content
unchanged. RAG (vector + graph) returns equivalent results on two peers with the same pinned model.
