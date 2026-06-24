# ADR-026: Peer-verifiable KB integrity (signed hash-chained membership + signed ops)

**Status:** Accepted (design). Phased implementation tracked separately (P2P epic, Phase 3-4).
**Extends:** ADR-017 (mTLS-as-identity), ADR-018 (identity-anchored RBAC), ADR-021 (membership audit
log), ADR-022 (SV-reconcile), ADR-023 (epoch-fenced rebase).
**Depends on:** #72 (unpredictable daemon-issued epoch token) — a security prerequisite.
**Builds on:** ADR-025 (the daemon mesh this must secure).

## Context

ADR-023 secures write-access with a **server-authoritative** epoch fence: the daemon is the *sole
canonical authority*, advancing per-member epochs and rejecting any op authored under a stale
(pre-grant) client_id. ADR-018's `kb_access` gate likewise trusts the daemon's view of the `kbc:`
collection doc. This is correct for the v0.14 single-hub topology.

**ADR-025 breaks the honest-server premise.** In a daemon mesh, peer A receives membership state and
content ops *relayed through* peer B's daemon, which A does **not** trust. Today:
- The `kbc:` collection doc is a CRDT, but its membership/role/policy mutations are **unsigned** — a
  malicious daemon can fabricate "alice is owner" or forge an epoch advance, and a receiving peer cannot
  tell a forged mutation from a legitimate one.
- The epoch fence (ADR-023 part 3) is enforced **only on the daemon that owns the doc**; a relaying peer
  has no cryptographic basis to re-enforce it.

So "decentralized KB management" requires what the user named **blockchain verification of peers and KB
contents**: membership and op provenance that **any peer verifies cryptographically**, without trusting
the relay. (ADR-023 already analyzed and deferred the fully-leaderless answer — causal-hash-DAG
capability anchoring — as research-grade; this ADR adopts the pragmatic, peer-verifiable staging that
gets us there.)

## Decision

Adopt a **staged model: A now, B documented**.

### A — Signed, hash-chained membership + signed content ops (owner-sequenced, peer-verifiable)

The KB **owner's Ed25519 key** is the per-KB authority. Two cryptographic structures make its decisions
peer-verifiable; the daemon mesh only *transports* them.

