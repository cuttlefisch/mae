# ADR-091: MCP tool dispatch carries a session handle

**Status:** Accepted. Implements `docs/DECISIONS_FOR_REVIEW.md` decision #9 (2026-08-03).
**Extends:** ADR-051 (per-session permission policy + `DrivenWindow` isolation — this ADR
generalises the per-MCP-session record ADR-051 introduced from "window state" to "session state").
**Relates to:** ADR-085 (the "not offered, not offered-then-refused" principle this ADR applies to
three tools), ADR-086 (the outcome contract the new refusals honour), ADR-050 (external-editor MCP
pairing — the deployment shape that made this visible), ADR-088 (carried authority — explicitly
*not* what this is; see "What this is not").

## Context

Nine tools — `ask_user`, `delegate`, `ai_set_mode`, `ai_set_profile`, `ai_set_budget`,
`propose_changes`, `log_activity`, `read_transcript`, `web_fetch` — were registered in
`ai_specific_tools` and therefore appeared in an external MCP client's `tools/list`,
`search_tools`, and `request_tools` results. `ask_user` sat at the default **Core** tier, so it was
in the *first* list any paired external agent saw — VS Code Copilot, Claude Code through the shim,
v0.15's headline use case.

None of the nine were reachable through `crates/ai/src/executor/tool_dispatch.rs::dispatch_tool`.
An external `tools/call` for any of them fell through every branch to `Err("Unknown tool: {name}")`.

They worked for exactly one caller: MAE's own embedded `AgentSession`, which intercepts them inside
its own event loop (`crates/ai/src/session/handle_prompt.rs`) before dispatch is ever reached. It
can do that because it owns the state they touch — `self.transcript_path`, `self.budget`,
`self.current_mode`/`self.current_profile`, and for `ask_user`/`propose_changes` a
`tokio::sync::oneshot` that parks the session's task awaiting a human reply.

