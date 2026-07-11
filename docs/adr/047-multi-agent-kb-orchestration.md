# ADR-047: Multi-agent orchestration for bulk KB work

**Status:** Proposed.
**Extends:** ADR-045 (the reliability-tiered local models this dispatches to as workers),
ADR-046 (the CLI-harness workers this orchestrates).
**Relates to:** ADR-030/031/034 (AI enrichment writing typed relationships into node text;
derived-intelligence projection — the actual work the workers perform).

## Context

MAE has a single-level supervisor/sub-agent primitive today (`delegate`,
`crates/ai/src/tool_impls/ai_tools.rs:72-98` + `crates/mae/src/session/handle_prompt.rs:977-1009`)
but no batch/parallel orchestration above it. Note: "multi-agent" is already an overloaded term
in this repo — issue #237/PR #236 use it for a *review process*, not runtime agent orchestration;
this ADR is scoped strictly to the latter.

The named, concrete gap motivating this work: **the full AI-driven KB management/enrichment/
structuring lifecycle does not work reliably for local models today.** Bulk KB enrichment — many
independent nodes, each needing a "search the KB, then propose a typed link" reasoning step — is
embarrassingly parallel, but is currently attempted (when attempted at all) as one long,
open-ended single-agent task. That shape is a poor fit for a weaker local model even with
ADR-045's harness layer, and it wastes the one property local inference actually has going for it
at scale: no per-token cost.

Multi-agent orchestration's usual objection for hosted APIs is real — published overhead
estimates for multi-agent setups run 58%-285% extra tokens versus single-agent, which is why MAE
has never built more than the single-level `delegate`. **That overhead collapses for self-hosted
models.** This is the specific, narrow case where paying it is obviously worth it — not a general
argument for multi-agent everywhere in MAE. Current industry practice also has a clear answer for
*which* topology fits a coordinator that must validate results before they touch shared state:
**orchestrator-worker is the dominant production pattern** (roughly 70% of real deployments) over
pure swarm or peer-to-peer patterns, precisely because it keeps a validation/merge point in the
loop.

MAE already has the pieces this needs, just not connected: `MiniDialogKind::Confirm`/
`MiniDialogContext::BabelConfirm` (the UI-level y/n confirm-then-effect pattern built for #269's
fix in PR #307) is a directly reusable shape for "review N proposed links before committing."

## Decision

Extend `delegate` into an **orchestrator-worker** pattern, **scoped specifically to KB batch
operations** — not a general-purpose multi-agent framework:

1. **Coordinator** partitions a batch of KB nodes needing enrichment (e.g. all nodes matching a
   given `kb_agenda` staleness/orphan query) into independent units of work.
2. **Workers** are ADR-046 CLI-harness-style sub-agents, each constrained to a **single node's**
   enrichment task — deliberately narrow scope per worker, not an open-ended multi-step mandate.
   Each worker runs behind ADR-045's guardrail layer, since these are exactly the local models
   that layer exists for.
3. **Validate/merge gate before commit.** Worker outputs (proposed typed links, proposed
   restructuring) are collected and presented via a **batch-level** confirm gate — reusing the
   `MiniDialogKind::Confirm` pattern at the batch level ("review these N proposed links"), not a
   per-node interactive prompt that would defeat the point of batching. Nothing is written to the
   KB before this gate passes.
4. **Scope boundary.** This mechanism is explicitly *not* generalized to other multi-agent use
   cases yet. If a second real use case appears later, it earns its own evaluation of whether to
   extend this pattern or build a distinct one — not an assumption that this becomes a general
   framework by default.

**Narrowing note on "review before commit."** The single-node interactive case is *already*
satisfied and is not part of this ADR's remaining gap: `mae-agent` (`crates/agent-cli`) already
gates every Write-tier tool call — including `kb_add_link`/`kb_set_role` — behind an inline human
confirm prompt via its existing `ConfirmingExecutor`/confirm-prompt UI whenever a human is driving
that harness interactively. No new confirm mechanism is needed for that path. What item 3 above
adds is strictly the **batch-level** review this ADR is actually scoped to solve — reviewing N
proposed changes across many nodes from a coordinator/worker run in one gate, which the per-node
interactive prompt does not (and, per item 3, deliberately should not) provide on its own.

## Consequences

**Positive.** Directly targets the named gap with the topology industry practice says actually
fits it — a coordinator with a validation point, not an unsupervised swarm writing to the KB.
Local-model economics make the pattern's usual cost objection moot here specifically. Reuses
existing UI primitives (`MiniDialogKind::Confirm`) and existing querying (`kb_agenda`) rather than
inventing new ones.

**Costs.** A coordinator/worker split adds real complexity (partitioning logic, result
aggregation, a new batch-review UI surface) that a single-agent approach wouldn't need — justified
here specifically because the enrichment task is genuinely parallel and the worker model is weak
enough to need narrow scoping per unit of work. This complexity is not free to maintain and should
not be paid for use cases that don't actually need parallelism.

## Alternatives rejected

- **A general-purpose multi-agent orchestration framework from day one.** Rejected — no second
  real use case exists yet to justify the generality; building it speculatively would violate
  principle #7's "no ad-hoc solutions... don't design for hypothetical future requirements" in
  the opposite direction (over-building for imagined future needs instead of under-building for
  a real one).
- **Swarm / peer-to-peer agent pattern** (workers coordinating directly with each other rather
  than through a coordinator). Rejected — orchestrator-worker is both the dominant production
  pattern and the better fit here specifically because a coordinator gives us the validate/merge
  gate before anything touches the KB; a swarm has no natural place for that gate.
- **Single-agent, larger-context enrichment sweep** (no orchestration, just a bigger prompt/more
  steps for one agent). Rejected — this is the status quo shape that doesn't work reliably for
  local models per the named gap; decomposing into narrow per-node units is the actual fix, not
  an alternative to it.

## Verification

- A batch of N independent KB nodes is enriched by N narrowly-scoped workers running concurrently
  against a local model, with zero worker writing directly to the KB — all proposed changes pass
  through the batch-level confirm gate first.
- Rejecting the batch-level confirm leaves the KB completely unchanged — the adversarial case
  (a bad batch of proposals) must be a no-op, not a partial write.
- A worker given a node outside its assigned scope (i.e., attempting to touch a second node) is
  rejected by the same per-tool `PermissionTier`/scope enforcement already in place — the
  orchestration layer does not get a bypass around existing permission enforcement.
- Token/wall-clock cost of the N-worker batch run against a local model is measured and compared
  to a single-agent sequential sweep of the same batch, confirming the parallelism is actually
  paying for itself in wall-clock time (the concrete benefit this ADR is justified by).
