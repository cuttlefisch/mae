# ADR-018: Identity-Anchored KB Access Control (roles + join policy)

**Status:** Accepted (implemented on `feat/crdt-collab-validation`).
**Supersedes:** the membership/identity portion of ADR-017 (label-based KB
ownership/membership and the `kb/share` "creator mismatch" rejection). ADR-017's
host-key TOFU pinning, mTLS-as-identity, and the Ed25519 key handshake remain in force.

## Context

The two-machine collab run surfaced a structural flaw. Shared-KB ownership and
membership were keyed on a **mutable, non-unique human label string** (plus a
self-claimed `collab-user-name`), even though the daemon already verifies a
cryptographic identity per connection:

- The daemon derives a full `PeerIdentity { label, fingerprint, pubkey }` from the
  verified mTLS client cert, but only the **label** was used downstream
  (`authenticated_label()`).
- `KbCollectionDoc` stored `creator`/`members` as label strings; every access check
  was string equality on labels.
- `authorized_keys` allowed two distinct keys to share a label, and `revoke(label)`
  removed all of them. So distinct keys were indistinguishable for access control,
  and re-stamping a mismatched claim to a label **launders** identity rather than
  verifying it.
- There was no policy model (no roles, no permissive/restrictive); psk/none sessions
  bypassed membership. A self-claimed `creator` that differed from the label was
  *rejected*, breaking legitimate sharing (issue I-7).

## Decision

1. **Principal = the Ed25519 key fingerprint** (`SHA256:<base64>`). It is the **sole
   subject** for KB access control. The human label is a display alias resolved from
   the key; `collab-user-name` is cosmetic and never authoritative.
2. **Roles per KB**, keyed by principal: `owner | editor | viewer`, with **hierarchical
   inheritance** `owner ⊇ editor ⊇ viewer`.
3. **Per-KB join policy**: `restrictive | invite | permissive`, default **`invite`**.
   Non-member join: `restrictive`→deny, `invite`→record pending for owner approval,
   `permissive`→auto-add as **viewer** (least privilege).
4. On `kb/share` the daemon **derives the owner from the verified cert principal** and
   ignores any client-claimed creator — no rejection (I-7 removed).
5. `authorized_keys` enforces **label uniqueness**; revoke by fingerprint or unique
   label.
6. `none` mode is loopback-trusted (dev only); `psk` is a single coarse principal
   `psk:<keyid>`. Real per-identity policy requires `key` mode.

### CRDT schema (`KbCollectionDoc`, schema v2)

Root `collection` YMap: `schema=2`, `owner` (principal), `member_roles`
(YMap fingerprint → `{role,label}`), `join_policy`, `pending`
(YMap fingerprint → `{label,requested_at}`). Legacy `creator` (display) + `members`
YArray are kept read-only (a new key avoids a YArray→YMap type-swap hazard).

### Enforcement — `kb_access(kb_id, principal, op)` (complete mediation)

Every KB operation routes through one decision; no bypass.

| role \ op | Join | Read | Edit | Manage |
|---|---|---|---|---|
| owner | Allow | Allow | Allow | Allow |
| editor | Allow | Allow | Allow | Deny |
| viewer | Allow | Allow | **Deny** | Deny |
| non-member | by policy | Deny | Deny | Deny |

A raw `kbc:` collection write (`sync/update`) is owner-only (membership-smuggling
defense). Owner/membership/policy mutate only via the gated `kb/*` methods.

### Per-KB transport policy (P2P mesh — ADR-025)

`kb_access` gains a second dimension: the connection's **transport** (`Hub` for
the v0.14 TCP listener, `P2p` for the iroh mesh) is checked against the KB's
`transport_policy ∈ {Hub, P2p, Both}` (a new `KbCollectionDoc` key) **in addition
to** the role/policy table above. A KB is reachable over a transport only if its
policy exposes it there.

- **Absent ⇒ Hub-only** (conservative): enabling the mesh never silently exposes
  an existing hub share; a KB is mesh-reachable only once explicitly p2p-shared.
  Local-only KBs have no collection doc, so they carry no policy and are
  unreachable over any transport.
- **Owner-bypass** (correctness): the owner always reaches their own KB regardless
  of transport — otherwise a p2p-only KB would lock the owner out over their local
  editor's hub socket. Non-owner members + would-be joiners are transport-gated
  (a non-member's join over a non-exposed transport is denied *before* join policy).
- `transport_policy` is owner-set metadata (gated `Manage`), so `kb/share` **widens**
  it from the stored value (`transport_policy_raw()` distinguishes "never set" from
  an explicit Hub): a first p2p-share is `P2p`, while `kb-share` then `kb-share-p2p`
  becomes `Both`. Unlike membership (B-12 preserves it), transport is safe on re-share.

## Proven-architecture grounding

This is the canonical document-sharing model, not an ad-hoc design — we inherit its
guarantees:

- **NIST Hierarchical RBAC** (ANSI/INCITS 359-2012): roles + role inheritance; the
  table above is Core+Hierarchical RBAC over `{join,read,edit,manage}` on the KB.
- **Google Zanzibar / ReBAC** (Drive/Docs authz): `member_roles` is the relation-tuple
  set `kb:<id>#<role>@<principal>` for one object, with the subject anchored to a
  **stable, verifiable identity** (the key fingerprint) — exactly what Zanzibar's
  `@user` requires. Join policies map to Drive sharing modes (restricted /
  request-access / anyone-with-access).
- **OWASP Authorization**: deny-by-default; least privilege (permissive auto-grants
  *viewer*); **complete mediation** (single `kb_access` chokepoint); **server-side
  enforcement** (the daemon decides from the verified cert, never the client's claim);
  fail-secure; auditable (logs principal + label).

## Consequences

- Identity for shared KBs comes from a reliable, unique, cryptographic source; labels
  and `collab-user-name` cannot impersonate. A user's editor name need not match their
  admin-assigned label.
- Configurable sharing posture (restrictive/invite/permissive) + read-only viewers.
- **Migration:** existing v1 collections (label creator + members) are read with
  `migrate_if_legacy(resolver)` (resolves labels→fingerprints via `authorized_keys`,
  unresolved → transitional `legacy:<label>`). A v1 collection also degrades gracefully
  (no crash) and is fully re-bound when the owner re-shares (`set_owner`).
- **Scope / future work** (explicitly out of this ADR): ABAC attribute rules; full
  Zanzibar infra (global tuple store, cross-object hierarchy inheritance); wiring the
  `authorized_keys` resolver into the daemon's collection load for automatic v1 access
  preservation without a re-share; a `collab_kb_default_policy` editor option threaded
  into the shared collection.

  **Tracked:** #73 (wire authorized_keys resolver into daemon collection load) · #74 (collab_kb_default_policy editor option).

## Verification

Unit: `mae-mcp` (label uniqueness, revoke-by-fp, distinct-principals), `mae-sync`
(v2 schema, roles, policy, pending→approve atomicity, 2-client merge, migration).
Daemon: `kb_access` matrix + abuse cases (spoofed creator, label collision,
viewer-edit, removed-member, raw-collection smuggling, none-mode). E2E:
`scripts/collab-membership-e2e.sh` over real mTLS (join → pending → approve → join).
