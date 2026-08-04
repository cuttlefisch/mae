# ADR-090: Permission decisions are three-state (allow / ask / deny)

**Status:** Accepted and implemented (v0.15). Decisions 1-5 are all in the tree; see
*Implementation notes* at the end for where each landed and what is still open.
**Extends:** ADR-084 (enforcement at the effect — this ADR supplies the *decision* vocabulary
that enforcement returns), ADR-049 (`mae-agent` as the default AI surface — which already has
half of this and is the model for the rest).
**Relates to:** ADR-085 (the category axis), ADR-051 (per-session ceilings), ADR-088 (carried
authority).
**Tracking:** issue #592 (pre-v0.15 audit epic). Fixed in v0.15; see ADR-084 on why this was
never disclosed as an advisory.

## Context

ADR-084 D4 and ADR-085 both concluded that MAE's permissive default tier is a root cause worth
fixing: the shipped posture is *all categories × shell tier*, so the tier gate admits essentially
everything unless an operator opts into less. Both ADRs treated lowering it as a decision about
appetite for a breaking change.

Implementing D4 established that it is not. It is blocked on a missing mechanism.

**`PermissionPolicy` describes a three-state model that was never built.** The type's own doc
comment reads *"Policy for auto-approving or prompting for tool calls"*, and its field is
`auto_approve_up_to`: *"Maximum tier that is auto-approved **without user confirmation**"*
(`crates/ai/src/tools/categories.rs:238-242`). The name and the documentation both promise
escalation — approve below the ceiling, *ask* above it.

The dispatch path does not ask. `crates/ai/src/executor/tool_dispatch.rs:98` hard-denies:

```rust
    if !policy.is_allowed(permission) {
        return ExecuteResult::Immediate(ToolResult {
            /* … */ success: false,
            output: format!("Permission denied: {} requires {:?} tier", call.name, permission),
        });
    }
```

There is no pending-approval state, no prompt, and no path from a denial back to an allowed
execution. `is_allowed` is a binary ceiling.

**This is why the default is permissive.** With only allow and deny available, any default below
Shell hard-denies `shell_exec`, `run_build`, and `run_test` outright — the AI simply stops being
able to build or test. Setting the default to Shell was the only way to keep the product usable.
The fail-open default is therefore a *symptom*: the missing ask state forces a choice between a
safe default and a working editor, and usability won.

**Half the mechanism already exists, in the wrong crate.** `mae-agent`
(`crates/agent-cli/src/tui/confirm.rs`) has a genuine three-state model: `needs_confirmation(tier,
mode)` returns true when `tier > ceiling`, and the TUI renders a y/n/always prompt. Its own header
says *"Genuinely net-new: no human-in-the-loop permission-approval UI exists anywhere in MAE
today"*. So MAE's **default AI surface** (ADR-049) can ask, while the embedded and MCP dispatch
paths cannot — two permission implementations with different semantics for the same tiers.

The prior-art review found this to be the near-universal shape elsewhere. Claude Code evaluates
**deny → ask → allow**. Windsurf/Devin scope **Deny > Ask > Allow** per capability, with
*"If no rule matches, you're prompted for approval"* as the fallthrough. OpenHands defaults its
interactive CLI to *"Always ask for confirmation"* on every action. MCP's own security guidance
prescribes a minimal initial scope plus *"incremental elevation … when privileged operations are
first attempted"*. Every one of them can afford a restrictive default precisely because
restriction means *ask*, not *fail*.

## Decision (proposed)

**A permission check returns one of three outcomes, and every surface can represent all three.**

1. `PermissionPolicy` gains an explicit `Decision { Allow, Ask, Deny }` returned by the check, in
   place of the current boolean. `auto_approve_up_to` keeps its meaning — the ceiling below which
   the answer is `Allow` — and stops being a synonym for `Deny` above it.

2. **`Deny` is reserved for what policy forbids outright**, not for what merely exceeds the
   auto-approval ceiling. A session-declared ceiling (ADR-051), a category restriction (ADR-085),
   and an unparseable configuration all produce `Deny`. Exceeding `auto_approve_up_to` produces
   `Ask`.

3. **Every surface implements `Ask`, or declares itself non-interactive and maps `Ask` to `Deny`.**
   Mapping to `Deny` is the correct behaviour for a headless/`--prompt` run — `mae-agent` already
   does exactly this and says so (*"Tool calls exceeding this tier are denied, not confirmed (no
   human to ask)"*). What must not happen is a surface silently treating `Ask` as `Allow`.

4. **The confirm logic is consolidated, not reimplemented.** `mae-agent`'s `needs_confirmation` /
   `PermissionMode` is the third parallel tier vocabulary in the tree (after `config.rs`'s
   lowercase config spellings and `ai_event_handler.rs`'s CamelCase wire spellings, whose own doc
   comment already concedes it is "kept small and duplicated"). Per principle #8 the decision
   belongs in one place with surface-specific presentation around it — adding a fourth would be
   precisely the shape principle #15 forbids.

5. **Only once `Ask` exists does the default tier drop.** The target is a default where read
   operations are allowed, writes and shell are asked, and nothing is silently denied — matching
   Devin Local's documented default (*"read-only operations are auto-approved while writes and
   shell commands require your explicit approval"*).

## Consequences

**Positive.** Removes the forced choice between a safe default and a usable editor, which is what
has kept the fail-open default in place. Makes `auto_approve_up_to`'s name and documentation true.
Collapses two divergent permission implementations into one.

**Negative / Risks.** `Ask` requires a UI on every interactive surface — the embedded editor path
has none today, and building one is the bulk of the work. Prompts are also weak on their own:
Anthropic reports users approve **93%** of permission prompts, and MCP's own retrospective warns
that annotations and prompts *"aren't enforcement"*. `Ask` is therefore a usability mechanism that
makes a restrictive default affordable — not a security control in its own right, and it must not
be described as one.

