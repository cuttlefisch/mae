# ADR-064: A second native MAE frontend for visual-design workflows

**Status:** Proposed.
**Extends/updates scope of:** ADR-016 (its `BufferKind::Visual` work becomes a dependency, not
superseded).
**Depends on:** ADR-014, ADR-057.

## Context

ADR-057's evidence table names, as row 9 of its gap inventory, a specific and previously
unverified piece of the project owner's architecture vision: "other native MAE frontends sharing
the same KB/CRDT core" for workflow shapes the current GUI/TUI/headless render-mode split cannot
accommodate — visual-design workflows named explicitly as the first such shape. ADR-057's own
research pass found that the real design document for this, ADR-016, is still `Status: Proposed`
with its own Phases 2-3 unshipped, and that no ADR or ROADMAP entry anywhere in the repository
names an actual separate frontend *application* for visual design, as distinct from an in-editor
buffer kind. That gap is what this ADR closes. It does not re-derive ADR-016's interaction-model
design; it depends on it, extends it, and specifies the concrete new frontend that consumes it.

**What this ADR is not.** Three shapes could plausibly satisfy "MAE gets a visual-design
frontend," and only one of them actually tests the claim ADR-057 identified as unverified. It is
not a new `BufferKind` bolted onto the existing GUI — that would prove only that visual-design
*workflows* can exist inside the one frontend MAE already has, a real but strictly smaller claim
than "a second native frontend," and it is also not the thing ADR-016 itself already scopes
(ADR-016's Phase 3 `ArtifactType`/modality work is exactly what makes a canvas artifact
first-class *within* a buffer — necessary, but not sufficient, for a second application). It is
also not a standalone MCP-only external process talking to MAE the way `mae-mcp-shim` or VS Code
Copilot do — that would demote the whole exercise to "yet another external client," which is a
solved problem already (ADR-050 through ADR-056) and proves nothing new about whether MAE's
in-process Rust core genuinely supports more than one native frontend *application* sharing it
directly. The only shape that actually tests the vision's claim is a second binary, linked
against `mae-core` as a library the same way `crates/gui` and `crates/mae` are today, mutating
the identical KB/CRDT state the primary GUI mutates, at the same time, from a different process.

**The smallest genuinely-useful milestone is a spatial edit that round-trips through the existing
write path, not "draw arbitrary shapes."** It would be easy to scope this ADR's first shippable
increment as "an empty canvas app that can draw rectangles" — that is achievable in isolation and
proves almost nothing. The actual falsifiable claim ADR-057 needs tested is narrower and harder:
can a KB graph node, laid out and edited *spatially* in a second frontend, write back through the
*exact same* CRDT/KB mutation path (`kb_add_link`, `kb_update`) that text editing and the existing
GUI's KB graph view already use — not a parallel, canvas-specific persistence mechanism that
happens to write to the same database file. That is the concrete point at which "shares the
KB/CRDT core" stops being a design-doc assertion and becomes a testable architectural fact, and
it is why this ADR's Phase C (below), not an earlier "draw shapes" milestone, is the one this ADR
treats as the thesis-proving deliverable.

**Real-world precedent both validates the thesis and surfaces a genuine open problem this ADR
must resolve, not assume away.** AFFiNE/BlockSuite is the closest shipped precedent for the exact
claim this ADR tests: its `Y.Doc` — a yrs/Yjs document, the same CRDT family MAE already depends
on for `mae-sync` — is the single CRDT source of truth for both a document/text editor and a
whiteboard/canvas editor in the same product, with block-model updates and UI rendering flowing
through one shared code path regardless of which frontend surface mutated the data
(blocksuite.io/blog/document-centric.html, blocksuite.io/blog/crdt-native-data-flow.html). That
is real, production evidence the thesis is achievable, not merely theoretically plausible —
MAE is not proposing something no one has shipped.

