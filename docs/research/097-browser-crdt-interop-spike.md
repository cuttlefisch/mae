# Phase 0 spike: can a browser Yjs client converge with a real `KbNodeDoc`?

**Status:** Run and passing, 2026-08-05, against `main` at `33282fb9`.
**Gates:** ADR-099 (bidirectional sync transport) and ADR-100 (the browser edit surface).
Neither should be written before reading this.
**Artifacts:** `shared/sync/tests/browser_interop.rs` (Rust — owns every assertion),
`shared/sync/tests/browser/driver.mjs` (Node — plays the browser, asserts nothing).

**Bottom line up front.** The load-bearing assumption **holds**. Stock `yjs` 13.6.32, with no
MAE code and no shim, reads a real `KbNodeDoc` as live shared types, edits it at UTF-16 offsets
that land exactly where Rust expects, and converges byte-identically with two concurrent native
writers under every apply order. One genuine constraint was discovered and is recorded below;
it changes how the browser's reader must be written, not whether the approach works.

## Why this was built first

Every part of the browser-KB design assumes a browser can hold the *same* CRDT document the
native editor holds. If UTF-16 offsets, the yrs v1 update format, or the nested shared-type
layout did not survive the crossing into another runtime, the design would need a fundamentally
different shape — a server-side rendering model, or a translation layer with its own
convergence semantics. That is a bad thing to discover after writing the transport ADR.

The spike deliberately does **not** build a WebSocket, a UI, or a daemon route. Those are
known-solvable engineering. The unknown was the data crossing, so that is all this tests.

## Method

Shape borrowed from `crates/export/tests/browser/`, the repo's existing Layer-2 pattern. Rust
generates fixtures from real `mae-sync` types and owns every assertion; a Node subprocess plays
the untrusted second runtime using the stock `yjs` package. The Node half asserts nothing about
convergence, so it cannot launder a Rust-side bug into a pass.

Test inputs were chosen to break naive implementations rather than flatter them: a non-BMP emoji
(2 UTF-16 code units, 4 UTF-8 bytes), CJK text (1 unit, 3 bytes), an ADR-030 typed link, and a
`:PROPERTIES:` drawer of the kind `shared/kb/src/org.rs` really stores inside bodies. Client ids
are realistic derived values, never `1` — `crates/core/tests/kb_sync_n_peer_e2e.rs` documents
`client_id = 1` stand-ins as the anti-pattern that let a real convergence bug hide.

## What was proven

| Claim | Test | Result |
|---|---|---|
| A browser sees a real node as **live CRDT types**, not a flattened decode | `browser_reads_a_real_kb_node_doc_as_live_shared_types` | **Holds.** `title`/`body` arrive as `Y.Text`, `tags`/`links`/`aliases` as `Y.Array`, `props` as `Y.Map`, with content identical to Rust's view including the emoji, CJK and drawer. |
| A browser edit at a **UTF-16 offset** lands where Rust expects | `a_browser_edit_at_a_utf16_offset_lands_where_rust_expects_it` | **Holds.** The insertion point sits immediately after the non-BMP emoji, so a byte- or char-offset implementation would land elsewhere and fail. |
| **3 writers converge under every apply order** | `three_writers_converge_identically_under_every_apply_order` | **Holds.** Browser + native GUI + MCP client, three distinct client ids, all 6 permutations converge to an identical full materialization — and each writer's intent survives (no silent loss). |
| The merge does not **duplicate the untouched base** (the #625 oracle) | `merging_a_browser_edit_does_not_duplicate_the_untouched_base` | **Holds.** A whole-document-replace implementation would pass a naive "both edits present" check while doubling everything around them; this catches that. |
| An **offline** browser edit does not clobber a concurrent native edit | `an_offline_browser_edit_does_not_clobber_a_concurrent_native_edit` | **Holds.** The browser edits a stale base while the native side advances; reconnect merges both. |
| A **hostile/corrupt** browser update cannot corrupt the document | `a_hostile_browser_update_is_rejected_without_corrupting_the_document` | **Holds.** Empty, garbage, all-`0xff` and `[0,0]` inputs neither panic nor mutate state. |
| The convergence oracle **can fail** | `the_convergence_oracle_detects_a_dropped_browser_update` | **Holds.** Negative control, in the spirit of `MAE_E2E_NEGATIVE=1` in `scripts/collab-encrypted-e2e.sh`. Without it every assertion above would be unfalsifiable. |

The suite also reports whether the Node harness was present (`interop_harness_is_present`), so a
silently all-skipped run cannot masquerade as green. During development one test genuinely
failed before being corrected, which is direct evidence the suite executes rather than skips.

## The finding: a browser must probe containers, never the schema marker

`schema_v` (ADR-093) is stamped **lazily** — only by a v2 setter (`set_kind`, `set_aliases`,
`set_properties`, … via the scalar/array/map setters in `shared/sync/src/kb/node.rs`).
`KbNodeDoc::new` eagerly seeds the v2 *containers* `aliases` and `props` per ADR-093 D4, but
never stamps the version.

So a freshly created node is **structurally v2 and reports v1**. The browser sees no `schema_v`
at all, while `aliases` and `props` are present and usable.

This is defensible on the Rust side: every reader already tolerates an absent key,
`schema_version()` returns 1 by design, and not stamping avoids churning a replicated document
on an unchanged save. It is a trap for a second runtime writing a reader from scratch — which is
exactly what the browser is, and exactly why the spike found it.

**Rule for the browser reader, pinned by `a_browser_must_probe_containers_not_the_schema_marker`:
probe the container, never the marker.** The marker, once present, is trustworthy — the test
asserts that setting any real v2 field makes `"2"` visible to the browser.

This is closely related to, but distinct from, the separately-filed defect where
`upsert_with_crdt`'s fresh-lineage branches (`shared/kb/src/lib.rs:1104-1120`) omit
`write_v2_fields`. That one is a bug; this one is a documented semantic that a naive second
implementation would get wrong.

## What this does *not* prove

Stated plainly so the gated ADRs do not over-claim:

- **Nothing about transport.** No WebSocket, no daemon route, no authentication was exercised. The updates crossed as files. ADR-099 still owns the transport decision and must not cite this spike as evidence for a specific one.
- **Nothing about multiplexing.** The plan's "N docs over one connection, connection count stays 1" criterion needs a real transport and remains open.
- **Nothing about scale or latency.** Single small node, no concurrency load. ADR-054's ~8-concurrent-session measurement stands unrevised.
- **Nothing about the edit surface.** Whether org content can be *usefully* edited in a browser — the live-preview question — is ADR-100's problem and needs its own spike. This proves only that the bytes and offsets survive.
- **Nothing about `awareness`.** MAE's awareness is a custom notification shape (`shared/sync/src/awareness.rs`), not the y-protocols binary encoding; collaborative cursors were not tested.

## Reproducing

```bash
cd shared/sync/tests/browser && npm install     # one-time; node_modules is gitignored
cargo test -p mae-sync --test browser_interop
```

Without `npm install` the interop tests skip rather than fail, and
`interop_harness_is_present` says so on stdout.
