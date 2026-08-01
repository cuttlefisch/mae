# ADR-079: Guidance Nodes Travel With a KB Export as a Colophon, Not as Enforcement

**Status:** Accepted, implemented. Extended by PR #567's own integration (see the note at the
end of Decision) to residency-filter guidance nodes, closing a gap this ADR's original scope
didn't cover.
**Relates to:** ADR-077 (the module this ships as part of); ADR-048 (AI residency policy for
sensitive KBs — the residency-filtering extension).

*Ported from a downstream feature branch's own `kb/adrs/0004-guidance-nodes-colophon.org` during
PR #567's integration; the real-KB example paths/ids in the original have been generalized (see
Context) rather than porting a specific user's actual note content/ids into a public ADR.*

## Context

A curated KB export (a guide assembled from a subgraph reachable from one seed node) is often
written against real editorial standards — a writing-style guide, a fact-checking discipline, a
translation-provenance note — that exist elsewhere in the source KB. A real question came up
during this feature's development: should the export tool itself somehow *enforce* those
standards, or at least encode them?

It cannot, and should not try to. This tool is a deterministic renderer — it turns already-written
`GraphExportNode`/`GraphExportEdge` data into an interactive HTML page. Fact-checking a technical
claim, judging whether prose matches a target persona, or catching two notes making the same claim
in different words are all editorial judgments an AI or human author makes *before* content
reaches this tool, not something a rendering pipeline can perform on its input. Conflating "the
tool renders guidance content" with "the tool enforces the guidance" would suggest a guarantee
this tool cannot make.

What the tool *can* do, and is worth doing: let the standards a curated guide was actually written
against travel with the guide itself, visible to any reader, not just discoverable by someone who
already knows to go check the source KB for a practice note. This is a transparency feature, not a
correctness feature.

## Decision

Add an explicit, optional list of "guidance node" ids to the export input (`guidance_ids`),
alongside the main seed/subgraph. These nodes are always included in the export regardless of BFS
depth or reachability from the seed (the same "always included, not subject to normal traversal"
treatment the anchor/seed node already gets), and rendered in a visually distinct "About this
guide" / colophon section — separate from the curated topic content itself, so a reader can tell
at a glance that a linked writing-style note is meta-content about the guide, not part of its
subject matter. Guidance nodes are excluded from the interactive chord graph and the Previous/Next
reading-order walk, while staying resolvable via a colophon click or an ordinary in-body link. The
tool does not interpret, apply, or check content against these nodes in any way; it only displays
them.

**Extension during PR #567's integration (residency filtering):** the original design had no
AI-residency check on `guidance_ids` at all — each id resolves independently of the seed, possibly
into a different KB instance than the one the seed's own dispatch-time residency check covers. A
pre-merge security review found this was a real gap (not just theoretical): an agent that knows or
guesses an id in a residency-restricted KB could pull its full content into a permitted export's
colophon. `execute_kb_export_subgraph_html` now post-filters each resolved guidance node through
the same residency-check primitive `kb_links_from`'s own per-target check already uses, and
reports any omission explicitly in the tool's returned status rather than silently dropping it.
This ADR's core decision (guidance nodes as a transparency-only colophon) is unchanged; only the
input-resolution step gained the same content-safety treatment every other multi-target KB tool
already has.

## Consequences

**Positive**

- A reader can see exactly which standards a guide claims to follow without leaving the exported
  page or knowing to search the source KB.
- Keeps the tool's own responsibility boundary honest: it renders, it does not review.
- Reuses the existing "always-included node" mechanism (the anchor node already bypasses normal
  reachability rules) rather than inventing a second inclusion pathway.

**Negative / Risks**

- Guidance nodes add visible surface area to every export that uses them.
- Nothing stops a curator from listing guidance nodes a guide doesn't actually follow — the
  feature makes standards *visible*, not *true*; that gap is inherent to a transparency-only
  design and accepted here rather than solved.

## Alternatives Considered

**Have the tool itself perform automated content checking against the guidance nodes.** Rejected
outright: fact-checking and style-review are judgment calls a deterministic renderer cannot make.

**Don't add this at all — leave guidance discoverable only via the source KB.** Rejected: the
standards a guide follows would otherwise be visible only to someone with source-KB access, not to
the guide's actual readers, and the fix (an always-included node list + a distinct render section)
is small relative to that value.
