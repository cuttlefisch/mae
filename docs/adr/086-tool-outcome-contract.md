# ADR-086: A tool result states whether the caller's requested postcondition holds

**Status:** Accepted.
**Relates to:** ADR-050 (cross-editor MCP compatibility — `structuredContent`/`outputSchema` are the
mechanism this ADR uses), ADR-084 (permission enforcement — a refusal is one of the outcomes this
contract must express), ADR-056 (category denial, likewise).
**Tracking:** audit epic #592; the ~15 findings across 11 subsystems listed under *Context*.

## Revision note

The drafted rule was: *"an operation that changed nothing returns an error, or an explicit
`no-op` — never a success message describing work it did not do."* A prior-art review
([practice: prior-art review](../../assets/devpractices/practice-prior-art-review.org)) established
that this tests the **wrong predicate**, and that testing "did state change" would introduce a real
regression. The rule below tests the postcondition instead. The original phrasing is preserved here
so the record shows what was corrected and why.

## Context

A pre-v0.15 audit found ~15 instances of one defect shape across 11 unrelated subsystems: **an
operation that did not do what was asked reports success.** Representative cases:

- `buffer_write` on a read-only buffer returns a success message describing the write.
- `kb_enrich` returns `"complete"` when every embedding failed.
- `open_file` / `create_file` decide success by testing `editor.status_msg.contains("Error")`
  (`crates/ai/src/tool_impls/file.rs:34-36`) — a substring match against a human-facing UI string.
- An LSP workspace edit silently drops edits for files that are not open, and reports the edit
  applied.
- `p2p/join_ticket` returns a ticket on a daemon with no mesh running.

The consumer of these results is an AI agent. MCP routes tool-execution errors into the *result*
rather than as JSON-RPC protocol errors precisely so the model sees them and can adapt — so a tool
returning prose success is not merely mislogging, it is misinforming the only party that could
correct course.

## Decision

**A tool result answers one question: does the state the caller asked for now hold?** Three
outcomes, and the discriminator is the postcondition — not whether bytes changed.

| The requested postcondition… | and we… | Outcome |
|---|---|---|
| holds | changed something | **success**, describing what changed |
| holds | changed nothing (already satisfied) | **success**, explicitly marked as a no-op, describing *current state* |
| does not hold | refused, could not attempt, or failed partway | **error**, with a typed reason |

1. **Never infer outcome from prose, and never emit outcome only as prose.** Deciding success by
   `status_msg.contains("Error")` is the defect in its purest form: it couples control flow to a
   human-facing string that changes for unrelated reasons, and it fails open when the message is
   reworded. RFC 9457 states the general rule directly — *"Consumers SHOULD NOT parse the 'detail'
   member for information; extensions are more suitable and less error-prone ways to obtain such
   information."* Every call site that inspects `status_msg` to decide success is replaced by
   propagating a real `Result` from the operation that knows.

2. **"Changed nothing" is not by itself an error.** This is the correction. An idempotent retry that
   finds the requested state already satisfied has *succeeded*. AWS's guidance is explicit that even
   a technically-correct `AlreadyExists` error is bad design here, because the caller cannot tell
   whether their request or an earlier one produced the state, which breaks automated retry. Agents
   retry constantly and without idempotency keys, so a "no change ⇒ error" rule would turn
   `kb_add_link` on an existing link, or `kb_set_policy` to the policy already in force, into
   failures — and the agent would then "fix" a problem that does not exist.

3. **A no-op is distinguished, not silent.** Where an operation succeeded by finding the state
   already correct, the result says so explicitly rather than describing work it did not perform.
   HTTP's 304 is the shape: a distinguished, non-error outcome. Prose like `"Wrote 40 lines"` when
   nothing was written is forbidden whether or not the outcome was ultimately fine.

