# ADR-069: Force-directed edge bundling for dense chord-diagram interiors

**Status:** Proposed (scoped, not implemented — see "Non-goal" below).
**Relates to:** issue #462 (multi-KB chord graph view), ADR-068 (full-corpus DOI/LOD
rendering — the render-time-tiering precedent this design's caching architecture reuses),
this session's edge-rendering pass (weight-driven opacity + configurable width, opt-in
opacity-taper toward a curved edge's midpoint — the two lower-risk techniques implemented
alongside this ADR).
**Tracking:** issue #501 (this ADR's own tracking issue). **Relates to:** issue #495
(GPU-offload/parallelization research — the sibling "properly scoped, deliberately
deferred" item this ADR's posture mirrors; #501 explicitly defers GPU parallelization for
edge bundling to whatever comes out of #495's own research, rather than a second
uncoordinated GPU dependency).

## Context

Live testing of the multi-KB chord graph view against a real, dense KB instance surfaced:
with enough nodes and internal links, the curved chords converging toward each diagram's
own center overlap so heavily that the diagram's interior reads as a solid, "filled
circle" — the individual connections it's meant to show become illegible exactly where
they overlap most.

This session already shipped two lower-risk, well-evidenced mitigations (see the two
commits immediately preceding this ADR): weight-driven edge opacity (surfacing real
ADR-030-authored relationship strength instead of a flat blur) and an opt-in opacity-taper
toward each curve's own midpoint (fading the overlap-heavy interior while keeping both
endpoints fully visible). Both are real improvements, but neither is the *purpose-built*
fix for this specific failure mode — dense edges *converging on the same interior region*,
independent of any one edge's own weight or curve shape. That fix, established in the
information-visualization literature for exactly this symptom, is edge bundling.

This ADR scopes that fix — the algorithm choice, the architecture, and the known
performance realities — without implementing it. The reasoning for that split is in
"Non-goal" below.

## Real-world grounding

- **Hierarchical edge bundling** (Holten, *Hierarchical Edge Bundles: Visualization of
  Adjacency Relations in Hierarchical Data*, TVCG 2006) routes each edge through a spline
  toward the least-common-ancestor of its two endpoints in an existing tree/hierarchy,
  bundling edges that share tree structure. **Not directly applicable to MAE**: a chord
  diagram's per-instance node set is a flat list (`chord_ring_positions`,
  `crates/canvas/src/kb_graph.rs`) with no hierarchy to route through — inventing one
  (e.g. clustering nodes into an ad-hoc tree first) would be a second, unvalidated design
  problem bolted onto this one.
- **Force-directed edge bundling (FDEB)** (Holten & van Wijk, *Force-Directed Edge Bundling
  for Graph Visualization*, EuroVis/Computer Graphics Forum 2009) needs no hierarchy: each
  edge is discretized into control points, subdivided iteratively, and control points on
  *compatible* edges (similar angle, length, position — the paper's own compatibility
  measures) attract each other like springs, converging on shared bundled paths. This is
  the applicable variant for MAE's flat per-diagram layout.
- **FDEB is genuinely expensive.** Contemporary summaries describe it as "relatively
  slow... consumes large amounts of memory for average and large graphs," with real
  optimization literature built specifically to address this: an edge-compatibility
  pre-clustering step (DBSCAN) to cut pairwise-comparison cost before the spring
  simulation runs (*Interactive 3D Force-Directed Edge Bundling*), Barnes-Hut-style spatial
  approximation, and full GPU parallelization reported to accelerate FDEB "by an order of
  magnitude" (*Parallelized Force-Directed Edge Bundling on the GPU*). A lighter,
  non-iterative alternative — kernel-density-estimation-based bundling — is also documented
  as suited specifically to interactive/immersive settings where FDEB's iterative cost is
  too high.
- This confirms FDEB is not a per-frame-affordable computation for MAE's reactive render
  loop without real mitigation — consistent with, not a special case of, the general
  lesson this session's own DOI-tiering perf fix already learned: expensive graph-derived
  computations must be cached and invalidated on topology change, never recomputed on
  every zoom/pan tick.

## Proposed architecture (for a future implementation pass)

