# ADR-065: KB + daemon drift corrections (federation health, scheduler ticks, daemon-mode docs, transclusion parity)

**Status:** Accepted.
**Extends:** ADR-029, ADR-030, ADR-031, ADR-035, ADR-054.
**Tracking:** issues #390, #391, #392, #393 (one per item, filed independently of the
`ADR-057` epic tracker — see "No dependency" below).

No dependency on ADR-057's ratification — these are bug fixes (principle #15), not new
architecture, and should ship first, independently of whether any of the other proposed ADRs
in this set are accepted.

## Context

During the broader MAE architecture review that produced ADR-057 through ADR-066, four concrete
implementation bugs surfaced. Each is a real, verified drift between an already-accepted ADR's
design and the current code — exactly the case CLAUDE.md principle #15 describes ("bugs are
drift signals, not just defects... check whether root cause traces to a place where
implementation fell behind an already-decided ADR or a tracked epic issue... fix the drift for
that whole feature area"). None of the four requires any new architectural decision to be made
first; each is independently real, independently fixable, and should be prioritized and merged
on its own schedule rather than gated behind the larger initiative's design debate.

1. **`FederatedQuery::health_report` only reports the primary instance.**
   `shared/kb/src/query.rs:359-361` is a one-line pass-through — `self.primary.health_report()`
   — even when additional federated instances are registered. Its sibling method in the exact
   same `impl` block, `id_title_body_triples` (`:341-357`), correctly iterates `self.instances`
   and merges results, proving the aggregation pattern is already known-correct in this file and
   simply wasn't applied consistently to the health-check method.

2. **Two of three daemon scheduler ticks are literal no-op stubs.** `daemon/src/scheduler.rs`'s
   `run()` method drives three `tokio::select!` arms on independent intervals. `health_tick`
   (`:72-108`) is fully wired: it runs a real hygiene scan off the async executor via
   `spawn_blocking` (per ADR-054's ban on blocking a shared worker thread with a synchronous
   CozoDB scan) and logs results. `watcher_tick` (`:64-67`) and `maintenance_tick` (`:68-71`)
   each contain only a `// TODO` comment and a counter increment — the scheduling
   infrastructure itself is proven correct by `health_tick`; it is specifically these two tick
   bodies that were never filled in.

3. **`daemon_mode` is undocumented in `docs/EXTERNAL_EDITOR_MCP_PAIRING.md`.** The document has
   zero mentions of the `daemon_mode` option (`off`/`on-demand`/`shared`, ADR-035) despite the
   daemon's involvement being directly relevant to how external-editor pairing behaves in
   practice — whether KB queries from a paired agent hit the in-process embedded KB or a shared
   daemon instance is exactly the kind of operational detail a reader wiring up VS Code/Copilot
   pairing needs and currently cannot find.

