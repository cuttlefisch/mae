# ADR-068: Full-corpus multi-KB retrieval + degree-of-interest render-time level-of-detail

**Status:** Accepted (implemented).
**Extends:** issue #462 (multi-KB chord graph view — the grid/small-multiples composition
this design renders on top of).
**Supersedes:** issue #477 (density LOD/clustering — filed as a follow-up during #462's
initial ship, deferred as new work; this ADR delivers and generalizes it with a
focus-aware model grounded in real prior art rather than an ad-hoc threshold).
**Relates to:** issue #474 (federated `health_report` reconciliation — the
`linked_in_degree`/degree-ranking convention this design's Degree tier reuses), the same
session's live-usage bug-hunt pass (issues #479, #485 — the chord view's own health-check
role motivated pulling full-corpus rendering into scope now rather than later).
**Tracking:** issue #462 (parent), #477 (superseded by this ADR).

## Context

The multi-KB chord graph view (#462) extracts each KB instance's diagram as a depth-1,
300-node-capped BFS ego-network from a single hub/index node
(`Editor::populate_graph_buffer`'s Multi-mode branch, `crates/core/src/editor/graph_view_ops.rs`,
via `shared/kb/src/lib.rs`'s `extract_subgraph`). This matches single-KB semantics and is a
reasonable default, but live usage against a real multi-instance federation (6 registered
KBs, one with 953 nodes, primary carrying MAE's own 2,600+-node manual+user-notes corpus)
surfaced that a user wanting to survey their whole federated KB web sees only a small
neighborhood of each instance — not what "multi-KB" implies.

The pragmatic fix (raise the cap, surface truncation honestly) was considered and rejected
as the *complete* answer: there is no precedent anywhere in this codebase for a
non-ego-network render, and naively removing the cap for up to 7 simultaneously-composed
diagrams reintroduces exactly the "thousands of dots on a chord ring" legibility problem
issue #477 already scoped and deferred. The two problems — pull everything, but don't
drown the user in it — are the same problem, and needed one grounded design, not two patches
bolted together.

## Real-world grounding

This is not a novel visualization technique invented for MAE — it composes three
established, separately-precedented ideas:

- **Furnas's Degree-of-Interest (DOI) model** (*Generalized Fisheye Views*, CHI 1986;
  *Degree-of-Interest Trees*, AVI 2008): `DOI(x | focus=y) = API(x) − D(x,y)` — an
  element's a-priori importance minus its distance from the current focus point. Elements
  above a threshold render at full detail; below it, they're elided. As focus moves
  (the user navigates to a different node), DOI recomputes and the visible set updates.
  This is the direct mechanism for "as the user opens a given node, the view updates
  accordingly."
- **Semantic zoom's monotonic-nesting property** (surveyed across semantic-zoom
  literature, e.g. multi-level tree-based interactive graph visualization): an element
  visible at a coarser level of detail stays visible at every more-detailed level too — no
  flickering in and out as zoom/focus changes, only progressive reveal.
- **Cross-KB links as graph-theoretic bridge edges** (cut-edges): in the composed
  multi-diagram scene, an inter-instance link is — absent a second parallel cross-link
  between the same instance pair — the *only* connection between two otherwise-separate
  diagram components. This gives a principled, structural justification (not an arbitrary
  threshold) for pinning cross-KB links at permanently maximal importance, satisfying "no
  matter the zoom level" as a guarantee rather than a tunable that could theoretically be
  crossed.

Together: DOI decides per-node detail as a function of structural importance and distance
from wherever the user currently is; the monotonic-nesting property is the correctness
contract that detail only ever *adds*, never *removes*, information as you zoom in; the
bridge-edge framing is what makes cross-KB links a hard tier rather than a scored, tunable
signal.

## Decision

### 1. Extraction: full corpus, not capped BFS (only when opted in)

`KnowledgeBase::extract_full_corpus(cap, protected, include_body)`
(`shared/kb/src/lib.rs`) pulls every node via `list_ids(None)` rather than a BFS from one
starter, truncating by the same degree-sort-descending logic `extract_subgraph`'s own
truncation already used (factored into a shared `collect_and_categorize` helper both
functions now call — a behavior-preserving refactor, provable by the full pre-existing
`extract_subgraph` test suite passing unmodified) — but exempting only an explicit
`protected` set, not every node. `protected` is computed by the `crates/core` caller (which
has `Editor::kb_owner_of` — `shared/kb`'s `KnowledgeBase` has no cross-instance visibility
by design) via a new `Editor::kb_cross_instance_link_sources` pre-pass
(`crates/core/src/editor/kb_ops/registry.rs`): every node id in an instance with ≥1 real
outgoing cross-instance link, discovered once per instance in O(nodes × avg-out-degree).
This is the load-bearing correctness detail: a naive "make every node a starter" approach
would have made the existing node-cap safety net a no-op entirely; computing bridges
*before* truncating everything else by degree means a cap can never silently drop a bridge
edge before DOI gets a chance to protect it.

Gated entirely behind a new master opt-in option, `kb_graph_multi_kb_full_corpus` (Bool,
default `false`) — with it off, `GraphViewMode::Multi`'s extraction is byte-identical to
pre-ADR-068 behavior, proven by the complete existing Multi-mode test suite passing
unmodified at the default. `GraphViewMode::Single` is untouched entirely, at any setting.

### 2. `API(x)`: hard tiers, not a weighted score

Deliberately a small ordered set of tiers (`ApiTier::Bridge > Hub > Degree > Ordinary`,
`crates/core/src/graph_view.rs`), evaluated top-down, first match wins — not a
weighted-sum score. A scored system would need tuning constants and would make "cross-KB
links are always visible" a probabilistic outcome of tuning rather than a guarantee; a hard
tier makes it a structural fact. `Bridge` tier (any node that is a source or target of a
detected cross-instance link) is never even evaluated against distance or zoom — it is
`RenderTier::Full` unconditionally. `Hub` reuses the diagram's own existing default-center
resolution (`default_center_for_owner`) verbatim. `Degree` reuses the existing
`node_degree`/`linked_in_degree` ranking convention from issue #474's work.

### 3. `D(x, focus)`: distance from current focus, decoupled from re-extraction

`GraphView.doi_focus: Option<String>` is a new, separate concept from `center_node` (the
topology's BFS/extraction seed) — changing focus must not re-extract or re-lay-out the
scene, since that would both be expensive on every navigation and would reshuffle node
positions, destabilizing the "stable spatial map" feel semantic zoom's monotonic-nesting
property depends on. `maybe_follow_kb_graph_view`, when full-corpus mode is active, updates
`doi_focus` and bumps a lightweight `doi_generation` counter (mirroring the existing
`GraphView.generation`/background-layout-race-guard idiom from this session's earlier
work) — no `populate_graph_buffer` call. Distance itself is a plain multi-source BFS over
the already-in-memory node/link adjacency (`KnowledgeBase::hop_distances_from`), computed
independently *per diagram* — a related diagram (reached only via a cross-link) measures
distance from its own cross-link landing point, not a unified cross-instance hop-count,
deliberately avoiding the need to invent an ambiguous cross-KB distance metric given
independently-authored KBs can share bare node ids.

### 4. Rendering tier: a render-time overlay, not baked into extraction

Layout is computed exactly once per populate, with a stable, deterministic node ordering.
`GraphView` gains `node_api_tier` (topology-derived, cached like `node_degrees` already is)
and a `DoiTierCache` (the same single-slot memoization shape as the existing
`LabelWinnerCache`, invalidated by `doi_generation`, not the full topology `generation`).
`compute_node_tiers` produces `RenderTier::Full | Clustered | Hidden` per node;
`flatten_scene_graph_cached` consults it — `Hidden` skips the draw call but the node stays
in `scene`/`describe_state()` (this codebase already has a house rule,
`describe_state_is_unaffected_by_culling_or_lod`, that render-time LOD must never reshape
scene-level introspection — respected here, not violated); `Clustered` nodes aggregate into
one stub per (diagram, bucket), reusing the existing boundary-link "... (+N)" visual
convention rather than inventing a new one. `GraphViewNodeState` gets one additive
`render_tier` field so the AI peer sees the same tiering a human does (CLAUDE.md principle
#3 — AI as peer, not a degraded observer).

### 5. Cross-KB links always visible: pinned anchors, not a per-frame decision

Bridge-tier nodes are excluded from the clustering candidate pool up front — never
eligible for `Clustered`. This guarantees a cross-KB edge always terminates at a stable,
individually-rendered position, never a cluster stub's position (which could shift
frame-to-frame as membership changes). This is the edge-level expression of monotonic
nesting: an always-visible edge must anchor to something that is also always visible.

### 6. New options (5-part `OptionRegistry` pattern, all additive)

`kb_graph_multi_kb_full_corpus` (master gate), `kb_graph_full_corpus_node_cap` (pathological-
scale safety net, distinct from `kb_graph_node_count_cap` which keeps its existing meaning
for Single mode), `kb_graph_doi_zoom_threshold`, `kb_graph_doi_distance_falloff` (kept as
one simple hop-count knob, not an exposed formula — deliberately not over-engineered),
`kb_graph_dense_cluster_threshold` (subsumes #477's own proposed option), `kb_graph_cluster_group_by`.

## Consequences

**Positive.** Delivers full-corpus visibility without the "thousands of unreadable dots"
failure mode, using a principled model (DOI + monotonic nesting + bridge edges) rather than
an ad-hoc heuristic that would need to be re-justified later. Fully backward-compatible —
one boolean gate, off by default, Single mode untouched. Reuses existing memoization
(`LabelWinnerCache`'s shape), existing degree-ranking conventions (#474), and existing
visual language (boundary-stub "+N") rather than inventing parallel mechanisms.

**Costs.** Genuinely new rendering-pipeline surface area (tiering, a second cache,
focus/distance plumbing) — real code to maintain, not a small patch. The per-diagram
(not globally unified) distance metric is a deliberate simplification that slightly
understates true cross-KB proximity in exchange for not inventing an ambiguous
cross-instance hop-count.

**Explicit limitations:**
- TUI rendering degrades to a coarser summary (individual Bridge/Hub nodes + a clustered
  count) rather than full GUI-parity clustering — a full-corpus flat-text listing would be
  a worse regression than a summarized one; exact per-node TUI clustering parity is not
  attempted.
- The Degree tier's cutoff (top quartile by degree) is a plain module constant, not a
  user-facing option — judged not worth exposing a tuning knob for; revisit if real usage
  shows it's wrong for common corpus shapes.

## Alternatives rejected

- **A weighted-sum importance score instead of hard tiers.** Rejected — would make
  "cross-KB links are always visible" a tunable outcome rather than a structural
  guarantee, and requires justifying arbitrary weights.
- **A single unified cross-instance distance metric.** Rejected — requires resolving
  ambiguity from independently-authored KBs sharing bare node ids; per-diagram local
  distance avoids inventing this without losing the practical benefit (a diagram's own
  cross-link entry point standing in for "close to what you're currently looking at").
- **Baking DOI/tiering into extraction instead of render time.** Rejected — would force
  re-extraction and node-position churn on every focus change, and would violate this
  codebase's existing `describe_state_is_unaffected_by_culling_or_lod` house rule.

## Verification

Adversarial test suite (implemented alongside this ADR, `crates/core/src/editor/graph_view_ops.rs`
and `shared/kb/src/lib.rs`): bridge-under-pressure (a far, low-degree, cross-link-endpoint
node stays `Full` across varied focus positions and threshold settings), monotonic nesting
(relaxing thresholds only grows the visible set, node-for-node), extraction-cap-vs-bridge-
protection (a low-degree sole bridge survives truncation under a tight cap), rapid
focus-churn staleness (generation-stamped, stale computations discarded), the degenerate
no-cross-links case (bounded clustering, stub counts sum exactly to elided count), position
stability (two populates differing only in focus produce byte-identical node coordinates),
backward-compatibility (`kb_graph_multi_kb_full_corpus=false` reproduces the complete
pre-ADR-068 test suite unmodified), and cross-backend/AI-peer parity (`describe_state()`'s
`render_tier` matches exactly what the GUI draws).
