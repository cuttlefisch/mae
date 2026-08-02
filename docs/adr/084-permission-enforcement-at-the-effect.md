# ADR-084: AI permission tiers are enforced at the effect, not at the tool name

**Status:** Accepted.
**Extends:** ADR-051 (per-session `PermissionPolicy` — this ADR fixes *where* that policy is
consulted, without changing how a session declares one).
**Relates to:** ADR-049 (`mae-agent` as the default AI surface — the embedded session this ADR brings
under the policy), ADR-085 (`ToolCategory` describes subject matter, not blast radius — the sibling
axis), ADR-056 (session-scoped category dispatch, whose guarantees depend on this one holding).
**Tracking:** private security advisory `GHSA-qwh8-m8j6-563h`.

## Context

MAE advertises four AI permission tiers — `readonly`, `write`, `shell`, `privileged` — as the control
that bounds what an AI agent may do. `SECURITY.md` described them as "enforced before every tool
execution with no bypass vectors."

A pre-v0.15 audit established that the tier is consulted at exactly **one** place:
`crates/ai/src/executor/tool_dispatch.rs`, on the MCP tool-dispatch path. Three consequences follow,
each independently verified and each reachable without anything exotic:

1. The embedded `AgentSession` — the `:ai` surface and `delegate()` sub-agents — carries no
   `PermissionPolicy` at all, so it never consults one.
2. The `ai_tier` option updates a status-bar string. The value that is actually enforced comes from a
   different source with no write path between them, and an unrecognised value resolves **open**, to the
   most permissive tier.
3. `eval_scheme` is `Write` tier, and the Scheme runtime can reach process execution. A session capped at
   `write` therefore reaches shell — including transitively, since some editor commands enqueue Scheme
   that does so.

The common shape is that **the tier is checked against a tool's name at one gate, while the effect the
tier exists to prevent is reachable through several other doors.** Gating names does not bound effects.

This matters beyond the editor: v0.15 ships MAE as a headless MCP backend for external editors, and that
initiative's own premise (ADR-051, ADR-056) is that the server-side policy is the only real boundary,
because a client's "always allow" is not one.

## Decision

**Enforcement is applied at the effect, and every entry point shares one enforcement path.**

1. **The policy moves to where every path already converges.** `AgentSession` gains a
   `PermissionPolicy`, and the embedded/`delegate()` path consults the same check the MCP path does.
   There must be no tool-dispatching entry point that can reach an effect without passing it.

2. **Effects that exceed a tier are gated at the effect itself, not at the surface that requests them.**
   Concretely, the Scheme primitive that spawns a process checks the ambient tier before spawning,
   rather than `eval_scheme` being raised to `Shell` wholesale. This was chosen deliberately over the
   simpler reclassification:
   - It preserves a `write` session's ability to evaluate ordinary Scheme (`(buffer-name)`,
     `(buffer-insert …)`), which reclassification would remove entirely.
   - It catches the transitive path, where a command enqueues Scheme that reaches the effect. Gating the
     entry point would miss exactly that route, which is how the audit's verified exploit worked.
   - It is small: of the ~204 editor-facing Scheme primitives, one spawns a process. This is a
     deny-list of one, not a classification of the whole API.

3. **Unrecognised tier values fail closed.** An unparseable or unknown tier resolves to the *most*
   restrictive tier, never the most permissive. This is the general form of a fail-open bug that the
   audit found at two independent call sites; both adopt this rule.

4. **`ai_tier` either reaches the enforced policy or ceases to exist.** An option that is registered,
   `:set-save`-persistable and rendered in the status bar, but which changes nothing, is worse than no
   option — it actively misinforms. Principle #7 requires user-visible behaviour to be genuinely
   configurable; a decorative control is a violation of it, not a partial implementation.

## Consequences

**Positive**

- The tier becomes a property of the system rather than of one code path, so a new entry point cannot
  silently escape it — new surfaces converge on the same check by construction.
- `SECURITY.md`'s claim becomes true rather than aspirational, and ADR-051/ADR-056's guarantees rest on
  something real.
- Fail-closed parsing removes a whole family of "typo widens access" bugs, not just the two found.

**Negative / Risks**

- Gating at the effect means the refusal surfaces later than a name-based gate would — a tool call is
  admitted, then denied when it reaches the effect. The error must therefore say plainly which tier was
  required and which was in force, or it will read as a malfunction rather than a policy decision.
- A deny-list of shell-capable primitives is only correct while it is complete. It is guarded by the
  enforcement test below rather than by vigilance; any new primitive that spawns a process must either
  join the gate or fail that test.

## Enforcement

A test that iterates every registered tool and asserts its declared `PermissionTier` is honoured on
**every** entry path — embedded session, MCP dispatch, `execute_command`, and `eval_scheme` — not only
on the MCP one. Following the precedent of
`crates/ai/src/executor/mod_tests.rs`'s existing whole-registry annotation test, and of
`every_registered_option_is_reachable_via_get_option`, the point is that the assertion is driven by the
registry rather than by a hand-written list, so it cannot fall behind what actually ships.

Paired with a test asserting that a process-spawning primitive is refused below the required tier,
including via the transitive command-enqueues-Scheme route.

## Alternatives considered

**Raise `eval_scheme` to `Shell`.** Simplest and most conservative, and rejected on cost: it makes a
`write`-tier session unable to evaluate any Scheme at all, including pure expressions, for a risk carried
by one primitive. It also does not address the transitive route on its own.

**Classify all ~204 Scheme primitives by tier.** Principled and disproportionate. The audit found one
primitive that spawns a process; a full classification is a large, permanently-maintained surface to
prevent a problem a one-entry gate prevents today. Revisit if a second such primitive appears.

**Leave the embedded session ungated on the grounds that the human invoked it.** Rejected: the human
invokes the session, but the *content* driving its tool calls routinely comes from sources the human did
not author — a cloned repository, a federated KB, fetched web content. The tier exists precisely to bound
what that content can cause, so exempting the path where it is most directly in play inverts the control.
