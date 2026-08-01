# ADR-082: `required_tag` — a Hard Tag Filter for `kb-export-subgraph-html`, Independent of `node_cap`

**Status:** Accepted, implemented.

## Context

`kb-export-subgraph-html` extracts a subgraph via seed + BFS depth + `node_cap` (a safety net on
reachable-set size, never a selection criterion). This works well when "the neighborhood of one
node" is genuinely what a caller wants, but it silently assumes the seed's own link structure
matches the caller's real intent.

A real production case surfaced this gap directly: exporting a "terraform onboarding" walkthrough
from a personal knowledge base. The obvious seed — a general "terraform" reference hub — pulled in
100+ loosely-related reference nodes via BFS, none of which were part of the curated onboarding
walkthrough; a smaller, differently-named node ("Terraform: Zero to Running") was the actual
intended entry point, and picking the wrong seed produced a plausible-looking but wrong export with
no error or warning. The KB already had the correct nodes tagged `terraform-onboarding` — there was
simply no way to tell the export tool "only include nodes carrying this tag," so correctness
depended entirely on picking the right seed and the right depth, by luck.

## Decision

Add `required_tag: Option<String>` to `mae_kb::SubgraphSpec` (and thread it through
`kb_export_subgraph_html`'s MCP tool args and the `(kb-export-subgraph-html ...)` Scheme
primitive, per CLAUDE.md principle #3 — human and AI drive the identical code path).

Semantics, applied inside `extract_subgraph` itself (not at a calling layer, so `node_cap` and the
tag filter compose correctly):

- The BFS walk still traverses through EVERY reachable node up to `max_depth` — an untagged node
  remains a valid stepping stone to a tagged node beyond it. The tag restricts the RESULT set, not
  the traversal.
- The seed itself is always kept, regardless of its own tags — it anchors the export's layout and
  reading order even when it isn't itself tagged with the required tag.
- The filter is applied AFTER the full BFS walk but BEFORE `node_cap` truncation, so `node_cap`
  counts the tag-filtered candidate set, not raw traversal size, and an excluded untagged node gets
  demoted to a boundary-link stub via the exact same mechanism `node_cap`'s own cutoff already
  uses — no new link-classification code needed (CLAUDE.md principle #8).
- Matching is exact-string (`node.tags.iter().any(|t| t == tag)`), the same convention
  `KnowledgeBase::nodes_by_tag`'s `tag_index` already uses — deliberately NOT the Cozo-backed
  `AgendaFilter::Tag`'s substring convention (see ADR-083's own tag-matching discussion for why
  *that* filter stays substring instead): `required_tag` is a brand-new mechanism with no existing
  callers to stay compatible with, so it gets the more predictable exact-match semantics from day
  one rather than inheriting a different subsystem's historical convention.

`SubgraphResult` gains `tag_filtered_count: usize`, reported alongside `hidden_node_count`
(`", N more node(s) excluded by required_tag"` in the tool's status message) — the same
never-silent-truncation convention this tool already uses for `node_cap`. The two counts are
independent and reported separately, never conflated.

## Consequences

**Positive**

- Export correctness no longer depends on picking exactly the right seed/depth combination — a
  caller can name the invariant they actually want ("only onboarding-tagged content") directly.
- Reuses `extract_subgraph`'s existing boundary-link demotion mechanism for excluded nodes — zero
  new link-handling code.
- `SubgraphSpec`/`SubgraphResult` are shared types also used by the native KB graph view
  (`crates/core/src/editor/graph_view_ops.rs`) — every existing caller passes `required_tag: None`
  and sees byte-identical behavior to before this field existed (verified by a dedicated regression
  test).

**Negative / Risks**

- `SubgraphSpec` has no `Default` impl, so every future field addition (not just this one) requires
  a mechanical update at all 4 struct-literal construction sites. Not fixed here — a real
  `Default` impl would be a separate, broader decision affecting an established type.

## Alternatives Considered

**Client-side-only filtering (the exported page's existing tag-filter picker, which dims
non-matching nodes visually).** Already exists, but is a display affordance, not an extraction
control — the excluded nodes are still fully present in the payload (a real content-inclusion
concern for a tool whose whole purpose is producing a shareable, scoped artifact), and a caller
still can't get a *smaller*, more focused export this way. Kept as a complementary UI feature, not
a substitute for a hard server-side filter.

**Filter by tag as a totally separate BFS seed set (start the walk from every tagged node
directly, instead of one seed + a tag filter).** Would lose the seed's own anchoring role (Home
button, reading-order walk, distinct styling) and turn this into a fundamentally different
multi-seed extraction mode — a much larger change for a need the single-seed-plus-filter design
already satisfies.