But three independent, production canvas tools — tldraw, Excalidraw, and Figma — all deliberately
declined to use a general-purpose CRDT for spatial/position data specifically, and did so for a
reason that matters directly to MAE. tldraw's own documentation states plainly that "general-
purpose CRDTs aren't built for canvas data" (tldraw-tldraw.mintlify.app/sync/introduction), and
Figma's engineering blog describes their multiplayer sync approach as "inspired by CRDTs" but
deliberately not actually one, precisely because it can lean on a central server as the tiebreaker
for conflicting property writes on the same shape (figma.com/blog/how-figmas-multiplayer-technology-works/).
Excalidraw's collaboration layer follows the same shape — a reconciliation server resolving
property-level conflicts, not a leaderless CRDT merge. **MAE's local-first, no-central-authority
stance (CLAUDE.md principle #12) forecloses that exact escape hatch.** MAE has no server that is
always present and always authoritative to lean on as Figma does; the daemon is explicitly an
optional optimization (ADR-014, ADR-035), not a guaranteed always-on arbiter. That means this ADR
cannot simply assert "coordinates converge via yrs, the same way text already does" and treat the
spatial-conflict problem as already solved by inheritance from MAE's existing, well-proven text-
CRDT work (ADR-002, ADR-006, ADR-010). It must specify a concrete, deliberately decided primitive
for spatial data specifically, and that primitive must be validated before the broader
implementation work is built on top of it — which is exactly why Phase B′, below, exists as its
own gating phase rather than being folded silently into Phase C.

The recommended default hypothesis, consistent with yrs's own existing semantics and requiring no
new CRDT machinery: an LWW-register (last-write-wins) per coordinate. `VisualElement`'s existing
shape (`crates/core/src/visual_buffer.rs`) already stores `x`/`y`/`w`/`h` as independent `f32`
fields per element (see `Rect { x, y, w, h, .. }`); mapped into a `Y.Map` per element with one map
key per coordinate, each key is independently resolved by yrs's own native last-write-wins
per-key conflict rule — no custom merge function required. This matches, in spirit, what Figma's
centralized reconciliation achieves (a well-defined winner for a conflicting write to the same
property) without requiring an actual always-on central server, because yrs's per-key LWW
resolution is already a deterministic rule available for free from the CRDT library MAE already
depends on. But this is a hypothesis, not a decided design, until it is validated by a real
synthetic-concurrent-drag test — which is Phase B′'s entire purpose, and which must complete
*before* Phase C's broader work begins, not be discovered as a bug partway through Phase C's
implementation.

Separately, Martin Kleppmann's formally-verified "highly-available move operation for replicated
trees" algorithm (martin.kleppmann.com/2021/10/07/crdt-tree-move-operation.html) is the
established, already-solved answer to the reparent-vs-delete race Phase E's cross-frontend
collaboration work will hit — it is used in production by Loro (loro.dev/blog/movable-tree) for
exactly this reason. It should be the named, cited target correctness property for Phase E's
design, not an informally-described scenario MAE re-derives from scratch under time pressure —
this is CLAUDE.md principle #8 ("shared computation... if two renderers compute the same thing,
extract it") in its purest form, generalized one level: reuse a solved, formally-verified
algorithm from published research and a real production implementation, rather than reinventing
a weaker ad-hoc version of the same guarantee.

Two further concrete risks come from real, symptom-level evidence rather than speculation about
what *might* go wrong, and both must become named test cases rather than assumed-covered
incidentally by a general convergence test. First, AFFiNE's own live bug reports show connector/
binding elements losing sync with the shape they are attached to when one client moves the shape
(github.com/toeverything/AFFiNE/discussions/2713) — this is exactly the "structural typed-link
plus spatial position" combination Phase C introduces (a KB typed link whose endpoint is a node
now also carrying a canvas position), and it is named explicitly as a required Phase C test case
below, not left to be covered incidentally by a general N-way convergence test that was never
designed with that specific failure shape in mind. Second, Zed's own CRDT engineering writeup
(zed.dev/blog/crdts) states plainly that collaborative undo breaks the simple per-user-stack model
once multiple independent actors mutate the same document. MAE's existing per-user `UndoManager`
(CLAUDE.md principle #11) was designed and tested for concurrent *text* edits specifically; Phase
E's concurrent *structural* edits arriving from two genuinely different frontend applications — a
GUI user and a canvas-frontend user, not two windows of the same frontend — exercise a scenario
the existing undo design was never built or tested for. That scenario needs its own explicit test
for what "undo" should mean when a GUI user's undo would otherwise silently revert a
canvas-frontend user's subsequent, causally-later structural edit to the same node, rather than
being assumed safe by inheritance from a mechanism that has never actually been exercised this
way.

## Decision

Five phases, A through E, with a required design-spike phase B′ inserted between B and C and
gating C's start — not run concurrently with it.

**A — finish ADR-016's own currently-unshipped work as an explicit prerequisite, tracked under
ADR-016's own issues, not duplicated here.** ADR-016's Phase 2 (extracting transient overlays out
of `Mode` into a stacked `overlay_stack`) and Phase 3 (the full `ArtifactType`/per-artifact
modality axis, `register-artifact-type`/`register-modality`, and the `canvas`/`kb-graph` modules
those primitives enable) are load-bearing for this ADR: a second frontend that mutates canvas
CRDT state needs the same first-class, kernel-independent interaction model ADR-016 designs for
in-editor canvas buffers, and duplicating that design specifically for a second binary instead of
finishing ADR-016 once would be exactly the kind of parallel reimplementation CLAUDE.md principle
#8 forbids. This ADR references ADR-016's Phases 2-3 as a dependency it consumes; it does not
re-open or re-specify them, and no new issue should be filed here for work ADR-016 already owns.

**B — binary and crate structure, following ADR-014's established conventions exactly.** A new
`crates/canvas-frontend` binary crate, linking `mae-core` and the existing `mae-canvas` crate
(`crates/canvas/src/{scene,layout,interaction,kb_graph}.rs`) directly as libraries — the identical
pattern `crates/gui` (winit + Skia, linked into the `mae` binary via the `gui` feature) and
`crates/mae` itself already use: a thin binary crate composing shared library crates, not a
process that talks to the editor over IPC. This is deliberately **not** a new `BufferKind` inside
the existing GUI binary (that proves only "visual workflows exist inside the GUI," the smaller
claim rejected in Context above) and deliberately **not** a standalone MCP-only external process
(that would demote the deliverable to "another external client," failing the in-process-shared-
core requirement this ADR exists to prove). Per ADR-057's Gate W, `crates/canvas-frontend` is an
end-user client binary, and Gate W's cross-platform requirement — Linux, macOS, and Windows
release and CI targets from day one, unlike `mae-daemon` which stays explicitly Linux-only — binds
on this crate from its first commit, not as a retrofit; it reuses ADR-066's established client-
build/packaging pattern rather than re-deriving a separate one for a second client binary.

**B′ — spatial-position conflict-resolution design spike, required to complete before Phase C
begins.** This phase's sole deliverable is a decided, tested answer to how concurrent position/
size edits to the same visual element converge — the open problem the tldraw/Excalidraw/Figma
evidence in Context establishes MAE cannot assume away by analogy to its already-solved text-CRDT
convergence story. The LWW-register-per-coordinate hypothesis described in Context (`x`/`y`/`w`/
`h` as independent `Y.Map` keys, each resolved by yrs's native per-key last-write-wins rule) is
this spike's starting point, not its assumed conclusion. The spike's exit criterion is the
synthetic concurrent-drag test specified in Verification below passing deterministically — if the
LWW-per-coordinate hypothesis fails that test (for example, if independently-resolved x/y/w/h keys
converge to a combination no participant ever actually produced, such as one client's x paired
with another's y producing a position neither client dragged to), this phase's job is to find and
decide a corrected primitive — e.g. a single composite position key resolved atomically rather
than four independent scalar keys — before Phase C's implementation work begins, not after a bug
report from it.

**C — the KB-graph-as-canvas view becomes genuinely editable, and this is the thesis-proving
deliverable.** Moving or connecting nodes in `crates/canvas-frontend`'s canvas writes back through
the exact same typed-link (ADR-030) and node-property CRDT write paths `kb_add_link` and
`kb_update` already expose to every other caller — the same MCP tools the GUI's own KB graph view
(`BufferKind::Graph`, `crates/core/src/graph_view.rs`) and any AI peer already use, not a
canvas-specific write function that happens to update the same underlying tables. This is the
shared-core thesis ADR-057 flagged as unverified, made concretely falsifiable via the real
multi-writer convergence tests specified below. Phase C must explicitly include, as a named test
case rather than an incidental one, the AFFiNE-observed connector/binding-desync scenario: a
typed link's endpoint node is moved by one frontend (canvas position edit) while the link itself
is concurrently edited by another (a text-frontend `kb_add_link`/`kb_update` call) — the combined
"structural link plus spatial position" hazard the general convergence test was never specifically
designed to catch.

**D — generalize beyond the KB graph view specifically.** Freeform `VisualElement` composition
becomes its own artifact type in `crates/canvas-frontend`, backed by the same CRDT/KB persistence
a visual document already implies by design (`crates/core/src/visual_buffer.rs:1-46`'s existing
scene-graph shape) — not a new persistence format invented for the standalone case. This phase is
explicitly scoped after C, not before it, because C is what proves the write path is genuinely
shared; D extends that already-proven path to a new artifact shape rather than introducing a
second, unproven write path first and hoping it converges with C's later.

**E — cross-frontend collaboration: a GUI user and a canvas-frontend user editing the same KB
concurrently.** This is the hardest and most important proof point for the entire "other native
frontends" claim in ADR-057's vision, because it is the first place concurrent *structural* edits
— not just concurrent *text* edits, which yrs already handles well and MAE has already proven in
production — are genuinely exercised, from two different processes with two different interaction
models, not two windows of the same frontend. Reparent-vs-delete convergence (the GUI reparents a
link in the KB graph while the canvas-frontend simultaneously deletes that same link) must be
designed against Kleppmann's named, formally-verified algorithm cited in Context, not re-derived
informally from scratch. Phase E must also include the cross-frontend-undo semantics test named
in Context (the Zed lesson) as a required, explicit test — not an assumed-safe inheritance from
the existing per-user `UndoManager`, which was never designed or tested for concurrent structural
edits arriving from a second frontend application.

## Consequences

**Positive.** This is the first architectural proof, not just design-doc assertion, that MAE's
Rust core genuinely supports more than one native frontend application sharing its KB/CRDT engine
in-process — the specific claim ADR-057 flagged as unverified and materially more ambitious than
what MAE's docs currently assert. A successful Phase C closes that gap concretely: a spatial edit
made in one process and a typed-link edit made in another converge through one write path, proven
by real multi-writer tests rather than trusted by inheritance from the text-CRDT story. It also
forces ADR-016's own stalled Phases 2-3 to actually ship, since this ADR structurally depends on
them (Phase A) rather than being able to defer them indefinitely as "someday" work the way a
`Status: Proposed` ADR with no consumer can drift. And it produces, as a side effect of Phase B′,
a decided and tested spatial-CRDT primitive MAE did not previously need and does not currently
have any answer for — closing a real gap in MAE's CRDT story (text-only, today) rather than
merely adding a new frontend on top of an assumption that was never actually validated.

**Costs (honest).** This is one of the three largest child ADRs in the ADR-057 initiative, named
explicitly in ADR-057's own Consequences section as comparable in scope to the entire ADR-050
through ADR-055 external-editor MCP pairing initiative that preceded it — it should not be
compressed into a single implementation phase or treated as a quick follow-on to a smaller ADR.
Phase B′ is a genuine, open design problem, not a formality: the tldraw/Excalidraw/Figma evidence
in Context is three independent production teams choosing *not* to solve this the way MAE's
default hypothesis proposes, and there is a real chance the LWW-per-coordinate primitive fails its
own validation test and needs a second design iteration before Phase C can start, which would
push out this ADR's whole downstream schedule. A second client binary is a second thing Gate W's
Linux/macOS/Windows CI matrix must cover from day one (per ADR-057), and a second thing every
future `mae-core` API change must consider the blast radius of — a `mae-core` change that only
`crates/gui` and `crates/mae` currently need to accommodate now also has `crates/canvas-frontend`
as a third consumer whose specific spatial-CRDT and cross-frontend-undo assumptions (Phase E) make
it a more demanding consumer than a typical new caller, not merely one more entry in a linker
graph.

## Alternatives rejected

- **Ship this only as a `BufferKind::Visual` mode inside the existing GUI.** Rejected — this
  proves "visual workflows exist," which is real but strictly smaller than, and a different claim
  from, "a second native frontend," the thing ADR-057's row 9 actually identifies as unverified.
  Note that ADR-016 remains fully valid and complementary under this decision: it is not
  superseded or replaced, and its Phases 2-3 are consumed directly as this ADR's Phase A
  prerequisite, not duplicated or reimplemented.
- **A web/Electron app instead of a native Rust binary linked against `mae-core`.** Rejected — it
  would not share the core in-process at all; it would need its own separate sync/IPC layer to
  reach the real engine (the same category of "yet another external client" this ADR's Context
  section already rejects for the MCP-only-process alternative), and it directly contradicts the
  project's whole Rust-core architecture rationale (CLAUDE.md's "Rust over other cores" decision)
  by reintroducing exactly the kind of separate-process, separate-language boundary MAE's core
  design exists to avoid.
- **A parallel, frontend-specific reimplementation of KB-write logic specifically for the canvas
  frontend, instead of reusing the exact same `kb_add_link`/`kb_update` paths text editing already
  uses.** Rejected. Logseq's own documented real experience maintaining two parallel
  representations of essentially the same product — a file-based version and a database-based
  version — "doubled" their engineering cost and caused real regressions
  (discuss.logseq.com/t/why-the-database-version-and-how-its-going/26744). This is direct, real
  evidence that CLAUDE.md principle #8's discipline — one write path, shared by every frontend, no
  parallel reimplementation — is not optional architectural polish for this ADR; it is a
  load-bearing decision with a documented real cost when skipped, from a comparable project that
  actually paid that cost.

## Verification

Per CLAUDE.md principle #14, every phase below is verified adversarially — falsifying the
implementation, not confirming a single happy path in a fixed order.

- **Phase B.** A Cargo dependency-graph check (`cargo tree` or equivalent, run in CI) must confirm
  `crates/canvas-frontend` genuinely depends on `mae-core` as a linked library dependency in its
  own `Cargo.toml`/build graph, not merely that it talks to a running editor process over IPC or a
  socket. This falsifies "shares the core" as a bare, unverified claim in a design document — the
  test inspects the actual build graph, not a comment or a README assertion.
- **Phase B′ — the synthetic concurrent-drag test.** Two simulated clients independently set
  conflicting `x`/`y`/`w`/`h` values on the same `VisualElement` at overlapping/racing times (real
  concurrent writes against a shared `Y.Doc`, not a serialized before/after pair). The chosen
  primitive's convergence must be fully deterministic given the same input ordering and must match
  the deliberately decided semantics from the spike — not merely "doesn't crash." A test that only
  checks for absence of a crash would pass on a nondeterministic or silently-data-losing
  implementation just as easily as a correct one, so the assertion must pin the actual converged
  value(s) against the decided semantics. This test must exist and pass *before* Phase C's own
  tests are considered meaningful at all — Phase C's convergence tests would otherwise be silently
  validating structural (link/node) correctness on top of an undecided or broken spatial-merge
  foundation, which would make a passing Phase C test suite give false confidence about the whole
  system.
- **Phase C — a ≥3-writer convergence test (N-way per principle #14, not a 2-writer happy path).**
  Three concurrent writers act on the same node/link within one convergence window: the GUI
  text-edits a typed link, `crates/canvas-frontend` drag-edits that same link's endpoint node's
  spatial position, and an MCP client (e.g. `mae-mcp-shim` or an external agent) concurrently calls
  `kb_add_link` on the same node. All three writers' edits must converge with no writer's edit
  silently lost or silently overwritten without a defined resolution rule accounting for it. A
  real adversarial/malformed input — a NaN, ±∞, or absurd-magnitude coordinate value written by
  the canvas frontend, not a cherry-picked convenient one — must not corrupt the shared node's
  state as subsequently read back from the GUI or the TUI; the malformed value must be rejected or
  clamped at a defined boundary, never silently propagated into a state a text-only reader then
  chokes on. The AFFiNE connector/binding-desync case named in Decision (endpoint node moved by
  one frontend while the link itself is edited by another) must converge to a defined,
  non-dangling state — never a link pointing at nothing, and never a link silently retaining stale
  coordinates after its endpoint has moved.
- **Phase D.** Save/close/reopen — including across a `mae-daemon` restart, not just an in-process
  editor restart — must reproduce byte-identical `VisualElement` state via the shared persistence
  path. There must never be a parallel, frontend-specific persistence mechanism whose output could
  silently diverge from what the GUI or TUI would read for the identical underlying CRDT state;
  the test asserts byte-identical bytes on disk (or an equivalent canonical serialization), not
  merely "looks the same when rendered."
- **Phase E.** The GUI reparents a link in the KB graph while `crates/canvas-frontend` concurrently
  deletes that same link. The merge outcome must be a defined, explicitly tested resolution
  following Kleppmann's named algorithm — not an unspecified last-writer-wins that silently drops
  one side's intent with no signal to either user — matching the rigor CLAUDE.md's ADR-010 already
  established for text-CRDT correctness, extended here to structural graph edits for the first
  time. Separately, a GUI-issued undo must not silently revert a canvas-frontend user's subsequent,
  causally-later structural edit to the same node — the Zed cross-actor-undo lesson cited in
  Context, reproduced here as a real, executable test with two live processes, not merely assumed
  safe by inheritance from the existing per-user `UndoManager`, which has never actually been
  designed or tested for this cross-frontend-structural-edit scenario before this ADR.
