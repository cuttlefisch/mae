# ADR-040: Identity key rotation & rebind (cross-signed, history-preserving)

**Status:** Accepted (design, 2026-06-29 — open questions resolved with the maintainer, see §Resolved
decisions). Closes security-review finding **I2** (`docs/SECURITY_REVIEW.md` §3, issue #158). Implemented
as one identity arc with **ADR-041** (I1 key separation — they land together).
**Extends:** ADR-017/018 (asymmetric peer auth + identity-anchored access control), ADR-026 (signed,
hash-chained membership op-log), ADR-037 (E2E content-key wraps in the op-log).
**Relates:** ADR-023 (epoch fence), ADR-039 (identity/authz hardening — the local blocklist is the
compromise-recovery primitive here), ADR-038 (member pubkey storage).
**Prior art:** Matrix cross-signing (MSC1680), Keybase sigchains, age/SSH key rotation, TUF key rotation.

## Context

A MAE principal **is** a single Ed25519 identity key. The fingerprint `SHA256:b64(sha256(pubkey))`
(`shared/mcp/src/identity.rs`) is the access-control subject everywhere:

- **Membership** — `SignedMembershipOp.author`/`subject` are fingerprints; `verify_signed` binds
  `author_pubkey ↔ fingerprint ↔ signature` (`shared/sync/src/membership.rs`).
- **Content keys** — the per-KB content key is sealed to the member's Ed25519 pubkey and rides the op-log
  as `wrapped_key` (`content_crypto::wrap_to_member`, authored by `kb.rs::author_member_admit` /
  `author_rotate_on_remove`).
- **Transport** — the same key backs mTLS (`tls.rs`), the trusted-peer `authorized_keys`, the TOFU
  `known_hosts` pin, **and** the P2P mesh node-id (`Identity::secret_bytes()` → the iroh `EndpointId`
  literally *is* the key).

**The gap (I2):** rotating the key makes you a **brand-new principal** — you lose every KB membership and
every content-key wrap (they were sealed to the old pubkey). There is no way to say "this new key is still
me." This blocks any responsible key-hygiene story and is a stated gate on E2E GA.

**The decisive constraint:** the membership op-log is **append-only + hash-chained + immutable** (ADR-026).
A peer derives identical state by replaying it. So the naive fix — *re-sign every historical op with the
new key* — is **wrong**: it rewrites history, invalidates every peer's derived state and the chain hashes,
and races concurrent ops. Rotation must be **additive**, never a rewrite.

## Decision

### 1. The `Rebind` op — the old key cross-signs the new key

Add a membership op `Rebind { old_fp, new_fp, new_pubkey }`, **signed by the OLD key**, appended to the
op-log like any other op (no history rewrite). Semantically: *"the holder of `old_fp` attests that
`new_fp` is the same principal, going forward."* This is exactly the cross-signing / sigchain model: an
existing trusted key endorses a successor, and everyone who trusted the old key now trusts the new one —
**without** re-issuing past statements.

- `verify_signed` on the Rebind binds `old_pubkey ↔ old_fp ↔ sig` as usual (the old key must sign).
- A Rebind is only **honored** if `old_fp` is a current member at the op's causal point (you can only
  rebind an identity you actually hold in this KB).

### 2. Derivation: alias the successor to the predecessor (additive)

`derive_valid_members*` gains a post-pass: for each honored `Rebind(old→new)` in causal order, the derived
membership entry for `old_fp` is **transferred** to `new_fp` — same `role`, `epoch`, `invited_by`,
`can_invite` — and `old_fp` is **retired**:

- The old key's **historical** ops stay valid (immutable, still verify) — history is intact.
- The old key's **future** ops (authored *after* the Rebind in causal order) are **no longer honored** —
  retirement is the point (a rotated-away key stops acting). Chained rebinds compose (`a→b→c` ⇒ `c`).
- A second Rebind of an already-retired key, or by a non-member, contributes nothing (fail-closed).

### 3. Content keys: the owner re-wraps to the successor

The content key was sealed to `old_pubkey`; the successor can't yet decrypt. On observing an honored
`Rebind(old→new)` for one of its KBs, the **owner** authors a new wrapped op delivering the **current**
content key to `new_fp`, sealed to `new_pubkey` — reusing the `author_rotate_on_remove` re-wrap machinery
(wrap-to-pubkey, re-assert role/epoch verbatim, no downgrade). `derive_content_key` then resolves for the
new key. The old wraps remain (the successor, holding both keys during a planned rotation, can still read
history; a non-owner relay still sees only ciphertext). For an **E2e** KB the owner is therefore implicitly
in the loop — a rebind isn't *readable* until the owner re-wraps — which is the correct authority boundary.

