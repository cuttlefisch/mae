# ADR-039: Identity & authorization hardening for E2E KB sharing

**Status:** Accepted (design). Tracks #131. Records the decisions from the prior-art-grounded identity +
authorization security review (`docs/SECURITY_REVIEW.md`, #155) — fixes in #157 (authz) + #158 (identity).
**Extends:** ADR-017/018 (peer auth + access control), ADR-023 (epoch fence), ADR-026 (signed op-log).
**Pairs with:** ADR-037 (content encryption) + ADR-038 (editor-authored membership).

## Context

The content-encryption review validated the crypto; the identity + authorization review then audited the
foundation E2E sits on. Both schemes were found **sound at the core** — no auth-bypass, no
privilege-escalation; capability attenuation, owner-only governance, the strong-removal/quorum resolver, and
transport gating are all correct. The defects cluster at **(a) the boundary between the new signed op-log and
the legacy `member_roles` store and (b) enforcement-point coverage**, plus identity **lifecycle**
defense-in-depth. This ADR records the hardening decisions so the holistic story (`docs/KB_SHARING.md`) rests
on a single, consistent authorization model.

## Decision

### D1 — One unified op-log fence on every write path (A1 + N1 + A2, #157)

A member's **role, epoch, and the local blocklist** are all derived from the **one signed op-log**, via **one
shared fence helper**, called on **both** the hub (`kb/node_update`) and mesh (`dialer::apply_doc`) write
paths (complete mediation; principle #8). This closes:
- **A1 (HIGH):** the epoch fence read the epoch from the legacy `member_roles` map (`epoch_of`→0 for a
  mesh-admitted member) while the role came from the op-log → any non-epoch-0 member was wrongly fenced. Fix:
  derive the epoch from `ValidMember.epoch` (op-log) for anchored KBs.
- **N1 (HIGH):** the ADR-023 fence ran only on the hub path. Fix: the shared helper runs on the mesh relay
  too.
- **A2 (MEDIUM):** `verify_content_op` derived membership with an empty blocklist. Fix: thread the peer's
  persisted `MembershipView` (blocklist + cascade) into every content/authz check.
Grounded in: single source of authority (Google Zanzibar) + complete mediation (SPKI/SDSI, UCAN revocation).
*(ADR-038's dual-write is the interim hub mitigation for A1; this unification is the real fix.)*

### D2 — Encryption mode is a signed op; the seal path is fail-closed (F2)

The `Encryption::E2e` mode must **not** be an unsigned collection-map flag (a relay could flip `e2e→none` →
the victim emits plaintext). It is recorded as a **signed, owner-authored `SetEncryption` op** in the op-log
(mirroring `SetGovernance`); the editor reads the authoritative mode from the latest signed op, and the seal
path is **fail-closed** — once the signed log asserts E2e, a writer **never** authors plaintext for that KB.
Grounded in: MLS binds the ciphersuite into the signed GroupContext (RFC 9420).

### D3 — E2e ⇒ SingleOwner governance (F4 / N2)

Content-key authority is rooted at the genesis owner. Under `Governance::Quorum` a quorum can remove the
owner from *governance* but cannot rekey (no other principal authors honored `wrapped_key` ops) — the key
freezes. For v1, **E2e KBs are restricted to `SingleOwner`**: enabling E2e on a quorum KB is rejected, and
`SetGovernance(Quorum)` on an E2e KB is rejected. *(Later: let any current `Role::Owner` author rotation ops
under the active governance, with causal-order tiebreak — deferred.)*

### D4 — Anchor pinned to the authenticated owner (F1 / A3)

`derive_content_key` must **not** re-derive its trust anchor TOFU-style from the relay-supplied collection. It
cross-checks the genesis anchor against the **authenticated owner**: `COLL_OWNER_KEY` (the daemon binds it to
the mTLS principal) on the hub, and the **ticket node-id** (which `dialer::connect_verify_anchor` already
verifies) on the mesh. A relay cannot substitute a forged genesis to inject a key. Grounded in: RFC 7250
requires an out-of-band identifier↔key binding.

### D5 — Identity lifecycle hardening (I1–I6, #158 — staged)

Documented now; fixes staged (none block v0.14, but gate E2E GA / a Windows release):
- **I2 — key rotation / rebind:** add a signed rebind op (old key endorses the new key, replayed through the
  op-log so peers transfer membership + re-wrap). Matrix cross-signing is the model. *(ADR follow-up before
  E2E GA.)*
- **I1 — single-key separation:** derive per-context X25519 / subkeys via HKDF from the master Ed25519 seed
  rather than reusing it directly for both signing and key-exchange. *(Before E2E GA.)*
- **I4 — Windows perms:** enforce ACL tightening or warn (no silent world-readable keys). *(Before any
  Windows release — principle #13.)*
- **I3 — at-rest:** opt-in passphrase / OS-keychain wrap for the identity + content keys (0600 + disk
  encryption stays the default).
- **I5 — two-layer revocation:** member removal must rotate the content key to exclude an already-joined
  ex-member (ADR-037 §D3 / 3c); document that `authorized_keys` ≠ membership ≠ content-key access.
- **I6 — TOFU first-contact:** recommend OOB fingerprint verification / `strict` policy; the mesh ticket
  closes the window in-band.

## Threat model — what it protects / what it doesn't

**Protects (after D1–D4):** a removed/blocked principal cannot write on any path; a relay cannot downgrade
encryption, substitute the trust anchor, or make a valid member look fenced; a quorum KB cannot silently
freeze its key (it's disallowed for E2e in v1). **Does NOT yet protect (D5 staged):** a compromised at-rest
key file; cross-context key-reuse (mitigated by domain separation); identity continuity across a key
rotation. FS/PCS remain out of scope (ADR-037 §D4).

## Consequences

- One authorization model: role + epoch + blocklist from the signed op-log, enforced identically on hub +
  mesh — removing the op-log↔legacy split that produced A1/N1.
- E2e is `SingleOwner`-only in v1 (a documented, lifted-later constraint).
- The identity lifecycle gaps are tracked + staged, not silently shipped; the docs (`SECURITY_REVIEW.md`,
  `KB_SHARING.md`) state them honestly.

## Verification

- Adversarial tests (principle #14) per fix: a non-epoch-0 mesh member edits successfully; a stale-epoch op
  is fenced on **both** paths; a blocked principal's signed op is rejected; a relay's `e2e→none` flip is
  ignored (signed mode + fail-closed); a forged genesis fails the anchor cross-check; enabling E2e on a
  quorum KB is rejected.
- `make ci-all` (both workspaces, GUI) green; the implementation-pass security review re-runs before 3d's
  confidentiality gate is declared meaningful.
