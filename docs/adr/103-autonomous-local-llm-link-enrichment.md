# ADR-103: Autonomous local-LLM KB link enrichment

**Status:** Proposed. Phased (A–F), each phase independently shippable behind an adversarial test that
fails first. **Depends on ADR-101** (a machine link has no durable home until structured CRDT edges
exist) and **is bounded by ADR-102** (a 24/7 concurrent writer needs the engine to absorb it). The
confidence-calibration method (D4) and threshold policy (D5), plus the over-linking guardrails (D8),
are **grounded in the Phase-0 prior-art review** (`docs/research/103-kb-link-enrichment-prior-art.md`),
whose one-line verdict shapes the whole design: *the confidence number is an input to a conservative,
empirically-calibrated, human-backstopped gate — never the gate itself.*

**Extends:** ADR-034 item 1 ("derive relationships" — the un-ticketed placeholder this ADR finally
fills), ADR-061 (embedding enrichment — this reuses its vector cache and sweep/lease machinery and is
the *link* half ADR-061 explicitly deferred: *"No 'derive relationships' AI logic exists anywhere yet"*),
ADR-045/046/047 (local-model harness / CLI surface / multi-agent — deferred bulk path).
**Relates to:** ADR-101 (structured edges — the write target), ADR-102 (engine — the write load),
ADR-048 (AI residency), ADR-018 (KB roles), ADR-024 (attention bus — review notifications),
principle #16 (the threshold/residency are controls the agent must not reach).

**Evidence:** `daemon/src/{scheduler.rs,enrichment.rs,config.rs}`, `shared/kb/src/enrichment.rs`
(`search_cached_embeddings`, `plan_enrichment_scan`), `shared/kb/src/cozo_store/agenda.rs`,
`crates/ai/src/{ollama.rs,guardrail.rs}`, `crates/core/src/kb_sharing.rs` (tri-surface pattern),
`crates/mae/src/ai_residency.rs`, `crates/ai/src/tools/authorization.rs`.

## Context

A KB is only as useful as its link graph. Humans and AI agents add material continuously, but
connecting a new note to the right existing nodes — within a KB and across federated KBs — is manual,
so orphans and dead-ends accumulate and the graph decays. MAE already computes and caches embeddings
in a background daemon sweep (ADR-061), and it already has the health queries that identify decayed
nodes (`agenda.rs`: orphan / dead-end / weakly-linked / stale, verified federation-aware via
`kb_agenda`). What is missing is the step in between: **using a local model to decide which nodes
should connect, with what relationship, at what confidence — and applying that as a reviewable,
provenance-bearing link.** No such code exists anywhere (ADR-061 §575 confirms it).

This ADR builds that step as a **24/7 background process using a local Ollama model**, so a
self-hosted deployment enriches its own KB with no per-token cost and no data leaving the host. It
must be *sustainable*: precision-oriented (a wrong link pollutes the graph and erodes trust more than
a missing link costs), human-supervisable (a review queue, not blind auto-apply), and bounded (budget,
quota, residency, lease).

### What already exists to build on (verified)
- **Embedding discovery:** `search_cached_embeddings` (`shared/kb/src/enrichment.rs`) does k-NN over
  the populated cache; wired to `kb_vector_search`. *Caveat (PREREQ-C):* today it is **primary-store-
  only** — federated instances degrade to lexical. Cross-federation discovery needs the fix in D8.
- **Candidate signals:** `kb_agenda` is federation-aware (merges per-instance) for orphan / dead-end /
  weakly-linked / stale.
