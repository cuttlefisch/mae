# ADR-040: Identity key rotation & rebind (cross-signed, history-preserving)

**Status:** Proposed (design). Closes security-review finding **I2** (`docs/SECURITY_REVIEW.md` §3,
issue #158). No code yet — this ADR is the decision-of-record; implementation is staged behind it.
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

## Open questions (flagged for review — parked, not blocking the ADR)

- **Q1 — Owner co-sign?** Should a Rebind in an owner's KB also require the owner's counter-signature
  (owner gates identity changes), or is old-key self-endorsement enough? *Recommendation:* self-endorsement
  suffices for the membership transfer; the owner is already implicitly in the loop for E2e (must re-wrap),
  so an explicit co-sign adds little for E2e and would block rotation in non-E2e KBs the owner is offline
  for. Lean **no mandatory co-sign**; revisit if a KB wants stricter identity governance.
- **Q2 — Pre-registered recovery key** for self-service *compromise* recovery (the deferred right side of
  the fork). Worth a follow-up ADR? Prior art: Matrix master-key reset, TUF root rotation.
- **Q3 — Interaction with HKDF per-context subkeys (#158 I1).** If we adopt I1 (derive signing/wrap/TLS
  subkeys from one master seed), a rebind rotates the *master seed*, transparently rebinding all subkeys —
  ADR-040 should be specified against the master identity, and I1 should land first or concurrently.
- **Q4 — Expiry / rotation cadence.** Should rebinds carry an `expires_at` to encourage periodic rotation,
  or stay purely event-driven? (N3 in the review already flags wall-clock `expires_at` fragility.)

## Implementation sketch (staged, post-acceptance)

1. `membership.rs`: `MembershipAction::Rebind`; `Rebind` op + `verify_signed`; the derive post-pass
   (alias + retire) in `derive_valid_members_governed`; unit tests (the adversarial set above).
2. `kb.rs`: `author_rebind(old_secret, new_fp, new_pubkey)` + owner re-wrap on observing a Rebind.
3. Daemon `collab_handler`: accept a `Rebind` op via `kb/collection_op` (owner-gated like other signed
   ops; key-blind); epoch fence unaffected (the successor inherits the epoch).
4. Editor: `(rotate-identity)` Scheme/cmd/MCP — generate new key, fan out Rebinds across joined KBs,
   update `known_hosts`; `mae-daemon authorize` for the new pubkey.
5. Dialer: re-anchor a peer on its `new_pubkey`/node-id from the Rebind.
6. Docs: `SECURITY_REVIEW.md` I2 → resolved-by-ADR-040; `E2E_ENCRYPTION.md` §7 (the read-access caveat).
