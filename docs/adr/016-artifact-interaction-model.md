# ADR-016: ArtifactType Axis & Interaction Model for Non-Text Artifacts

**Status:** Proposed
**Date:** 2026-06-15
**Supersedes:** None
**Depends on:** ADR-015 (unified keymap resolution chain)

## Context

MAE's `Mode` enum conflates three orthogonal concerns:

1. **Input modality** — `Normal` / `Insert` / `Visual` (vi editing discipline).
2. **Transient UI overlays** — `Command` / `Search` / `FilePicker` /
   `FileBrowser` / `CommandPalette` / `ConversationInput` (ephemeral input
   capture).
3. **Artifact-specific input** — `ShellInsert` (terminal raw input).

This text-centric model can't express MAE's direction. Per principle #11
(CRDT-first), non-text artifacts are first-class: canvas/visual documents are
`YMap`/`YArray` (`BufferKind::Visual`, the `mae-canvas` crate), KB nodes are yrs
docs. A canvas has its own modalities (select / draw / connect) with a
manipulation vocabulary (select-shape, move, resize, connect) that has nothing to
do with `Normal`/`Insert`. Today there is no clean seam to give such a buffer its
own interaction model — it would mean more `Mode` variants and more kernel
`match` arms.

ADR-015 established a unified, registry-driven keymap resolution chain. This ADR
extends that model to decouple the conflated axes and make non-text artifacts
first-class interaction surfaces.

## Decision

Model interaction as **four orthogonal axes**, all registry-driven:

| Axis | Meaning | Source |
|---|---|---|
| **Modality** | persistent input interpretation of the focused artifact | per ArtifactType |
| **ArtifactType** | the CRDT thing focused: `text` (YText), `visual`/canvas (YMap/YArray), `kb-graph` (yrs), `terminal` (PTY) | registered |
| **Buffer-kind context** | the buffer's role (git-status, file-tree, help, navigation, …) | registered (ADR-015) |
| **Transient overlay** | ephemeral, *stacked* input capture (command-line, search, palette, which-key keypad, completion) | registered overlay stack |

The resolution chain (ADR-015) gains an artifact-interaction layer and an overlay
layer: `[overlay] > [artifact-interaction] > [buffer-kind context] > [modality] >
[base]`.

### Transient overlays leave `Mode` (Phase 2)

`Mode` shrinks to input modality only. Transient UI becomes an `overlay_stack:
Vec<OverlayId>` with per-overlay `capture: bool` (capturing = command-line/leader
replace the chain; non-capturing = completion layers over the modality, so e.g.
completion-in-search composes — impossible with the current mutually-exclusive
enum). `leader_active` becomes the leader overlay on that stack. New primitives:
`register-overlay` / `push-overlay` / `pop-overlay`. `ShellInsert` becomes the
`terminal` artifact's raw modality.

### ArtifactType + per-artifact modalities (Phase 3)

Each artifact type declares its modalities and interaction keymaps from a module:

```scheme
(register-artifact-type "canvas" "select")
(register-modality "canvas" "select"  "canvas-select")
(register-modality "canvas" "draw"    "canvas-draw")
(register-modality "canvas" "connect" "canvas-connect")
(bind-context-keymap "kind" "visual" "canvas")
```

The `text` artifact seeds `normal`/`insert`/`visual` (everything today, unchanged
— it is the default artifact).

### CRDT-mutation contract + AI peer

An artifact interaction command **must mutate the underlying yrs doc**, never the
render mirror (the same contract text already follows: edits mutate `YText`, not
ropey). Because every human keybinding resolves to a command name and the AI
tool-call interface invokes the same command names, a canvas command bound to a
key is the identical command the AI peer calls via MCP — no separate "AI mode"
(principle: the AI is a peer). A test asserts canvas commands go through the CRDT
path.

## Consequences

- Non-text CRDT artifacts get first-class, flavor-independent interaction without
  kernel patches.
- `Mode` stops being a grab-bag; overlays compose (stack) instead of being
  mutually exclusive.
- One command surface serves human keys and AI tool-calls for every artifact.

## Status / Migration

Proposed; to be implemented as follow-up PRs after ADR-015's Phase 0/1 land:

- **Phase 2** — extract transient overlays from `Mode` (overlay stack +
  `#[deprecated]` `Mode::Command` shim during migration of ~30 call sites).
- **Phase 3** — `ArtifactType` axis, `register-artifact-type` /
  `register-modality`, `modules/canvas/` + `modules/kb-graph/`.

Risks: hot-path regression (mitigated by the ADR-015 chain cache + benchmarks);
`Mode` shim leakage (mitigated by `#[deprecated]` surfacing every call site);
overlay capture semantics (modeled explicitly + tested for completion-in-search).