- **Sweep machinery:** `run_enrichment_sweep` + per-`op_kind` lease (`daemon/src/enrichment.rs`,
  `kb_lease.rs`) — a second, distinct `op_kind` for link enrichment runs without colliding with the
  embedding sweep (verified). In-process enrich-now (`execute_kb_enrich`, Phase E) is the daemon-
  optional twin to mirror (principle #12).
- **Local model:** `OllamaProvider` + `GuardrailProvider` (rescue-parse / retry / loop-detect).
  *Caveat (PREREQ-C):* schema-constrained decoding (`with_format_constrained`) exists but is **dead
  code** — must be wired or we rely on guardrail rescue-parsing (D9).
- **Review surface pattern:** the tri-surface introspection snapshot (`KbSharingSnapshot` read by the
  `*KB Sharing*` buffer + `kb_sharing_status` MCP + `(kb-sharing-status)` Scheme) is reusable.
  *Caveat:* that buffer is human-triggered via plain `display_buffer`; an **agent-triggered** review
  buffer must use `display_buffer_for_agent()` + `with_ai_dispatch_scope()` or it steals the window.

## Decision

### D1 — The enrichment loop
For each **target node** (D2), discover **candidate nodes** (D3), have the local model **judge** each
(source, candidate) pair (D4), assign a **calibrated confidence** (D4), and **gate** the result (D5):
above threshold → an `accepted` structured edge (ADR-101, projected live); below → a `pending`
structured edge in the review queue (D6). All writes are structured CRDT edges via the ADR-092 single
write path — **never a body-text mutation, never a direct cozo write.**

### D2 — Candidate selection ("notes needing review/enrichment")
Two sources, unioned, federation-scoped (`KbScope::All`):
- **Explicit flag** — a node tagged `:needs-enrichment:` (or an equivalent property). No schema change
  (reuses `Node.tags`/`properties`); set by a human or an agent to prioritize a node.
- **Health backlog** — `kb_agenda` orphan / dead-end / weakly-linked / stale results, already
  federation-aware. This is the standing, self-replenishing work queue.
Selection is bounded per sweep (batch size, config) and ordered explicit-flag-first, then by decay
severity, so a large backlog degrades gracefully rather than thrashing.

### D3 — Candidate discovery ("find the nodes to connect")
For a target, fetch top-k semantically-related nodes via `search_cached_embeddings` over the embedding
cache ADR-061 populates, **across federated instances** (requires D8). Embeddings do the recall;
the model does the precision. Discovery never proposes a link by similarity alone — similarity only
selects *which pairs the model judges*.

### D4 — Relation judgment as *reasoning over a fixed vocabulary*, then a multi-signal confidence
Frame the model's job as **judgment, not extraction.** Prior art is decisive here: LLMs are relatively
strong as an "inference assistant" and collapse as "few-shot information extractors" (GPT-4 one-shot RE
41.9 F1 vs 69.4 fine-tuned; [arXiv 2305.13168](https://arxiv.org/html/2305.13168v3)), and small models
degrade most. So the prompt supplies **one focused (source, candidate) pair** (MAE's own
`docs/MODEL_SUPPORT.md` evidence: small models succeed on one focused task, fail on long plans), and
asks a bounded question: *does one of the known `rel_types` hold between these two nodes, or `none`?*
`none` is an **easy, explicitly-encouraged default** (over-linking comes from a model reluctant to
decline — D8). The `rel_type` is constrained to the fixed vocabulary (D9), never free-form.

**Confidence is multi-signal, not a self-reported number.** The Phase-0 crux finding: a raw verbalized
score from a 7B–14B model is untrustworthy out-of-domain, and confident errors are *systematic* — the
"most consistent" model was wrong 48% of the time at high confidence, with identical wrong answers
across every sample on 28% of hard cases ([arXiv 2607.08065](https://arxiv.org/html/2607.08065v1)). So
the gate combines three signals: **embedding similarity** (the candidate prior) × **verbalized
confidence** (ask for it — better-calibrated than logprobs for instruction-tuned models, Tian et al.
[EMNLP 2023](https://aclanthology.org/2023.emnlp-main.330/)) × **self-consistency over 2–3 samples**,
with **unanimous agreement treated as necessary-but-not-sufficient** (unanimity is where systematic
errors hide). The mapping from these signals to the [0,1] gate scalar is **empirically calibrated
against a hand-labeled sample of the deployment's own KB** — not a paper number — because calibration
does not transfer across distributions (Kadavath P(IK)-on-new-tasks,
[arXiv 2207.05221](https://arxiv.org/abs/2207.05221)). Confidence stays an opaque calibrated scalar
behind a trait so the method can evolve without touching D5/D6.

### D5 — Three-band confidence gating (threshold is agent-unsettable — principle #16)
**Three bands, not two** (the HITL-standard shape): `confidence ≥ auto_apply` → `accepted`;
`review_floor ≤ confidence < auto_apply` → `pending` (review queue); `< review_floor` → `rejected`
(discarded, not queued — keeps the queue signal-dense).

**Default policy: start conservative, earn looser over time.** Ship the auto-apply band **narrow** —
default **`auto_apply = 0.95` *and* full sample agreement**, `review_floor = 0.70` — and loosen only
after the human accept-rate on the review queue justifies it (D10's feedback signal). This is stricter
than the ≥0.90 practitioner rule of thumb, deliberately, because a wrong auto-applied link is a silent,
persistent graph pollutant while a queued miss is cheap and recoverable — the cost asymmetry argues for
precision over recall. **Cross-KB (federated) proposals are held to a stricter bar or routed entirely
to review in early operation** (distribution shift + no shared context; D8).

**The threshold and residency are controls the agent cannot change.** Two enforcement paths, chosen to
make asymmetry structural rather than a checked-and-refused option:
- **Background sweep:** its threshold/residency/enabled live in **daemon config** (`daemon.toml`,
  `[link_enrichment]`), which the agent has **no write path to** (verified: editor options and daemon
  config are separate surfaces; `~/.config/mae/**` is refused to AI writes). This is the cleanest #16
  story — the control is simply not on any agent-reachable surface.
- **In-process enrich-now:** its threshold is an OptionRegistry option that opts into agent-unsettable
  gating. This requires generalizing the `ai_tier` special-case (`authorization.rs`) into a reusable
  `min_set_tier` on `OptionDef` (PREREQ-C). Until that generalization lands, the in-process path
  ships **read-only-to-agent** by using the daemon-config value or a privileged-only setter.

### D6 — Materialization = a status flip, reviewed on the same substrate
Accepting a `pending` edge (by a human in the review buffer, or by crossing the threshold) is a
**status flip** on the ADR-101 structured edge; the projector then promotes it to a live cozo edge
(ADR-101 D4). Rejecting flips to `rejected` (a tombstone, not a delete, so the enricher does not re-
propose it). **No content is rewritten**; nothing is appended to any body. This is what makes the
whole feature retractable and auditable.

### D7 — Review surface (tri-surface parity, agent-safe display)
A `*KB Enrichment*` management buffer listing `pending` edges (source, candidate, rel_type,
confidence, rationale, model) with accept/reject/accept-all-above-X actions, mirroring the
`KbSharingSnapshot` pattern: one introspection snapshot read by the buffer **+** an MCP tool
(`kb_link_suggestions` / `kb_review_link`) **+** Scheme primitives — full human/AI *work* parity
(principle #3), while the *controls* (threshold, residency) stay human-only (principle #16). If the
buffer is opened by an agent tool, it uses `display_buffer_for_agent()` + `with_ai_dispatch_scope()`
so it never steals the user's window (the `*KB Sharing*` buffer does **not** model this — it is
human-triggered; this ADR must not copy that part).

### D8 — Residency, roles, lease, federation (the guardrails)
- **Residency re-established at the sweep**, not inherited. The store write path and MCP dispatch gate
  are *not* the same; a background daemon writer bypasses the MCP-level `check_kb_residency`. The
  sweep performs the residency check itself before reading any node (mirroring
  `plan_enrichment_scan`'s pre-read gate), so a `LocalModelsOnly` KB is never judged by a hosted
  model.
- **Roles:** writing an edge requires Editor+ on the target KB (ADR-018).
- **Lease:** a distinct `op_kind = "link-enrichment"` lease (verified non-colliding with embedding).
- **Federation embedding access (PREREQ-C, D3's dependency):** give federated instances queryable
  embedding caches / `KbStore` handles so cross-KB discovery is real, not primary-only.
- **Budget/quota:** reuse tenant quota + `BudgetConfig`; a local model is unpriced (free), but the
  sweep is rate/batch-bounded so it cannot starve interactive queries (ties to ADR-102's concurrency
  budget).

### D8b — Over-linking & decay guardrails (Phase-0 findings 1/6)
No shipped note tool auto-writes semantic links unattended; stepping past that line requires explicit
guards against the hairball and the trust-cliff:
- **Density cap:** at most top-k highest-confidence machine edges per node (config), so a hub node
  cannot accrete a hairball. Excess candidates are dropped, not queued (logged).
- **`none` is the encouraged default** (D4) — the primary over-linking control is a model willing to
  decline.
- **Staleness re-validation:** an `accepted` machine edge is re-judged when *either* endpoint's body
  changes (hook on node update); an edge that no longer validates is flipped back to `pending` (or
  `rejected`), because auto-links rot silently into wrong assertions when notes drift (soft link rot).
- **Trust-cliff protection:** a per-KB **review-heavy initial mode** (all machine edges → `pending`
  regardless of confidence) for the first N accepted-or-rejected decisions, until the human has seen
  the system be right repeatedly; the auto-apply band activates only after that. An **auditable,
  bulk-revertible trail** of every auto-applied edge (structured-edge `provenance/model/created_at` +
  the review buffer's accept-all/revert-all) so a user can inspect or undo en masse — a handful of
  visible false links otherwise destroys trust in the whole feature.

### D9 — Two-phase decoding: reason free-text, then constrained-emit (PREREQ-C)
Phase-0 finding #5 is specific: constrained decoding reliably fixes *format* (grammar-constrained small
models match or beat much larger unconstrained ones on well-formedness — [DOMINO
arXiv 2403.06988](https://arxiv.org/pdf/2403.06988)), **but forcing structure around the reasoning
costs 10–15% accuracy** ("format tax"), worst for small models, because JSON mode forces answer fields
before the chain-of-thought finishes ([EMNLP 2024](https://aclanthology.org/2024.emnlp-industry.91.pdf)).
So the judgment call is **two-phase**: (1) free-text reasoning about whether/how the pair relates and
why; (2) a **schema-constrained** final emission `{relation ∈ fixed-enum ∪ none, confidence, evidence}`
via `with_format_constrained` (wired here — it is dead code today, PREREQ-C), the `relation` field
grammar-restricted to the `rel_types` set so the model **cannot invent a relation type**. Prefer a
**Qwen2.5-class** local model (repeatedly better than Llama at structured output). A malformed or
out-of-vocabulary emission is quarantined (logged, skipped), never written as a degenerate edge.

### D10 — 24/7 orchestration
A second pass in `run_maintenance_tick` (`daemon/src/scheduler.rs`) alongside the embedding sweep,
gated by `[link_enrichment].enabled` (default off), residency- and lease-checked per store, with
observability: nodes scanned, pairs judged, edges proposed/accepted/queued, acceptance rate,
per-model precision (from human accept/reject feedback — a standing calibration signal). The in-
process enrich-now path mirrors ADR-061 Phase E for the daemon-less user.

## Consequences

**Positive** — the graph is continuously, cheaply (local model) maintained; every machine link is
provenance-tagged, confidence-scored, reviewable, and retractable; the human stays in control of the
precision/recall dial; reuses the embedding/sweep/lease/tri-surface infrastructure rather than
inventing parallel machinery.

**Negative / risks** — over-linking / hairball if the threshold is too low (mitigated by precision-
oriented default + review queue + acceptance-rate monitoring); small-model judgment is noisier than a
frontier model (mitigated by focused prompts, constrained decoding, calibration, and the human gate);
the acceptance-rate feedback loop must not silently drift the threshold (it informs humans, it does
**not** auto-tune the agent-unsettable threshold); depends on ADR-101 and PREREQ-C landing first.

## Explicitly out of scope
- Multi-agent orchestration for a huge backlog (ADR-047, deferred; single-pass sweep first).
- Changing how *human* links are authored (ADR-101 D3 keeps them in body text).
- Non-link enrichment (summaries, tags, todo inference) — a possible sibling, not this ADR.
- The DB engine (ADR-102) and the structured-edge substrate (ADR-101).

## Phased implementation (each phase fails a test first)
- **A** — PREREQ-C fixes: federated embedding access (D8), confidence surfaced on read, `min_set_tier`
  on `OptionDef`, schema-constrained-decoding-or-rescue decision (D9).
- **B** — candidate selection (D2) across federation; batch/order bounding.
- **C** — two-phase judgment (D9: free-text reason → constrained enum emission) + multi-signal
  confidence (D4) behind a swappable trait; malformed/out-of-vocab quarantine; calibrate on a labeled
  KB sample.
- **D** — three-band gating + structured-edge write (D5/D6) via ADR-101; residency re-established at
  the write (D8); density cap (D8b).
- **E** — review surface (D7): buffer + MCP + Scheme on one snapshot; agent-safe display;
  accept/reject/accept-all/**revert-all**.
- **F** — 24/7 daemon pass (D10) + in-process twin; staleness re-validation on endpoint edit (D8b);
  review-heavy initial mode (D8b); observability + acceptance-rate feedback that *informs* (never
  auto-tunes) the threshold.

## Adversarial tests (principle #14 — encode the attacker/failure model)
- A `LocalModelsOnly` KB is **never** read by a hosted provider even when the sweep is misconfigured to
  one (residency blocks before any node is read — the sweep-level gate, not the MCP gate).
- The agent **cannot** lower the threshold or flip residency via `set_option`/`set-option!`/MCP (both
  the daemon-config path and the `min_set_tier` path).
- A below-threshold edge is `pending` and is **not** in the live projected graph; accepting it
  promotes it; rejecting it is a tombstone the next sweep does **not** re-propose.
- A deliberately-wrong candidate (semantically near but unrelated) is judged "no link" or lands below
  threshold — a false-positive-rejection oracle, not a happy-path pass.
- A malformed model output is quarantined, never written as a degenerate/mis-typed edge.
- Cross-federated-KB: a link proposed between a primary node and a federated-instance node is correct
  and reviewable (guards the primary-only regression).
- N-way daemon race on the link-enrichment lease converges to one sweeping writer.
- Calibration: on a labeled fixture, the chosen multi-signal method separates true from false links
  better than raw single-shot verbalized confidence (the D4 method-selection oracle).
- Density cap: a hub node never accretes more than top-k machine edges no matter how many candidates
  clear threshold (hairball guard, D8b).
- Staleness: editing either endpoint's body re-judges the edge and flips a now-invalid `accepted` edge
  off the live graph (soft-link-rot guard, D8b).
- Review-heavy initial mode: during the initial window a 0.99-confidence edge is still `pending`, not
  auto-applied (trust-cliff guard, D8b).
- Two-phase emission: the `relation` field is always in the fixed vocabulary or `none` — a model
  attempting a free-form relation is rejected, not written (D9).
