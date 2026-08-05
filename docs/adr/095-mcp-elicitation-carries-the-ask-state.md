# ADR-095: MCP elicitation carries the Ask state to external clients

**Status:** Proposed.
**Extends:** ADR-090 (permission decisions are three-state — this ADR delivers its `Ask`
state to the one surface that still maps `Ask` to a denial, closing the first of that ADR's
two *Still open* items).
**Relates to:** ADR-051 (per-session permission ceilings — the existing client-declared
mechanism, which runs in the opposite direction and whose bound this must not cross),
ADR-056 (tool-category allowlist — the other narrowing declaration), ADR-091 (session handle
for MCP tool dispatch — where the per-session state this needs already lives), ADR-084
(enforcement at the effect).
**Evidence:** `docs/AI_AGENT_FRICTION_AUDIT.md` §C, from an external agent session that
dead-ended on this path.

## Context

ADR-090 established that a permission check returns `Allow`, `Ask`, or `Deny`, and that
"every surface implements `Ask`, or declares itself non-interactive and maps `Ask` to
`Deny`." Its D3 was explicit that a surface must never treat `Ask` as `Allow`, and that
mapping to `Deny` is correct **when there is nobody to ask**.

Four of five surfaces are right. Following `ExecuteResult::NeedsApproval`
(`crates/ai/src/executor/tool_dispatch.rs:121-129`) to every non-test consumer:

| Surface | Site | Behaviour |
|---|---|---|
| `mae-agent` TUI | `crates/agent-cli/src/main.rs:508` | prompts `y`/`a`/`n` |
| Embedded session | `crates/ai/src/session/handle_prompt.rs:45` | prompts via `ConfirmToolCall` |
| Embedded race guard | `crates/mae/src/ai_event_handler.rs:264` | denies — a mid-turn policy race; nothing ran |
| `mae --self-test` | `crates/mae/src/terminal_loop.rs:822` | denies — headless by definition |
| **External MCP** | `crates/mae/src/ai_event_handler.rs:1223` | **denies** |

The external MCP denial is the one case where a human *is* present — sitting in front of
VS Code, Claude Code, or another paired editor — and the answer is still a hard denial.

The justification is recorded in two places. `SECURITY.md:50` and
`docs/EXTERNAL_EDITOR_MCP_PAIRING.md:324` both say: *"MAE implements no MCP elicitation, and
the requesting client is not the local human."* ADR-090 lists "An interactive `Ask` for
external MCP" under *Still open*, judging that resolving one "needs `all_tools` + the policy
at the keypress site, i.e. the pending-op-applied-in-the-event-loop pattern across all three
loops."

**The first half of that justification is now falsifiable, and the second half is smaller
than it looks.**

- MAE speaks MCP **`2025-11-25`** (`shared/mcp/src/protocol.rs:11`) — the revision that
  specifies elicitation — and accepts four revisions back (`:15`).
- Clients declare the capability. Claude Code's real handshake, captured verbatim as a
  regression fixture at `shared/mcp/src/lib.rs:3116`, sends
  `"capabilities": {"roots":{}, "elicitation":{"form":{},"url":{}}}` — matching the shape the
  specification requires.
- **MAE discards it.** The server's `initialize` handler (`shared/mcp/src/lib.rs:767-832`)
  reads `protocolVersion`, `clientInfo`, `declaredProvider`, `permissionCeiling`, and
  `toolCategoryAllowlist`, and never reads `capabilities`. `ClientSession`
  (`shared/mcp/src/session.rs:30-52`) has no field to hold it.

So MAE already parses four self-declared client parameters on this exact code path, and
already trusts two of them (`permissionCeiling`, `toolCategoryAllowlist`) from any client
precisely because they can only *narrow* authority. The client states, in the same object,
that it can carry a prompt to its human — and that one field is dropped.

The result is one-directional: a client can restrict itself and can never be asked to
approve. There is no agent-facing counterpart either; `request_tools` grants tool *schemas*,
not permission.

This is not free. Because `Ask` cannot reach this path, a working paired-editor deployment
must pre-authorize statically — `docs/EXTERNAL_EDITOR_MCP_PAIRING.md:326` says a paired
deployment "needs an explicit `auto_approve_tier`." Combined with the audit's §B1 (the
generated config template still advertising `shell`), the path of least resistance for a new
user is a permanently elevated MCP session. ADR-090 rejected exactly this outcome when it
declined to drop the default tier without an `Ask` state.

## Decision

**Negotiate `capabilities.elicitation` at `initialize`, and use it to carry `Decision::Ask`
to clients that declared they can be asked.**

1. **Parse and persist the capability.** `initialize` reads `params.capabilities.elicitation`
   and stores the declared modes on `ClientSession`, alongside the existing
   `declared_permission_ceiling` and `declared_tool_categories`. Per the specification, an
   empty `elicitation: {}` object means form mode only.