1. **Signed hash-chained membership log** (`kbc:` schema v3). Each membership/policy/epoch mutation is a
   record `{ seq, prev_hash, op, signer_fp, epoch_assignments, sig }` where
   `prev_hash = H(prev.op ‖ prev.prev_hash)` and `sig = Ed25519_sign(owner_sk, H(seq ‖ prev_hash ‖ op))`.
   The chain is **append-only and tamper-evident**: a peer recomputes each `prev_hash` and verifies each
   `sig` against the owner pubkey (which is itself anchored by the owner fingerprint = the KB principal,
   ADR-018). A relaying daemon **cannot** insert, reorder, backdate, or omit a mutation without breaking
   the chain or a signature. This is the same proven construction as Keybase sigchains / git history —
   pure-Rust (`ed25519` + `sha2`, already in the lockfile), O(1) per-record verification, ~96 B overhead
   per mutation. It is the cryptographic realization of the ADR-021 audit log (#71): one structure serves
   both integrity *and* forensics.

2. **Peer-enforceable epoch fence.** ADR-023's `derive_kb_client_id(fingerprint, epoch)` + new-ops check
   moves from "daemon-only" to **"any peer."** A receiving peer reads the signed membership chain
   (verifying it per §1), learns each member's current **epoch**, and applies the *exact* ADR-023 rule
   locally: every new op (beyond the peer's node SV) must be authored under the author's current-epoch
   client_id, else the lineage is fenced (rebase). Because the epoch lives in the signed chain, the fence
   is now **trustless** — it no longer depends on the relaying daemon being honest.

3. **Signed content ops (attribution).** Node ops carry the author's fingerprint + epoch + an Ed25519
   signature over the op bytes, so a peer verifies *who* authored an op and *under which epoch*, binding
   §2's fence to a verifiable author rather than a relay's claim.

**#72 is a hard prerequisite.** Peer-side fencing is only sound if the epoch token is **unpredictable**
(daemon/owner-issued nonce, not a guessable counter) — otherwise the ADR-023 "pre-rotation attack"
(precompute `derive(fp, E+1)`, author viewer-era ops under the future epoch) defeats a peer's fence just
as it would the daemon's. ADR-023 already tracks this as #72; it ships **before** Phase 4.

**Owner-sequenced, not leaderless.** The owner remains the sequencer of membership; peers verify the
owner's signed decisions but do not vote on them. This preserves ADR-018's RBAC semantics and the
ADR-023 invariant verbatim, now peer-verifiable. Liveness for *membership changes* needs the owner (or
delegated co-owners) reachable; **content editing stays fully offline-capable** (the fence is local).

### B — Leaderless capability / auth-DAG (documented migration, not built)

For fully ownerless content-op authority (no sequencer at all), the endgame is **causal-hash-DAG
capability anchoring** (each op references the hashes of its causal predecessors; a grant is a DAG node;
an op is valid iff it cryptographically descends from a grant and precedes any revocation — ADR-023's
deferred research item). When triggered (demand for true peer-authority / guest write-authority, library
maturation), mirror **Ink&Switch Keyhive *concap*** or **p2panda-auth** (convergent access-control CRDT).
**Explicitly do NOT mirror Matrix state-resolution** — its complexity has produced state-reset CVEs; it
is the cautionary tale, not the template.

### Rejected — BFT / Raft quorum

Replicating authority via consensus among member-daemons gives strong consistency but **requires an
online quorum**, which **contradicts MAE's local-first / offline-first principle** (CLAUDE.md #12): a
peer on a plane could neither edit nor change membership. Architecturally wrong for document sync; both
Matrix and Keyhive rejected it.

## Adversarial exploit-path review

- **Forged membership ("make me owner") / forged epoch advance** → rejected: the mutation isn't in the
  owner-signed hash chain (no valid `sig`, or `prev_hash` mismatch). A relaying daemon cannot mint
  authority.
- **Reorder / omit / backdate membership** → rejected: append-only hash chain; any gap or reorder breaks
  `prev_hash`.
- **Removed-member writes / pre-grant cascade (B-19)** → rejected at **every** peer, not just the owner's
  daemon: the author's ops are under a stale epoch (read from the signed chain) ⇒ fenced (ADR-023 §3,
  now peer-side).
- **Pre-rotation attack** → defended by #72 (unpredictable epoch); without #72 the peer fence is as
  bypassable as the daemon's, which is why #72 gates Phase 4.
- **Op replay / forged authorship** → signed content ops bind author+epoch to the op bytes; a replayed or
  mis-attributed op fails signature or epoch verification.
- **Backdating via signing** (the trap ADR-023 named) → A does **not** claim leaderless content-op
  authority; the owner-signed epoch chain is the authority, so a client re-stamping its own viewer-era op
  still authored it under the stale epoch ⇒ fenced. True backdating resistance for *leaderless* ops is
  exactly what B (causal-hash-DAG) adds later.
- **Honest-server assumptions remaining after A:** none for membership/write-authority. *Confidentiality*
  from a relaying daemon is **not** provided by A (a relay still sees plaintext CRDT) — that is the
  deferred E2E-encryption work, called out so it is not mistaken for solved.
- **Membership ≠ connectivity (silent eviction by disconnect).** Membership is authoritative only via this
  signed chain; it is **never** conferred or revoked by transport state. A member that goes offline (sleep,
  dynamic-IP change, outage — ADR-025 §lifecycle) stays a member and re-syncs by node-id on return.
  Removal is an explicit signed `prev_hash`-linked op, so a dropped/idle connection — or the v0.14 idle
  *document* eviction — must **not** be read as membership loss. A peer that simply stops talking cannot be
  treated as removed (that would be a relay-driven authority change, the exact thing A forbids).

## Consequences

- Membership + write-authority become **peer-verifiable**; the daemon mesh (ADR-025) can relay freely
  without being trusted for correctness.
- `kbc:` gains schema v3 (signed chain). **Migration:** v2 docs are accepted read-only until the owner
  re-shares (re-signs the chain); first post-feature write upgrades to v3. v0.14 hub mode keeps working.
- One structure (the signed hash chain) discharges both #71 (audit log) and mesh integrity.
- Cost: an Ed25519 signature per membership mutation + per content op (negligible; verification is O(1)).
  Owner reachability is required for *membership* changes (not editing).
- Reviewer guardrail: any membership mutation or content op **applied without** passing chain/signature/
  epoch verification on the receiving peer is a security regression — reject it. Transport auth (ADR-025)
  is necessary but never sufficient.

## Verification

Unit: hash-chain build + verify (tamper/reorder/omit each rejected); owner-sig verify; signed-op
author/epoch verify; peer-side `derive_kb_client_id` fence parity with ADR-023. Security-negative E2E on
a **real-daemon mesh** (no central server): a malicious relay daemon that forges a membership mutation,
reorders the chain, or relays a stale-epoch op is rejected by the receiving peer; a removed member's
writes don't cascade; a revoked key's ops are refused. Mirrors the ADR-023
`viewer_era_edits_do_not_cascade_on_grant` oracle, now asserted **peer-side**. Gate: #72 landed; v0.14
single-hub security tests stay green.