4. **Transclusion (`#+TRANSCLUDE:`) composition is never re-derived on MCP-driven writes.** The
   `#+TRANSCLUDE:` org directive composes a meta-node's body from its member nodes'
   (`shared/kb/src/cozo_store/blocks.rs::compose_meta_body`). Today that re-derivation is
   triggered from exactly one call site — `crates/core/src/editor/kb_ops/dispatch.rs:321`, on
   exit from the `kb-widen` narrowed-buffer-editing workflow (i.e., only when a human edits a
   transcluded member through the file/narrow-buffer path). `kb_create_node` and
   `kb_update_node` (`crates/core/src/editor/kb_ops/nodes.rs:198-216,449-462`), the functions
   backing the MCP `kb_create`/`kb_update` tools, never call it — confirmed by tracing both
   down to `CozoKbStore::insert_node`/`update_node`
   (`shared/kb/src/cozo_store/kb_store_impl.rs:16-20`), which call `update_links_for_node` (the
   ADR-030 typed-link re-derivation) but nothing analogous for transclusion. This is a direct
   asymmetry with the sibling typed-link path: `update_links_for_node`'s own doc comment
   describes exactly this class of conformance gap and states the fix pattern — re-derive from
   the same write path every write goes through, not only from one UI-specific call site. A
   transclusion-composed node created or edited via `kb_create`/`kb_update` therefore silently
   has a stale or entirely-missing composed body, with no error and no indication anything is
   wrong. This is a real, user-visible bug in exactly the AI-peer-authoring workflow (principle
   #3: "the AI is a peer, not a plugin") this whole project's vision cares most about.

## Decision

1. **Mirror `id_title_body_triples`'s aggregation pattern in `health_report`.** Iterate
   `self.instances` in addition to `self.primary`, merging into a response shape that reports
   **per-instance** health, not a single merged-only summary. A merged-only response would hide
   exactly which specific federated instance is unhealthy — defeating the entire purpose of a
   health check, whose value lies in pinpointing the problem, not confirming that *something*
   in the federation is fine.

2. **Implement `watcher_tick` and the deterministic half of `maintenance_tick`.**
   `watcher_tick`: drain pending file-watcher events and trigger incremental reimport of
   changed files. `maintenance_tick`: integrity check, statistics collection, and compaction —
   explicitly **not** the AI-enrichment sweep. That sweep is a separate, larger new capability
   claimed by ADR-061 Phase C on this same tick function; this item and ADR-061 Phase C must
   coordinate (via review of each other's diff, not independent landing) so the two efforts do
   not implement conflicting logic in the same `tokio::select!` arm.

3. **Document `daemon_mode` in `EXTERNAL_EDITOR_MCP_PAIRING.md`.** Add a section covering the
   three modes and their effect on where a paired external agent's KB queries actually land.
   Cross-link with ADR-060 Phase G's own multi-tenant-deployment documentation addition to the
   same file so the two documentation efforts land as one coherent addition rather than
   diverging or duplicating each other.

4. **Call the meta-node re-derivation from the same write path `update_links_for_node` is
   already correctly called from.** Reuse the established fix pattern (principle #8 — don't
   invent a second one for a structurally identical problem): trigger `compose_meta_body`-driven
   re-derivation for any node that is itself (or is a member referenced by) a `#+TRANSCLUDE:`
   composition, from `CozoKbStore::insert_node`/`update_node`, the same trait-level chokepoint
   `kb_create_node`/`kb_update_node` and the `kb-widen` UI path both already funnel through.

## Consequences

**Positive.** Each fix closes a real, independently-verifiable gap between documented/assumed
behavior and what the code does today. Item 1 makes federation health checks actually useful
for a multi-instance deployment. Item 2 makes the daemon's scheduler do the maintenance work its
own architecture already assumes it does. Item 3 removes an operational blind spot for anyone
configuring external-editor pairing against a shared daemon. Item 4 removes a silent correctness
bug from the AI-peer authoring path specifically — the path this project's design principles
treat as first-class, not secondary to human editing.

**Costs.** Item 1's response-shape change (single value → per-instance breakdown) is a
call-site-visible API change for any consumer of `health_report`, both internal (the `kb_health`
MCP tool / `*KB Sharing*`-adjacent surfaces) and any external tooling built against the current
shape — call sites need updating alongside the fix, not after. Item 2 introduces real I/O
(file-watcher drain, reimport, integrity/compaction) onto a previously inert tick; if either
body's runtime grows unexpectedly, it competes with `health_tick`'s already-`spawn_blocking`'d
scan for the same interval-driven scheduling loop and may need its own `spawn_blocking` treatment
for the same ADR-054 reason. Item 4 adds a lookup ("is this node a `#+TRANSCLUDE:` member of some
meta-node?") to every `insert_node`/`update_node` call, not only ones that touch transclusion —
this must be cheap (indexed, not a full scan) or it becomes a write-path tax paid by every KB
write regardless of whether transclusion is in use anywhere in that KB.

## Alternatives rejected

- **Bundling each fix into its most topically-related larger ADR** (e.g., folding item 4 into
  ADR-059, items 1/2 into ADR-060/ADR-062) instead of a standalone tactical ADR. Rejected: these
  four fixes are independently real and independently shippable. Reviewers and issue-filers
  should be able to prioritize and merge each one immediately without waiting on ratification of
  the much larger, more contested architectural decisions proposed in the other nine ADRs in
  this set. Bundling them in would create an artificial dependency that delays real,
  already-understood bug fixes behind unrelated design debate — precisely the outcome this
  standalone ADR exists to avoid.
- **Leaving `health_report` merged-only (return an aggregate boolean/summary instead of
  per-instance detail).** Rejected — cheaper to implement, but defeats the diagnostic purpose of
  the check; considered and rejected within item 1's own decision above, not treated as a
  separate live alternative.

## Verification

Adversarial where applicable, per CLAUDE.md principle #14 — each case below is chosen to test
whether the fix actually does what it claims, not to confirm the happy path.

1. Register a federation with a healthy primary and a second instance whose backing store is
   corrupted or unreadable. Confirm `health_report`'s output surfaces the second instance as
   unhealthy, individually — not silently omitted from an aggregated result, and not merely
   swapped in place of the primary's own (still-healthy) report.
2. For both ticks: verify that an interruption mid-tick (mid-reimport for `watcher_tick`,
   mid-scan for `maintenance_tick`) cannot leave the daemon in a state where resuming
   double-applies already-completed work or silently drops pending work — see the Status note
   below for how this is verified in practice (a direct proof of each tick's idempotency
   property, since a real process kill cannot be simulated deterministically for a
   `spawn_blocking` task in a unit test).
3. Documentation-only item — verified by a link-check / doc-review pass confirming the new
   `daemon_mode` content is accurate and correctly cross-linked with ADR-060 Phase G's addition
   to the same file, not by a code test.
4. Author a node with `#+TRANSCLUDE:` via `kb_create` (the MCP/AI authoring path, not file
   import). Then, in a **separate**, second `kb_update` call, edit the transcluded member node.
   Then re-read the composing node. `compose_meta_body`'s output must reflect the member's
   *edited* content, not merely its content as it existed at creation time — a shallow fix that
   only re-derives once, at creation, would pass a weaker single-step test but fail this one;
   the two-call structure is the point of the test.

## Status note (implementation, principle #15's "not just a symptom patch")

All four items are implemented, tested, and verified in both directions (each adversarial test
was confirmed to genuinely fail against the pre-fix code before the fix landed, per this
project's established verify-both-directions discipline) — `cargo fmt --check`/`cargo clippy
--workspace --all-targets -- -D warnings`/`cargo test --workspace` clean across both the editor
and daemon workspaces.

Two honest scope corrections surfaced during implementation, on evidence, not assumption:

- **Item 2's "compaction" is deliberately NOT implemented.** Investigation found Cozo's Rust API
  is Datalog-only with no compaction/VACUUM primitive exposed for either its sqlite or sled
  storage engines; reaching around Cozo to issue a raw `VACUUM` against its backing SQLite file
  risks violating invariants Cozo's own storage layer expects to hold — a real, unverified risk,
  not a stylistic omission. `daemon/src/maintenance.rs`'s module doc comment states this
  explicitly. `maintenance_tick` ships with its stats + integrity-check half only; compaction is
  left for a follow-up once a safe primitive exists.
- **Item 2's "kill mid-tick" verification is a proof of the idempotency property, not a literal
  process kill.** `tokio::spawn_blocking` tasks cannot be faithfully interrupted mid-flight in a
  deterministic unit test (Tokio runs them to completion on their OS thread regardless of
  `abort()`), so the adversarial tests instead prove the actual property that makes an
  interruption safe: `watcher_tick`'s reimport is idempotent by construction (`IngestMode::Full`
  re-derives every node from current file content and upserts by id), and `maintenance_tick`'s
  scan is read-only (no partial-write state can exist to reconcile). Both properties are
  exercised directly — a partial-then-full reimport sequence converges correctly
  (`scheduler.rs`'s `watcher_tick_reimport_converges_after_a_partial_then_full_pass`), and
  repeated scans of unchanged content produce identical results
  (`maintenance.rs`'s `maintenance_scan_is_stable_across_repeated_runs`).

Also found and fixed while implementing item 2: the daemon's own `instance_stores`/
`registry.instances` were confirmed to have **zero production population path** — populated
only in test scaffolding today. `watcher_tick`/`maintenance_tick` are written to iterate
whatever's actually registered (correctly a no-op today against the daemon's real, single
`daemon-kb.cozo` primary store, which is populated via collaborative RPC writes, not an org
directory) and activate automatically the moment a federated, org-directory-backed instance is
ever registered — no further changes needed when that lands (tracked separately under ADR-060).