2. **`Ask` becomes an elicitation, not a denial — only when declared.** At
   `ai_event_handler.rs:1223`, a session that declared form-mode elicitation receives an
   `elicitation/create` request instead of `into_denied(MCP_SURFACE)`. A session that
   declared nothing keeps today's behaviour verbatim. The specification requires this:
   *"Servers MUST NOT send elicitation requests with modes that are not supported by the
   client."* Sending an undeclared mode is a client-side `-32602`.

3. **Form mode, boolean schema, and never URL mode.** The request carries a `message` naming
   the tool and the tier it needs, and a `requestedSchema` containing a single boolean. URL
   mode is not used and must not be: the specification reserves it for out-of-band flows and
   forbids form mode for credentials, neither of which applies to an approve/deny question.

4. **All three response actions map to existing outcomes.** The specification defines
   `accept`, `decline`, and `cancel`. `accept` with a true value applies
   `PermissionPolicy::with_one_time_approval(tier)`; `accept` with false, `decline`, `cancel`,
   and a timeout all resolve to today's denial via the existing `timeout_deferred_mcp_reply`
   path. Refusal is an ordinary outcome, not an error.

5. **The reply is parked, not blocked.** Use the existing `deferred_mcp_reply` mechanism
   (`crates/mae/src/headless_loop.rs:199,290,328`), which already parks MCP replies for
   LSP/DAP round-trips. This ADR adds a new *kind* of parked reply, not a new mechanism —
   which is what makes it materially smaller than ADR-090's estimate.

## Consequences

**Positive.** Removes the last surface where `Ask` degrades to `Deny` despite a human being
present. Makes a restrictive `auto_approve_tier` viable for paired editors, so operators stop
being pushed toward blanket pre-authorization. Strictly additive: clients that do not declare
elicitation are byte-for-byte unaffected, so no config change and no migration.

**Negative / Risks.**

*Ask is not a security control.* ADR-090 says so and this ADR repeats it: Anthropic reports
users approve **93%** of permission prompts, and MCP's own guidance warns that prompts
"aren't enforcement." This makes a restrictive default *affordable*; it does not harden
anything, and must never be described as doing so.

*Whose consent it is.* ADR-090's second objection — "the requesting client is not the local
human" — is not fully dissolved, and the specification agrees: *"Servers MUST NOT rely on
client-provided user identification without server verification, as this can be forged."* An
`accept` therefore proves that **someone with control of the client consented**, not who they
were. That is strictly more than today's denial establishes, and strictly less than the
embedded session's keypress. The ADR takes the position that this is sufficient for moving
the *auto-approval* line — which is all it can move — and insufficient for anything else,
which is why D4 forbids it from touching the hard ceiling.

*A new prompt-fatigue surface.* A misconfigured client could be asked frequently. The
specification's `-32602` and client-side rate limiting bound this, but a per-session
"approve always" is deliberately **not** proposed here: `ApproveAlwaysThisSession` is still
treated as one-time in `mae-agent`, and ADR-090 records that a real per-session allowlist is
its own follow-up. Doing it here would be a second decision wearing this one's clothes.

*Elicitation is a request from server to client*, so the transport must support
server-initiated messages on an established session. This is already true for the paths that
carry `notifications/*`, but it should be confirmed for every transport MAE ships before
implementation, not assumed.

## Alternatives considered

**A `request_permission` tool the agent calls.** Rejected. A tool call cannot authorize
itself: the request would travel through the very channel being gated, and would have to be
dispatched at some tier before it could ask for one. It also invites the model to treat
elevation as a routine step rather than an exception. Elicitation is server-initiated by
design, which is the correct direction for a question the *server* needs answered.

**Tell operators to raise `auto_approve_tier`.** This is the status quo
(`EXTERNAL_EDITOR_MCP_PAIRING.md:326`), and ADR-090 already rejected the general form: "the
predictable result is that users set `auto_approve_tier = "shell"` in config — restoring the
same posture while adding the false comfort of a configured value." The audit's §B1 shows
MAE's own generated template pushing users there today.

**Implement `Ask` by blocking the MCP dispatch thread on a local keypress.** Rejected: it
asks the wrong human. The person at the paired editor made the request; the person at the
MAE window may not be present at all, and on a headless daemon there is no window. It also
reintroduces the cross-loop keypress problem ADR-090 flagged, which parking the reply avoids.

**Do nothing until ADR-084 D7 lands.** Rejected as an ordering error: D7 concerns making the
`ai_tier` option reach the enforced policy — a *live-mutable policy* problem. This ADR needs
only a per-session, per-call one-time approval, which `with_one_time_approval` already
provides. They are independent, and this one is not blocked.
