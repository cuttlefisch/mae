# ADR-092: One write path for a KB node

**Status:** Proposed. Design accepted; implementation phased (Phases 0–6, each independently
shippable with a test that fails first). Phase 7 — folding node front matter into the body CRDT —
is deferred to its own ADR and is what currently bounds Decision 4.
**Extends:** ADR-029 (KB source of truth = CRDT, CozoDB = a deterministic rebuildable projection —
this ADR is that decision's **write** side; nothing here may write Cozo directly). ADR-029 states
the write-path inversion as a goal; this ADR names the single function that realises it and
enumerates every path that must move onto it.
**Relates to:** ADR-030 (in-text link grammar — the source text a human edits is that grammar, and
the inverse must be identity on it, never a normaliser), ADR-035 (editor↔daemon boundary — the
chosen chokepoint is already the one MCP tools call, so it is RPC-shaped without further work),
ADR-073 (the Proposed read-only HTML KB view; a future write-capable evolution reuses this seam
rather than inventing a second one), ADR-033 (KB-wide operation coordination — a single-node edit
is deliberately *not* a bulk operation and takes no lease).

## Context

A KB node created by `kb_create`, `(kb-create)`, or received over CRDT has **no human edit surface
at all**. `Editor::help_edit_source` (`crates/core/src/editor/help_ops.rs:1355`) resolves
`kb_node_source_file(id)` and, absent a file, reports `"No source file for '{id}'"`. `SPC n e` is
the only "edit this node" affordance, so on a KB whose nodes were never ingested from disk it is a
dead end. MAE is about to host shared KBs for many users, where clients hold no files at all — so
that is the ordinary case, not an edge case.

Designing the fix showed the missing surface is the **smaller** half of the problem. There are five
paths that write a node's content, and exactly one is CRDT-correct:

| Path | CRDT op | Evidence |
|---|---|---|
| `kb_update_node_with` (MCP `kb_update`, Scheme, commands) | yes | `kb_ops/nodes.rs:499` |
| buffer `:w` → `kb_reimport_file` | **no** | `file_ops.rs:319-336` — no `kb_sync_target`, no enqueue |
| org-dir watcher drain | **no** | same shape as above |
| `kb_widen_meta` | **no** | `kb_ops/dispatch.rs:322-347` mutates `primary` directly |
| meta-body recompose | **no** | `kb_ops/dispatch.rs:344` |

The consequence for a hosted deployment is concrete: on a host that shared a *file-backed* KB,
editing the `.org` and pressing `:w` writes the local store and **never broadcasts**, silently
clobbering whatever a peer's CRDT edit had already landed. `kb_widen_meta` additionally matches only
`self.kb.primary.get_mut(...)` with no `else` branch and then reports success
(`dispatch.rs:325`/`:369`) — an edit to a member living in a federated instance is discarded while
the user is told it was saved.

Underneath all of them sits a correctness bug in the CRDT layer itself. A node's `title` and `body`
are `YText` (`shared/sync/src/kb/node.rs:37`), but `set_body` (`node.rs:190`) and `set_title`
(`node.rs:165`) perform `remove_range(0, len)` + `insert(0, new)` — a wholesale replace. Two peers
editing the same body from a shared base converge, lose neither edit, and **duplicate the entire
untouched base**:

```
Line one.     ← shared base
Line two.
From A.
Line one.     ← shared base, duplicated
Line two.
From B.
```

Two people editing a 500-line node concurrently produce a 1000-line node with everything doubled.
No test caught this because the two "convergence" tests only ever have peers edit *different*
fields — `three_client_concurrent_edits_converge` (`node_tests.rs:248`) says so in its own comment —
and the one same-field test, `two_clients_merge_body` (`node_tests.rs:71`), asserts only
`a.body() == b.body()`. That oracle is worthless here: **CRDT gives convergence for free**. The
meaningful oracle is that the shared base appears exactly once.

The fix already exists in-tree and is used by the buffer layer: `TextSync::reconcile_to`
(`shared/sync/src/text.rs:533`), a character-level LCS diff whose UTF-16 offsets already match what
`set_body` computes. KB nodes never got it — two implementations of "set text on a YText", one
correct (principle #8).

Finally, the obvious-looking edit surfaces are both unusable, and knowing *why* is what forces this
ADR's Decision 3:

- **The rendered KB buffer cannot be made writable.** `strip_kb_body_noise` (`help_ops.rs:377`)
  removes `:PROPERTIES:`/`:LOGBOOK:` drawers and leading `#+` keywords before display, and
  `render_kb_body` (`help_ops.rs:420`) rewrites `[[TARGET][DISPLAY]]` down to bare `DISPLAY`,
  keeping the target only in a `KbLinkSpan` whose byte offsets go stale on the first keystroke.
  Reading the rendered rows back would silently destroy every link.
- **`node_to_org` ∘ `parse_org` is not a round-trip.** `parse_org` (`shared/kb/src/org.rs:75`) sets
  `body` to `rewrite_links(content)` — the *entire file*, front matter included — and hardcodes
  `NodeKind::Note`, dropping `kind` and `aliases`; `todo_state`/`priority` are emitted by
  `node_to_org` but never parsed back. Re-serializing doubles the front matter on every save.

## Decision

1. **`kb_update_node_with` (`crates/core/src/editor/kb_ops/nodes.rs:499`) is the sole node-content
   mutator.** It alone carries `kb_write_blocked`, thin-startup mirror hydration, owner resolution
   across primary ∪ federated instances, seed-node refusal, and the `kb_sync_target` CRDT-vs-direct
   branch. Every other path — buffer save, watcher drain, meta widen, meta recompose — routes
   through it. A new content-write path that does not is a defect, not a variant.

2. **CRDT text mutation is incremental, never wholesale.** A `YText` two peers can edit is updated
   by character-level diff. The diff core is extracted from `TextSync::reconcile_to` into a single
   shared function consumed by both the buffer layer and `KbNodeDoc`, so the correct behaviour has
   exactly one implementation.

3. **The human edit surface is the node's *normalized org source text*** — not its rendered view,
   and not necessarily a file. A new serialize/parse pair in `mae-kb` (not `mae-core`, so the daemon
   and any future HTTP write surface reuse it) replaces the non-round-tripping existing functions,
   and must be **identity** on whichever in-text link grammar it was handed rather than a normaliser.

4. **What is editable is bounded by what actually syncs.** `KbNodeDoc` carries only
   `id/title/body/tags/links/meta` (`shared/sync/src/kb/node.rs:17-22`) and `apply_crdt_doc`
   (`shared/kb/src/lib.rs:431`) writes back only title/body/tags. Until Phase 7 folds front matter
   into the body CRDT, the properties drawer is **not offered** for editing on a shared node.
   Offering a field that silently never reaches peers would manufacture permanent divergence — worse
   than the current dead end, because it looks like it worked.

5. **Surface selection is configurable, and the default reproduces today's behaviour exactly**
   (principle #7). A file-backed node in an unshared KB keeps opening its file, byte-identical. The
   node buffer is what fills the gap where there is no file, or where the file is an import artifact
   of a shared KB.

## Consequences

**Positive.** A KB node becomes editable by a human on a hosted, file-less deployment — the gap this
started from. Two live data-loss paths close: the shared file-backed `:w` that never broadcasts, and
`kb_widen_meta` silently discarding federated-instance edits while reporting success. Concurrent
editing of the same node body stops corrupting it. Four CRDT-blind write paths collapse to one, so
the next feature that writes a node inherits seed protection, owner resolution and sync for free
rather than re-deriving them. Because the chokepoint is the same function MCP `kb_update` already
calls, human and AI reach identical behaviour with no new tool (principle #3), and a future HTML
editor (ADR-073) has a seam to bind to instead of a second write path to invent.

**Costs, honestly.** The editable scope is deliberately *narrower* than a file: properties, kind,
todo state and priority are excluded until Phase 7, so "edit a node" does not yet mean "edit
everything a `.org` file can express" — and that must be stated in the UI, not discovered. A new
`BufferKind` carries the usual obligations (keymap, mode gating, close/reindex handling). The
character-level diff is O(n·d) where the wholesale replace was O(n); node bodies of file-ingested
nodes are whole files, so this needs measuring rather than assuming. And Decision 1 means several
migrations land as *behaviour changes* — a seed member that previously appeared to widen
successfully now fails, correctly.

**Explicitly not in scope.** Wiring the projector (`daemon/src/projector.rs` — `Projector::new` and
`set_change_feed` still have zero non-test callers, and `ProjectionStores` has no production
implementation) is ADR-029's **read** side and stays tracked separately. This ADR does not make
daemon-side queries reflect CRDT edits; it makes the edits correct and single-pathed.

## Alternatives rejected

- **Make the rendered KB buffer writable in place.** Rejected on evidence: the render pipeline is
  lossy in two independent ways (drawer/keyword stripping, and link markup rewritten to display
  text), no body byte-range is recorded anywhere, and the link spans that survive are static offsets
  invalidated by the first edit. Building the inverse renderer needed to make this safe is strictly
  more work than serializing the node, and it would be a second representation to keep correct
  forever.
- **Keep the file as the edit surface and make it a projection** (save routes through the CRDT;
  remote edits rewrite the `.org`). Rejected because it serves none of the deployments this is for:
  a browser cannot reach the notes directory, an external editor paired over MCP has no guarantee
  the notes directory is in its workspace, and a hosted participant has no files at all. It also
  requires maintaining a permanent bidirectional text↔CRDT bridge, which is precisely where the
  lineage-severing and stale-file bugs already live.
- **Reuse `node_to_org` / `parse_org` directly as the serialize/parse pair.** Rejected on
  inspection, not taste: `parse_org` returns the whole file as the body and drops `kind`, `aliases`,
  `todo_state` and `priority`, so a save cycle doubles the front matter and silently discards
  fields. The new pair exists because these two cannot be composed, and fixing them in place would
  change the meaning of the org *ingest* path that many nodes already depend on.
- **Encode node identity in the buffer name**, as `kb-narrow` does. Rejected because that precedent
  is broken: `parse_narrow_buffer_name` (`kb_ops/dispatch.rs:296`) splits at the *first* colon, so a
  namespaced id — which every real id is — parses wrong across the whole input domain. A typed
  buffer view is used instead, matching `BufferView::Kb(Box<KbView>)`.
- **Offer the full org serialization including the properties drawer from day one.** Rejected per
  Decision 4 — the CRDT does not carry properties, so on a shared node those edits would persist
  locally, never sync, and survive every inbound remote update, leaving peers permanently and
  silently disagreeing.

## Verification

- **The concurrent-edit oracle is selective, never peer-equality.** Three peers edit the same body
  from a shared base; every apply order is exercised; the assertion is that the shared base appears
  **exactly once** *and* every edit survives. This test must fail before Decision 2 lands — it does
  today.
- **Round-trip is a property, not an example.** The serialize/parse pair round-trips field-by-field
  over a fixed adversarial corpus (non-ASCII, every link grammar including the `user:` namespace and
  a `?rel=` metadata suffix, a `#+begin_src` block containing a fake link, a `:LOGBOOK:` drawer, a
  body whose first line is `#+…`, empty body, CRLF, mixed-case property keys) plus generated inputs.
  Negatives must fail: no `:ID:`, two `:ID:`s, an unterminated drawer.
- **Every migrated path proves it now broadcasts.** For the buffer save, the watcher drain and
  `kb_widen_meta`: on a shared instance, the write enqueues a CRDT update per affected node id; and
  a peer edit followed by a local save loses neither (selective oracle on both edits).
- **The negative cases must be refused, at open time where possible.** Editing a seed node, editing
  a `cmd:` node (whose content is live command metadata present in no node body), and a save whose
  parsed `:ID:` does not match the node being edited.
- **A save that fails must not lose the user's text.** Parse failure leaves the buffer open,
  `modified` true, and its text byte-identical, and does not fire `after-save`.
- **Human and AI produce identical state.** An AI-driven `buffer_write` + save and a human `:w`
  yield byte-identical node content.
- **The default must be a no-op for existing users.** Every existing `help_edit_source_*` test
  passes unmodified under default option values, plus an explicit test that the defaults reproduce
  pre-change behaviour.