## Alternatives considered

**Lower the default tier without an ask state.** Rejected: it converts a security weakness into a
functional regression, and the predictable result is that users set `auto_approve_tier = "shell"`
in config — restoring the same posture while adding the false comfort of a configured value.

**Leave the default permissive and rely on operators to restrict.** This is the status quo, and
the audit is evidence against it: the default is what nearly everyone runs, and ADR-085's finding
shows even a deliberately-restricted operator got shell execution.

**Give only `mae-agent` the ask state (i.e. do nothing).** Rejected: it is already true, and it
means MAE's security posture depends on which surface the user happens to be on, with the external
MCP path — the one ADR-051/056 argue is the only real boundary — being the weakest.


## Implementation notes (v0.15)

**Where the decision lives.** `crates/ai/src/tools/decision.rs` defines `Decision { Allow, Ask,
Deny(DenyReason) }`; `PermissionPolicy::decide(tool_name, tier)`
(`crates/ai/src/tools/categories.rs`) is the single PDP. `is_allowed` is gone — a bool cannot
carry three states, and leaving it would have let a call site keep the old semantics.

`PermissionPolicy` gained `hard_ceiling: Option<HardCeiling>` to carry D2: a session-declared
ceiling (ADR-051) and an unparseable declaration (ADR-084 D4) set it and produce `Deny`; the
config/env `auto_approve_tier` sets only `auto_approve_up_to` and produces `Ask` above it.

**How `Ask` reaches surfaces.** `ExecuteResult` gained `NeedsApproval(ApprovalRequest)`. Because
every existing `match` on it was exhaustive, adding the variant made `rustc` name every surface that
had to decide — the same Capsicum technique ADR-084 D3 uses for the Scheme registration sites.

| Surface | `Ask` |
|---|---|
| `mae-agent` TUI | prompts (`y`/`a`/`n`) — the pre-existing overlay, now driven by `decide` |
| Embedded session (`:ai`, `delegate()`) | `AgentSession::decide_and_present` emits `AiEvent::ConfirmToolCall`; the human answers with `:ai-accept`/`:ai-reject` (reusing `PendingInteractiveEvent`, not a new mechanism) |
| `mae-agent --prompt` | denies via `ask_denied_message` |
| External MCP dispatch | denies via `ApprovalRequest::into_denied(MCP_SURFACE)` |
| `mae --self-test` | denies, likewise |

A human approval is carried back as `AiEvent::ToolCallRequest { approved_tier }` and applied with
`PermissionPolicy::with_one_time_approval(tier)`, which raises **only** the auto-approval ceiling.
The hard ceiling and the category allowlist survive it, so an approval — or a forged `approved_tier`
— can never promote a `Deny` (`approval_can_never_promote_a_deny`).

**D4 (consolidation).** `PermissionTier::parse` + `PermissionTier::VALID_SPELLINGS`
(`crates/ai/src/types.rs`) are now the one tier vocabulary. `mae::config::parse_permission_tier` is
a thin alias; `mae-agent`'s `PermissionMode` enum and `needs_confirmation` are **deleted**, replaced
by `policy_for_mode(&str) -> Option<PermissionPolicy>`. `confirm.rs` is presentation only. Two
latent bugs fell out of the merge: the config parser was case-sensitive (so ADR-051's CamelCase wire
values `"ReadOnly"`/`"Privileged"` never actually parsed — they were silently taking the
unparseable-declaration path), and `mae-agent`'s `FullAuto` mode was redundant with a `Privileged`
ceiling.

**D5 (the default).** `PermissionPolicy::default()` is now `ReadOnly`, and
`resolve_permission_policy` takes its built-in default *from* that value rather than repeating a
literal — so `mae`, `mae-agent`, and the embedded session cannot disagree about "unconfigured".
Breaking change; recorded in `SECURITY.md` and the release notes.

**ADR-084 D2/D7, partly closed alongside.** `AgentSession` now carries a `PermissionPolicy`
(threaded from `bootstrap::setup_ai`; `delegate()` sub-agents inherit the parent's verbatim), and
`drain_pending_scheme_evals` takes an `ambient_tier` and wraps evaluation in
`SchemeRuntime::with_ambient_tier` — so D3's per-primitive tiers stop being inert. The tier passed
is `PermissionPolicy::ambient_scheme_tier()`, deliberately the *`Allow`* line and not the hard
ceiling: guest Scheme cannot be prompted mid-evaluation, so anything merely askable must not be
ambiently granted. The human keypress path passes `HUMAN_AMBIENT_TIER` (`Privileged`) — the user
already has a shell, and bounding their own keystrokes by the AI's policy would be nonsense.

**Still open.**

- **ADR-084 D7's other half**: the `ai_tier` editor option still only paints the status bar. Making
  it reach the enforced policy means a live-mutable policy shared between the main thread and the
  spawned `AgentSession` task, which is a different change from this one.
- **An interactive `Ask` for external MCP.** The mechanism exists —`deferred_mcp_reply` already
  parks replies for LSP/DAP — but resolving one needs `all_tools` + the policy at the keypress site,
  i.e. the pending-op-applied-in-the-event-loop pattern across all three loops (terminal, GUI,
  headless). Until then a paired external editor must set `auto_approve_tier` explicitly, which is
  documented in `docs/EXTERNAL_EDITOR_MCP_PAIRING.md`.
- **`ApproveAlwaysThisSession`** in `mae-agent` is still treated as a one-time approve. A real
  per-session allowlist is a deliberate follow-up, not an oversight.
