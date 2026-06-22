# ADR-021: Durable, Auditable Membership & Policy (compliance foundation)

**Status:** Accepted (architecture-supporting). The **full feature is tracked separately**
(append-only audit log + policy engine); this ADR fixes the compliance *foundation* and the
architectural guardrails so that feature slots in without rework.
**Extends:** ADR-017 (host-key TOFU + mTLS), ADR-018 (identity-anchored access control),
ADR-020 (replicated KB CRDT artifact; B-12 membership-preservation fix).

## Context

MAE is a **trusted-peer** system: a shared KB has an owner who grants/revokes roles to
identified peers (key fingerprints). For this to be safe in production, **every change to a
member's status or to a KB's policy must be tracked and traceable** — who granted/revoked
which role to which principal, under which policy, when, and by whom. This is a
**non-negotiable compliance requirement**, the membership analogue of the durable,
checksum-verified transactional record we already keep for KB *content* (ADR-003 content-hash
verification + the daemon WAL).

B-12 exposed that membership was not yet treated this way: the daemon's authoritative
collection (which holds members/roles/pending) was **clobbered** by the owner's own
reconnect/re-share (`share_doc` delete+replace), silently revoking every trusted member on each
owner restart. That is disqualifying for compliance. B-12's fix (preserve the collection on
re-share; merge nodes) makes membership **durable and non-clobberable** — the necessary
foundation, but not yet the full auditable record.

The target direction is **policy-driven access**: define policies and apply them to members —
RBAC today (owner ⊇ editor ⊇ viewer × join policy), evolving toward a more modern
attribute/relationship model (ABAC/ReBAC) as needs grow.

## Decision

### 1. Membership/policy is durable, mediated, and never overwritten by sync (in place now)

These properties are the architecture that the full feature builds on; they MUST be preserved:

- **Complete mediation (ADR-018).** Every KB operation routes through the single `kb_access`
  engine (`daemon/src/collab_handler.rs`) — role × policy × operation. There is no bypass.
- **Single mutation chokepoint.** Membership/policy changes happen only via the dedicated,
  authenticated daemon methods — `kb/share` (initial owner bind), `kb/add_member`,
  `kb/remove_member`, `kb/approve_member`, `kb/set_policy` — each persisted through
  `persist_and_broadcast_collection`. **No other path may mutate membership.** In particular,
  content sync / re-share must never alter it (B-12).
- **Durable + identity-anchored.** Membership lives in the WAL-persisted `kbc:<kb>` collection
  keyed on the verified **principal** (Ed25519 fingerprint), never a self-claimed label
  (ADR-018 strict binding). It survives owner restart (B-12) and daemon restart (WAL reload).
- **Roles + policies are first-class.** `Role { Owner, Editor, Viewer }` (hierarchical) and
  `JoinPolicy { Permissive, Invite, Restrictive }` are explicit types in `shared/sync/src/kb.rs`,
  not booleans — extensible toward richer policy without a schema rewrite.

### 2. Architectural guardrails (enforce now, so the audit log/policy engine slot in)

- Keep **all** membership/policy mutations behind the chokepoint methods above. A new mutation
  (e.g. a future "grant attribute") is added as another mediated method, not an ad-hoc edit.
- Never let a sync/replication path (`sync/update`, `kb/node_update`, `kb/share` re-share)
  modify the collection's membership map. (`deny_collection_smuggling` + B-12 enforce this.)
- Identify the **actor** of every mutation from the authenticated session principal (not a
  param), so the future audit record is trustworthy by construction.

### 3. Full feature — tracked separately (not built here)

A follow-up effort delivers the **lifetime compliance record** and the **policy layer**:

- **Append-only, hash-chained membership/policy audit log** in the daemon SQLite, mirroring the
  content WAL + checksum pattern: each row = `(seq, ts, kb_id, event {member_added | member_removed
  | role_changed | approved | policy_changed | shared | left}, principal, actor_principal, role,
  policy, prev_hash, hash)` where `hash = H(prev_hash ‖ canonical(event))` — tamper-evident,
  queryable for compliance, survives restarts. Appended at each chokepoint method (§1) from the
  authenticated actor.
- **Policy definition + application** beyond today's role×join-policy: named policies applied to
  members/groups, evolving RBAC → ABAC/ReBAC (attributes/relationships). Exposed through the
  management UX (ADR-020 `*KB Sharing*` buffer) and the same mediated methods.
- Export/verification tooling (replay the chain, verify the head hash) for audits.

## Consequences

- Membership is durable and traceable enough today to be safe (no silent revoke; mediated,
  identity-anchored, WAL-backed), and the codebase is shaped so the auditable log + policy
  engine are **additive** (new mediated methods + one append-only table), not a refactor.
- **Guardrail for reviewers:** any change that mutates membership/policy outside the chokepoint
  methods, or lets a sync path touch the collection's member map, is a compliance regression —
  reject it.

## Verification

Unit (now): `kb_share_preserves_membership_on_owner_reshare` (daemon) — re-share preserves
approved members. Future: audit-chain append + verify (head-hash recomputation), tamper
detection (mutating a row breaks the chain), and policy-application coverage; e2e: grant →
restart owner → member retained + an audit entry exists for the grant.
