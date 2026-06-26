# ADR-037: End-to-end content encryption (relay/host cannot read KB content)

**Status:** Accepted (design). **First cut is implementable now** (per-KB content key); the forward-
/post-compromise-secret evolution is deferred. Tracks #131. The confidentiality counterpart explicitly
named-as-deferred in **ADR-025:256** and **ADR-026:161-163**.
**Extends:** ADR-017 (mTLS transport auth/encryption — hub), ADR-025 (QUIC transport encryption — mesh),
ADR-026 (signed membership op-log — reused as the key-distribution channel).
**Pairs with:** ADR-036 (signed content ops — same content-op substrate: integrity + confidentiality).
**Relates:** ADR-029 (CRDT-as-truth — encrypting the CRDT payload is transport-agnostic by construction).

## Context

Two encryption layers, only one of which exists today:

- **Transport (in transit) — EXISTS.** Hub `key` mode authenticates + encrypts with **mTLS** (ADR-017);
  the P2P mesh rides **QUIC/TLS** end-to-end (ADR-025 — the iroh *relay* "sees only ciphertext"). A dumb
  packet forwarder cannot read traffic.
- **Content confidentiality (at rest in the relay / at the host) — DOES NOT EXIST.** A KB **member's
  daemon, a hosting daemon, or a relaying daemon that terminates the QUIC session — and the v0.14 hub
  server — all see PLAINTEXT CRDT content.** ADR-026 integrity makes a relay unable to *forge* content;
  it does nothing to stop it *reading* content. ADR-025/026 both flagged this deferred so it would "not
  be mistaken for solved."

The user requirement: advance content confidentiality for **both hub and P2P traffic**. A node operator
hosting/relaying a KB they are not a member of (or a hub server) must not be able to read it.

**Design insight (decisive).** Encrypt at the **CRDT-payload layer**, not the transport layer. Because
the CRDT is the source of truth (ADR-029) and flows identically over hub, mesh, or both, encrypting the
payload is **transport-agnostic** — one mechanism covers hub + P2P + both at once, and rides the *same
content-op substrate* as ADR-036 (sign **and** encrypt the op = integrity **and** confidentiality).

## Decision

**Encrypt content-op payloads with a per-KB symmetric content key, distributed to members through the
ADR-026 membership op-log.** The daemon stays **key-blind**: it verifies the ADR-036 signature and relays
ciphertext; it never holds the content key (unless it is itself an editing member).

### D1 — Per-KB symmetric content key (AEAD)

Each KB collection has a current **content key** `k` (ChaCha20-Poly1305 / XChaCha20-Poly1305 — pure-Rust,
misuse-resistant nonces). Editors encrypt the content-op payload (the yrs update bytes) under `k` before
push; receiving members decrypt under `k` on apply. The ADR-036 signature is computed over the
**ciphertext + header** (encrypt-then-sign: a peer verifies authorship + authorization *before*
attempting decryption, and a relay verifies integrity without the key). The yrs CRDT operates on
plaintext after decryption — convergence is unchanged (ADR-022/029).

### D2 — Key distribution via the membership op-log (reuse ADR-026)

The membership op-log is already the trusted, signed, peer-verified channel for "who is a member." Extend
it to carry the **wrapped content key**: each member's `Admit` op carries `k` **wrapped to that member's
public key** (X25519 ECDH from their Ed25519 identity via the standard birational map, or a dedicated
per-identity KEM key published once). A new member's admit op thus delivers `k`; no side channel, no
key server. A peer derives the content key the same way it derives membership — trustlessly, from the
anchored signed log.

### D3 — Rotation on membership change

Removing a member must deny them **future** content. On a `Remove`/`revoke` op, the owner/admin **rotates**
to a fresh `k'` and re-wraps it to the remaining members (a new key-distribution op). Near-term =
**eager re-wrap** (simple; O(remaining members) per removal; no forward secrecy — the removed member can
still read history they already had, which is unavoidable for already-delivered plaintext). This is the
**YELLOW, implementable-now** cut.

### D4 — Deferred destination (RED/research): group ratchet

