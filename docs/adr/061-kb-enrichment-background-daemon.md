# ADR-061: KB enrichment as a background daemon responsibility

**Status:** Accepted — all six phases (A–F) implemented. See this doc's own per-phase
"Implementation note (principle #15)" sections for what shipped vs. each phase's original
Decision text, including named scope limits and corrections.
**Extends:** ADR-031, ADR-033, ADR-034, ADR-057.
**Depends on:** ADR-045, ADR-060.

## Context

ADR-057 row 4 names the gap this ADR closes precisely: "KB enrichment (AI-driven derivation of
new KB content, not just storage)" is part of the vision's 5-layer model — `mae-daemon` is
described there as "a genuine server ... handling KB maintenance, enrichment, and optimization"
— but today's reality, confirmed with fresh research rather than carried forward from an earlier
draft, is that **zero AI-driven enrichment exists anywhere in the codebase**. `store_embedding`
(`shared/kb/src/cozo_store/vector.rs:10`) is fully implemented — it writes a per-node+model
vector into the `embeddings` relation and its HNSW index — but a repository-wide search for its
callers turns up nothing outside test modules (`kb_graph_validation_embeddings.rs`,
`cozo_store/tests/vector_tests.rs`, and the trait-forwarding shim in `kb_store_impl.rs`). The
write path exists; nothing calls it in production. The read side is symmetric evidence of the
same gap: `kb_vector_search`'s executor (`crates/ai/src/tool_impls/kb.rs:1776`) already has a
dedicated regression test asserting it "fails gracefully and points to alternatives" — because
today the HNSW index it queries is permanently empty, and the tool's only honest behavior is to
say so and redirect the caller to full-text search. The vector-search infrastructure is wired
end-to-end for reads; it has simply never been fed.

This is not a new problem statement invented for this ADR. ADR-031 §5 (decision item 5,
"Enrichment is a local projection; multi-peer enrichment is coordinated") already named the exact
shape of the eventual mechanism — a single-user, daemon-less editor computes its own
vectors/enrichment in-process, while multi-peer enrichment is a **deduplication** optimization
coordinated by a lease — and explicitly flagged the reason it wasn't built yet: "the prerequisite
for *any* of this is an embedding provider, which MAE does not yet ship... so enrichment is future
work regardless of daemon mode." That prerequisite is what changes with ADR-045 (AI provider
parity & local-model harness), which formalizes `crates/ai/src/`'s already-provider-agnostic
`AgentProvider` trait across Claude/OpenAI/Gemini/DeepSeek/Ollama and brings the Ollama path to
genuine parity rather than a narrow bugfix. Once that abstraction is solid, the blocking
prerequisite ADR-031 named is resolved, and KB enrichment moves from "future work, blocked" to
"future work, ready to phase in" — which is what this ADR does.

The coordination mechanism this ADR needs was also already designed, and designed with
enrichment specifically in mind. ADR-033 (coordinating KB-wide operations — advisory lease +
epoch fencing) states in its own metadata block, verbatim: "**Feeds:** ADR-034 (the coordinator
that the lease elects performs the compute-once enrichment)." That line has sat unactioned since
ADR-033 was accepted — the lease/fence primitive it designed for "KB-wide AI enrichment,
rebuilding embeddings" as a named example was built and tested against no real enrichment
workload, because no enrichment workload existed. ADR-034 (cross-peer sharing of derived
intelligence) is the companion half: it already specifies the exact cache key
(`content_hash, embedding_model_id, chunk_version`) and the trust model (membership-gated,
opt-in verify) for sharing the *results* of that compute-once run across peers on the same KB.
Both ADRs are "Accepted (design); implemented" for their general mechanism, but neither has ever
been exercised end-to-end by a real caller, because the one caller they were designed for —
enrichment — was never built. This ADR is that caller.

Two more pieces of standing design bound this ADR's scope. First, ADR-048 (AI residency policy
for sensitive KBs) already gates every `kb_*`/`help_open` tool that could expose content to a
non-local provider through a single enforcement point (`check_kb_residency`,
`crates/mae/src/ai_residency.rs`) keyed on `editor.ai.provider`/`primary_ai_residency`; a KB
flagged `LocalModelsOnly` must never have its content routed to a hosted embedding API, and this
ADR's Ollama-first embedding path exists specifically so that guarantee holds for enrichment too,
not only for chat/completion calls. Second, ADR-057 row 5 documents that the daemon's own
scheduler is only two-thirds wired: `maintenance_tick` and `watcher_tick`
(`daemon/src/scheduler.rs:60-70`) are literal `// TODO` stubs that increment a counter and do
nothing else, while `health_tick` (`daemon/src/scheduler.rs:72-108`) is genuinely wired to the
hygiene scan via `tokio::task::spawn_blocking`, kept off the async executor per ADR-054's
concurrency-hardening rationale. ADR-065 item 2 is the tracked owner of unstubbing
`maintenance_tick` in general (integrity check, statistics, compaction); this ADR claims only the
AI-enrichment half of that same tick's eventual responsibility, so the two pieces of work must be
sequenced against the same function without either silently reimplementing the other's half.

Finally, Phase D of this ADR (lease-coordinated dedup across daemon peers sharing a KB) is more
than a design nicety once more than one daemon process can legitimately be running against the
same KB at once — which is exactly the condition ADR-060's genuine multi-tenant daemon work
establishes as a first-class, supported topology rather than an edge case. This ADR's Phase D is
written against that eventual topology and depends on it landing to be meaningfully tested at
its intended N-way scale (see Verification, item D), though the lease/fence mechanism itself
(ADR-033) already works correctly today for the smaller number of independent daemon processes a
user can already run by hand.

## Decision

Build KB enrichment in six phases, each reusing an already-designed mechanism rather than
inventing a parallel one, per CLAUDE.md principle #8.

**A — a pluggable embedding provider, reusing the existing chat/completion provider
abstraction.** Embedding generation is added as a capability on the same `AgentProvider` trait
family `crates/ai/src/` already uses for Claude/OpenAI/Gemini/DeepSeek/Ollama (ADR-045), not a
new, separately-configured provider interface. Ollama is a genuine day-one option, not a
follow-on: KBs flagged `LocalModelsOnly` under ADR-048 must be able to enrich their own content
without a single byte of it ever reaching a hosted provider, and that is a hard requirement, not
a nice-to-have — ADR-048 exists specifically to guarantee sensitive KBs never leave the local
machine, and an enrichment feature that silently routed content to a hosted embedding API on a
residency-restricted KB would violate that guarantee exactly as seriously as a chat call would.
The residency check for enrichment reuses `check_kb_residency` — the same single enforcement
point ADR-048 already gates chat/completion calls at — rather than adding a second, potentially
inconsistent check.