4. **Refusals are errors, and every one of the audit's findings is a refusal.** A read-only buffer,
   a daemon with no mesh, embeddings that all failed, edits dropped for unopened files — in none of
   these does the requested state hold afterwards. gRPC's `FAILED_PRECONDITION` names this category
   exactly: *"system not in required state for operation."* This is what makes the rule cheap to
   apply — the fix for all ~15 is `Err`, and none of them is the idempotent-retry case that
   Decision 2 protects.

5. **No partial success.** An operation that half-applied reports an error naming what did and did
   not happen, rather than a success qualified in prose. AIP-193: *"APIs should not support partial
   errors. Partial errors add significant complexity for users, because they usually sidestep the
   use of error codes, or move those error codes into the response message, where the user must
   write specialized error handling logic."* `kb_enrich` reporting `"complete"` with a failure count
   buried in text is that anti-pattern. Where partial application is genuinely unavoidable, the
   result is an error whose payload enumerates the per-item outcomes.

6. **The machine-readable channel is `structuredContent`.** MCP's 2025-06-18 revision added
   `structuredContent` with an `outputSchema`, and the conformance language makes it enforceable
   rather than conventional: servers *MUST* provide structured results conforming to a declared
   schema, and clients *SHOULD* validate them. That is where a typed outcome belongs; the text block
   remains for the model to read. `isError` stays the coarse signal.

## Consequences

**Positive**

- The agent can distinguish "done", "already done", and "refused" without parsing English, which is
  the only way it can choose between proceeding, retrying, and escalating.
- Removes a whole class of silent-corruption bugs where the agent builds on a false premise —
  by far the worst outcome of this defect, since the agent's next several actions compound it.
- `status_msg` goes back to being a UI string with no control-flow load, so rewording it stops being
  a behaviour change.

**Negative / Risks**

- Tools currently return `Result<String, String>`; a fully typed outcome envelope across ~770 tools
  is disproportionate. This ADR therefore requires the *semantics* everywhere (refusal ⇒ `Err`) and
  the *structured envelope* only where a tool already returns JSON. That is a deliberate partial
  application, and it means the no-op marker is by convention in the JSON body rather than by type
  for the rest.
- Turning silent successes into errors is a visible behaviour change for any agent workflow that was
  (wrongly) proceeding past them. That is the point, but it belongs in release notes.
- Distinguishing "already satisfied" from "refused" requires each operation to actually check its
  postcondition. In some cases that check does not exist yet and must be written; where it cannot be
  written cheaply, the honest outcome is an error, not an assumed no-op.

## Enforcement

- A test asserting no tool implementation decides success by substring-matching `status_msg`. This
  is greppable and the current violations are enumerable, so it lands as a shrinking allowlist with
  each entry citing its issue — the ratchet pattern already used by `docs/AUDIT_BASELINE.json`.
- Per-defect regression tests that exercise the *refusal* path and assert `Err`: a read-only buffer,
  an enrich where every embedding fails, a workspace edit naming an unopened file, a `join_ticket`
  with no mesh. Per principle #14 the assertion is on the failing case, not the happy path.
- Where a tool has a genuine idempotent-retry path, a test asserting the *second* identical call
  still succeeds — so Decision 2 does not decay into Decision 4 through over-zealous fixing.

## Alternatives considered

**"Changed nothing ⇒ error", as drafted.** Rejected on evidence. It breaks idempotent retry, which
for an agent-facing API is a common path rather than a corner case, and it is contradicted by both
AWS's retry guidance and Google's AIP-134/135 `allow_missing` design, where an already-satisfied
request explicitly returns the resource unchanged rather than failing.

**Return a typed outcome enum from every tool.** Correct and disproportionate for ~770 tools whose
signature is `Result<String, String>`. Revisit if the tool surface is ever refactored for another
reason; the semantics in this ADR are what matter, and they are expressible in the existing type.

**Leave prose parsing in place but make the strings more reliable.** Rejected — this is the
"stringly-typed" trap RFC 9457 warns against, and it makes every UI-copy change a potential
control-flow regression.
