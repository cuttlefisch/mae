# ADR-015: Unified Layered Keymap Resolution Chain

**Status:** Accepted
**Date:** 2026-06-15
**Supersedes:** None
**KB Source**: `concept:adr-keymap-resolution`

## Context

MAE's keymap system computed "which keymap(s) apply right now" three different
ways over the same data:

- **Dispatch** (`key_handling/normal.rs`) used `current_keymap_names()`'s flat
  `(primary, Option<fallback>)` pair — try the primary keymap, then the single
  fallback.
- **which-key** (`editor/mod.rs::merged_which_key_entries`) merged that same flat
  pair.
- **describe-bindings** (`editor/option_ops.rs`) walked `Keymap.parent` as an
  N-level chain *and* appended the fallback.

They agreed only by convention: every overlay keymap's `parent` happened to equal
the hardcoded `fallback`, and every chain happened to be exactly two deep. A
3-deep chain (e.g. `git-log → git-status → normal`, which the data model already
permits) would **dispatch and display differently** — a latent correctness bug.

Worse, the routing itself was hardcoded: `current_keymap_names()` was a Rust
`match` on `Mode` + `BufferKind` + `Language`, and `BufferMode::keymap_name()`
hardcoded 8 of 16 buffer kinds, with org/markdown wired inline behind a comment
("stays hardcoded until `Language::keymap_name()` exists"). Adding a new context
— a shared "navigation" mode for read-only buffers, or interaction keymaps for
non-text CRDT artifacts (canvas, KB-graph) — meant editing the kernel `match`,
violating MAE principle #7 (no hardcoding; no kernel patch to extend) and #6
(runtime redefinability).

## Decision

Adopt **one resolver, one chain**, sourced from a **data-driven registry**.

### One chain for dispatch + display

`Editor::keymap_chain() -> Vec<String>` returns the ordered resolution chain,
most-specific layer first: the primary keymap plus its `parent` ancestry,
followed by the fallback plus its ancestry (deduped, cycle-safe). All three
consumers iterate this one chain:

- dispatch: the first layer returning `Exact` **or** `Prefix` wins (a higher
  layer's `Prefix` must not be shadowed by a deeper layer's binding, or multi-key
  sequences like `dd`/`gg`/`SPC b b` break);
- which-key: merge entries across the chain (a more-specific layer shadows a
  deeper one);
- describe-bindings: collect bindings across the chain.

`Keymap::lookup` stays the single-map hot primitive (no parent walk inside it).
`Keymap.parent` is demoted to "how a layer declares its next link" + introspection
sugar — resolution reads the chain, never the embedded pointer directly. For the
existing 2-deep keymaps the chain is byte-identical to the old behavior, so the
change is behavior-preserving while making divergence structurally impossible.

### Data-driven, Scheme-registerable routing

A `KeymapRegistry` on `Editor` maps buffer context (`BufferKind` / `Language`) to
the context keymap that overlays the modality keymap. It is **kernel-seeded**
(`KeymapRegistry::kernel_defaults()`, derived from the legacy
`BufferMode::keymap_name()` table so a bare `mae-core` with no Scheme behaves
identically), re-seeded on `reset_keymaps_to_kernel`, and extended by modules via
the Scheme primitive `(bind-context-keymap SELECTOR-TYPE SELECTOR-VALUE KEYMAP)`
(`"kind"` / `"language"` selectors), wired through the existing
SharedState-queue + `apply_to_editor` drain. New contexts are modules, not kernel
edits.

The first consumer is the shared **`navigation`** context: a kernel-created
keymap (parent `normal`) that read-only nav buffers (dashboard, file tree,
modules, help, git-status, agenda, debug, shell) route through — flavor-
independent movement (`j/k` + `C-n/C-p` + arrows) and both `SPC` and `C-;`
opening the leader keypad, so the doom and non-modal flavors behave identically
there.

### Performance

The chain is built per keystroke from O(1) lookups; for the current shallow
keymaps it allocates a small `Vec<String>` (2–3 short names) at human typing
cadence — negligible. A later phase interns keymap ids and memoizes the chain
(invalidated only on mode/buffer/overlay/language/registry change) to make the
hot path zero-alloc.

## Consequences

- Dispatch and display can never disagree; arbitrarily deep chains are correct.
- New buffer kinds, languages, and shared contexts are added from Scheme modules
  with no kernel patch (principle #7), live-redefinable (principle #6).
- Sets up the artifact-interaction model for non-text CRDT buffers (see ADR-016):
  the same chain mechanism carries an artifact-interaction layer.

## Migration

- **Phase 0** (done): introduce `keymap_chain` and route all three consumers
  through it; behavior unchanged.
- **Phase 1** (done): data-driven `KeymapRegistry` + `bind-context-keymap`;
  navigation context; reload-pipeline unification; `keymap_flavor` authority.
- **Phase 2 / 3** (ADR-016): extract transient overlays from `Mode`; add the
  `ArtifactType` axis and per-artifact modalities.