**B — a content-addressed cache/queue keyed on `(content_hash, model_id, chunk_version)`.** This
matches ADR-031's own original spec exactly (decision item 2: "the cache key is `(content_hash,
embedding_model_id, chunk_version)` — not content alone, because a model or chunking change must
invalidate"). The cache is persisted to disk across daemon restarts, not held only in memory: an
in-memory-only cache would silently re-embed every node on every daemon restart, discarding real
compute and, for hosted models, real paid API spend, on every routine restart — a cost regression
this ADR must not introduce even accidentally.

**C — scheduler wiring off the existing `maintenance_tick`.** Enrichment is dispatched from
`daemon/src/scheduler.rs`'s `maintenance_tick`, coordinated explicitly with ADR-065 item 2 so the
two pieces of work do not collide implementing the same function: this ADR claims the AI-driven
enrichment half of `maintenance_tick`'s eventual responsibility, ADR-065 claims the deterministic
integrity-check/compaction half. The enrichment sweep uses `spawn_blocking`, matching the
already-proven pattern the sibling `health_tick` hygiene scan already uses
(`daemon/src/scheduler.rs:72-108`) to keep a synchronous CozoDB scan (and, here, a synchronous or
long-polling embedding-provider call) off the async executor per ADR-054 — this ADR does not
invent a new async-dispatch pattern for the tick two lines away from one that already works.

**D — lease-coordinated dedup across multiple daemon peers sharing a KB.** When more than one
daemon is capable of enriching the same KB (the multi-tenant topology ADR-060 makes routine),
enrichment claims the ADR-033 advisory lease using exactly its "bulk sweep pattern" (§3): one
writer runs the sweep under the lease, applies the whole batch as a single atomic CRDT
transaction so no peer ever observes half-applied enrichment state, and the ADR-023 epoch fence
(the correctness primitive ADR-033 reuses, not a new one) rejects any late write from a holder
that has since been superseded. This ADR adds zero new coordination primitives — it is purely a
new *consumer* of the existing lease/fence pair, which is principle #8 in its most literal form:
ADR-033 was designed with this exact caller in mind (its own "Feeds: ADR-034 ... compute-once
enrichment" line), and this phase is that caller finally showing up. Once a peer's sweep
completes, ADR-034's compute-once sharing takes over: the AI-generated relationships/metadata are
baked into node text as ordinary CRDT content (free sync, no peer re-runs the enrichment), and
the resulting vectors are shared peer-to-peer via the content-addressed artifact cache keyed
identically to phase B's local cache, gated on KB membership per ADR-034's trust model.

**E — a `daemon_mode=off` contract: enrich-now, never automatic.** Per CLAUDE.md principle #12,
the in-process embedded KB is the floor, not a fallback, and enrichment must not silently become
a feature class that only exists for users who run a daemon. When `daemon_mode=off` (ADR-035),
there is no scheduler process to run a background sweep, so enrichment is exposed instead as an
explicit, low-priority, user-invoked "enrich now" command/tool that runs the same embedding
provider and cache logic in-process, synchronously with respect to the invoking command but never
automatically and never on the interactive editing hot path. This is what keeps the no-daemon
floor genuinely fully usable rather than silently missing a whole feature class for users who have
deliberately chosen not to run a daemon — exactly the guarantee principle #12 requires.

**F — wire `kb_vector_search` to the now-populated cache and blend it into
`kb_federated_search_scoped`.** `kb_vector_search`'s executor
(`crates/ai/src/tool_impls/kb.rs:1776`) today queries an always-empty HNSW index and degrades
gracefully by design (its own regression test, `kb_vector_search_fails_gracefully_and_points_to_
alternatives`, exists precisely because there was previously nothing to query). Once phases A–D
populate the index, this phase removes the "always empty" condition and additionally blends
vector-similarity hits into `kb_federated_search_scoped`
(`crates/core/src/editor/kb_ops/search.rs:278`) alongside its existing full-text-search results,
rather than shipping vector search as a second, disconnected search mode a caller has to remember
to invoke separately. This mirrors ADR-057's "one search experience" framing: a caller asking
`kb_search`/`kb_search_context` a question should get the benefit of both signal types without
needing to know the KB has an embeddings cache at all.

## Consequences

**Positive.** MAE ships a real answer to the one KB-substrate capability ADR-057's evidence table
flags as entirely unbuilt (row 4), closing the gap between the stated vision and the shipped
product. The implementation reuses four already-designed, already-partially-implemented
mechanisms end-to-end for the first time — ADR-031's local-projection/cache-key design, ADR-033's
lease/fence coordinator, ADR-034's compute-once sharing protocol, and ADR-045's provider
abstraction — rather than adding a fifth, parallel one, which is the clearest possible validation
that those four ADRs' designs were sound: this ADR is the first real exercise of ADR-033/034
against a genuine workload rather than a synthetic placeholder. `kb_vector_search` and the vector
half of RAG-style `kb_search_context` queries go from permanently-empty to genuinely useful with
no new tool surface a caller has to learn. Sensitive KBs get enrichment without compromising
ADR-048's residency guarantee, and users who never run a daemon do not lose the feature class
entirely.

**Costs (honest).** A persistent, content-addressed cache is new on-disk state the daemon (and,
for `daemon_mode=off`, the editor process) must manage, migrate, and account for in size/storage
docs. Embedding generation is the first daemon workload that spends real external API cost/time
on a background tick, which means `maintenance_interval_secs` tuning now has a cost dimension it
did not have before (a `health_tick`-style hygiene scan is free; an enrichment sweep against a
hosted provider is not) — operators running against hosted models need visibility into that cost,
which this ADR's scope does not itself design a dashboard for but must not make impossible to add
later. Phase D's lease-coordinated dedup is only exercised at its intended multi-daemon scale once
ADR-060 lands; until then it is correct but under-loaded, a known and named limitation rather than
a silent gap. The `daemon_mode=off` enrich-now path duplicates none of the daemon's coordination
logic (there is nothing to coordinate with a single process) but does mean the in-process editor
briefly does synchronous provider I/O when a user explicitly invokes it — an accepted, opt-in
cost, not a hot-path regression, since principle #12's "editor must not depend on the daemon" is
exactly what E is designed to preserve.

## Alternatives rejected

- **Synchronous embedding generation on every `kb_create`/`kb_update` call inside the interactive
  editor process.** Rejected — this would block the interactive editor on a slow external API
  call on every single edit, directly contradicting CLAUDE.md principle #12, which requires the
  daemon (and any background-compute feature) to be an optimization the editor can live without,
  never a requirement sitting on the interactive editing hot path. A user typing into a buffer
  must never wait on a network round-trip to a hosted embedding API before their keystroke lands.
- **Building a new coordination primitive specific to enrichment instead of reusing ADR-033/034.**
  Rejected — ADR-033/034 already solved the general "N daemons, one job, don't duplicate work"
  problem, including naming enrichment as their own intended first consumer. Reinventing that
  coordination logic here would be exactly the duplicated-logic anti-pattern CLAUDE.md principle
  #8 exists to prevent, and would leave two independently-maintained lease/fence implementations
  to keep in sync for no benefit.
- **Treating vector search as a permanently separate tool/mode from `kb_federated_search_scoped`
  and `kb_search_context`.** Rejected — this would ship enrichment's benefit behind a second
  surface a caller has to know to reach for, contradicting ADR-057's "one search experience"
  framing of the vision and adding an unnecessary decision point ("did I mean FTS or vector
  search?") to every RAG-style query.

## Verification

Per CLAUDE.md principle #14, verification is adversarial and phased against each Decision item,
not a single happy-path pass, and — for Phase D specifically — exercised **N-way (≥3 daemons)**,
not the 2-way case that can hide a coordination bug a 3-way race exposes.

- **A.** A provider failure (timeout, malformed response, provider unavailable) must degrade the
  affected content to a "not yet enriched" state that remains fully FTS-searchable via the
  existing text-search path — never corrupt the enrichment cache, and never block unrelated KB
  operations on the same or a different KB. A hosted-provider configuration pointed at a
  `LocalModelsOnly`-residency KB must be rejected at the exact same enforcement point ADR-048
  already gates chat/completion calls at (`check_kb_residency`) — verified by a real call that
  is denied, not documented intent, and specifically *not* a second, independently-implemented
  check that could drift out of sync with the first.
- **B.** Re-embedding identical content must be a guaranteed cache hit **across a daemon
  restart** — this specifically tests on-disk persistence, not just in-memory memoization within
  one process lifetime; a naive in-memory-only cache would pass a same-process test but fail this
  one, and the test must be structured so it can only pass with genuine persistence. A `model_id`
  or `chunk_version` bump must force re-embedding of exactly the affected cache entries — verified
  by asserting entries under the old key remain untouched and only entries under the new key are
  recomputed, not the whole cache and not zero entries.
- **C.** Kill the daemon process mid-sweep and restart it; resumption must not double-process
  nodes that were already completed before the kill (verified by asserting no duplicate
  provider calls for already-cached content-hashes), and must not silently lose nodes that were
  still pending (verified by asserting every node in the original sweep's scope is eventually
  enriched after resumption, not just the ones completed before the kill).
- **D.** **At least three daemons** racing to claim the enrichment lease simultaneously on the
  same KB — exactly one must actually perform the sweep, and the other two must back off cleanly
  with no duplicate provider calls and no duplicate cache writes. Kill the lease holder mid-sweep:
  TTL expiry must let another daemon resume the work, and epoch-fencing must reject a late write
  arriving from the crashed original holder after it has already been superseded by the new
  holder — this is the specific "paused/slow lease holder comes back and tries to write stale
  data" attack ADR-033 already names as a threat in its own Consequences section ("a paused/slow
  lease holder's late bulk write... is rejected"); this test exercises it against a real
  enrichment payload (actual node content, actual generated vectors), not a synthetic placeholder
  sweep that could pass without ever exercising the fence's real write path.
- **E.** Verify **zero background timer or thread exists at all** when `daemon_mode=off` — not
  merely that the enrich-now path works when invoked, but that nothing runs automatically in the
  absence of a daemon (verified by asserting no scheduled task, timer, or background thread is
  spawned anywhere in the `daemon_mode=off` boot path, so a user who never touches the enrich-now
  command incurs zero background cost, not just zero *automatic* enrichment cost).
- **F.** `kb_vector_search` against a populated cache must return real, ranked hits (not the
  graceful-degradation message its current test asserts) once content has been enriched, and its
  existing degrade-gracefully behavior must still hold for content that has not yet been enriched
  — both paths verified, not just the newly-working one. `kb_federated_search_scoped` must show a
  measurable blend of FTS and vector-similarity results for a query where the two signal types
  would otherwise disagree (a query whose best FTS match and best vector match are different
  nodes), proving the blend is real composition and not one signal type silently shadowing the
  other.

## Implementation note (Phase A, principle #15)

`AgentProvider` (`crates/ai/src/provider.rs`) gained `async fn embed(&self, model: &str, inputs:
&[String]) -> Result<Vec<Vec<f32>>, ProviderError>` with a default implementation returning a
new `ErrorKind::Unsupported` (never retryable — distinct from a transient failure a caller might
back off and retry). `model` is a separate parameter, not `self.config.model`: an embedding model
(e.g. `nomic-embed-text`) is a different model from whatever chat model a provider is configured
for, and Phase B's cache key is literally `(content_hash, model_id, chunk_version)`, so the model
must be nameable per-call rather than implied by unrelated provider state. `OllamaProvider`
implements it for real against Ollama's current `/api/embed` endpoint (not the legacy singular
`/api/embeddings`) — verified against Ollama's own API docs before wiring, since a wrong
request/response shape would silently corrupt every cached vector downstream. Response parsing
is extracted into a pure `parse_embed_response` function (mirroring the existing `parse_response`
pattern), unit-tested directly since this crate has no HTTP-mocking test harness for a full
round-trip test.

**A real architectural gap found and fixed while wiring the residency check** (Verification
item A's "hosted-provider configuration pointed at a `LocalModelsOnly` KB must be rejected"):
`check_kb_residency`'s existing `is_local_provider`/`LOCAL_AI_PROVIDERS` lived in `crates/core`
— but ADR-061's real caller (Phase C's scheduler-driven sweep) runs inside `mae-daemon`, a
separate binary that does not depend on `mae-core` at all (ADR-014's two-workspace split), and
has no `Editor` to route through. `is_local_provider`/`LOCAL_AI_PROVIDERS` relocated to
`mae_kb::federation` (`shared/kb`, which both workspaces already depend on directly, and which
already owns the `AiResidency` enum itself) with a re-export from `crates/core/src/ai_residency.rs`
so no existing caller broke. A new pure `residency_permits_provider(residency: AiResidency,
provider: &str) -> bool` sits next to it — the daemon can call this directly against a
`KbInstance.ai_residency` value with no `Editor` in the loop, while the editor's own
`check_kb_residency` gate can (in a later phase) delegate its leaf provider-vs-residency decision
to the same function instead of duplicating the comparison.

**Honest scope note**: Phase A's own Verification item A also specifies "a provider failure...
must degrade... never corrupt the enrichment cache" — not fully testable at this phase alone,
since there is no cache yet (Phase B). What Phase A does verify: `embed()`'s default trait
implementation is classified `Unsupported`/non-retryable (not a generic error a caller might
mistakenly retry), and `OllamaProvider::parse_embed_response` correctly handles a missing
`embeddings` field, a non-numeric vector component, and preserves batch ordering (a real,
distinct-vectors test, not a single hand-picked value repeated, so a transposition bug would
actually be caught). The cache-corruption-immunity property is Phase B's own verification
obligation once the cache exists to corrupt.

`cargo test`/`cargo clippy --all-targets -- -D warnings`/`cargo fmt --check` clean across both
the editor and daemon workspaces (confirming the `shared/kb` relocation doesn't regress either).

## Implementation note (Phase B, principle #15)

A new `embedding_cache` CozoDB relation (`shared/kb/src/cozo_store/schema.rs`), keyed exactly on
`(content_hash, model, chunk_version)` per this section's own spec. **Deliberately a separate
relation from `embeddings`, not an extra key column on it** — a real constraint discovered while
designing this phase, not assumed: `embeddings` (the relation `vector_search`/`graphrag_search`
actually query) is HNSW-indexed with a **fixed `<F32; 384>` vector width** (all-MiniLM-L6-v2's
dimension), confirmed by direct read of its `::hnsw create` DDL. Phase A's `embed()` is fully
provider/model-agnostic and could return a vector of *any* dimension (Ollama alone ships models
at 384/768/1024+ dims) — storing a non-384-dim vector in `embeddings` would be a type mismatch.
This cache never needs similarity search (only exact-key lookup), so its `vec` column is a plain
variable-length `[Float]` list, not the fixed-width HNSW type — meaning the cache itself is NOT
locked to any one dimension. **The 384-dim limitation on the *searchable* `embeddings` relation is
real and not fixed by this phase** — it's Phase F's concern (the phase that actually wires
`kb_vector_search`/`kb_federated_search_scoped` to read from `embeddings`), named here so it isn't
silently discovered again later.

`content_hash` reuses `mae_kb::activity::body_hash` (FNV-1a-64 over property-drawer-stripped body
text) — the existing per-node change-detection hash already used by `crates/core/src/editor/
kb_ops/activity.rs` — rather than inventing a second, parallel content-hashing scheme (principle
#8). Two new `CozoKbStore` methods, `get_cached_embedding`/`put_cached_embedding`, are the only
new API surface; persistence-across-restart is free (same on-disk CozoDB store every other KB
relation already lives in, no separate cache file/mechanism).

Three tests added to `shared/kb/src/cozo_store/tests/vector_tests.rs`, matching this section's
own Verification bullets exactly: `cached_embedding_survives_a_real_daemon_restart` (the store is
FULLY dropped and reopened at the same on-disk path, not just checked in-process — the specific
thing a naive in-memory-only cache would fail), `model_or_chunk_version_bump_invalidates_only_
the_affected_entries` (asserts a miss under the new key, an UNCHANGED hit under the old key, and
both keys correctly coexisting once the new one is populated — not "the whole cache invalidates"
and not "nothing invalidates"), and a plain clean-miss baseline.

**Scope note**: this phase builds the storage primitive only. Computing a node's `content_hash`
and actually calling `get_cached_embedding`/`put_cached_embedding` around a real `embed()` call is
Phase C's scheduler-wiring job, not this phase's — matching the ADR's own phase boundaries.

`cargo test -p mae-kb --lib`/`cargo clippy --all-targets -- -D warnings`/`cargo fmt --check`
clean across the editor and daemon workspaces; `cargo build --workspace --features gui` clean.
(Two pre-existing, unrelated test failures --
`migrate::tests::sled_to_sqlite_{is_idempotent_and_noop_when_not_sled,preserves_nodes_links_and_
backs_up}` -- confirmed via `git stash` to fail identically on a clean checkout, an environmental
gap where this local build's editor-workspace `mae-kb` doesn't have the sqlite cozo engine
compiled in; not a regression from this phase.)

## Implementation note (Phase C, principle #15)

**A structural constraint found during implementation, not anticipated by the Decision text**:
the daemon workspace has ZERO dependency on `crates/ai` (or `mae-core`) — confirmed by grep,
zero hits for `AgentProvider`/`OllamaProvider`/`embed(` anywhere in `daemon/src/`. This is by
design (ADR-014's editor/daemon workspace split exists specifically so the daemon doesn't pull in
`mae-core`'s large, editor-shaped dependency graph), but it means Phase C cannot simply call
`crates/ai::OllamaProvider::embed()` the way Phase A's own chat-adjacent embed path does. Resolved
the same way Phase A's own implementation note resolved an identical wall for
`is_local_provider`/`residency_permits_provider`: split the Ollama `/api/embed` call into a pure,
dependency-light half (`mae_kb::embedding_client::{build_ollama_embed_request,
parse_ollama_embed_response}` — request/response JSON shaping, zero I/O, no `reqwest` dependency
at all) shared by every caller, plus a per-caller HTTP-transport half using whichever client that
caller already owns: `crates/ai::OllamaProvider::embed()` now delegates its request-building and
response-parsing to the shared functions (removing ~50 lines of duplicated logic, principle #8)
while keeping its own async `reqwest::Client`; the daemon's new `OllamaEmbedBackend`
(`daemon/src/enrichment.rs`) builds its OWN async `reqwest::Client` (the daemon already depends on
`reqwest` directly for its OAuth listener) around the same shared shaping functions.

The store-facing sweep logic (list nodes → hash → check cache → collect misses; write results back
to the cache) lives in `shared/kb/src/enrichment.rs` as two plain, synchronous functions —
`plan_enrichment_scan`/`apply_enrichment_results` — deliberately NOT taking an injected embed
callback/trait: this keeps `mae-kb` free of any async-runtime or `async-trait` dependency (a real
risk otherwise, since some `mae-kb` callers, e.g. `build_manual_kb`/`build_practices_kb`, are plain
synchronous binaries with no tokio runtime running at all). The actual embed step is entirely
owned by the caller between the two blocking calls. `plan_enrichment_scan` takes `&dyn KbStore`
(not the concrete `CozoKbStore`) so it works against the editor's own `KbState::store: Option<Arc<
dyn KbStore>>` handle (needed for Phase E below) without a downcast — this required promoting
`get_cached_embedding`/`put_cached_embedding` from `CozoKbStore` inherent methods to `KbStore`
trait methods (with a `NotSupported` default, mirroring the existing `store_embedding`/
`vector_search` precedent on the same trait) and adding the `CozoKbStore` override.

`daemon/src/enrichment.rs`'s `run_enrichment_sweep` orchestrates one full sweep of one store:
`plan_enrichment_scan` runs inside `tokio::task::spawn_blocking` (ADR-054 — never a synchronous
CozoDB scan inline on the async executor, matching the sibling `health_tick`/`run_maintenance_scan`
pattern exactly), the batched `/api/embed` calls run directly on the async executor between the two
blocking passes (genuinely async, no `Handle::block_on` bridging needed since nothing here runs
inside a `spawn_blocking` closure itself), then `apply_enrichment_results` runs in a second
`spawn_blocking`. Wired into `daemon/src/scheduler.rs`'s `run_maintenance_tick` as a SEPARATE pass
after the existing deterministic maintenance loop (not folded into the same loop), gated by a new
`[enrichment]` section in `daemon/src/config.rs`'s `DaemonConfig` (`enabled: bool`, default
`false` — an operator must opt in explicitly, since this is real external API cost/time on a
background tick, exactly as this ADR's own Costs section already named). Residency is resolved
per-store from the SAME `KbRegistry` the daemon's other access checks already read
(`primary_ai_residency` for the primary store, `KbInstance.ai_residency` for named instances) and
checked BEFORE any node's content is read (`plan_enrichment_scan`'s own structural guarantee) —
never a second, independently-implemented residency check.

**Verification C, addressed precisely**: "kill mid-sweep and restart; resumption must not
double-process nodes already completed, must not lose nodes still pending" is satisfied FOR FREE
by Phase B's own content-addressed cache — a killed/restarted sweep's next `plan_enrichment_scan`
call naturally sees already-cached content-hashes as hits (skipped) and not-yet-cached ones as
targets (retried), with no separate checkpoint/resume bookkeeping needed. Two adversarial test
suites prove this at two layers: `shared/kb/src/enrichment.rs`'s
`resuming_after_a_partial_prior_run_only_targets_the_still_unembedded_nodes` (pure plan/apply
layer, a pre-populated single cache entry simulating a kill-after-one-node) and `daemon/src/
enrichment.rs`'s `a_batch_failure_does_not_lose_or_duplicate_other_batches_work` (the full
orchestration layer, using an injectable `EmbedBackend` trait with a fake backend that fails for
one specific batch — proving the SAME resumption property holds when the interruption is a
mid-sweep provider failure, not just a killed process, and that OTHER batches' work is neither
lost nor duplicated). Two more daemon-level tests (`maintenance_tick_does_not_touch_the_cache_
when_enrichment_is_disabled`, `maintenance_tick_skips_enrichment_for_a_local_models_only_primary_
kb_against_a_hosted_provider`) exercise the actual `run_maintenance_tick` wiring, not just the
lower-level sweep function in isolation — the second one points `base_url` at an address nothing
listens on and asserts the tick completes without ever attempting to reach it, proving the
residency gate runs before any network call, not merely before any *successful* one.

**Explicitly out of scope for this phase, named rather than silently absent**: Phase D's
lease-coordinated multi-daemon dedup. With a single daemon per KB (today's only real topology —
ADR-060's multi-tenant work shares one daemon process across tenants, not multiple daemons racing
on the same KB), there is no concurrent-claim race to coordinate, only the restart-resume case
Phase B's cache already handles.

`cargo test`/`cargo clippy --all-targets -- -D warnings`/`cargo fmt --check` clean across both
workspaces; `cargo build --workspace --features gui` clean.

## Implementation note (Phase D1, principle #15)

**Split into D1 (the lease primitive itself) / D2 (enrichment as its first caller) / D3 (ADR-034
sharing) across separate PRs, landed in that order** — this note covers D1 only. D2/D3 follow as
their own PRs once D1 is proven stable, matching the phased-landing discipline already established
for A/B/C/E above.

**A real, adversarially-discovered CRDT-safety bug, not anticipated by the Decision text**: the
first data-model attempt nested the lease claims one level (`YMap<op_kind -> YMap<claim_key ->
record>>`), mirroring the signed-oplog's own append-only-set pattern. A round-trip test simulating
two never-synced daemons concurrently claiming the SAME not-yet-existing `op_kind` submap failed —
after merging, one peer's entire claim was silently dropped, not unioned with the other's. Root
cause: yrs resolves two independently-created `MapPrelim::default()` values assigned to the SAME
not-yet-existing key via last-writer-wins on the OUTER key itself, not a merge of the nested
maps' contents — a real, general CRDT gotcha (concurrently creating a container for the first
time is unsafe; concurrently inserting a new key into an ALREADY-established shared map is the
safe, ordinary case). Fixed by flattening to a single `YMap<claim_key -> record>` (op_kind stored
as a field per entry) and eagerly seeding the outer map in every `KbCollectionDoc` constructor —
exactly the same pattern `member_roles`/`pending`/`nodes` already use, confirmed by direct read
before assuming it was safe. The round-trip test (`two_concurrent_daemon_claims_converge_to_the_
same_deterministic_winner`, `shared/sync/src/kb/tests/collection_lease_tests.rs`) now passes and
stays in the suite as a standing regression guard against reintroducing the nested form.

**ADR-033's own text needed two corrections during design, folded in rather than implemented
literally and wrong**: (1) "reuse the ADR-023 epoch as the fencing token" cannot mean the literal
per-member authorization epoch — bumping it on a lease grant would fence the loser's unrelated
ordinary edits too, collateral damage ADR-033's own Consequences section never describes. Built
instead as a narrow, separate `(kb_id, op_kind)` generation counter — the *pattern* ADR-033
describes ("a KB-wide operation carries an epoch"), at a dimension that doesn't collide with
per-member authorization. (2) "broadcast on the ADR-024 attention bus" doesn't correspond to any
real inter-daemon channel — `NotificationCenter` is single-editor-process UI presentation, and the
real daemon-mesh gossip transport (#89) is still the tracked bottleneck. Used ADR-033's own named
fallback instead: an in-band LWW claim in the collection doc, already fully shipped end-to-end via
the existing `persist_and_broadcast_collection` relay — zero new transport. `NotificationCenter`
(`notify_enrichment_lease_status`, `crates/core/src/editor/notify_ops.rs`) is the LOCAL
presentation of that already-synced state, not the transport itself.

`kb/claim_lease` (`daemon/src/collab_handler/kb_lease.rs`) is gated `KbOp::Edit`, not `KbOp::Manage`
— any Editor-role member may claim it, unlike `kb/collection_op`'s owner-only gate, since
enrichment/embedding work is an ordinary editing capability, not KB governance.

**Honest scope note**: `enforce_lease_generation_fence` (the write-time re-check) ships in this
phase, fully unit-tested directly, but has no production caller yet — Phase D2 (issue #420's
second half) wires `run_enrichment_sweep`'s commit path to call it. Named explicitly (`#[allow
(dead_code)]` with a doc comment, not a silent gap) rather than deferred without a trace.

Verification item D's **N-way (≥3 daemon) race** requirement is met at the primitive level in this
phase (`collab_handler_lease_race_tests.rs`: 3 members claim from the same pre-claim state,
dispatched out of order, converge to exactly one deterministic winner regardless of order; a
non-member is denied outright). The "zero duplicate provider calls" behavioral half of item D
(losers making no calls into a fake `EmbedBackend`) requires D2's real caller to exist and is that
phase's own verification obligation, not testable against code that doesn't exist yet.

`cargo test -p mae-sync`/`cargo test --lib` (daemon workspace)/`cargo test -p mae-core notify_ops`
all clean; `cargo clippy --all-targets -- -D warnings`/`cargo fmt --check` clean across both the
editor and daemon workspaces; `cargo build --workspace` clean.

## Implementation note (Phase D2, principle #15)

**A real crate-boundary constraint found during implementation, not anticipated by the Decision
text**: `daemon/src/enrichment.rs` (where `run_enrichment_sweep` needs to check the lease before
committing) is BINARY-crate-only (declared in `main.rs`, not `lib.rs`), while
`collab_handler::kb_lease::DaemonLeaseFence` (the production fence implementation, since it needs
`load_collection`/`enforce_lease_generation_fence`, both `collab_handler`-internal) is
LIBRARY-crate code (`lib.rs`'s `pub mod collab_handler`) — and a library can never implement a
trait defined in its own downstream binary (wrong dependency direction). Resolved by putting the
`LeaseFence` trait itself in a new small, standalone library module,
`daemon/src/lease_fence.rs` (`pub mod lease_fence` in `lib.rs`) — neither `enrichment` nor
`collab_handler` "owns" it; both depend on it, `enrichment.rs` via `mae_daemon::lease_fence::
LeaseFence` (crate-external reference, matching `main.rs`'s own existing `use mae_daemon::{...}`
convention), `collab_handler::kb_lease` via `crate::lease_fence::LeaseFence` (same crate).

**Real architectural gap found while mapping a local store to its collab `kb_id`**: the
scheduler's `stores: Vec<(String, Arc<CozoKbStore>)>` loop had no existing forward mapping from a
local store entry to the `kb_id` string the collab layer's `kbc:{kb_id}` collection doc uses —
`instance_stores` is keyed by UUID (`daemon/src/handler.rs`), a different namespace. Resolved via
`KbRegistry`'s own `primary_shared`/`primary_collab_id` (primary) and `KbInstance.shared`/
`collab_id` (named instances, looked up by the SAME `registry.instances.iter().find(|i| &i.uuid ==
name)` pattern the residency lookup right next to it already uses) — no new mapping invented, and
a store with `collab_id: None` (the common, single-daemon case) is correctly skipped entirely: it
uses [`NoFence`](../../daemon/src/lease_fence.rs), nothing to coordinate with a single copy of the
data.

`claim_lease_for_scheduler` (`collab_handler/kb_lease.rs`) deliberately does **not** go through the
`kb_access` gate `handle_kb_claim_lease`'s external RPC path uses — the scheduler is the daemon's
own internal maintenance tick operating on data it already has full local access to via
`CozoKbStore` directly, not an authenticated network caller. Mirrors `kb_governance.rs`'s
`handle_kb_block_unblock_principal` precedent for a local, already-trusted-daemon operation.
`session_id: 0` is a safe sentinel for `persist_and_broadcast_collection`'s `broadcast_except`
(confirmed: real session ids are allocated from 1 upward, `mae_mcp::session::NEXT_SESSION_ID`).

**Scope limit named explicitly, not silently dropped**: no mid-sweep lease renewal is implemented
in this phase — a long sweep spanning multiple embed batches relies on `lease_ttl_secs` (new
`EnrichmentConfig` field, default 300s) being generous enough to cover it without renewal. The
fence is checked exactly once, immediately before `apply_enrichment_results` — this is where a
kill-holder-mid-sweep race actually needs to be caught (the embed loop's network calls are where
real wall-clock time elapses), not before every batch.

**Verification D's behavioral half** ("losers make zero calls into a fake `EmbedBackend`", deferred
from Phase D1's own note) is covered by `enrichment.rs`'s new
`a_fence_rejection_discards_the_batch_instead_of_committing` test — though scoped honestly:
the embed calls DO still happen (the fence is checked at commit time, not before embedding starts,
matching ADR-033's own "a late WRITE is rejected" framing, not "compute is prevented outright");
what's asserted is that the cache stays genuinely empty afterward (a real store read, not just the
returned error string). The kill-holder-mid-sweep race itself is exercised twice: the pure-logic
version (Phase D1's own test, unchanged) and a NEW test exercising the actual production
`claim_lease_for_scheduler` + `DaemonLeaseFence` pair through a real `DocStore`
(`collab_handler_lease_race_tests.rs`) — genuine TTL expiry through wall-clock time, using a
1-second TTL + a short real sleep (a `ttl_secs: 0` claim was tried first and found to be a
degenerate case: `is_expired`'s boundary `now >= claimed_at + ttl` collapses to an empty
valid-interval at `ttl=0`, so even the claim's OWN immediate read-back reports "no current holder" —
not a bug, but it meant that shortcut couldn't stand in for a real TTL-expiry exercise).

`cargo test` (both `--lib` and the binary target — `enrichment`/`scheduler` are binary-crate-only,
so `--lib` alone does not exercise them) clean across the daemon workspace (165 lib + 131 binary
unit tests, all e2e suites); `cargo clippy --all-targets -- -D warnings`/`cargo fmt --check` clean;
`cargo build --workspace --features gui` clean on the editor workspace (unaffected, but confirmed
since `mae-sync`/`mae-kb` are shared crates).

## Implementation note (Phase D3, principle #15)

**ADR-034's own "implemented in Phase G" status line was stale/wrong, corrected in this PR**
(`docs/adr/034-cross-peer-derived-artifact-sharing.md`) — Phase G belongs to ADR-060's own phase
lettering, not ADR-061's, and zero sharing-protocol code existed anywhere before this PR (confirmed
by grep for `share_derived_artifacts`/an advertise-request-serve protocol/the
`(content_hash, embedding_model_id, chunk_version)` sharing key). Same class of correction as
ADR-033's own status line, fixed in Phase D1.

**A second real crate-boundary constraint, same shape as Phase D2's `LeaseFence`**: `kb/
fetch_artifact` needs to read the local KB content store's embedding cache
(`CozoKbStore::get_cached_embedding`), reached via `DaemonState`/`resolve_kb_store` — both
BINARY-crate-only (`main.rs`'s `mod handler;` is private, not declared in `lib.rs` at all), while
the RPC dispatch it must serve from (`collab_handler::handle_doc_request_inner`) is LIBRARY-crate
code. Resolved identically to `LeaseFence`: a new small library module,
`daemon/src/artifact_store.rs` (`ArtifactStore` trait + a `NoArtifactStore` default for a KB with
no local replica), with the real implementation (`handler::DaemonArtifactStore`) living in the
binary crate.

**Threading `Arc<dyn ArtifactStore>` through the collab dispatch chain, confirmed low-risk before
attempting it** (not assumed): a dedicated research pass found exactly ONE real dispatch call site
(`run_session`), reached via exactly two call chains (hub TCP, P2P mesh) — both already having
`Arc<Mutex<DaemonState>>` in scope at or one hop from the call site — and direct precedent for
adding a required parameter to this exact function twice before (the `transport`/`auth_pubkey`
additions), each a similarly-shaped, contained diff. Threaded through
`handle_client_with_auth`/`handle_client`/`handle_client_authenticated`/`run_session`/
`handle_doc_request_inner` (append-last, matching the established parameter-ordering precedent) and
`p2p::serve`; ~10 call sites total across `main.rs`/`p2p.rs`/`dialer.rs`/the daemon's own
integration tests, all mechanical, all still green.

`kb/fetch_artifact` is gated `KbOp::Read` (any member, including Viewer) — reading a derived
artifact is no more sensitive than reading the content it was derived from. Also gated on the KB's
own `share_derived_artifacts` toggle (new per-KB `KbCollectionDoc` field, ADR-034's own "coordinator
opts in" design) — even a genuinely cached artifact is not served while sharing is disabled
(defaults to `false`/opt-in, matching this codebase's `TransportPolicy`/`Encryption` precedent for
every other new-capability toggle). "Coordinator" is deliberately not a separate stored field —
it's derived directly from the ADR-033 lease (`current_lease("enrichment", now).holder_fp`), reusing
Phase D1's election/tiebreak machinery rather than building a second one, per ADR-034's own text.

**Honest scope limits, named rather than silently dropped**:
- **Relationship/metadata baking (ADR-034 item 1) is NOT built in this phase.** No "derive
  relationships" AI logic exists anywhere yet (Phase C only computes vectors) — this ADR wires the
  *sharing mechanism* for embeddings, not a new relationship-derivation pipeline, which is out of
  ADR-061's scope entirely. Building an unreachable scaffold with no caller and no way to exercise
  its correctness beyond "does it compile" was judged worse than not building it (CLAUDE.md: no
  half-finished/speculative code) — this sub-part is deferred to whenever that derivation logic is
  designed, tracked as a real gap, not silently absent.
- **The model-pin-mismatch decision is the REQUESTER's, not built here.** ADR-034: "a peer fetching
  with a mismatched pin recomputes locally." This daemon simply reports what it has cached under the
  exact requested `(model, chunk_version)` key; there is no automatic requester/client call site in
  this phase at all (mirroring Phase D1's own precedent — the lease primitive shipped with no
  automatic caller until D2 wired one specific consumer). A future caller decides when to ask and
  what to do with a mismatch or a `has_artifact: false`.
- **No dedicated toggle command/tool for `share_derived_artifacts` in this phase** — the
  `KbCollectionDoc` field + accessor/setter exist and are tested directly; user-facing
  command/Scheme-API/MCP-tool wiring to flip it interactively is deferred, matching this PR's focus
  on the ADR-034 mechanism itself over its full product surface.

Adversarial coverage (CLAUDE.md principle #14): a non-member is denied at the exact same `kb_access`
gate every other read path uses (the attacker case ADR-034 names: "an artifact offered by a
non-member is ignored"); a member is served nothing while `share_derived_artifacts` is disabled,
even for a genuinely cached vector (not merely documented — asserted against a real cache hit that's
still refused); a genuine cache miss is reported distinctly from both the disabled-sharing and
denied cases (a caller must be able to tell "nothing cached yet" from "you're not allowed" from
"sharing is off").

`cargo test`/`cargo clippy --all-targets -- -D warnings`/`cargo fmt --check` clean across the
daemon workspace (169 lib + 131 binary unit tests, all e2e suites, no regressions from the dispatch
threading); `cargo test -p mae-sync` clean (306 tests, 7 new for the per-KB settings, including a
genuine two-peer concurrent-setting-change convergence test).

## Implementation note (Phase E, principle #15)

Re-read the Decision text carefully before implementing: it specifies the enrich-now path runs
"synchronously with respect to the invoking command" — a genuinely blocking call, not an
async-bridged one. This resolved what would otherwise have been a hard architectural question
(how does a plain synchronous AI-tool executor, `crates/ai/src/tool_impls/kb.rs`'s
`execute_kb_enrich`, reach an async embedding call without risking a "cannot start a runtime from
within a runtime" panic depending on the calling thread's own execution context): use a genuinely
BLOCKING HTTP client (`reqwest::blocking::Client`, via a new `mae_kb::embedding_client::
ollama_embed_blocking`, gated behind the existing `remote-hub` Cargo feature that already unlocks
`reqwest`'s optional `blocking` feature for ADR-062's `RemoteHubQueryLayer`) rather than bridging
into the async `OllamaProvider::embed` used for chat. `crates/ai/Cargo.toml` now enables `mae-kb`'s
`remote-hub` feature UNCONDITIONALLY (not behind `mae-ai`'s own opt-in flag) — `mae-ai` is already
an unconditional dependency of the shipped `mae` binary, so this guarantees the enrich-now path
compiles into every standard build without needing an extra opt-in flag most users would never
pass, matching principle #12's "genuinely usable floor" requirement.

Implemented as a new AI tool, `kb_enrich` (`crates/ai/src/tools/kb_tools.rs` + `crates/ai/src/
executor/kb_exec.rs` + `crates/ai/src/tool_impls/kb.rs::execute_kb_enrich`), not a plain
`crates/core` editor command — a deliberate scope decision, not an oversight: reaching an embedding
provider requires `crates/ai` (`mae-core` has no AI-provider access at all, by design, to avoid a
circular dependency), so the tool naturally lives where the provider access already exists. New
Scheme-configurable options (`ai_embedding_provider`/`ai_embedding_model`/`ai_embedding_base_url`/
`ai_embedding_api_key_command`/`ai_embedding_chunk_version`, mirroring the existing `ai_provider`/
`ai_model`/etc. naming) back new fields on `AiState`, following the exact same registry-def +
`get_option`/`set_option` match-arm pattern every other option already uses (principle #7 — no
config.toml-only or hardcoded settings).

**Scoped to the primary KB only, named rather than silently absent**: federated instances
(`editor.kb.instances`) are held as in-memory `KnowledgeBase` values in the editor process, not
`Arc<dyn KbStore>` handles — there is no store to enrich for them from this process today. Extending
enrich-now to federated instances is a real follow-up, not attempted here.

**A second, layered residency check, not a duplicate of the first**: `execute_kb_enrich`'s own
internal `plan_enrichment_scan` call already gates on the `ai_embedding_provider` config (which
model actually processes the content — the same axis Phase A/C gate on). Separately, `crates/mae/
src/ai_residency.rs`'s `check_kb_residency` (the REQUESTER-facing gate, run before any `kb_*` tool
call reaches its executor at all) needed a new `"kb_enrich" => PrimaryOnly` classification — a
DIFFERENT axis: whether the calling AI agent itself (`requester_provider`, e.g. a hosted Claude
session driving the tool call) may even invoke `kb_enrich` and see its output at all, since a
failed node's id can appear in the returned `errors` array (real node-identity leakage, not
content, but enough that "no per-row filter" `PrimaryOnly` applies, matching `kb_raw_query`'s own
precedent). Missing this classification was caught immediately by the existing `every_kb_tool_and_
help_open_is_explicitly_classified` regression guard (fails CLOSED for any unclassified `kb_*`
tool, per that module's own design) — confirming the guard does exactly the job it was built for.
Three new adversarial tests added there matching the existing `kb_raw_query`/`kb_view_query`
pattern: denied for a non-local requester against a restricted primary, allowed for a local
requester, allowed when the primary is unrestricted.

**Verification E, addressed precisely**: "verify zero background timer or thread exists at all
when `daemon_mode=off`" is satisfied structurally — `execute_kb_enrich` is a plain synchronous
function with no `std::thread::spawn`/`tokio::spawn`/interval anywhere in its call chain (the
BLOCKING HTTP client, not the daemon's `DaemonScheduler`, which only ever runs inside the separate
`mae-daemon` binary and is never constructed by the editor when `daemon_mode=off`). Verified by
`kb_enrich_is_a_plain_synchronous_call_with_no_background_thread_or_timer`, which asserts the call
returns near-instantly for an empty KB rather than merely asserting on some indirect proxy. Five
more tests cover the working path end to end without a live Ollama server (an unreachable
`127.0.0.1:1` `ai_embedding_base_url` stands in for "the provider is down/misconfigured"): the
no-store error case, the residency-blocked case (asserted via the same unreachable-address
technique — if residency were bypassed, the call would fail trying to actually reach it instead of
returning the clean pre-embed "skipped" result), the fully-cached "up to date, zero network
attempts" case, a genuine provider-failure case (asserts the failure is reported in the `errors`
array, not silently dropped or panicked), and the `limit` argument correctly capping how many
nodes are attempted.

`cargo test`/`cargo clippy --all-targets -- -D warnings`/`cargo fmt --check` clean across both
workspaces; `cargo build --workspace --features gui` clean.

## Implementation note (Phase F, principle #15)

**Decision F's own text is corrected here, not just implemented as originally written.** It says
this phase wires `kb_vector_search` to "the now-populated [HNSW] index" — but Phase B's own
Implementation note (already in this file) flagged that the HNSW `embeddings` relation is
hardcoded `<F32; 384>`, permanently mismatched with the shipped default embedding model
(`nomic-embed-text`), and named this as "real and not fixed by this phase — it's Phase F's
concern." This phase is that concern, resolved as Phase B's note anticipated: `kb_vector_search`
is wired to a NEW `mae_kb::enrichment::search_cached_embeddings` (`shared/kb/src/enrichment.rs`),
a brute-force cosine k-NN scan directly over the `embedding_cache` relation Phase B/C already
populate — not the fixed-width HNSW index at all. KB sizes here are in the thousands of nodes at
most, so a linear scan is single-digit-millisecond; no index-dimension coupling to the
user-configurable `ai_embedding_model` option is needed.

> ### ⚠ Correction (D2, principle #17): the latency sentence above was never measured, and is wrong
>
> **Measured on that path:** 72ms at 500 nodes, 221ms at 2,000, **1,287ms at 8,000** — not
> "single-digit-millisecond". The mechanism, also measured: the scan issued **2N Datalog queries**
> (`get_node` then `get_cached_embedding`, per node) to answer one search. Neither the cosine
> arithmetic nor the `crdt_doc` column was the cost — swapping `get_node` for `get_node_light`
> changed nothing measurable — it was per-query overhead at roughly 80µs × 16,000.
>
> **The deeper error is architectural, and it is what "route around the index" cost.**
> `embedding_cache` is *content-addressed*: it answers "have we already paid to embed this exact
> content?", which is an exact-key question. A content-addressed cache **cannot be searched** without
> re-hashing every node body to map hashes back to nodes — which is precisely why the scan had to
> fetch every node. Phase F used a cache as an index.
>
> Bulk-fetching alone does not rescue it: pulling 8,000 × 768 values out of the cache's `[Float]`
> column still measures **507ms**, because every element is a boxed `DataValue`. The same vectors in
> a fixed-width `<F32; 768>` column: **26ms** — a **19× difference** that is the whole reason
> semantic search was slow.
>
> **D2's resolution keeps both relations, each doing its own job.** `embedding_cache` stays
> content-addressed and `[Float]` (never scanned, so width is irrelevant and dimension-agnosticism is
> a virtue there). The searchable `embeddings` relation — which this note correctly identified as
> unusable, but for the wrong reason — is **not** deleted; its real defect was the *hardcoded* width,
> not fixed width as such. It is now created **lazily, at the width of the first vector written**, so
> it follows whatever `ai_embedding_model` emits and re-pins when that changes. Re-pinning is
> lossless in network terms because `embedding_cache` still holds every vector ever computed.
>
> The HNSW index is dropped rather than re-dimensioned: an ANN index is what forces a compile-time
> width, and it is the wrong structure for a corpus that mutates on every edit (HNSW deletion is
> awkward; its graph overhead of `M × 8–10` bytes/element is ~3× a quantized vector). A brute-force
> scan of a fixed-width column at 26ms/8,000 nodes is ample.
>
> **After D2:** 3.5ms at 500 nodes, 18ms at 2,000, 146ms at 8,000 — **8.8–20× faster**. Reuses `body_hash`/`get_cached_embedding`
exactly as `plan_enrichment_scan` already does (principle #8) — a node is a hit only if a prior
`kb_enrich` sweep already embedded it under the SAME `(model, chunk_version)` pin; a mismatch is a
silent miss, not an error, matching the cache's own contract.

**The blend into `kb_federated_search_scoped` is real, but lives one layer up from the ADR's own
line reference.** `crates/core` has no async runtime/HTTP client at all (by design — embedding a
query is a network call), so `kb_federated_search_scoped` itself cannot embed a query string. The
function instead gained a new sibling entry point, `kb_federated_search_scoped_with_vector`
(`QueryVector<'_>` struct: an ALREADY-EMBEDDED vector + its `(model, chunk_version)` pin), which
fuses the existing lexical ranking with `search_cached_embeddings`'s vector ranking via Reciprocal
Rank Fusion (`score(id) = Σ 1/(60 + rank)`, standard RRF constant) — rank position, not raw score,
since FTS relevance and cosine distance aren't on a comparable scale. `execute_kb_vector_search`
(the un-stubbed `kb_vector_search` tool) is the one caller that has a query vector on hand (via
the same blocking-embed path `execute_kb_enrich` already established in Phase E) and calls this
entry point — so `kb_vector_search`'s own result is deliberately the fused lexical+semantic
ranking (real hybrid search), not a pure-vector-only list. Plain `kb_search`/`kb_search_context`
are UNCHANGED (still call the original `kb_federated_search_scoped`, no embedding call added to
every lexical search) — blending is additive, reached only through the tool that already pays the
embed cost. Primary KB only, matching Phase B/C/E's own scope limit: federated instances have no
`Arc<dyn KbStore>` handle to search cached embeddings against.

