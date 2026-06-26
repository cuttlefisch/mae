# ADR-026: Peer-verifiable KB integrity (signed membership op-log + derived validity)

**Status:** Accepted; **Phase 2b implemented** (PR #102). In `mae-sync`: the signed membership
**op-log**, per-peer **derivation**, the p2panda-auth **strong-removal resolver**, inviter-removal
**cascade**, **local blocklist**, and **quorum** governance — all unit-tested. Wired into the daemon:
sign-on-mutate for owned KBs; `kb_access` peer-verifies derived membership for **anchored** (joined)
KBs. **#72 landed** (PR #99), so the peer-enforceable epoch fence (§A5 / Phase 4) is unblocked.
**Remaining:** signed **content** ops (Phase 4 second half — now designed in **ADR-036**); quorum sourced
in the daemon gate (mae-sync ready, #132); E2E content **confidentiality** (the deferred read-protection
called out below — now designed in **ADR-037**); the fully-leaderless auth-DAG (§B, research — ADR-039).
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

Adopt a **signed membership OP-LOG with per-peer derived validity** (the p2panda-auth model), plus a
peer-enforceable epoch fence. Membership is **capability-based, signed, audited, revocable, time-boxed,
and convergent**; the daemon mesh only *transports* it. (This is richer than the originally-sketched
owner-sequenced linear chain — concurrency + quorum + delegation demanded the op-log.)

### A1 — Membership is a signed op-LOG (CRDT set of ops; derived validity, not stored verdict)

The `kbc:` collection stores an **append-only CRDT set of signed membership ops** (keyed by each op's
`chain_hash`, so concurrent appends converge). Each op = `MembershipOp{kb_id, action(admit/remove/
set_role/revoke), subject, role, can_invite, author, issued_at, expires_at, prev_hash}` + `sig` +
`author_pubkey`. `prev_hash` links each op to the author's view-head, forming a **causal DAG**;
`chain_hash = H(canonical_bytes ‖ sig)` (Keybase-style, binds the signature). Current membership is
**derived** by every peer (`derive_valid_members`) by replaying the log — **never** read as a trusted
verdict from a relay. Pure-Rust (`ed25519` + `sha2`, in-lockfile), O(1) per-op verify, ~tiny per op. This
*is* the ADR-021 audit log (#71): one structure for integrity + forensics + the `invited_by` provenance.

**External trust anchor (load-bearing).** Verification binds the log's **genesis** owner-op `author_pubkey`
to an *external* anchor — the daemon's own pubkey for KBs it owns, or the **join-ticket node-id** (= owner
pubkey) for KBs it joined (ADR-025). Never the self-attested collection. A relay that ships a forged
collection fails the anchor check; the genuine owner pubkey then roots the chain, and each later op's
author pubkey is established by the op that admitted that author.

### A2 — Capability-based named invites (UCAN-grounded)

A grant is an `Admit` op **naming** issuer (`author`) → subject, with an optional delegable `can_invite`
capability and a timebox (`expires_at`). An op is valid only if (causal-capability, p2panda-auth): its
`sig` verifies, `fingerprint_of(author_pubkey) == author`, the **author was a valid member holding the
capability at the op's causal position**, **attenuation** holds (granted role ≤ author's), and it is
in-timebox. **Mutual verification** (the user requirement): both the inviter (signature + capability) and
the joiner (the joining peer's iroh `remote_id` == the admitted/named principal) are verified before
admission — the signature is the inviter's non-repudiable, *offline* attestation. Two grant paths:
request→approve (the approval signs the admission) and a pre-issued named invite; both yield a signed,
named, audited member op.

### A3 — Convergent resolver: p2panda-auth "strong removal" (verbatim — do NOT invent)

Concurrent conflicts resolve by p2panda-auth's implemented **strong-removal** ruleset (Matrix state-res's
home-grown rules shipped state-reset CVEs — this is the cautionary tale): a **remove/demote invalidates
that member's concurrent actions, transitively**; **mutual removal** (A removes B ∥ B removes A) ⇒ **both
removals apply**, their other concurrent actions invalidated; **re-add** ⇒ valid again but pre-removal
concurrent ops stay invalidated (= our `retired` tombstone + `next_epoch` fresh token, #72). The tiebreak
for a genuine same-target conflict is **higher `chain_hash`** — deterministic + **Sybil-resistant**;
explicitly **not seniority** (p2panda flags it Sybil-prone) and **not a wall-clock**. `derive_valid_members`
walks the DAG, finds concurrent bubbles, builds an invalidation filter, invalidates dependents ⇒ every
honest peer derives the identical set with no coordinator. **Cascade** on inviter removal is per-KB
configurable: `revoke_on_inviter_removal ∈ {pending_only (default), cascade_all, retain}`.

### A4 — Compromise revocation: configurable governance + local blocklist

Revoke a malicious/compromised member, *even the owner* (members are uniform — no creator immunity, per
the prior art), as a **configurable stance**:
- **Local blocklist** (always-available, per-peer self-protection): a daemon-local set; `derive_valid_members`
  drops a blocked principal **locally** and the daemon refuses their ops/sync — unilateral, immediate, can
  block even the owner. Never forges global membership; only restricts what *this* peer accepts.
- **Per-KB `governance`**: `single-owner` (default — one root, irrevocable via the chain; handle a
  compromised owner via every peer's blocklist + re-key/fork) **or** `quorum` (an admin set + threshold
  `m-of-n`: removing any admin/owner needs `m` distinct admins to each append a `revoke` op; the derivation
  **tallies co-signatures** so a single compromised admin can't unilaterally remove others; the strong-
  removal resolver settles concurrency). This pulls a constrained slice of B (below) forward.

### A5 — Peer-enforceable epoch fence (unchanged from ADR-023, now over the op-log)

A receiving peer reads each member's current **epoch** from the verified op-log and applies ADR-023's
`derive_kb_client_id(fingerprint, epoch)` rule locally: a new op must be authored under the author's
current-epoch client_id, else the lineage is fenced (rebase). The fence is **trustless** — it no longer
depends on a relay being honest. **#72 is a hard prerequisite**: peer-side fencing is sound only if the
epoch token is **unpredictable** (else the ADR-023 pre-rotation attack defeats a peer's fence too).

**Scope = membership.** Signed *content* ops (node-edit attribution) are a deferred follow-on; ADR-023's
epoch fence already governs content. Liveness for *membership* changes needs an authorizer (owner/admin)
reachable; **content editing stays fully offline-capable** (the fence + derivation are local).

### B — Fully leaderless capability / auth-DAG (documented migration, beyond the quorum slice)

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

- **Forged membership ("make me owner") / forged epoch advance** → rejected: the op isn't a valid signed
  op-log entry (no valid `sig`, `fingerprint_of(author_pubkey) ≠ author`, or the author held no capability
  at that causal point). `derive_valid_members` simply never counts it; a relaying daemon cannot mint
  authority.
- **Forged genesis / collection-swap** → rejected by the **external trust anchor**: the genesis owner-op's
  `author_pubkey` must equal the daemon's own pubkey (owned KB) or the **join-ticket node-id** (joined KB,
  ADR-025). A relay that ships a self-attested collection naming itself owner fails the anchor check.
- **Reorder / omit / backdate membership** → the op-log is a CRDT *set* keyed by `chain_hash`, ordered by
  the `prev_hash` causal DAG, not by relay arrival; an injected gap or reorder breaks `prev_hash` linkage
  and the op is orphaned (never reachable from the anchored genesis). Omission is liveness, not safety
  (every honest peer that *has* the op still derives it).
- **Over-broad grant / privilege escalation** → rejected by **attenuation**: an op granting a role above
  the author's own does not contribute. Capability (`can_invite`) must trace to the owner.
- **Expired / stale invite** → rejected by the **timebox** (`expires_at`); revoked-inviter invites
  cascade per `revoke_on_inviter_removal`.
- **Remove-the-remover / concurrent membership race** → resolved deterministically + identically on every
  peer by the **strong-removal resolver** (mutual removal ⇒ both out; higher-`chain_hash` tiebreak), with
  no coordinator and no state-reset.
- **Compromised admin removing the rest** → blocked under `quorum` governance: a single admin's `revoke`
  doesn't reach threshold; `m` distinct admin co-signatures are tallied before the removal takes effect.
- **Compromised owner** → every peer's **local blocklist** drops the owner *locally* (immediate, needs no
  consensus); `quorum` governance additionally allows a co-signed network-wide removal (members are
  uniform, no creator immunity).
