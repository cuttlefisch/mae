# ADR-041: Identity key separation — a published X25519 wrap key (signing ≠ key-exchange)

**Status:** Accepted (design, 2026-06-29). Closes security-review finding **I1** (`docs/SECURITY_REVIEW.md`
§3, issue #158). Implemented as **one identity arc with ADR-040** (rotation) — they land together: a rebind
rotates the master identity *and* republishes the wrap key.
**Extends:** ADR-037 (E2E content-key wraps), ADR-026 (signed membership op-log — carries the published key),
ADR-038 (member pubkey storage).
**Relates:** ADR-040 (rotation/rebind — coupled), ADR-017 (mTLS identity).
**Prior art:** NaCl/libsodium `box` (separate signing vs box keys), Signal (identity key ≠ prekeys),
key-separation guidance (filippo.io; the EdDSA double-pubkey oracle, arXiv:2308.15009).

## Context

A MAE identity is **one Ed25519 seed**, and ADR-037 wraps the per-KB content key to a member by converting
that *same* Ed25519 key to X25519 via the standard birational map (`ed25519_pub_to_x25519` /
`ed25519_secret_to_x25519`) and doing a sealed-box ECDH. So **one key is used for both signing and
key-exchange** (review finding **I1**). The wrap KDF is domain-separated from the signing context (so it is
not a live break — flagged WEAKNESS, not BUG), but signing↔key-exchange reuse runs against standard
key-separation guidance: distinct algorithms over the same secret invite cross-protocol / unknown-key-share
surprises, and a future signing-scheme change would entangle confidentiality.

**The constraint that shapes the fix.** The obvious move — derive a per-context X25519 wrap subkey from the
master seed with HKDF — works for *my own* secret, but **a sender wrapping to me has only my Ed25519 public
key** and cannot HKDF-derive my wrap key from it (HKDF needs the secret). So separation requires the
recipient to **publish** an X25519 wrap *public* key, bound to their identity.

## Decision

Each identity gains a distinct **X25519 wrap keypair**, derived from the master seed and **published**:

1. **Derivation (holder side).** `wrap_secret = clamp(HKDF-SHA256(master_seed, info="mae-x25519-wrap/v1"))`
   → its X25519 public key is `wrap_pub`. One seed still backs everything (no new secret to store), but the
   wrap key is a separate, domain-separated subkey — never the signing key. (We already avoid an `hkdf`
   crate per the #98 crypto-coherence note; `mae` derives via `SHA-256(tag ‖ seed)`-style expansion as in
   `content_crypto::derive_wrap_key`, kept domain-separated and versioned.)
2. **Publication.** `wrap_pub` is published **in the signed membership op-log**, bound to the principal:
   it rides the member's `Admit` op (from the join/`PendingRequest`, alongside the Ed25519 pubkey ADR-038
   already carries) and is therefore **signed by the owner** and integrity-checked like any membership fact.
   A principal's own self-published `wrap_pub` (genesis self-admit) is signed by its Ed25519 key — so the
   binding "this identity ↔ this wrap key" is always cryptographically asserted, never relay-supplied.
3. **Wrap (sender side).** `wrap_to_member` takes the recipient's **published `wrap_pub`** (looked up from
   the op-log), not the ed25519→x25519 conversion. The sealed-box ECDH + KDF are otherwise unchanged
   (ephemeral X25519 → ECDH → domain-separated KDF → AEAD), including the F6 low-order-DH rejection.
4. **Unwrap (recipient side).** `unwrap_as_member` re-derives `wrap_secret` from the master seed (HKDF) and
   opens — the ed25519→x25519 secret map is gone from the wrap path.
5. **Coupling with ADR-040 rotation.** A `Rebind` carries the **new `wrap_pub`** (derived from the new
   master seed) alongside the new Ed25519 pubkey; the owner re-wraps the content key to the new `wrap_pub`.
   Rotation thus rotates *both* keys coherently — the reason I1 and I2 land as one arc.

The Ed25519 key keeps signing membership + content ops, backing mTLS, and being the mesh node-id. Only the
**content-key wrap** moves to the dedicated X25519 key. (TLS already uses Ed25519 directly per RFC 7250; no
ECDH-over-the-identity-key reuse remains after this.)

## Migration

E2E is pre-GA (no deployed encrypted KBs to preserve), so we **switch** the wrap path rather than carry a
dual format: new `Admit`/genesis ops publish `wrap_pub`; `wrap_to_member` requires it. An E2e KB created
before this change is re-enabled (one-way, owner action) to publish wrap keys — acceptable pre-GA and far
simpler than a detect-both compatibility shim. The versioned KDF tag (`/v1`) leaves room for a future change.

## Threat model & tests (principle #14)

- **Wrong/forged wrap key:** a `wrap_pub` not signed into the op-log (relay-injected) is not honored — wraps
  resolve only against op-log-published keys. Test: a tampered/relay-supplied `wrap_pub` wraps to nothing a
  member can open.
- **Key separation holds:** signing the same message and wrapping under the same identity use *distinct*
  keys (assert `wrap_pub != ed25519_to_x25519(ed_pub)` for generated identities; round-trip wrap/unwrap
  under the published key).
- **Round-trip + rotation:** wrap→unwrap over many fresh identities; after a Rebind, the content key
  re-wrapped to the new `wrap_pub` unwraps with the new master seed and **not** the old (forward exclusion
  of the retired key on the wrap path too).
- **Low-order DH still rejected (F6):** unchanged; re-verified on the new wrap key.
- **Daemon stays key-blind:** it stores + relays `wrap_pub` and the wrapped blobs, never a secret.

## Consequences

**Positive**
- Signing and key-exchange use distinct keys — closes I1; removes the last identity-key ECDH reuse.
- One seed still backs the identity (no extra secret at rest); the wrap key is a deterministic subkey.
- Composes cleanly with ADR-040: rotation republishes the wrap key in the same `Rebind`.

**Negative / costs**
- The membership op (Admit/genesis/Rebind) carries an extra 32-byte key; `derive_content_key` looks up the
  published `wrap_pub`. Small.
- Pre-GA migration: existing E2e KBs re-enable to publish wrap keys (documented, one-way).
- A bit more identity surface (a second published key) to test and document.

## Implementation

Lands in **PR 1 of the identity arc** (before ADR-040's rotation): `identity.rs` (derive + expose
`wrap_pub`/`wrap_secret`), `content_crypto.rs` (wrap/unwrap take the published key; drop the secret-side
ed25519→x25519 map from the wrap path), `membership.rs`/`kb.rs` (publish `wrap_pub` on admit/genesis;
`derive_content_key` resolves it), the daemon stays key-blind. Then ADR-040's `Rebind` carries the new
`wrap_pub`.