`dispatch_tool`'s signature was `(editor: &mut Editor, call: &ToolCall, requester_provider:
Option<&str>)`. **There was no session handle at all.** That is the actual defect: not nine
individually-forgotten tools, but a missing capability — *per-session state reachable from the MCP
dispatch path*, which did not exist for anything.

The defect was invisible to every existing test because each tool was individually correct
(registered, tiered, schema-checked by `dispatch_contract_tests`). The bug lived only in the
relationship between the registry and the dispatcher, and nothing asserted that relationship.

## Decision

**1. `dispatch_tool` gains a session handle, resolved from the MCP session id the dispatch scope
already receives.**

`Editor::with_ai_dispatch_scope_for_session` — the enforced boundary every MCP-originated dispatch
already routes through (issue #372, ADR-051) — records `ai.dispatch_session_id` for the extent of
the call, saved and restored exactly like the `ai_dispatch_depth` counter beside it. Tool
implementations then resolve their own session's state with `Editor::agent_session_mut()`:

- **`Some(sid)`** → the per-MCP-session record. Two connected agents have independent modes,
  profiles, budgets, activity logs, and transcripts.
- **`None`** → the process-wide record, which belongs to MAE's own embedded agent. For mode and
  profile the accessors additionally write through to the `ai_mode`/`ai_profile` options — exactly
  the effect `AiEvent::UpdateMode`/`UpdateProfile` produce when the embedded session runs the same
  tool. An MCP call must not be a weaker version of the same tool.

The state itself (`AgentSessionState`) lives in `McpSessionState` alongside ADR-051's window state,
as **one record per session** rather than one map per concern: the key, the lazy-population rule,
the coarse eviction bound (`MAX_TRACKED_MCP_SESSION_WINDOWS`), and the lifetime are all identical,
so two maps would be two things to keep in sync (principle #8).

Recording the id on the scope rather than threading a parameter through the ~13 category
dispatchers is deliberate. The scope is *already* the single enforced boundary and already
maintains per-dispatch state this way; adding a parameter that twelve of thirteen dispatchers
would immediately ignore buys nothing and makes the next such addition harder.

**2. Six tools become genuinely dispatchable** — `ai_set_mode`, `ai_set_profile`, `ai_set_budget`,
`log_activity`, `read_transcript`, `web_fetch` — routed through the new
`crates/ai/src/executor/session_exec.rs`.

`web_fetch` needs no session state at all; it was unreachable only because `dispatch_tool` is
synchronous (it holds a `!Send` `&mut Editor` on the main thread) while the session's
implementation is `async`. It gets a blocking transport, following `kb_enrich`'s existing
`reqwest::blocking` precedent. The **policy** — scheme allow-list, HTML stripping, 32 KB truncation
— is shared through `crates/ai/src/web.rs` rather than reimplemented, so the two transports cannot
drift on what they accept or how much they return.

**3. Three tools are withheld from every external discovery surface** — `ask_user`,
`propose_changes`, `delegate`. Making "pause and wait for a human reply" work mid-`tools/call` for
an external client is a UX question, not a wiring one, and nothing in this ADR answers it. Per
ADR-085's stated shape — *"the fix is that they are not offered, not that they are offered and then
refused"* — they are absent from `tools/list`, `search_tools`, and `request_tools` for external
callers, and remain fully available to the embedded agent, which is the one context where they
work.

This is a **capability** decision, not a permission one. An external client at `Privileged` tier
still cannot run them, because there is nothing on its side of the connection to run them with.

**4. The invariant that stops this recurring.**
`dispatchability::tests::no_advertised_tool_is_unroutable` asserts, over the live registry, that
nothing MAE advertises to an external MCP client is a tool `dispatch_tool` cannot route. Its
absence is why nine tools sat in `tools/list` for as long as they did.

It is source-text based (reading the routing chain) rather than dispatch-based, because actually
calling ~210 tools to see which answer `Unknown tool` would mean really running
`editor_save_state`, `kb_reimport`, `run_build`, and every `command_*` mirror including `quit`.
Same heuristic class as `dispatch_contract_tests`; a surprising result is a prompt to read the
dispatcher, not to add an exemption.

## What this is not

`dispatch_session_id` is **identity and routing, never authority**. It answers "which session is
calling", not "may it do this" — the tier and category gates in `execute_tool_dispatch_body` answer
that, and nothing here may be used to widen them. ADR-088 (carried provenance) remains the open
question about *authority*, and this ADR neither advances nor relaxes it.

## Consequences

- **Not closed: a running `AgentSession` cannot be reached from dispatch.** The embedded agent's
  task owns `self.budget`/`self.current_mode` by value on another thread, and `AiCommand` has no
  variant for either. So an MCP `ai_set_budget` with no session in scope updates the editor's
  record, and a *currently-streaming* embedded turn keeps its own copy until the next session
  build. Closing this needs new `AiCommand` variants and is deliberately out of scope: the
  external-client case (the point of decision #9) is fully served without it.
- **`read_transcript` over MCP is honest but usually empty.** MAE writes transcripts for its own
  agent sessions; it does not (yet) record one for an external MCP client, so the tool refuses with
  a message that says so rather than returning an empty success (ADR-086). Recording per-MCP-session
  transcripts is a natural follow-up and the field is already on the record.
- **Breaking for external clients that discovered the interactive three.** They previously got
  `Unknown tool` on call, so no working integration can regress — but a client that *listed* them
  will see three fewer tools. Worth a release note.
- **The per-session record now holds unbounded-ish data** (an activity log). Bounded at
  `MAX_SESSION_ACTIVITY_ENTRIES` per session, on top of ADR-051's existing bound on the number of
  tracked sessions.

## Verification

Adversarial-first per principle #14, in `executor/session_dispatch_tests.rs`:

- **N-way, not 2-way:** three concurrent sessions set mode/profile/budget/activity in *interleaved*
  order, and each is asserted to hold only its own values — a last-writer-wins bug cannot hide in a
  two-session test run to completion.
- **Leak oracles in both directions:** per-session writes must not move the editor-global option;
  the embedded path must.
- **Refusals leave no residue:** every rejected mode/profile is asserted not to have mutated the
  record, not merely to have returned an error.
- **The security-relevant half:** `web_fetch`'s scheme allow-list is exercised against
  `file://`/`data:`/`javascript:` on the new blocking path — a permissive check there would be a
  local-file read reachable from any Shell-tier MCP client — with a separate assertion that the
  refusal is validation and not the old `Unknown tool`.
- **Nesting:** an inner dispatch scope must restore the outer session's id, never clear it.
- **Both sides of the exclusion:** the three are absent from external `search_tools`/`request_tools`
  *and* still present for the embedded agent. A filter applied unconditionally would silently
  disable `ask_user` for the human's own agent — a regression dressed as a fix.