- **Removed-member writes / pre-grant cascade (B-19)** → rejected at **every** peer, not just the owner's
  daemon: the author's ops are under a stale epoch (derived from the signed op-log) ⇒ fenced (ADR-023 §3,
  now peer-side), and the strong-removal resolver transitively invalidates their concurrent grants.
- **Pre-rotation attack** → defended by #72 (unpredictable epoch); without #72 the peer fence is as
  bypassable as the daemon's, which is why #72 gates Phase 4.
- **Backdating via signing** (the trap ADR-023 named) → membership authority is the signed op-log + epoch,
  not a client's self-stamp; a client re-stamping its own viewer-era *content* op still authored it under
  the stale epoch ⇒ fenced. True backdating resistance for *leaderless content* ops is exactly what
  signed content ops + B (causal-hash-DAG) add later.
- **Honest-server assumptions remaining:** none for membership/write-authority. *Confidentiality* from a
  relaying daemon is **not** provided (a relay still sees plaintext CRDT) — that is the deferred
  E2E-encryption work, called out so it is not mistaken for solved.
- **Membership ≠ connectivity (silent eviction by disconnect).** Membership is authoritative only via this
  signed chain; it is **never** conferred or revoked by transport state. A member that goes offline (sleep,
  dynamic-IP change, outage — ADR-025 §lifecycle) stays a member and re-syncs by node-id on return.
  Removal is an explicit signed `prev_hash`-linked op, so a dropped/idle connection — or the v0.14 idle
  *document* eviction — must **not** be read as membership loss. A peer that simply stops talking cannot be
  treated as removed (that would be a relay-driven authority change, the exact thing A forbids).

