# Collaborative KB Sync — Bug Chain & Testing-Methodology Lessons

> Working record from the two-machine (alice ⇄ daemon ⇄ bob) live validation of
> shared-KB CRDT sync on `feat/crdt-collab-validation` (ADR-020). Captures the bugs
> found, **why every one of them was invisible to the existing test suite**, the
> observability that made live debugging tractable, and concrete requirements for a
> robust e2e harness so this class never recurs.
>
> Companion docs: `docs/adr/020-replicated-kb-crdt-artifact.md` (design),
> `docs/collab-test-notes-alice.md` / `-bob.md` (raw run logs).

## 1. The bug chain — a layered failure

The feature failed end-to-end, but as **six distinct bugs stacked on one pipeline**.
Each fix unblocked the next layer — a hallmark of a path that was never exercised
as a whole. In edit-flow order (emit → wire → receive → merge):

| # | Layer | Root cause | Fix | Commit |
|---|-------|-----------|-----|--------|
| **B-8** | Emit (editor→daemon) | `kb/node_update` hand-rolled as a JSON-RPC **notification** (no `id`); the daemon routes no-`id` messages to a notification sink that only relays `sync/awareness` → silently dropped before the apply+broadcast handler. | Single shared wire builder (`mae_sync::wire`); `kb/node_update` is a **request** (carries `id`); daemon loud-`warn!` on a dropped known-method notification. | `d1e04cee` |
| **B-13** | Receive subscribe | A joined member never added the KB docs to the editor's `shared_docs` set, so inbound `sync_update` for `kb:<node>` hit the "ignoring unsubscribed doc" arm and was dropped. | ShareKb/KbJoin now subscribe owner+member to `kbc:<kb>` + each `kb:<node>` (mirror the text-buffer subscribe-on-join). | `4602ce4b` |
| **B-14** | Merge lineage (join) | yrs merges on **lineage** (client_id + op history), not the node-id string. Two peers that independently built the same node (both imported the org fixture) have incompatible lineages; their `title`/`body` `YText`s are different yrs objects at the same map key → merge no-ops (map LWW discards one side) → `changed=false`. | `KnowledgeBase::adopt_remote_node` rebuilds the node from the owner's encoded state on **join** (adopt lineage, don't merge same-id siblings). | `490d9a3c` |
| **B-15** | Emit content | `upsert_with_crdt`, when the node already had a `crdt_doc`, rebuilt from the OLD bytes and **ignored the new title/body fields** → every edit after the first re-broadcast stale content (identical `update_len` on the wire). | Apply the edited fields onto the existing lineage via `set_title`/`set_body`. | `490d9a3c` |
| **B-16a** | Merge lineage (owner) | The B-14 adopt only converges the **member** on join. The **owner's** `to_collection` built the share payload via `node.to_crdt_doc()`, which for a never-edited node mints an **ephemeral, non-persisted** lineage each call (and is `&self`, so it can't persist) → owner's local doc never matched the lineage peers adopted → a peer's edit no-opped on the owner (bob→alice). | `kb_prepare_share_lineage` establishes + persists a canonical `crdt_doc` for every shared node (with write-through) **before** encoding the payload. | `1652fcf4` |
| **B-16b** | CRDT identity | `kb_update_node` hardcoded `client_id = 1` for **every** peer → two peers indistinguishable to yrs → concurrent same-node edits collide in client-1's clock space and diverge. | Derive a **stable, unique** `client_id` per peer from the durable collab identity fingerprint (`derive_kb_client_id`), set once at startup. | `1652fcf4` |

Still open (membership/UX, not core sync): **B-12** (owner re-share is a destructive
`share_doc` delete+replace on the daemon → silently revokes approved members + resets
the node lineage on every owner restart; fix = CRDT-merge on re-share, not reset);
the main-thread stall during join; and the `*Collab Status*` launch-steal (B-11).

## 2. The meta-pattern — why every bug was invisible to tests

This is the important part. Each bug had a passing test suite around it. The suite
passed because **the tests didn't reproduce production conditions** — five distinct
anti-patterns:

1. **Parallel-implementation tests (B-8).** The one KB wire e2e was `#[ignore]`d AND
   used a *hand-rolled* client that sent `kb/node_update` **with** an `id` (the
   correct shape) while production sent it without. The test validated a parallel,
   correct implementation — not the shipping path. → **A test must drive the same
   serialization production uses.** Fix: one shared `mae_sync::wire` builder used by
   both the editor emit path and the e2e client.

2. **Shared-lineage tests (B-14, B-15).** *Every* CRDT merge test created **one** doc
   → `encode_state()` → applied to a doc built `from_bytes()` of **those same bytes**.
   That guarantees a shared lineage — the one condition under which the bug cannot
   occur. The realistic case (two **independently constructed** same-id docs) was
   never modeled. → **Construct independent peers with independent doc histories.**

3. **Dummy-value masking (B-16b).** Tests hand-picked **distinct** client_ids
   (`alice=1`, `bob=2`) while production hardcodes `1` for **both**. *A test that
   passes different standin values than the code uses cannot catch a hardcoded-value
   bug* — the test supplied the very correctness (distinct ids) the code was missing.
   → **Drive the production path so the test inherits whatever the code hardcodes,
   and assert the parameter actually VARIES across peers.**

4. **Single-direction coverage (B-16a).** alice→bob (member receive) was exercised;
   bob→alice (owner receive) was not — and the owner path had its own divergence. →
   **Test both directions, and concurrent edits, explicitly.**