1. **Compute bundled paths on populate/topology-change, not per-frame.** MAE already has a
   background-thread mechanism purpose-built for exactly this class of expensive,
   topology-derived, cacheable computation: `GraphLayoutIntent`/`graph_layout_bridge`
   (`crates/mae/src/graph_layout_bridge.rs`), today used for `ForceLayout::step`. Bundled
   edge control-point paths would be computed the same way — queued as a background
   request keyed to the current populate's `GraphView.generation`, applied via the same
   `apply_graph_layout_result` staleness-guard pattern (a result whose stamped generation
   no longer matches the current one is discarded, not applied) already proven for
   force-directed layout. This directly mirrors ADR-068's own `DoiTierCache` lesson: split
   the EXPENSIVE part (here, the bundling simulation) from the CHEAP part (rendering the
   already-bundled paths, safe on every frame) so a zoom/pan tick never re-triggers the
   expensive half.
2. **Render the bundled path as a polyline or a sequence of quadratic/cubic Bezier
   segments** through `VisualElement::Line`/`Curve` — no new `VisualElement` variant
   needed, consistent with this session's opacity-taper implementation reusing existing
   primitives rather than requiring GUI/Skia backend changes.
3. **Bound the cost with a real mitigation, not an unmitigated port of the base
   algorithm** — the literature's own answer: an edge-compatibility pre-clustering pass
   (only simulate springs between edges likely to actually bundle, cutting the pairwise
   comparison space before the expensive part runs) is the most directly portable
   mitigation into a single-threaded Rust implementation without a GPU dependency; Barnes-
   Hut approximation is a plausible second lever if the clustered cost is still too high in
   practice. GPU parallelization is explicitly out of scope until/unless issue #495's own
   GPU-offload research lands — this ADR should not invent a second, uncoordinated GPU
   dependency.
4. **Opt-in, matching this session's own established posture** for any new,
   potentially-costly rendering feature (`kb_graph_multi_kb_full_corpus`,
   `kb_graph_edge_taper_enabled`) — off by default, promoted only once proven stable and
   performant against a real dense federation.

## Non-goal (why this ADR scopes, not implements)

A correct, adversarially-tested FDEB implementation — the compatibility measures, the
iterative subdivision/spring simulation, the compatibility-clustering performance
mitigation, the background-thread caching integration, and a real dense-KB performance
benchmark — is comparable in size to the original #462 multi-KB chord view plus ADR-068's
full DOI/LOD system *combined*. Implementing it in the same pass as the two smaller,
already-shipped mitigations would either rush the algorithm (the exact failure mode this
codebase's CLAUDE.md principle #14 exists to prevent — no ad-hoc, unvalidated shortcuts on
a real algorithm) or block those smaller, genuinely-ready wins on a much bigger effort.
This ADR exists so that future work starts from a real design, not a blank page — mirroring
issue #495's own GPU-offload posture: researched and scoped now, implemented in its own
dedicated pass.

## Alternatives rejected

- **Hierarchical edge bundling.** Rejected — needs a tree structure MAE's flat per-diagram
  layout doesn't have; inventing one would be a second unvalidated design problem.
- **Implementing FDEB now, unmitigated.** Rejected — the literature is explicit that naive
  FDEB is too slow for average/large graphs; shipping it without a real mitigation
  (compatibility clustering, at minimum) would very likely just move the "dense KB is slow"
  complaint from rendering to layout computation.
- **Kernel-density-estimation bundling instead of FDEB.** Not rejected outright — flagged
  as a legitimate lighter-weight alternative worth evaluating empirically once real
  implementation work starts, since it's documented as better-suited to interactive
  settings; this ADR doesn't commit to FDEB over KDE-bundling, only to FDEB (not
  hierarchical) as the *applicable family* given MAE's flat layout.

## Verification (for the future implementation this ADR scopes, not this ADR itself)

Whoever implements this should adversarially test (CLAUDE.md principle #14): a real dense
fixture (not a cherry-picked small graph) shows a measurable reduction in edge-crossing/
overlap-area versus unbundled rendering; the background-thread caching never re-runs the
bundling simulation on a zoom/pan-only tick (mirroring
`continuous_zoom_never_recomputes_doi_candidates_only_finalizes_cheaply`'s exact proof
pattern from this session's own DOI-cache-split fix); a stale bundling result from a
superseded populate is discarded via the generation-stamp guard, not applied; bundled paths
remain anchored to their real node endpoints (a bundled edge that drifts away from its
actual source/target would be a correctness regression, not just a cosmetic one); and a
real before/after performance benchmark against the dense KB instance that motivated this
ADR, not a synthetic best-case fixture.
