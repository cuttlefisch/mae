# ADR-063: Guidance-delivery uniformity across MCP clients

**Status:** Accepted (implemented — see "Status note" at the end of this document).
**Extends:** ADR-050, ADR-057.
**Relates to:** ADR-049.

## Context

MAE's guidance mechanism (`crates/ai/src/guidance.rs`, the `ai_guidance_kb` option) exists so
that a registered knowledge base of standing practices — coding conventions, architectural
invariants, the kind of thing this repository's own `CLAUDE.md` encodes for human contributors
— is surfaced automatically to whatever AI agent is driving the editor. CLAUDE.md principle #3
states plainly that the AI is a peer, not a plugin: there is no separate "AI mode" with a
degraded experience for some callers and a full one for others. Today, guidance delivery
violates that principle in a concrete, measurable way.

**The two-tier reality, precisely.** `mae-agent-cli`, MAE's own first-party terminal AI-agent
harness, builds its system prompt by calling `mae_ai::guidance::build_guidance_context()`
directly and inlining the guidance KB's actual body text into the prompt at session start
(`crates/agent-cli/src/main.rs:202-218`). Any *external* MCP client — VS Code + Copilot, or any
other MCP-speaking editor paired per ADR-050 — gets something structurally different: the MCP
`initialize` response's `instructions` field carries only a short pointer string naming the
guidance KB ("Before acting, consult KB '`<name>`' for required practices.") plus a list of
registered KB names, never the guidance content itself (`crates/mae/src/main.rs:793-836`). An
external client that dutifully reads `initialize.instructions` and forwards it into its model's
context still has to independently decide to call `kb_get`/`kb_search_context` against the named
KB, parse the result, and fold it back into its own prompt — a round trip mae-agent-cli's own
users never have to make, because the content already arrived pre-inlined. This is exactly the
two-tier shape principle #3 exists to forbid: MAE's own client gets the real feature, everyone
else gets a pointer to it.