True **forward secrecy + post-compromise security** (a compromised key can't decrypt past *or* future
content) needs a **group key-agreement ratchet** — **BeeKEM / Ink&Switch Keyhive** (or MLS RFC-9420 as a
reference). That replaces the static per-KB key + eager re-wrap with a tree-based ratchet. Deferred:
library maturity + complexity; the D1–D3 cut is the pragmatic floor and the migration target is recorded
so it isn't mistaken for solved (the ADR-026 §B discipline).

### D5 — Hub transport floor (small, related)

ADR-017:145 notes hub PSK/default mode without `collab_tls` relies on an external stunnel for wire
encryption. Decision: make **`key`-mode mTLS the documented transport floor** for any KB that uses content
encryption (defense in depth — content is encrypted regardless, but the metadata/topology shouldn't ride
plaintext when a member is using key mode anyway). A `collab_tls` default flip is an option, non-blocking.

### Implement-now vs ADR-only

The first cut (D1–D3) is **YELLOW/buildable** with in-tree-adjacent crypto (`chacha20poly1305`,
`x25519-dalek` — siblings of the `ed25519-dalek` already vendored). **Recommendation: implement the first
cut** alongside ADR-036, since both ride the content-op layer and the key rides the membership op-log
that already exists. If integration proves larger than a focused slice, this ADR stands alone as the
accepted design and the implementation defers — the user accepted ADR-only as the floor.

## Threat model — what it protects / what it doesn't

**Protects:** a non-member relaying/hosting daemon — and the hub server — cannot **read** KB content
(ciphertext only); combined with ADR-036 they can neither read nor forge. Confidentiality is independent
of transport (covers hub + mesh + both).

**Does NOT protect (named, not mistaken for solved):**
- **Metadata** — op sizes, timing, which node changed, membership graph (the signed op-log is plaintext by
  design — it must be peer-verifiable). Traffic-analysis resistance is out of scope.
- **Forward / post-compromise secrecy** — the D1–D3 static key + eager re-wrap does not provide it; a
  removed member retains what they already decrypted. → D4 (BeeKEM), deferred.
- **A malicious *member*** — members hold `k` by definition; encryption is confidentiality *from
  non-members/relays/hosts*, not from a peer you admitted. Membership controls who that is (ADR-026).
- **Compromise of an identity key** — leaks that member's wrapped `k`; mitigated by rotation-on-removal
  + re-key (ADR-038), fully by D4.

## Consequences

- KB content becomes **private to members** across hub + P2P; an operator can host/relay KBs they cannot
  read — a real local-first/self-host property (a relay needn't be trusted with content).
- Cost: one AEAD encrypt/decrypt per content op (~ns/byte, negligible vs yrs work); per-member key wrap on
  admit; an O(remaining) re-wrap on removal. The content key + wraps add a small amount to the collection
  doc.
- **Migration / mode interaction:** encryption is **opt-in per KB** (a collection flag); unencrypted KBs
  (v0.14, single-trusted-hub) are unchanged. Encryption requires key mode (members have identities) — the
  mesh requires that anyway. The daemon being key-blind means **headless hosting of an encrypted KB cannot
  read/serve content beyond relaying ciphertext** — consistent with the threat model (a host you don't
  trust shouldn't read it), and a constraint to surface in the headless-host work (#111).
- Reviewer guardrail: the daemon must never log/persist plaintext content for an encrypted KB, and must
  never require the content key to relay. Encrypt-then-sign ordering (verify before decrypt) is load-bearing.

## Verification

Unit: AEAD round-trip; wrap/unwrap `k` to/from a member key; encrypt-then-sign verify-before-decrypt;
rotation produces a `k'` the removed member's key cannot unwrap. **Security-negative oracle on a real
2-daemon mesh:** a **non-member relay/host daemon sees only ciphertext** and cannot reconstruct node text;
a **member decrypts + converges** (SV advances); after a member is removed + key rotated, the removed
peer **cannot read** subsequent ops. Confidentiality is asserted independent of transport (same result
hub vs mesh). Cross-OS (principle #13). If ADR-only this stage, the ADR is reviewable on its own and the
oracle lands with the implementation.