5. **Wrong-altitude assertions (B-8, B-13).** KB tests asserted `pending_kb_updates
   == 1` (editor-side **enqueue**) — never the wire round-trip, daemon apply, or peer
   **receipt**. An enqueue that is dropped on the wire still passes. → **Assert
   convergence (the peer's content changed), not that an update was emitted.**

6. **Safe-range stand-ins (B-17), found by the N-peer harness.** Every prior CRDT test
   hand-picked **tiny** client_ids (`1`, `2`, `0xA11CE`) — all comfortably inside yrs's
   53-bit `ClientID` range. Production derives the id from a 64-bit FNV-1a hash of the
   identity fingerprint, which routinely sets bits above 53. yrs `ClientID::new`
   debug-panics on that and, in *release*, silently force-sets/strips the top 11 bits —
   so two fingerprints differing only above bit 53 collide on one lineage (a B-16-class
   bug, invisible in release). The ADR-022 N-peer harness derives each peer's id with the
   **real** `derive_kb_client_id` over distinct fingerprints, so the first run panicked
   immediately. → **Tests must source values from the production derivation, not
   hand-picked constants — a constant chosen to "look realistic" still dodges
   value-range bugs the real generator hits.** (Same family as #3, one level deeper: not
   just *which* value but its *range*.)

A useful razor distilled from #1, #3, and #6:
> **If a test passes parameters or serialization that differ from what the production
> code path produces — including values from a different generator or numeric range —
> it can only test a parallel universe, never the shipping bug.**

## 3. Observability that made live debugging tractable

Two-machine CRDT bugs are nearly impossible to reason about analytically (we were
repeatedly wrong about yrs semantics; the live + unit evidence corrected us). What
worked:

- **A single greppable trace target, `kb_sync`**, threading one edit end-to-end on the
  editor: `kb edit: broadcast-gate decision {gate_hit}` → `drain: send kb/node_update
  {rowid,bytes}` → `bg: written to wire (awaiting apply-ack) {req_id}` → `daemon
  confirmed applied` → `ack: durable row removed`. This localised B-8 to the *wire*
  in minutes (editor said "written", daemon said nothing).
- **Symmetric daemon `info!`**: `kb/node_update: received` / `: applied wal_seq=…`,
  and a loud `warn!` when a request-only method arrives as a notification. The
  presence/absence of `received` cleanly separates "never arrived" from "rejected".
- **An honest `changed` signal** (`content_hash` before/after the yrs apply), NOT a
  hardcoded `true`. `changed=false` on a delivered update was the fingerprint of the
  lineage bugs (B-14/B-16) and is what let us distinguish "dropped" from
  "applied-but-no-op". Guard this honesty — a stubbed success would have hidden
  B-14/B-16 entirely.
- **Live MCP + log tailing**: drive edits via the MCP `kb_update` tool on one editor,
  tail the daemon log from a marked baseline line, and `kb_get` the peer's copy. The
  `update_len` on the wire being byte-identical across edits is what exposed B-15.

## 4. Requirements for a robust e2e harness (the deliverable)

A KB-sync e2e that would have caught all six bugs — encode these as the contract:

1. **Two independent peers.** Distinct identities → distinct derived client_ids; each
   constructs its copy of a node **independently** (don't seed bob from alice's
   bytes). This is the single most important property — it's the difference between
   shared-lineage (always passes) and divergent-lineage (the real world).
2. **Drive the production path, not a parallel one.** Edit via the real edit API
   (`kb_update_node` / the MCP `kb_update` tool); serialize via the shared
   `mae_sync::wire` builders. Never hand-roll a "correct" message or hand-pick a
   "correct" parameter the production code wouldn't produce.
3. **Real wire + real daemon.** Round-trip through `handle_client` (the daemon's
   request/notification dispatch), not an in-process shortcut that bypasses framing
   and method routing. Assert the daemon **applied** (`{applied:true}` / `wal_seq`
   advanced) AND that a second connected client **received the broadcast**.
4. **Both directions and concurrent.** member→owner AND owner→member; plus concurrent
   same-node edits that must **converge to one value on both peers** (this is what
   catches a shared/hardcoded client_id — see `concurrent_edits_diverge_under_shared_
   client_id_but_converge_under_distinct`).
5. **Lifecycle.** join (adopt) → live edit (apply) → **restart survival** (durable
   reload) → **offline-merge** (edit while disconnected, converge on reconnect) →
   re-share/membership (B-12).
6. **Regression markers that assert the BROKEN behavior.** e.g. "two peers with the
   SAME client_id diverge" and "a `kb/node_update` without an `id` is dropped (hangs
   awaiting a response)". These fail loudly if the code regresses to the dummy value,
   catching it in CI instead of a two-machine run.

### Where this lives today (the seeds)

- `shared/kb/src/lib.rs`:
  - `divergent_lineage_merge_noops_but_adopt_converges` — two independent peers,
    adopt-on-join + chained edit (B-14/B-15).
  - `two_peers_editing_same_node_converge_through_distinct_client_ids` (B-16, fix).
  - `concurrent_edits_diverge_under_shared_client_id_but_converge_under_distinct`
    (B-16, the dummy-value regression marker).
- `crates/core/src/editor/kb_ops.rs`:
  - `prepare_share_lineage_persists_canonical_doc_so_owner_converges` — owner path
    (B-16a), drives the editor.
- `crates/mae/tests/collab_tcp_e2e.rs` / `daemon/tests/collab_e2e.rs`:
  - `kb_node_update_applies_and_broadcasts_to_peer` — real wire round-trip via the
    shared builder (B-8); proven to FAIL (hang) when the builder omits the `id`.
- `shared/sync/src/wire.rs`: `all_request_builders_carry_an_id` — the mechanical net.
- **`crates/core/tests/kb_sync_n_peer_e2e.rs` (ADR-022) — the deliverable.** N real
  `KnowledgeBase` peers with distinct **production-derived** client_ids, an in-test hub,
  driving the real CRDT path: share/join/bidirectional/concurrent/offline/restart for
  N∈{2,3,5}, plus the crash crux as sibling tests (`lost_row_adopt_clobbers_documents_the_bug`
  vs `lost_row_reconcile_converges`). This harness **found B-17** on its first run (item 6).
- `daemon/tests/collab_e2e.rs`: `kb_join_with_svs_returns_reconcile_diff_else_full_state`
  — the **real daemon** kb/join SV-reconcile path (diff vs full state + backward-compat).

### Closed (was: the gap)

The flagship **two-independent-peers + bidirectional + concurrent + restart + offline-merge**
coverage now exists at the editor-logic altitude (the N-peer harness above) and the daemon
SV-reconcile path is covered by a real-process-handler e2e. Together they make a two-machine
run a **confirmation rather than a discovery**. Remaining live validation: **T3c-stress**
(edit-then-instant-`kill -9` within the sled flush window) + a 3rd peer, which exercises the
one residual window (content never flushed) that no in-process test can model.
