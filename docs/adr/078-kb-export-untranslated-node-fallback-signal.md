# ADR-078: Untranslated-Node Fallback Gets an Explicit UI Signal, Not Silent Mirroring

**Status:** Accepted, implemented.
**Depends on:** CLAUDE.md architecture principle #14 ("adversarial testing, not confirmation")
and principle #9 ("don't silently truncate" / type safety at boundaries) — this decision is that
same "don't silently drop information" discipline applied to a UI fallback instead of a data
structure.
**Relates to:** ADR-077 (the module this behavior ships as part of).

*Ported from a downstream feature branch's own `kb/adrs/0003-untranslated-node-fallback-
signal.org` during PR #567's integration.*

## Context

This is the bug that originally motivated adding a real browser-execution ("Layer 2") test suite
for this feature at all. In a real exported guide with a bilingual EN/ES overlay, some nodes have
no Spanish translation. The export's data layer already handles this correctly by design: a node
without a translation has `title_es`/`body_es` mirror `title_en`/`body_en` exactly, so the page
never renders a broken or empty state. But that same design choice has an unintended consequence
at the UI layer: when a reader toggles the language button while viewing one of those untranslated
nodes, `currentLang` flips correctly, the toggle button's own label updates correctly, and yet the
visible title and body do not change at all, because `title_es === title_en` for that node. A
reader clicking the button repeatedly and seeing nothing happen has no way to distinguish "this
button is broken" from "this specific note has no translation" — and reasonably assumes the
former. This was reported directly by a real user as "the switch stops working."

This is structurally the same category of problem CLAUDE.md's principle #9 names for data
truncation ("verify that type conversions... don't silently truncate," operationalized elsewhere
in mae as the `hidden_node_count`-style "surface a count, never truncate silently" convention) —
here applied to a UI state transition instead of a data pipeline: a real, meaningful distinction
(translated vs. untranslated) was being silently collapsed into indistinguishable output.

## Decision

The language toggle stays exactly what it already is — a real, working *global* preference that
applies to every node, not a per-node setting. What changes: when the currently-selected node's
`body_es` is identical to its `body_en` (i.e., there is no real translation to show), the detail
panel displays a small, unobtrusive inline notice explaining the fallback (e.g. "This note isn't
translated yet — showing English") instead of silently rendering as if nothing were different.
This keeps the toggle's global semantics intact — a reader who prefers Spanish and navigates from
an untranslated node to a translated one immediately sees Spanish, with no re-toggling required —
while giving every individual node the chance to explain its own state. The Layer 2 browser test
suite (`crates/export/tests/browser/`) asserts this notice appears whenever `body_es === body_en`
for the selected node under `currentLang = "es"`.

## Consequences

**Positive**

- Closes the actual reported bug at its root: a reader can now always tell whether they're seeing
  a real translation or a fallback, node by node, without losing the global "prefer Spanish"
  setting.
- The fix is enforced going forward (the Layer 2 suite), not just a one-time content change.
- Generalizes cleanly to partial translations (title present, body missing, or vice versa) — the
  same "does the language-specific content actually differ from the fallback" check applies
  per-field, not just per-node.

**Negative / Risks**

- Adds a small amount of visible UI chrome to every untranslated node — a curated export with
  very few translated nodes could end up showing this notice on most pages.
- The "does body_es equal body_en" check is a straightforward equality comparison; if a future
  change makes EN and ES bodies coincidentally identical for a genuinely translated node (unlikely
  in practice), the notice would show incorrectly. The failure mode (an unnecessary notice, not a
  missing one) is the safe direction to fail in, so this isn't mitigated further.

## Alternatives Considered

**Disable the language toggle when the current node lacks a translation.** Rejected: the toggle
is a *global* preference, not a per-node control — disabling it on one node would be actively
wrong the moment the reader is one click away (via Next, or a body link) from a node that *does*
have a translation.

**Leave it as-is.** Rejected — this is the bug being fixed.
