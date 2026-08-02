# ADR-085: `ToolCategory` describes subject matter, not blast radius

**Status:** Accepted.
**Extends:** ADR-056 (`ToolCategory` session-scoped dispatch enforcement — this ADR corrects the
taxonomy that ADR's enforcement operates on; the enforcement mechanism itself is unchanged).
**Relates to:** ADR-050 D2 (the mechanically-derived `ToolCategory` taxonomy), ADR-055 (headless MAE as
an engine instance — the motivating use case), ADR-084 (permission enforcement at the effect — the
sibling axis this ADR keeps distinct).
**Tracking:** private security advisory `GHSA-qwh8-m8j6-563h`.

## Context

ADR-056 lets a session declare a `mcp_tool_category_allowlist`, and its motivating configuration is a
"knowledge and guidance only" headless engine: `mcp_tool_category_allowlist = "knowledge"`. Its own
conformance tests are named `knowledge_only_session_denies_execute_command` and
`knowledge_only_session_denies_shell_exec_git_push_buffer_write` — denying execution to a
knowledge-scoped session is unambiguously the intent.

The intent does not hold. `classify_tool_category` places `babel_execute` and `babel_tangle` in
`Knowledge` (`crates/ai/src/tools/categories.rs`, both by explicit arm and by the `babel_` prefix rule),
and both are declared `PermissionTier::Shell` (`crates/ai/src/tools/kb_tools.rs`). `babel_execute` runs
org-mode source blocks in twelve languages; `babel_tangle` writes blocks to arbitrary paths.

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

## Decision

**Split the taxonomy so that a category name does not silently span blast radii, and enforce that
property with a test rather than with care.**

1. `babel_execute` and `babel_tangle` move out of `Knowledge` into an execution-flavoured category. A
   session scoped to `knowledge` no longer sees them at all — the fix is that they are not offered, not
   that they are offered and then refused.

2. **Read-flavoured categories may not contain tools above the write tier.** This is the invariant that
   generalises the fix. It is enforced by a test iterating every registered tool and asserting that any
   tool in a read-flavoured category declares a tier below `Shell`. Adding a Shell-tier tool to
   `Knowledge` — or to any future read-flavoured category — fails the build.

3. The taxonomy stays *mechanically derived* (ADR-050 D2's prefix rules), so the `babel_` prefix rule
   moves wholesale rather than becoming a per-tool exception list. A prefix that spans two risk levels is
   itself the smell; if a future prefix does, it splits too.

We explicitly did **not** make a category grant imply a tier ceiling. That was considered and rejected:
it changes ADR-056's semantics so that two independent axes silently compose, which would surprise an
operator who deliberately set a high tier for a narrow category, and it would leave the dishonest
taxonomy in place while masking it. Keeping the axes independent and making each one truthful is the
smaller and more legible change.

## Consequences

**Positive**

- `mcp_tool_category_allowlist = "knowledge"` means what an operator reading it would assume.
- The invariant is machine-checked, so the next tool added under a knowledge-ish prefix cannot quietly
  widen the category's blast radius.
- ADR-056's enforcement mechanism is untouched — this is a data fix plus a guard, not a redesign.

**Negative / Risks**

- A session that legitimately wanted babel execution under a category allowlist must now name the new
  category explicitly. That is the intended behaviour change, but it is a behaviour change: any existing
  configuration relying on `knowledge` to reach babel will stop reaching it. Given the feature's age and
  that reaching shell via `knowledge` was never intended, this is a fix rather than a regression — but it
  belongs in release notes.
- "Read-flavoured" is a judgement encoded as a list of categories. The test makes the judgement explicit
  and reviewable rather than implicit, but it is still a list someone must maintain when adding a
  category.

## Alternatives considered

**Lower the default `auto_approve_tier` from the shell tier.** This would independently close this hole
and a family of related fail-open cases, and it remains worth doing on its own merits — but it is a
breaking change for every existing user, and it does not fix the taxonomy. A `knowledge` allowlist would
still be a dishonest description of what it grants; the grant would merely be smaller.

**Special-case `babel_*` in ADR-056's enforcement.** Rejected: it hides the general problem behind a
patch for the one instance found, which is exactly the "third parallel implementation" shape principle
#15 forbids. The invariant test costs about the same and covers the cases not yet written.