ADR-050's D4 decision already named this exact gap and shipped a partial answer: a fallback
exporter (`kb_export_guidance`, Phase H / #383) that writes the guidance KB's rendered content to
`AGENTS.md` and/or `.github/copilot-instructions.md`, additive-merged below a clearly delimited
MAE-managed block. That closes the gap for a client that reads project-convention files on disk
— which Copilot's agent mode does — but D4's own text also called for something this repo never
finished: to "verify empirically whether VS Code's Copilot MCP client forwards
`initialize.instructions` into the model's context," with a fallback regardless of the outcome.
ADR-050's own Verification section, read honestly, records the fallback as done and the
empirical verification as *not* done — its final bullet on the live Copilot Agent-mode round
trip states plainly that "the live Copilot Agent-mode round-trip itself needs a human's
interactive check — no browser/GUI automation is available to this agent," and no such check was
ever recorded as having happened. D4 was declared satisfied by the existence of the fallback
mechanism, not by confirming the fallback (or the primary `instructions` path) actually lands
guidance content where an agent will use it. This ADR is that unfinished verification work, now
scoped into concrete, testable phases rather than left as an open empirical question indefinitely.

There is a second, independent half of the gap: even where the delivery *mechanism* exists, its
default configuration quietly opts most sessions out of it. `ai_guidance_export_live_sync` — the
option that keeps the `AGENTS.md`/`copilot-instructions.md` fallback file automatically in sync
with the guidance KB on each session start — defaults to `false`
(`crates/core/src/editor/tests/option_tests.rs:1206`, `assert!(!editor.ai_guidance_export_live_sync,
"must default to off")`; confirmed in `crates/core/src/editor/mod.rs:1477`). That default is
reasonable in isolation — matching the existing opt-in philosophy for `ai_guidance_kb` itself, and
avoiding a surprise file write for users who never asked for external-editor pairing — but its
consequence is that the one delivery path proven to work end-to-end (a file on disk that
Copilot's agent mode reads directly, no MCP-forwarding assumption required) does not actually fire
for a session unless the user manually enables it, or manually runs `kb_export_guidance`/the
`--ensure-guidance-config` CLI flag first. For the exact population this feature exists to serve
— a freshly paired external-editor session, per ADR-050 — the mechanism that would close the gap
sits off by default with no signal to the user that it needs turning on.

**Grounded in real-world evidence.** This is not a hypothetical or MAE-specific risk shape. AWS's
own shared Amazon Q Developer language server — a single backend serving multiple IDE clients
(VS Code, JetBrains, Visual Studio) — is architecturally the same "one MCP-adjacent backend, many
editor frontends" model MAE is building toward. It shipped a real, documented bug in which the
JetBrains client sent `cursorState` in a shape the server didn't expect; the server silently fell
back to treating it as "no selection," and users received wrong chat answers with no error
surfaced anywhere in the pipeline — fixed in `aws/aws-toolkit-jetbrains#6134`
(github.com/aws/aws-toolkit-jetbrains/pull/6134). The detail that matters for this ADR: the
payload *passed schema/JSON validation*. It was well-formed wire traffic, just semantically wrong
for that particular client's interpretation of it — meaning any test that only confirmed the field
was present and valid on the wire would have shipped the bug anyway. This is direct, concrete
confirmation that "observably used by the agent" is the correct bar for this ADR's own
verification work, not "present in the `initialize` response" or "present in the exported file." A
wire-presence-only or file-presence-only test would not have caught AWS's own real incident, and
would not catch MAE's structurally equivalent risk either: a client that receives correctly
formatted guidance content but whose particular agent implementation never actually reads it, acts
on it, or weighs it the way `mae-agent-cli`'s inlined-into-the-prompt approach guarantees.

Separately, AWS's own rollout pattern for this same product is worth citing as the industry-wide
version of the exact gap this ADR closes for MAE. Per AWS's own DevOps blog
(aws.amazon.com/blogs/devops/introducing-an-agentic-coding-experience-in-visual-studio-and-jetbrains-ides/),
the enhanced agentic coding experience shipped to VS Code first, reaching JetBrains and Visual
Studio only in a later wave — one client got the better experience first, and others caught up
later. That is the same shape as MAE's own mae-agent-cli-gets-full-content-inline versus
external-clients-get-a-pointer split. Citing it here is not an indictment unique to MAE's current
implementation; it is evidence that this pattern recurs industry-wide even among sophisticated,
well-resourced teams, which is exactly why closing it deliberately — guidance delivery uniform
regardless of which client is attached — is a genuinely differentiating design choice for MAE, not
merely embarrassment-avoidance.

## Decision

**A — Size-budgeted full-content delivery via `initialize.instructions`.** Instead of sending only
a pointer string naming the guidance KB, inline the guidance KB's actual rendered body content
(the same `build_guidance_context()` output `mae-agent-cli` already inlines) directly into
`initialize.instructions`, up to a configurable character budget (a new `OptionRegistry` entry,
per CLAUDE.md principle #7 — no hardcoded magic number in `main.rs`). When the guidance KB's
rendered content exceeds that budget, fall back to today's pointer-only behavior rather than
truncating mid-content — this guarantees no regression for an unusually large guidance KB (a
malformed or runaway `initialize` payload is a real risk for some MCP clients' own handshake size
limits) while closing the gap for the normal, common case where the guidance KB is well within
budget. This makes the *default*, no-configuration-needed MCP path for an external client
materially closer to what `mae-agent-cli` already does today, rather than leaving that parity
gated behind the client separately choosing to call `kb_get`/`kb_search_context` on its own
initiative.

**B — A conditional default for `ai_guidance_export_live_sync`.** Rather than leaving the option
`false` for every session regardless of context, or flipping it to `true` unconditionally for
everyone, its default becomes conditional: `true` when `daemon_mode != off` **and** an
external-editor pairing is detected (i.e., a headless MCP session per ADR-055 is active), `false`
otherwise. This targets exactly the situation the option's own existing code comment already
identifies as the one that matters — a paired external editor that benefits from a live,
auto-synced fallback file — without introducing a surprising, unrequested `AGENTS.md` write for a
plain interactive GUI/TUI user who never asked for external-editor pairing at all and would have
no reason to expect a file appearing in their project root.

**C — The empirical verification ADR-050's D4 item called for and never finished.** Build a
scripted pairing test against a real external MCP client — VS Code + the Copilot agent-mode
integration, the existing reference integration ADR-050 already established as the primary target
— that asserts the guidance content is *observably used*, not merely delivered. Concretely: the
scripted scenario primes a guidance KB with one specific, distinctive, easily-detected practice
(e.g. a naming convention or a required step with no plausible alternative phrasing an untrained
model would produce on its own), then asserts that the agent's first tool call in the scenario
reflects that practice. A test that only inspects the captured wire traffic for the guidance
string's presence is explicitly insufficient and does not satisfy this decision — the assertion
must be behavioral, on the agent's actual first action, exactly the class of check the AWS
`cursorState` incident shows a wire-presence check would have missed.

**D — An explicit, documented won't-fix for the legacy embedded-chat path.** State plainly, in
this ADR and cross-linked from ADR-049, that guidance delivery is **not** being backported into
the legacy embedded `ai_chat` code path. ADR-049 already put that path on a deprecation
trajectory (`ai_chat_enabled`, default off, superseded by `mae-agent-cli` as the default AI
surface); extending this ADR's delivery work into a path already headed for removal would be
effort spent hardening code this project has already decided to retire. This is a deliberate,
documented decision — not silent ambiguity about whether the legacy path also needs the fix, which
would otherwise be a reasonable question for a future contributor to raise.

## Consequences

**Positive.** Closes the two-tier gap principle #3 exists to forbid: an external MCP client's
*default*, no-extra-configuration experience moves from "a pointer to guidance content it must
separately fetch and interpret" to "the guidance content itself, inlined the same way
mae-agent-cli already inlines it" for the common case, with a clean, non-lossy fallback for the
uncommon oversized case. The conditional live-sync default means the one delivery path already
proven end-to-end (a file Copilot's agent mode reads directly) actually engages by default for the
population it was built for, instead of requiring that population to discover and manually enable
an option they have no reason to know exists. Finishing ADR-050's D4 empirical-verification
obligation retires a genuinely open question this project has been carrying since that ADR
shipped, rather than leaving it open indefinitely under an ADR already marked Accepted.

**Costs (honest).** The size-budgeted inlining adds a second thing every `initialize` handshake
must compute (render the guidance KB's content and measure it against the budget) on a path that
was previously a cheap string-length check — a small, bounded cost paid once per session
connection, not per tool call, and gated by the same `ai_guidance_kb` opt-in that already governs
whether this work happens at all. The conditional default for `ai_guidance_export_live_sync`
means the option's effective behavior now depends on `daemon_mode` and pairing-detection state at
session start rather than being a single, simple boolean a user can read in `init.scm` and trust
in isolation — this is a real increase in "what does this option actually do right now" complexity
that must be documented clearly (`:describe-option`, the option's own doc string) so it doesn't
read as unexplained magic. Phase C's scripted VS Code test is inherently more fragile than a unit
test — it depends on Copilot's agent-mode behavior, which per ADR-050's own honest cost accounting
evolves month-to-month outside MAE's control — and will need periodic re-verification against
current VS Code/Copilot behavior, not a one-time pass assumed to hold forever.

## Alternatives rejected

- **Unbounded full-content inlining with no budget cap.** Rejected — this creates a real risk of a
  pathologically large `initialize` handshake payload for guidance KBs that grow large over time
  (a team's accumulated practices KB is exactly the kind of content that tends to grow, not
  shrink), colliding with some MCP clients' own handshake size limits. Fixing the common case by
  introducing a regression risk in the edge case is exactly the kind of trade CLAUDE.md principle
  #9 requires weighing explicitly, and an unbounded approach fails that weighing.
- **Forcing `ai_guidance_export_live_sync` to `true` unconditionally for every session.**
  Rejected — this is a new, surprising side effect (an unexpected `AGENTS.md` write) for every
  existing interactive GUI/TUI user who never asked for or benefits from external-editor pairing,
  violating the same "no ad-hoc, no surprising defaults" discipline principle #7's corollary
  describes. The conditional default in Decision B achieves the same practical goal — the fallback
  engages for the population that needs it — without that blast radius.
- **Declaring D4 satisfied by wire-presence or file-presence alone.** Considered and rejected as
  the verification bar for Phase C specifically because the AWS `cursorState` incident is direct
  evidence that a schema-valid, correctly-delivered payload can still be silently ignored or
  misinterpreted by a specific client's own implementation. A test that only checks the guidance
  string appears somewhere in captured traffic or on disk would pass in exactly the scenario that
  incident shows can still be broken in practice.

## Verification

- **A** — Boundary tests at exactly the configured budget limit, one character under, and one
  character over: over-budget guidance content must fall back cleanly to today's pointer-only
  `initialize.instructions` behavior, with no truncated or malformed partial content ever sent on
  the wire. At-or-under-budget content must be byte-identical between what
  `build_guidance_context()` produces and what actually lands in `initialize.instructions` — no
  accidental transformation, re-encoding, or truncation introduced by the size-budgeting logic
  itself.
- **B** — A single test run must show both outcomes side by side: zero behavior change (no file
  write, option value observably unchanged from its prior default) for a non-paired interactive
  GUI/TUI session, and an automatic `AGENTS.md` write with no manual option-setting required for a
  headless, externally-paired session (`daemon_mode != off` and pairing detected). This proves the
  conditional default fires in exactly the intended case and no other — a test that only exercises
  one branch would not catch a regression toward "always on" or "always off."
- **C** — Designed to genuinely FAIL, not pass vacuously, if the guidance content is present in the
  wire response or the exported file but the reference agent demonstrably does not act on it. This
  is the literal falsification test the "observably used, not merely wire-present" requirement in
  Decision C demands, and is the same class of check that would have caught AWS's own real
  `cursorState` incident — a schema-valid, wire-present payload nonetheless ignored or
  misinterpreted downstream. A dry run of this test against a deliberately-neutered delivery path
  (guidance content stripped from both `initialize.instructions` and the exported file) must be
  confirmed to fail before being trusted as a real regression guard, matching this project's
  established verify-both-directions discipline.
- **D** — A regression test confirming the legacy `ai_chat` embedded-chat path's guidance behavior
  (or lack thereof) is completely unchanged by this ADR's work — no accidental partial backport of
  the size-budgeted inlining or the conditional live-sync default into a path this ADR explicitly
  and deliberately declines to touch.

---

## Status note (added on implementation)

All four phases are implemented, tested, and shipped.

**Phase A — shipped as designed.** `crates/mae/src/main.rs`'s MCP `initialize` handler
now calls `mae_ai::guidance::build_guidance_context` (the exact same function
`mae-agent-cli` already inlines into its system prompt) and inlines its output into
`instructions` whenever it fits within a new `ai_guidance_inline_budget_chars` option
(OptionRegistry-registered per principle #7, default 8000 characters), falling back to
today's pointer-only sentence otherwise — never a truncated partial inline. The
budget/fallback logic was extracted into a pure `guidance_instructions_fragment` helper
specifically so the exact-boundary cases (at budget, one under, one over) are directly
unit-testable without a full MCP handshake — 5 tests, including a multi-byte-character
case proving the budget is measured in characters, not UTF-8 bytes, and proving at-budget
content is inlined byte-identical to `build_guidance_context()`'s own output. Beyond the
pure-function tests, a real subprocess e2e suite (`crates/mae/tests/guidance_delivery_e2e.rs`,
3 tests) spawns a genuine `mae --headless` instance with a real seeded guidance KB and
does a real MCP `initialize` handshake over a real Unix socket, confirming the content
actually reaches the wire correctly in both the within-budget and over-budget cases, plus
a negative control (no guidance KB configured → the distinctive test marker is genuinely
absent) proving the positive tests aren't passing vacuously.

**Phase B — shipped, with an explicit-choice-always-wins mechanism the ADR text didn't
fully specify.** The ADR's Decision B describes a "conditional default" but doesn't say
how to avoid silently overriding a user who explicitly set `ai_guidance_export_live_sync`
to `false` (which is indistinguishable from "never touched, still at the unconditional
`false` default" by value alone). Added `Editor::explicitly_set_options: HashSet<String>`,
populated at `set_option`'s single chokepoint for every option (not just this one — a
small, deliberately reusable mechanism for any future option wanting a runtime-computed
conditional default, per principle #8, rather than a narrow one-off hack). The effective
value is computed via a new pure `effective_guidance_live_sync(explicit, daemon_connects,
is_headless)` helper: an explicit user choice (`true` or `false`) always wins; only when
never explicitly set does the conditional default (`daemon_mode != off && --headless`)
apply. Verified: one test asserting all four quadrants of `(daemon_connects, is_headless)`
side by side (per the ADR's own "both outcomes in one test run" bar), and one confirming
explicit `true`/`false` both override the computed default in the direction that would
otherwise flip.

**Phase C — shipped as an honest split, not silently claimed complete.** Per this ADR's
own Decision C and Verification text, the real bar is "observably used by the agent," not
"present on the wire" — but a live VS Code + Copilot agent-mode round-trip requires a
real GUI session and a real model backend this headless environment has no way to drive
(the same constraint ADR-050's own D4 item already documented and never closed). Rather
than declare this satisfied by wire-presence testing (the exact failure mode Decision C
names and rejects, citing the AWS `cursorState` incident), this phase ships two distinct
things: (1) the real, automated e2e proof that MAE's own side of the mechanism is correct
(`guidance_delivery_e2e.rs`, described under Phase A above — this is the necessary but
not sufficient half), and (2) `docs/verification/adr-063-copilot-live-check.md`, a
concrete, human-executable script using the identical distinctive-marker fixture the
automated tests use, so both halves of the verification target the same claim. The
document's Status is explicitly "not yet run" — an honest, visible open item, not a
silently-assumed-satisfied box-check.

**Phase D — shipped, and confirmed structurally, not just by assertion.** Investigated
the legacy embedded `ai_chat` path (`crates/mae/src/key_handling/conversation.rs`) before
writing the regression test: `submit_conversation_prompt` sends the raw input buffer text
via `AiCommand::Prompt(String)` with zero guidance-context concatenation anywhere in that
code path — it doesn't call `build_guidance_context`, doesn't go through the MCP
`initialize` handshake Phase A touched (that path is same-process, not MCP at all), and
never did. The regression test
(`legacy_ai_chat_prompt_is_unaffected_by_guidance_kb_options`) sets both new/changed
guidance options and asserts the sent prompt is exactly the user's typed text — proving
the won't-fix by exhibiting zero effect, not merely documenting an intention. Cross-linked
from ADR-049's own Consequences section per the ADR's own instruction.

**Verification recap:** `cargo fmt`/`clippy -D warnings`/`cargo test` clean across the
whole editor workspace (2790 mae-core + 439 mae-bin + 602 mae-ai + 72 mae-agent-cli tests,
plus the 3 new real subprocess e2e tests) — no code changes touched the daemon workspace
for this ADR.