## Consequences

- Membership + write-authority become **peer-verifiable**; the daemon mesh (ADR-025) can relay freely
  without being trusted for correctness.
- `kbc:` gains schema v3 (`COLL_OPLOG_KEY` signed op-log). **Migration:** v2 docs (LWW member map) are
  accepted read-only until the owner re-shares (writes the genesis owner-op + replays existing members as
  signed `Admit` ops); first post-feature write upgrades to v3. v0.14 hub mode keeps working (psk/none
  auth = legacy unsigned path; signed membership requires daemon key-mode, which the mesh requires anyway).
- One structure (the signed op-log) discharges #71 (audit log), the `invited_by` provenance, and mesh
  integrity.
- Cost: an Ed25519 signature per membership mutation (negligible; verification is O(ops), the log is small
  — membership changes are infrequent). The growing log is bounded later by ADR-028 signed checkpoints
  (a follow-on optimization for fresh-peer onboarding, not a 2b blocker). An **authorizer** (owner/admin)
  must be reachable for *membership* changes; content editing stays fully offline-capable.
- Reviewer guardrail: any membership op **counted as valid without** passing anchor + signature +
  capability + attenuation + timebox + resolver checks on the receiving peer is a security regression.
  Transport auth (ADR-025) is necessary but never sufficient.

## Verification

Unit: op-log append round-trips; concurrent appends converge as a set; `MembershipOp` sign/verify
(tamper/wrong-key/forged-fingerprint each rejected); `derive_valid_members` replay (owner genesis admits;
valid inviter chain admits; forged/over-attenuated/expired/orphaned absent; external-anchor mismatch ⇒
empty). **Resolver oracles (p2panda-auth parity):** concurrent admit-by-inviter vs remove-inviter ⇒ admit
invalid on both peers; mutual admin removal ⇒ both out + their other concurrent grants invalid; re-add ⇒
valid but pre-removal ops still fenced; higher-`chain_hash` tiebreak deterministic under a **shuffled
apply order**; transitive invalidation of dependents. **Governance/blocklist:** quorum `revoke` needs
`m` distinct admins (a single admin's revoke doesn't take effect); local blocklist denies a blocked
principal — incl. a simulated compromised owner — on that peer only. Peer-side `derive_kb_client_id`
fence parity with ADR-023. Security-negative E2E on a **real-daemon mesh** (no central server): a
malicious relay that forges/reorders membership or relays a stale-epoch op is rejected by the receiving
peer; a removed member's writes don't cascade; a revoked key's ops are refused. Mirrors the ADR-023
`viewer_era_edits_do_not_cascade_on_grant` oracle, now asserted **peer-side**. Gate: #72 landed; v0.14
single-hub security tests stay green.
