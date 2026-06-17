# ADR-020: Shared KB as a Durable, Replicated CRDT Artifact (+ management UX)

**Status:** Accepted (implementing on `feat/crdt-collab-validation`).
**Extends:** ADR-017 (host-key TOFU + mTLS), ADR-018 (identity-anchored access control),
ADR-019 (durable emit gate, first-class join instances, reconnect reconstruction).

## Context

ADR-019 made the broadcast *gate* durable, but live two-machine testing proved the collaborative
KB feature is still not functional end-to-end:

- **B-8 — edits don't propagate.** An owner edit fires the gate, enqueues, and the editor drain
  sends `CollabCommand::KbNodeUpdate` to the command channel, but the daemon never receives/applies
  it (the node doc stays at the same `wal_seq`, then idle-evicts). The update is **silently lost in
  the editor→wire path**: `write_framed`'s result is discarded; a teardown/reconnect race routes the
  command to a silent-drop arm; the durable SQLite queue is acked before the wire write; and the
  daemon never counts the editing session as a connected client of the node doc, so the doc
  idle-evicts (observed at `idle_secs=347`).
- **B-10 — joined KBs lose their nodes on restart.** A collab-*joined* instance is created with an
  empty `org_dir`; the startup loader gates on `org_dir.exists()` and skips it ("KB instance dir
  missing, skipping") even though its CozoDB store exists on disk.
- **Lossy join.** `KbNodeDoc`/`KbCollectionDoc` already support CRDT state-vector merge, but join
  *inserts/overwrites* nodes instead of merging — a member's offline edits are lost on (re)join.
- **No replication-mode axis.** A shared KB should be `replicated` (durable local + bidirectional)
  or `hosted` (no local store; daemon-live) — today a broken-`replicated` silently degrades.
- **No management UX.** Users have no way to see their shared KBs, members, roles/permissions, or
  edit the topology. (`*Collab Status*` is plain text and also auto-steals the dashboard on launch.)

## Decision

A shared KB is a **durable, replicated CRDT artifact**. Four concrete decisions:

1. **Emit-pipeline durability contract — queue → send → confirm → ack.** A `kb/node_update` is never
   silently lost: in-memory updates are persisted to the durable SQLite queue before send; the
   background task acks a queued row only **after** the wire write succeeds; a write failure or a
   no-writer/disconnected state **re-queues** (never drops). The daemon counts the editing/sharing/
   joining session as a connected client of the collection + node docs (`track_client_connect` +
   `subscribe_doc`) so live docs are not idle-evicted while a member is connected (symmetric
   `track_client_disconnect` on teardown).

2. **Unify register + join into one durable instance + a disk-first startup loader.** A member's KB —
   whether locally registered or joined over collab — is the *same* first-class instance with a real
   durable `db_path`/CozoDB store. The startup loader reconstructs an instance from its store when
   `db_path` exists (even with an empty `org_dir`), and rescans the shared-KB data dir to recover
   instances missing from the registry (migration for old dir-less rows).

3. **Merge on join/reconnect, never overwrite.** The member applies joined/rejoined node + collection
   state via CRDT `apply_update` (idempotent yrs merge), mirroring the text-buffer reconnect path, so
   offline edits survive. (A daemon-side `kb/join` state-vector *diff* is a bandwidth optimization —
   full-snapshot **merge** is correctness-complete.)

4. **Explicit `replicated | hosted` replication mode (distinct from the `on_save|manual` sync-trigger
   axis), with a loud status taxonomy.** `replicated` = durable local + bidirectional. `hosted` =
   no local store; reads routed to the daemon query layer (hosted *edit* is deferred — see Future
   Work). Per-KB status is one of `Replicated{healthy}` / `Hosted` (replication disallowed by policy)
   / `ReplicationFailed{reason}` (mode is `replicated` but the local store is missing/empty) — never
   a silent empty instance.

**Management UX.** A basic **magit-style `*KB Sharing*` buffer** (the assembly of MAE's existing
interactive-buffer infrastructure — a `BufferKind` + view struct + `render_common` span module +
buffer-local Scheme keymap parented on `navigation` + cursor-context dispatch reusing the existing
collab command/intent action layer) lists shared KBs (mode + durability status), members + roles, and
pending requests, with at-point actions (share/unshare, add/remove member, set role, set policy, set
mode, approve). Emacs `customize` is the conceptual model; the git-status (magit) buffer is the
implementation pattern. Inline customize-style field editing is roadmap (Future Work).

## Root-cause addendum (live-validated, Stage 1.5)

Live two-machine testing localised B-8 to a **wire-protocol divergence**, not the durability
mechanics that Decision 1 first anticipated. The editor's whole emit path was correct end-to-end
(`gate_hit:true → drain → bg: written to wire`), yet the daemon received nothing: `kb/node_update`
was hand-rolled in the background task **as a JSON-RPC notification (no `id`)**, so the daemon's read
loop routed it to the notification handler (which only relays `sync/awareness`) and **dropped it
before the apply+broadcast request handler** — while text `sync/update` carried an `id` and worked.
The reason no test caught it: the one KB e2e was `#[ignore]`d *and* used a hand-rolled client that
sent the **correct** (id-bearing) shape — the test exercised a parallel implementation, not the
shipping one.

Fixes (all under Decision 1's "queue → send → confirm → ack" umbrella):
- **Single shared wire builder.** `mae_sync::wire` is now the one source of truth for the collab
  JSON-RPC messages, used by the editor emit path **and** the daemon e2e. `kb/node_update` (and
  `kb/share`/`kb/join`) are requests (carry an `id`); a unit test asserts every request builder has an
  `id`. Production and tests can no longer diverge on protocol shape.
- **Real ack on the daemon response.** The durable SQLite row is acked only on the daemon's
  `{applied:true}` (`KbUpdateAcked`), with an in-flight rowid set preventing re-send storms and
  cleared on disconnect; an error response surfaces loudly (`KbUpdateFailed`). The old code acked on
  local channel-send (before the wire), and enqueued to *both* the SQLite queue and an in-memory Vec
  (double-send) — both fixed (single-source enqueue).
- **Daemon defense-in-depth.** A request-only doc method arriving as a notification is now a loud
  `warn!` ("DROPPED … missing `id`"), so this class of regression is caught immediately, not chased.
- **Wire round-trip e2e.** `daemon/tests/collab_e2e.rs::kb_node_update_applies_and_broadcasts_to_peer`
  drives a real `kb/node_update` to the real handler and asserts both `applied:true` and that a second
  joined client receives the broadcast. Verified to **fail** (hang on the never-answered request) when
  the builder omits the `id`, and pass with it.

## Sequencing

Stage 1 (emit-pipeline hardening + merge-on-join + durable instances) lands first and is validated by
a live two-machine re-test gate (bidirectional propagation + restart survival + offline-merge). Stage
2 (replication mode + status, the Collab-Status launch fix, the `*KB Sharing*` buffer, and the
flagship e2e) follows once Stage 1 is green.

## Consequences

- Shared-KB content edits propagate both directions and survive editor + daemon restart; offline
  edits merge instead of being overwritten; joined KBs are durable + reloadable + visible in
  `kb_instances`; users can see and manage sharing through a first-class buffer; replication failures
  are loud, never silently degraded.
- **Migration:** old registries with dir-less joined instances are repaired by the disk-first loader
  + shared-KB rescan; `SharedKbMeta` gains a `replication_mode` field (`#[serde(default)]`).

## Future Work (deferred — tracked, not dropped)

- **D1 — Full `hosted` live-EDIT.** This ADR plumbs the mode + routes hosted *reads* + the loud
  status. Hosted *edit* (reads + writes against the daemon, no local store, for terabyte-scale KBs)
  needs a daemon edit-RPC surface, conflict/latency handling, and a separate write path through the
  daemon query layer.
- **D2 — Daemon `kb/join` state-vector diff.** Switch `kb/join` from full-snapshot to a true SV diff
  (member sends per-node state vectors; daemon replies `encode_diff_and_sv`, which already exists) as
  a bandwidth optimization once full-snapshot merge is proven.
- **D3 — Richer `*KB Sharing*` UX.** Inline customize-style value widgets / field editing, live
  member-presence, per-node topology view, role-change confirmations, and diff/preview of pending
  topology changes.
- **D6 — Configurable eviction policy.** Phase 1 defends against idle-eviction of live collab-KB docs;
  making the daemon idle-eviction threshold (and "exempt collab-KB docs while a member is registered")
  a configurable daemon option is deferred.

(D4 — `kb-edit-source` on joined nodes, and D5 — a `registry.save` clobber guard — are tracked as
follow-up tasks outside this ADR.)

## Verification

Unit: emit-pipeline ack-only-on-confirm + no-drop-on-disconnect; daemon `connected_clients` tracking
+ no idle-eviction while a session holds the doc; merge-on-join preserves a local edit; disk-first
loader reconstructs a joined instance; replication-status classifier. E2E (`daemon/tests/collab_e2e.rs`):
`kb_replicated_sync_full_lifecycle` — bidirectional propagation, no eviction, daemon-restart recovery,
offline-merge no loss, hosted read-routing. Live (two machines, MCP): edits propagate both ways +
survive restart, traced via `introspect` / `MAE_LOG=kb_sync=debug`.