**A drift correction to `crates/mae/src/ai_residency.rs` was required, not optional.**
`kb_vector_search` used to be classified `ScopedFederatedScan` (hard-deny outright when scope
includes a restricted KB — appropriate for its old permanent-stub behavior, which never returned
any real content). Now that it returns real `(Option<String>, Node)` results, leaving it under the
old shape would have been strictly safer (a stricter gate is fail-safe) but still wrong — exactly
the kind of drift principle #15 warns about. Reclassified to `ScopedFederatedScanFilterable`
alongside `kb_search`/`kb_search_context`, with `execute_kb_vector_search` now calling
`mae_core::ai_residency::filter_residency_exempt` on its own materialized results, matching its
siblings exactly. `ScopedFederatedScan` itself (and its now-orphaned `any_restricted_kb_label_in_
scope` gate-level pre-check helper) was removed rather than left as dead code once `kb_vector_
search` was its last real user — the #351 scope-narrowing property it existed for is preserved by
the Filterable path's own design (`kb_federated_search_scoped(query, scope)` already only includes
KBs within the resolved scope; per-node filtering then drops non-exempt content from whichever
restricted KB genuinely is in scope).

**Verification**: `shared/kb/src/enrichment.rs`'s `search_cached_embeddings` has dedicated tests
(ranks by ascending distance, ignores an uncached node, ignores a mismatched model pin, respects
`k`). `crates/core/src/editor/kb_ops/tests/kb_ops_vector_blend_tests.rs` has the ADR's own named
dual-signal verification test: a node lexically matching the query but semantically unrelated, and
a node semantically close (a near-identical cached vector) but lexically distant, both surface in
the blended top results — proving real fusion, not just "whichever signal ranks first" — plus a
regression test that the plain (no-vector) entry point is byte-for-byte unaffected.
`execute_kb_vector_search` itself is tested for every branch that doesn't require a live embedding
provider (no local store, unreachable provider, residency-blocked before any network attempt) —
matching Phase E's own established "unreachable `127.0.0.1:1`" technique for testing without a
live Ollama server; the real fused-ranking property is exercised at the
`kb_federated_search_scoped_with_vector` layer instead, since that's the fusion logic this tool
delegates to once it has an embedded query vector in hand.

`cargo test`/`cargo clippy --all-targets -- -D warnings`/`cargo fmt --check` clean across both
workspaces; `cargo build --workspace --release --features gui` clean.
