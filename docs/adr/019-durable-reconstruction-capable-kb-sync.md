# ADR-019: Durable, Reconstruction-Capable Shared-KB Sync

**Status:** Accepted (implemented on `feat/crdt-collab-validation`).
**Extends:** ADR-017 (host-key TOFU + mTLS), ADR-018 (identity-anchored access
control). ADR-018 secured *who* may access a shared KB; this ADR makes the KB
*content sync itself* actually work and survive long-running / multi-device
restart lifetimes.

## Context

Two-machine validation proved ADR-018 access control worked, but **shared-KB
content edits never propagated** — the core collaborative feature was
non-functional end to end. Root cause:

- The broadcast gate `editor.collab.shared_kbs: HashMap<kb_id, HashSet<node_id>>`
  was the **sole** input to `kb_update_node`'s decision to emit a
  `kb/node_update`. It was populated **only** by live `KbShared`/`KbJoined`
  events and **never reconstructed** on connect/reconnect/restart. The buffer
  path already did reconnect-resync; KBs did not.
- The share path **never wrote durable markers**: `KbInstance.collab_id`/`shared`
  stayed false and the registry was not saved, and the primary KB had **no
  `KbInstance` at all**. So nothing on disk recorded "this KB syncs."
- The **receive** path mirrored a write-side federation bug: an incoming
  `kb:<node>` update carried an empty `kb_id` and was always applied to
  `editor.kb.primary`, never the owning federated instance.
- **Joined KBs were dumped into `primary`**, not registered as instances — so
  they were absent from `kb_instances` and a joined peer's edits couldn't even
  resolve their owning KB to emit.

Net effect: any editor restart (and every joined guest) silently stopped
propagating, even though the durable daemon still held the shared collection.

## Decision

1. **The durable source of truth for "what syncs" is owning-instance state**, not
   the transient `shared_kbs` cache. A KB syncs iff its `KbInstance` carries
   `shared=true`/`collab_id` (or, for the primary KB, the new registry fields
   `primary_shared`/`primary_collab_id`). These are persisted to the XDG-first
   `kb-registry.toml`; the guest also has `kb/shared/<slug>/meta.toml`.
2. **The broadcast gate reads the durable markers.** `kb_update_node` derives
   "is this a shared-KB edit + which collab id" via `kb_collab_id_of(owner)` from
   the registry, so a shared KB keeps emitting across restart. `shared_kbs` is
   demoted to a reconstructable node-id index/cache.
3. **The share path stamps the durable markers** on a confirmed `KbShared`
   (primary fields or `instance.shared/collab_id`), then saves the registry.
4. **Joined KBs are first-class registered instances** (`kb_register_joined_instance`):
   nodes live in an addressable instance with a CozoKbStore under the shared-KB
   data dir, durable markers, present in `kb_instances`. Idempotent re-join.
5. **Reconstruction on startup + reconnect.** `reconstruct_kb_sync_gate()`
   rebuilds the cache from durable markers locally (no daemon round-trip — emit
   already works from the markers). On `Connected`, the editor re-subscribes
   (re-join) every durably-shared KB so it resumes **receiving**, queued via a
   `reconnect_intents` VecDeque drained one-per-tick through the existing single
   intent slot (idempotent via `subscribed_kbs`, cleared on disconnect).
6. **Receive routes to the owning KB.** `kb_apply_remote_update` resolves the
   owning instance (or primary) and applies + persists there; a new node routes
   by its node-id namespace prefix hint.
7. **First-class traceability** (not throwaway logging): `MAE_LOG=kb_sync=debug`
   greps one edit end-to-end; `introspect` exposes per-KB sync state + the
   "durable marker present but gate empty" divergence; daemon label resolution is
   live (I-10 tail).

## Design principles honored (CLAUDE.md)

#1 concurrency (tolerant KB row-load no longer stalls the main thread, B-5);
#7 Scheme-first (`collab_kb_sync_mode` option, no magic constants);
#9 downstream impact (per-phase tests; low-blast-radius additive intent queue);
#10 multi-client safety (idempotent re-subscribe; daemon membership authoritative);
#11 CRDT-first (reconstruction re-shares full `encode_state`; offline edits in the
durable pending queue re-apply and converge); #12 local-first (markers on disk;
reconstructs offline); #13 cross-platform (XDG-first KB path, B-6 — also a
correctness fix: the marker save + load paths must match).

## Consequences

- Shared-KB edits propagate **both directions** and **survive editor restart**
  (durable gate) and reconnect (re-subscribe). Joined KBs are addressable and
  appear in `kb_instances` (fixes B-3).
- **Migration:** old registries lack the new fields — all `#[serde(default)]`, so
  load is backward compatible. A pre-existing shared KB simply needs one re-share
  to stamp its durable marker. Nodes already joined into `primary` by the old
  path are superseded on the next join (idempotent instance registration);
  a startup migration of stranded `primary` federation nodes is future work.
- **Open operational concern:** daemon idle-eviction (`doc_store::evict_idle`) can
  delete an idle node doc with no connected clients; a subsequent partial update
  could diverge. Mitigation: reconstruction re-shares full state; exempting
  collab-KB docs from eviction while a member is registered is future work.

## Verification

Unit (mae-core/mae): gate fires from a durable marker with an **empty** cache
(the restart scenario); unshared KB does not emit even with a stale cache;
join registers a first-class instance (node in instance, not primary); receive
routes a new/existing node to the owning instance; `reconstruct_kb_sync_gate`
rebuilds from markers; share/join persist + reload markers. mae-mcp: live-revoke
verifier. Live (MCP, two machines): `introspect` shows the gate; alice edits a
`collabtest` node → daemon `kb/node_update` → bob converges (and vice-versa);
restart alice's editor → edits still propagate.