### 4. Transport + mesh re-authorization (out-of-band, by design)

The cryptographic membership transfer (1–3) is in-band, but the transport trust roots are deliberately
**out-of-band** (that's their security model) and must be updated by the operator:

- **`authorized_keys`** (daemon) — add `new_pubkey` (it carries the Rebind-endorsed label). The old key
  MAY be revoked once rotation is confirmed.
- **`known_hosts`** (editor→daemon TOFU) — re-pin if the rotated key is a *daemon's* host key.
- **Mesh node-id** — because the node-id *is* the key, a rebind **changes the node-id**. Peers learn the
  new node-id from the Rebind op's `new_pubkey` (node-id = fingerprint), so an already-anchored peer can
  re-dial the successor without a fresh ticket. (Dialer plumbing is the implementation's main mesh task.)

### 5. Scope: per-KB, editor-fanned-out

Membership is per-KB, so a Rebind is authored **into each KB's op-log** the principal belongs to. The
editor automates the fan-out: on `(rotate-identity)`, generate the new key, then for every joined KB author
+ push a Rebind (old key signs), and refresh transport roots. There is **no** global identity ledger in v1;
the per-KB op-logs are the source of truth (consistent with ADR-026 / ADR-029).

## The planned-rotation vs compromise-recovery fork (important)

Old-key self-endorsement works **only when the old key is still under the user's control** — i.e. *planned*
rotation (hygiene, device migration, suspected-but-uncompromised). It does **not** recover from a
**compromised** key: an attacker who holds the old key can sign a Rebind to *their* key. So:

- **Planned rotation → ADR-040 Rebind** (this ADR). Old key alive, signs the successor. Clean transfer.
- **Compromise recovery → owner eviction + local block, then re-join as a new principal.** The owner
  `Remove`s the compromised principal and rotates the content key (ADR-037 §D3, already shipped), and any
  operator `kb-block-member`s the old fingerprint locally (**ADR-039 A2 — the primitive shipped in #162**).
  The user re-joins with a fresh key as a **new** principal (no automatic membership transfer — that's the
  safe default when a key is burned). **Full self-service compromise recovery** (e.g. a pre-registered
  offline *recovery key* that can override a compromised signing key, à la Matrix's master-key reset) is
  **deferred** (open question Q2).

This split is the standard one (Matrix separates cross-signing from key reset/recovery); naming it is the
point — ADR-040 is *rotation*, not *compromise recovery*.

## Threat model

- **Adversary:** a malicious relay/host (key-blind), a non-member, a **retired** old key, and a peer that
  replays/reorders ops.
- A relay **cannot forge** a Rebind (needs the old key's signature, bound in the op-log).
- A relay **cannot reorder** to un-retire an old key (causal order is `prev_hash`/`chain_hash`, relay-
  independent — ADR-026/037 §5).
- A **retired** key's post-rebind ops are fenced (point 2). Its *content-read* of post-rebind material is
  denied for E2e KBs once the owner rotates the key on the next removal — but note a planned rotation does
  **not** by itself rotate the content key (the successor inherits read access via re-wrap), so the old key
  retains read access to content until an independent §D3 rotation. This is acceptable for *planned*
  rotation (the user still controls the old key); it is **not** a compromise-recovery property (hence the
  fork above).
- **Adversarial tests the implementation MUST carry** (principle #14): a forged Rebind (wrong signer) is
  rejected; a Rebind by a non-member contributes nothing; the successor inherits the *exact* role/epoch
  (no self-elevation — e.g. a viewer can't rebind into an editor); the retired key's later op is fenced;
  N-peer convergence on the aliased membership; for E2e, the successor cannot read until the owner re-wraps,
  and a relay stays key-blind throughout; chained rebinds `a→b→c` resolve to `c`.

## Consequences

**Positive**
- Responsible key hygiene becomes possible without losing membership or content access — gates E2E GA (I2).
- Reuses the existing op-log + re-wrap + epoch machinery; no new trust root, no central ledger.
- History stays immutable and peer-verifiable; the change is purely additive.
- Cleanly composes with ADR-039: the just-shipped local blocklist is the compromise-side primitive.

**Negative / costs**
- A rebind is O(KBs × members-needing-rewrap) owner work per KB; the editor fan-out is N op-log appends.
- The mesh node-id change requires dialer plumbing to re-anchor on `new_pubkey`.
- `derive_valid_members*` gains a rebind post-pass (O(ops)) — folds into the F8 incremental-derivation
  concern (#156).
- Planned rotation does not deliver post-compromise security (by construction — that's the fork's right
  side, deferred).

## Resolved decisions (review, 2026-06-29)

- **Q1 — Owner co-sign? → NO mandatory co-sign.** Old-key self-endorsement suffices for the membership
  transfer; the owner is already implicitly in the loop for E2e (must re-wrap), and requiring a co-sign would
  block rotation when the owner is offline. (Stricter per-KB identity governance is a possible later opt-in,
  not v1.)
- **Q2 — Compromise recovery → owner-mediated v1 NOW + pre-registered recovery key as the designed
  fast-follow.** Recovery is a release gate, so it is no longer "deferred":
  - **v1 (this arc, reuses shipped primitives):** a compromised principal is recovered by the **owner**:
    `Remove` the compromised member → §D3 content-key rotation (ADR-037, shipped) → operators
    `kb-block-member` the old fingerprint locally (ADR-039 A2, shipped #162) → the user **re-joins with a
    fresh key as a new principal**. Works today end-to-end; the cost is loss of identity/membership
    *continuity* on compromise (you come back as a new principal) and a dependency on the owner being
    reachable. Document this as the v1 recovery flow.
  - **v2 (designed next — see §Recovery-key design):** a **pre-registered offline recovery key** that can
    author a `Rebind` even when the primary is compromised, giving *self-service* recovery WITH continuity.
    Specified below so v1 doesn't paint us into a corner; implemented as the fast-follow.
- **Q3 — I1 coupling → land I1 WITH I2 as one identity arc** (ADR-041). The identity grows a **separately
  published X25519 wrap key** (the sender can't HKDF-derive a wrap key from only the recipient's *ed25519
  public* key, so the recipient must publish it). A `Rebind` then rotates the master identity **and**
  republishes the new wrap key coherently; the owner re-wraps to the new published wrap key. ADR-040 is
  specified against the **master identity**; the wrap-key mechanics live in ADR-041.
- **Q4 — Expiry → event-driven, NO `expires_at` on a Rebind.** Rotation is an explicit event; a wall-clock
  `expires_at` is already flagged fragile (review N3) and peers can disagree on it. Cadence is operational
  guidance, not a protocol field.

## Recovery-key design (v2 fast-follow — specified now, implemented after v1)

A pre-registered **recovery key** `R` (a second Ed25519 keypair, generated at identity setup and kept
**offline / out of the daemon**) is the authority that can rotate a *compromised* primary:

- **Registration.** When a principal is admitted (or via a later self-authored op), it records a signed
  `RecoveryKey { principal, recovery_pubkey }` — signed by the **primary** at a time it was uncompromised —
  into each KB's membership op-log. Peers learn "principal P authorizes recovery-key R" the same way they
  learn membership.
- **Recovery.** A `Rebind { old_fp, new_fp, new_pubkey }` may be signed by **either** the old primary
  (planned rotation, §Decision) **or** the registered recovery key `R` (compromise recovery). `derive_*`
  honors a recovery-key-signed Rebind because `R` was endorsed by the principal before compromise — so even
  if the attacker holds the primary, they cannot forge a recovery Rebind (they don't have `R`).
- **Why offline matters.** `R`'s only job is to sign a rebind; it never touches content or transport, so it
  can live on paper / a hardware token / a separate device. This is the Matrix master-key-reset / TUF
  root-rotation pattern: a rarely-used high-value key that recovers the frequently-used one.
- **Revoking a leaked recovery key.** A new `RecoveryKey` op (signed by the current primary) supersedes the
  old one (latest-wins, like a wrap). Losing `R` itself falls back to owner-mediated recovery (v1).
- **Threat note.** v2 closes the continuity gap of v1 (recover *as the same principal*) without trusting the
  owner to be online — at the cost of the recovery-key storage/UX. v1 ships first; v2 is the fast-follow.

## Implementation sketch (the identity arc — fewer, larger PRs)

**PR 1 — I1 key separation (ADR-041), the foundation.** Identity publishes a distinct HKDF-derived X25519
wrap key; `content_crypto` wraps to the *published* wrap key (not the ed25519→x25519 map); membership/admit
carries it; `derive_content_key` unwraps with the HKDF-derived wrap secret. Rotation builds on this.

**PR 2 — I2 rotation + owner-mediated recovery (this ADR).**
1. `membership.rs`: `MembershipAction::Rebind`; `Rebind` op (carries `new_pubkey` + the new published wrap
   key) + `verify_signed`; the derive post-pass (alias role/epoch + retire old) in
   `derive_valid_members_governed`; the adversarial test set (§Consequences).
2. `kb.rs`: `author_rebind(old_secret, new_fp, new_pubkey, new_wrap_pub)` + owner re-wrap to the new wrap
   key on observing a Rebind.
3. Daemon `collab_handler`: accept a `Rebind`, key-blind; epoch fence unaffected (successor inherits the
   epoch). **See the implementation addendum below** — owner rotation rides the existing owner-gated
   `kb/collection_op`; a non-owner member's self-`Rebind` needs a new verified member-authored path (PR2c).
4. Editor: `(rotate-identity)` Scheme/cmd/MCP — generate the new keypair, fan out Rebinds across joined KBs,
   update `known_hosts`; `mae-daemon authorize` the new pubkey. **Recovery v1** is the documented
   owner-flow (Remove → §D3 rotate → `kb-block-member` → re-join), no new code beyond the docs + verifying
   the path.
5. Dialer: re-anchor a peer on its `new_pubkey`/node-id from the Rebind.
6. Docs: `SECURITY_REVIEW.md` I1+I2 → resolved-by; `E2E_ENCRYPTION.md` §7 (planned-rotation read-access
   caveat); the manual's admin/maintenance section (rotation + recovery runbook — a release gate).

**PR 3 (fast-follow) — recovery key v2** (§Recovery-key design): `RecoveryKey` op + recovery-key-signed
Rebind path + offline-key storage/UX.

### Implementation addendum (2026-06-30): PR2 splits into PR2a / PR2b / PR2c

Implementing PR2 surfaced a daemon authorization constraint that cleanly splits the rotation work. The hub
write path `kb/collection_op` is **owner-only** (`KbOp::Manage`, ADR-018 — `collab_handler.rs`: "a non-owner
cannot inject collection ops"). A `Rebind` is authored by the **rotating member**, who may be a non-owner, so
the two cases need different paths:

- **PR2a — rebind core (shipped, #210).** `mae-sync` only: `MembershipAction::Rebind` + `maememb/v3` encoding
  (carries `new_pubkey` + `new_wrap_pubkey`); `authorized` arm (member-only, fingerprint-bound, **fresh**
  successor — a clobber-an-existing-member guard — no self-elevation); the `build_members` alias/retire
  post-pass (causal retirement via `membership_at`, provenance re-point); `owner_principal_chain` so the
  genesis-anchored readers (`derive_governance`/`derive_encryption`/`find_wrapped_content_key`) keep honoring a
  rotated **owner**; `kb.rs` `author_rebind` + `author_rebind_rewrap`. Full adversarial set + a peer-replica
  round-trip.

- **PR2b — OWNER rotation.** The owner rotating their *own* identity works through the **existing owner-gated
  path**: the owner signs the `Rebind` with their OLD owner key (still passes `Manage`), and
  `owner_principal_chain` resolves the successor so `owner2` passes `Manage` for subsequent ops. The editor
  `(rotate-identity)` (cmd/Scheme/MCP) generates the new keypair, and for each **owned** KB authors
  `author_rebind` + `author_rebind_rewrap` (to the owner's own new wrap key) against the network task's
  `kb_collections` replica (the #179 fail-closed base rule), ships both via `kb/collection_op`, then swaps the
  in-memory signing identity + content keys and persists the new key. Transport re-anchor (the node-id changes)
  is **out-of-band by design** (§4): `mae-daemon authorize` the new pubkey + re-pin `known_hosts`. No daemon
  change required. e2e: `MAE_E2E_ROTATE=1`.

- **PR2c — NON-OWNER member rotation (tracked: issue #213).** A member who is not the owner has **no path
  today** — the owner-`Manage` gate rejects their self-`Rebind`. PR2c adds a **member-authored Rebind write**
  to the daemon: a verified path (its own RPC, or a `Rebind`-aware branch of `kb/collection_op`) that accepts
  an op **iff** it is *specifically a `Rebind`*, its signature verifies, its author is a **current member**,
  and the successor is fresh + fingerprint-bound — explicitly **not** general `Manage`, and it must reject any
  other collection op so a member cannot widen their privilege. Plus the **owner-side reactive re-wrap**: when
  a `Rebind` for another member arrives in a `kbc:` delta on a KB this peer owns, the owner authors
  `author_rebind_rewrap` to deliver the content key to the successor (extends
  `refresh_kb_content_key_on_collection_delta`). This is the **first non-owner write to the collection op-log**
  — a new authorization surface — so it carries heavy adversarial tests (non-member rejected; a member's
  *non*-Rebind op rejected; stale-epoch; forged) and this addendum is its design of record.
