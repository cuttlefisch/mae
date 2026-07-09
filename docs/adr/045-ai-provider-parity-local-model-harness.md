# ADR-045: AI provider parity & local-model harness

**Status:** Proposed.
**Extends:** none (first ADR for the AI-integration area — `PermissionTier`, `AgentProvider`, and the
existing multi-provider support have never been formalized in `docs/adr/`).
**Feeds:** ADR-046 (the CLI/MCP-shim surface this harness runs behind), ADR-047 (the multi-agent
workers this harness makes viable for local models).

## Context

`crates/ai/src/` already has a fully provider-agnostic core: one `AgentProvider` trait, canonical
`ProviderResponse`/`ToolDefinition` types (`provider.rs`, `types.rs`), and a `PermissionTier`
(`ReadOnly < Write < Shell < Privileged`) enforced identically regardless of active provider.
Ollama support (`crates/ai/src/ollama.rs`, added in PR #289) is complete at the wire-protocol
level — it reuses `OpenAiProvider::serialize_tools` and correctly handles Ollama-specific
quirks (object- vs. string-encoded tool-call arguments, synthesized call IDs, `done_reason`
inference for `StopReason::ToolUse`) — and is well unit-tested for those quirks.

But it was added narrowly, to fix a `think`-field forwarding bug, not as a deep tool-calling-parity
effort:

- `docs/MODEL_SUPPORT.md`'s compatibility table has **zero** Ollama/local-model rows.
- `crates/ai/src/executor/context_limits.rs` marks every Ollama-relevant model family
  (`llama3*`, `qwen*`, `mistral*`, `phi-4`, `command-r*`) `Untested`.
- A real parity bug exists: `delegate()`'s provider dispatch
  (`crates/mae/src/ai_event_handler.rs:512-516`) only explicitly matches `"openai"`/`"gemini"`,
  silently defaulting everything else — including `"ollama"` — to `ClaudeProvider`. The main
  session's `setup_ai()` (`crates/mae/src/bootstrap.rs:542-548`) gets this right
  (`"ollama" => OllamaProvider`); `delegate()` fell out of sync. **An Ollama-primary session's
  `delegate()` sub-agents silently run on Claude today**, burning a hosted-API key the user
  never intended to use.
- No harness accommodations exist for models that are tool-calling-capable but less reliable
  than Claude/GPT/Gemini at it — MAE either works with a model as-is or doesn't.

Current industry practice (Ollama docs; community benchmarking; a documented engineering
playbook — sources cross-linked from the epic issue) is unambiguous on two points relevant here:
tool-calling reliability is uneven across local models (function descriptions over ~2-3 lines, or
more than a handful of exposed tools, measurably raise failure rate; 14B-32B is the community
floor for reliable tool use), and a **harness-side guardrail stack** — independent of the model —
can take a weak model from unreliable to production-grade. Ollama v0.5+ also added
JSON-schema-constrained structured outputs (the `format` parameter), directly usable to constrain
tool-call argument generation, which MAE's Ollama provider does not yet use.

## Decision

1. **Fix the `delegate()` dispatch bug.** Route it through the same provider-construction logic
   `setup_ai()` already uses, so a sub-agent always inherits the correct provider — no new
   concept, just closing the drift between two call sites that should have shared one.

2. **Formalize a model *reliability* tier**, distinct from and additive to `context_limits.rs`'s
   existing context-size tiers: `Verified` (passed `model_exam` at >=90%), `Marginal` (70-89% or
   an occasional hallucination), `Untested` (no `model_exam` run yet). Populated by actually
   running `model_exam` against real Ollama models (starting with a currently-recommended stable
   small model), not asserted from vendor claims.

3. **A provider-agnostic guardrail layer**, sitting between `AgentProvider` and MAE's existing
   tool registry, implemented once and applied by reliability tier (not by provider name):
   - **Response validation + rescue-parsing** — don't hard-fail on malformed tool-call JSON;
     attempt to recover intent before retrying.
   - **Targeted retry nudges** — a corrective re-prompt naming the exact tool/format expected,
     not a blind resubmission.
   - **Step enforcement** — track workflow stage programmatically; reject a premature/out-of-order
     tool call with an informative nudge rather than executing it.
   - **Context/budget-aware compaction** — reuse MAE's existing compaction rather than a
     per-provider variant.

   This is deliberately generic: any smaller/cheaper *hosted* model that shows up later gets the
   same treatment for free, because the layer keys on reliability tier, not on `provider ==
   "ollama"`.

4. **Use Ollama's `format` parameter** to constrain tool-call argument generation for any model
   below the `Verified` tier, reducing malformed-JSON retries at the source rather than only
   catching them after the fact.

## Consequences

**Positive.** Closes a real, currently-silent correctness bug immediately. Replaces an
all-`Untested` compatibility table with data. Gives local models the same harness-side scaffolding
industry practice says they need, without hardcoding it to one provider — the guardrail layer is
reusable infrastructure, not an Ollama special case (consistent with principle #7's "no ad-hoc
solutions").

**Costs.** The guardrail layer is new code sitting in the hot path of every tool call for
non-`Verified` models — must be measured for latency overhead, not just correctness. Tiering by
`model_exam` results means the compatibility table requires an actual local-model install to
populate (a real dependency for whoever runs Phase A of the epic, not a free assertion).

## Alternatives rejected

- **Leaving reliability entirely to model choice** (i.e., no harness layer, just "point people at
  Qwen3 8B and hope"). Rejected — community data (an 8B model going from 53% to 99% pass rate
  purely from harness engineering, no model change) shows the harness matters more than the model
  pick alone.
- **A bespoke Ollama-only guardrail path inside `ollama.rs`.** Rejected — every mechanism above is
  provider-agnostic by nature (JSON rescue-parsing, step tracking, retry nudges); burying it inside
  one provider's file would duplicate it the next time a second unreliable provider shows up,
  violating principle #8 (shared computation).

## Verification

- `delegate()` invoked from an Ollama-primary session produces sub-agent tool calls against the
  Ollama endpoint, not Claude's — verified by asserting on the constructed provider type, not just
  observed behavior (a mocked/stubbed Claude call would otherwise pass silently).
- `model_exam(action="plan")` → executed against a real local Ollama model → `action="grade"`
  populates a real row in `docs/MODEL_SUPPORT.md`'s compatibility table, replacing an `Untested`
  placeholder with an actual pass rate.
- A deliberately malformed tool-call JSON from a `Marginal`-tier model is rescued/retried by the
  guardrail layer rather than surfacing as a hard error to the user — the adversarial case
  (malformed output), not just the happy path.
- A `Verified`-tier model (e.g. Claude) sees no behavior change and no added latency from the
  guardrail layer being present but inactive for its tier.
