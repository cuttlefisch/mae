# ADR-085: `ToolCategory` describes subject matter, not blast radius

**Status:** Accepted. **Revised 2026-08-02** following an external prior-art review, which confirmed the
central decision and corrected two supporting ones (see *Revision note*).
**Extends:** ADR-056 (`ToolCategory` session-scoped dispatch enforcement — this ADR corrects the
taxonomy that ADR's enforcement operates on; the enforcement mechanism itself is unchanged).
**Relates to:** ADR-050 D2 (the mechanically-derived `ToolCategory` taxonomy), ADR-055 (headless MAE as
an engine instance — the motivating use case), ADR-084 (permission enforcement at the effect — the
sibling axis this ADR keeps distinct, and the owner of the fail-safe-default decision).
**Tracking:** private security advisory `GHSA-qwh8-m8j6-563h`.

## Revision note

The prior-art review found **no system, standard, or vendor** that derives a risk ceiling from a
subject-matter grant — the alternative this ADR rejected. That rejection stands, with stronger support
than the original reasoning had. Two supporting decisions changed:

- The original ADR listed "lower the default `auto_approve_tier`" under *Alternatives considered* as
  worth doing but deferred. That was backwards: the fail-open default is the root cause, and splitting
  the taxonomy only closes this instance of it. The decision now lives in **ADR-084 D4** and is a
  precondition for this ADR, not an alternative to it.
- The proposed per-tool invariant is necessary but insufficient, because risk composes across a
  session. Decision 4 below adds the session-level assertion.

## Context

ADR-056 lets a session declare a `mcp_tool_category_allowlist`, and its motivating configuration is a
"knowledge and guidance only" headless engine: `mcp_tool_category_allowlist = "knowledge"`. Its own
conformance tests are named `knowledge_only_session_denies_execute_command` and
`knowledge_only_session_denies_shell_exec_git_push_buffer_write` — denying execution to a
knowledge-scoped session is unambiguously the intent.

The intent does not hold. `classify_tool_category` places `babel_execute` and `babel_tangle` in
`Knowledge` (`crates/ai/src/tools/categories.rs:62-63,117`, both by explicit arm and by the `babel_`
prefix rule), and both are declared `PermissionTier::Shell` (`crates/ai/src/tools/kb_tools.rs:192,198`).
`babel_execute` runs org-mode source blocks in twelve languages; `babel_tangle` writes blocks to
arbitrary paths.

Both gates pass, and both pass *correctly*. The category gate admits them because they genuinely are
knowledge tools — executing a source block in an org document is a knowledge-work operation by any
reasonable reading. The tier gate admits them because the default `auto_approve_tier` is the shell tier.
Neither gate is buggy in isolation.

The fault is conceptual: **category and tier are orthogonal axes, and the allowlist reads as a safety
control while only expressing one of them.** A category answers "what subject is this tool about?" A tier
answers "how much damage can it do?" Those questions have independent answers, and restricting by subject
tells you nothing about blast radius. An operator writing `= "knowledge"` is reasonably expressing "this
session should only touch my notes" and is instead granting arbitrary code execution.

This is not hypothetical for v0.15: the headless engine is the release's headline deployment shape, and
`knowledge` is the allowlist value its own ADR uses as the example.

**This is drift, not a fresh call** (principle #15). ADR-056 already states the position — "Tier answers
'how mutating'; category answers 'which subsystem.' Both gates run … and both must pass" — but its
Context paragraph enumerates `babel_*` among the tools `Knowledge` "already covers exactly," for a
KB-and-guidance-only engine. The taxonomy premise drifted from the principle stated two paragraphs
above it.

## Decision

**Split the taxonomy so that a category name does not silently span blast radii, and enforce that
property with a test rather than with care.**

1. `babel_execute` and `babel_tangle` move out of `Knowledge` into a new `Execution` category. A
   session scoped to `knowledge` no longer sees them at all — the fix is that they are not offered, not
   that they are offered and then refused.

   **Implementation note (2026-08-03).** The invariant in Decision 2 was written first and run against
   the live registry before any tool was moved. It found **nine** violations, not the two the audit
   had identified by reading code:

   | Tool | Tier | Why it is not read-flavoured |
   |---|---|---|
   | `babel_execute` | Shell | runs org source blocks in twelve languages |
   | `babel_tangle` | Shell | writes blocks to arbitrary paths |
   | `org_export` | Shell | shells out for rendering |
   | `kb_enrich` | Shell | network calls to embedding providers |
   | `kb_register` | Shell | filesystem discovery of a KB instance |
   | `kb_reimport` | Shell | filesystem rebuild of a KB instance |
   | `kb_raw_query` | **Privileged** | arbitrary Datalog against the KB |
   | `kb_export_subgraph_html` | Shell | shells out to `npx` for mermaid |
   | `web_fetch` | Shell | real network fetch, in `Web` |

   The first eight moved to `Execution`. `web_fetch` did not: the fault there was the *category's*
   classification, not the tool's — `Web` had been marked read-flavoured on the assumption that
   "web" implies looking. It does not, any more than "git" does, and that assumption was the same
   subject-vs-blast-radius conflation this ADR exists to correct, committed while writing it. `Web`
   is now non-read-flavoured, which is why `web_fetch` needed no relocation.

   The `kb_`/`org_` outliers are named explicitly rather than splitting those prefixes wholesale,
   because ~55 of ~60 `kb_` tools and three of four `org_` tools are genuinely ReadOnly or Write.
   That mirrors the exact-name carve-outs already present for `shell_exec` and `ai_permissions`
   rather than departing from D5's mechanical-prefix design. `babel_` moved wholesale, as D5
   requires, because every `babel_` tool is Shell tier.

   No allowlist or exception was added to make the invariant pass.

2. **Read-flavoured categories may not contain tools above the write tier.** This is the invariant that
   generalises the fix. It is enforced by a test iterating every registered tool and asserting that any
   tool in a read-flavoured category declares a tier below `Shell`. Adding a Shell-tier tool to
   `Knowledge` — or to any future read-flavoured category — fails the build.

3. **Where a tool is borderline between two tiers, it takes the higher one.** Over-labelling costs an
   authorisation gate; under-labelling exposes a capability. This is the standing tie-break, so that
   "is this really shell-tier?" is never resolved downward by argument.

4. **The allowlist is also asserted at the session level, over the union of reachable effects.** The
   per-tool invariant in Decision 2 cannot see two real cases: that read sometimes outranks write
   across categories, and that a session's combined tool set exceeds any single tool's tier. The
   conformance test for a category allowlist therefore enumerates the *whole* resolved tool set for
   that allowlist and asserts properties of the union — not a hand-picked sample. The existing tests
   (`categories.rs:346-420`) pick three tools by name and are exactly the "unicorn values" shape
   principle #14 forbids; they are why this defect survived.

5. The taxonomy stays *mechanically derived* (ADR-050 D2's prefix rules), so the `babel_` prefix rule
   moves wholesale rather than becoming a per-tool exception list. A prefix that spans two risk levels
   is itself the smell; if a future prefix does, it splits too. **Caveat, stated because the evidence
   contradicts the general form:** the current literature holds that risk should be assigned "from tool
   semantics, not tool names." Prefix derivation is retained here only because MAE's prefixes are
   assigned by us and are already semantically meaningful, and because Decision 2's invariant catches a
   prefix that drifts. It is a convenience under a machine-checked guard, not a claim that names encode
   risk.

6. **Split until no category spans a tier, then stop.** Do not build the category × tier cartesian
   product. The failure mode on the other side is real — OAuth's scope-explosion problem — and the
   invariant, not granularity, is the deliverable.

We explicitly did **not** make a category grant imply a tier ceiling. Considered and rejected: it makes
two independent axes silently compose, surprising an operator who deliberately set a high tier for a
narrow category, and it leaves the dishonest taxonomy in place while masking it. Keeping the axes
independent and making each one truthful is the smaller and more legible change — and it is what every
comparable system does.

## Consequences

**Positive**

- `mcp_tool_category_allowlist = "knowledge"` means what an operator reading it would assume.
- The invariant is machine-checked, so the next tool added under a knowledge-ish prefix cannot quietly
  widen the category's blast radius.
- ADR-056's enforcement mechanism is untouched — this is a data fix plus a guard, not a redesign.

**Negative / Risks**

- A session that legitimately wanted any of the eight relocated tools under a category allowlist must
  now name `execution` explicitly. That is the intended behaviour change, but it is a behaviour change:
  any existing configuration relying on `knowledge` to reach `babel_*`, `org_export`, `kb_enrich`,
  `kb_register`, `kb_reimport`, `kb_raw_query`, or `kb_export_subgraph_html` will stop reaching them.
  Given that reaching shell via `knowledge` was never intended, this is a fix rather than a regression —
  but it belongs in release notes, and the list is longer than the two tools the audit found.
- Reclassifying `Web` as non-read-flavoured changes **no** runtime behaviour: `is_read_flavoured` is
  consulted only by the invariant test, and `web_fetch` stays in `Web`. A session allowlisting `web`
  grants exactly what it granted before.
- "Read-flavoured" is a judgement encoded as a list of categories. The test makes the judgement explicit
  and reviewable rather than implicit, but it is still a list someone must maintain when adding a
  category.
- Decision 4's union assertion will surface further composition findings. That is the point, but it
  means the category allowlist may need to tighten again before v0.15 ships.

## Alternatives considered

**Make a category grant imply a tier ceiling.** Rejected — see Decision. The prior-art review found zero
instances of this design and five vendors who hit this exact bug and fixed it by splitting the
subject-matter unit instead. Android is the cautionary case: it is the one major system that let a
subject-matter group carry grant semantics, and the result is silent permission expansion still shipping
today.

**Special-case `babel_*` in ADR-056's enforcement.** Rejected: it hides the general problem behind a
patch for the one instance found, which is exactly the "third parallel implementation" shape principle
#15 forbids. The invariant test costs about the same and covers the cases not yet written.

**Lower the default `auto_approve_tier`.** No longer an alternative — adopted as **ADR-084 D4**, which
this ADR now depends on. Splitting the taxonomy without it leaves the next Shell-tier tool in the new
execution category auto-approved by default.

## Evidence

Prior-art review, 2026-08-02.

- **Android** — `<permission-group>` "doesn't declare a permission itself, only a category"; yet grants
  are tracked at group granularity, and since 8.0 a same-group permission is granted "without prompting
  the user." Measured at 17% of multi-version apps experiencing silent permission expansion
  ([arXiv 2605.27667](https://arxiv.org/abs/2605.27667)). Google's remediation pattern is splitting
  (Android 13 `READ_MEDIA_*`, Android 11 background location), never capping. The docs' own warning:
  "permissions can change groups without notice."
- **GitHub** — `repo` grants "broad access … in perpetuity"; replaced by 50+ granular permissions at
  resource × (none|read|write|admin). Writing `.github/workflows` — code execution hiding inside
  `Contents: write` — was broken out into a separate `Workflows` permission requiring both grants. The
  same shape as the babel fix.
- **Google OAuth** — sensitive/restricted is a classification *applied to* a scope, and is not monotonic
  in the scope's verb: `gmail.labels` (read-write) is non-sensitive while `gmail.readonly` is
  restricted. `drive.file` exists precisely so a legitimate read-write case need not enter the
  restricted tier. No Google scope spans two tiers.
- **IETF** — RFC 8707 ("scope … is sometimes overloaded to convey the location or identity of the
  protected resource") and RFC 9396 ("not sufficient to specify fine-grained authorization
  requirements") both add a *separate* parameter rather than enrich scope.
- **MCP** — defines a risk vocabulary (`readOnlyHint`, `destructiveHint`) and **no** category concept,
  with fail-closed defaults. Its retrospective states the limit this ADR's Decision 4 responds to:
  "A tool's risk depends on what else is in the session."
- **AWS / Kubernetes** — permissions boundaries and SCPs are ceilings implemented as *separate policies
  intersected*, never grants derived from one another ("an SCP never grants permissions"). Structurally
  what `is_allowed(tier) && is_category_allowed(name)` already is. K8s RBAC's own docs note `get
  secrets` is "equivalent to higher privilege" — the read-outranks-write case Decision 4 covers.
- **Saltzer & Schroeder** — separation of privilege: "a protection mechanism that requires two keys …
  is more robust and flexible than one that allows access to the presenter of only a single key."
- *Capability Minimization as a Safety Primitive*
  ([arXiv 2606.13884](https://arxiv.org/abs/2606.13884)) — annotates a tool registry with domain and an
  *independent* risk tier; "a tool can be causally useful yet unsafe to expose." Source of Decision 3's
  tie-break and Decision 5's caveat.
