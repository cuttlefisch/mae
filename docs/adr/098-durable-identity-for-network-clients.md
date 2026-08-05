# ADR-098: Durable identity for network clients — devices, OIDC, and the binding table

**Status:** Proposed. Phased (A–E); Phase A is a prerequisite fix, not new design.
**Depends on:** ADR-097 (Browser MAE is a KB surface), ADR-017/018 (asymmetric peer auth +
identity-anchored access control), ADR-026 (signed, hash-chained membership op-log), ADR-036/037
(signed content ops + E2E content encryption), ADR-040/041 (identity rotation/rebind + key
separation), ADR-052 (OAuth 2.1 resource server).
**Extends:** ADR-040 (uses its `Rebind` as the device-loss recovery path rather than adding a
parallel one).
**Relates to:** ADR-023 (epoch fence — measured here and found *not* to be a revocation signal),
ADR-067 (replication policy), ADR-060 (multi-tenancy).
**Evidence:** four spikes, all executable and passing —
`docs/research/098-multi-device-identity-spike.md`,
`098-device-secret-storage-and-revocation-spike.md`,
`098-revocation-forward-secrecy-spike.md`,
`098-revocation-write-fencing-spike.md`.
**Blocked on:** issue #176 (Phase A).
**Tracking:** issue TBD.

## Context

Browser MAE (ADR-097) gives every browser profile its own keypair. MAE's membership is keyed on a
single Ed25519 fingerprint inside a signed, hash-chained, peer-verifiable op-log (ADR-026), so one
human with a laptop, a desktop and a phone is three fingerprints. The organisation this is being
built for manages people in Active Directory and authenticates through Authentik, neither of which
has any concept MAE's op-log understands.

Four questions had to be answered before this could be designed. All four were answered by
executing the code rather than reading the ADRs, and two of the answers were not what the design
documents implied.

**1. Can one member hold several device keys today?** No. ADR-040's `Rebind` is succession, not
concurrency: enrolling a second device *retires* the first, and a retired device cannot enrol a
third, so there is no fan-out path. Membership converges to the owner plus exactly one live device.

**2. Can the existing delegated-invite machinery express a device set?** Partly, and the failure is
instructive. A member holding `can_invite` genuinely *can* admit their own second device, and both
stay members — the concurrency `Rebind` lacks. But once devices are members, every membership
semantic applies to them and several are wrong: under the default `InviterRemovalPolicy`, removing
a member leaves their devices behind; devices appear in member lists and role tables as if they
were people; and the member still cannot revoke their own device. **Devices must be something a
member *has*, not members.**

**3. Is content-key delivery owner-rooted?** Yes, and this was proven twice by independent routes —
once through `Rebind`, once through delegated invite. `find_wrapped_content_key` honours a wrap only
when `owners.contains(&o.author)`, so a member holding the content key cannot hand it to their own
device. That it appears twice makes it a property of the design rather than of one op kind.

**4. Can revocation avoid the owner, or avoid rotation?** It avoids rotation but not the owner.
Removing a device stops its writes immediately with no content-key rotation anywhere in the path —
but the thing that stops it is the `kb_access` authorization gate, needing an owner-authored
`Remove`, not ADR-023's epoch fence. The fence *structurally* cannot carry revocation: it
discriminates on `derive_kb_client_id(principal, epoch)`, and since `kb_member_epoch` documents
"absent member ⇒ 0" while a fresh grant also sits at epoch 0, the discriminator is identical before
and after revoking a typical device.

**The shape those four answers make.** The owner is in the loop for every authority-changing
operation, and nothing routes around it. A design that puts *devices* in the CRDT therefore
multiplies owner actions by device count — which is precisely what does not scale in an
AD-managed organisation.

**Two further findings change what is affordable.** Rotation permanently strands history: a
continuously-authorised member who re-syncs from scratch after a rotation opens 3 of 9 ops, and
three rotations leave them 2 of 8 — cumulative, so rotating per device revocation degrades the KB
monotonically (issue #176, `FIXME(#237)`). But retaining every key wrapped to a member restores
complete history *without* giving a revoked device anything, because that device is never wrapped
the new key. **History availability and forward secrecy are independent, not a trade-off** — which
is why #176 is a prerequisite here rather than an adjacent bug.

## Decision

### D1 — The cryptographic subject is a stable **member** key, never a device key

Membership, signed content ops, epoch derivation and content-key wraps continue to name a single
Ed25519 fingerprint per human. That fingerprint identifies **the member**, not the machine they are
sitting at.

This is a decision about what a fingerprint *means*, and it requires **no protocol change at all** —
no new op kind, no derivation change, no new verification rule. Every property ADR-026 and ADR-036
already provide carries over untouched, which is the point: the peer-verifiable membership log is
the hardest thing in this system to change safely, and this design does not change it.

### D2 — Devices obtain the member key from recovery-key-sealed secret storage

A device becomes the member by *holding the member key*, obtained from an encrypted blob the daemon
stores and cannot read — the shape Matrix uses for cross-signing material (SSSS), reached here by
the same reasoning.

- The member secret is sealed under a key derived from a **user-held recovery key**, using
  `content_crypto`'s existing XChaCha20-Poly1305 AEAD and a domain-separated sha2 derivation. The
  spike demonstrated this against the shipped API: correct-key round-trip, four distinct wrong keys
  opening nothing, an exhaustive per-byte tamper sweep, and a byte-level key-blind leak scan. **No
  new dependency** — which matters, because MAE deliberately derives with sha2 rather than pulling
  an `hkdf` crate (a second `digest` major would break the coherence constraint recorded in
  `shared/sync/Cargo.toml`).
- The daemon stores ciphertext keyed by OIDC principal, outside the CRDT. It can withhold the blob
  (a denial of service) or substitute one (detected — AEAD fails under the user's recovery key). It
  cannot read it.
- The browser imports the unsealed key as a **non-extractable** WebCrypto `CryptoKey`. Ed25519 and
  X25519 are now in all major engines (Firefox 129, Safari 17, Chrome 137) at roughly 79% of web
  users, so a stated minimum-browser requirement is needed rather than assumed universality — see
  Consequences.

**Devices are invisible to the protocol in v1.** No device concept enters the op-log, because a
device cryptographically *is* the member. That is what makes D1's "no protocol change" true.

### D3 — The `principal ↔ fingerprint` binding lives in daemon state, outside the CRDT

OIDC authenticates the human. The mapping from an OIDC principal to a member fingerprint is a
**mutable table in daemon state**, never a fact in the signed log.

This is the single most important structural decision here, and it is about disaster recovery and
provider migration rather than cryptography. Membership lives in a signed, hash-chained,
append-only log that is replicated to every peer and included in every backup. An OIDC `sub` is
provider-scoped: migrating Authentik → Entra, or rebuilding Authentik, changes every `sub`. If the
`sub` were the subject, that migration would orphan **every grant in a log that cannot be
rewritten** — you would need the owner to re-sign a full re-grant per member per KB, and peers
holding the old log would still honour the old grants.

With the binding outside the CRDT, a provider migration rewrites a mutable table. Restoring from
backup restores a cryptographic subject exactly, with no dependence on the IdP's state.

MAE already ships the reverse of this binding: `kb/query.self_token`
(`daemon/src/collab_handler/mod.rs`) mints an OAuth token *from* a verified TLS fingerprint. This
is the same binding in the other direction.

### D4 — AD groups gate the **session**; CRDT membership stays owner-authored

Two revocation speeds, deliberately separated:

- **Session authorization** — checked per connection from the OIDC token and its group claims. An
  AD group removal takes effect on the next token refresh, with **no CRDT write and no owner
  involvement**. This is the fast path, and it is what an organisation actually means by "revoke
  Bob's access".
- **CRDT membership** — owner-authored, slow, replicated, peer-verifiable. Unchanged.

The daemon must **not** be given authority to author membership ops on the owner's behalf. A daemon
that can grant or remove membership is indistinguishable from the owner, which destroys the
peer-verifiable property the op-log exists to provide. The split above gets instant organisational
revocation without paying that price.

Note for implementers: Entra caps group claims at 200 with an overage claim requiring a Graph
callback, and ObjectId is the only universally stable group format. Group→role mapping is
configuration, not a signed fact.

### D5 — Device loss is handled by member-key rotation, not per-device revocation

Because a device holds the member key (D2), a lost device cannot be revoked individually. The
recovery path is ADR-040's existing `Rebind`: rotate the member key, the owner re-wraps the content
key to the successor, and the user's other devices are re-provisioned from secret storage under the
new secret.

This is a real limitation, and it is accepted deliberately because of how the costs fall:

| Event | Frequency | Owner actions |
|---|---|---|
| Add a device | common | **zero** |
| Lose a device | rare | one (rebind + re-wrap), i.e. today's rotation cost |

The alternative — per-device revocation via device certificates in the op-log — is a substantially
larger change (a new op kind, a two-level verification chain every peer must implement, derivation
changes) and buys a property that is only needed on the rare event. It is deferred to Phase E and
its own ADR, not rejected.

Per the forward-secrecy findings, note also what rotation does and does not achieve: it stops the
lost device reading *future* content, and cannot un-share what it already had. A KB where that is
unacceptable needs its content treated as disclosed, not merely re-keyed.

### D6 — Issue #176 is a prerequisite, and Phase A

Until a member retains every key wrapped to them and tries each per blob, any rotation permanently
truncates history for every member who later re-syncs. D5 makes rotation the device-loss path, so
shipping D5 before #176 would mean every lost laptop permanently damages the KB's readable history
for everyone else.

## Phases

- **A — fix #176** (prerequisite). Members retain all wrapped keys; `open_new_ops` tries each per blob. No new design; the spike already demonstrated the fix's shape and safety against the shipped API.
- **B — MAE secret storage.** The sealed-blob format, a properly specified recovery-key derivation *with a checksum so a mistyped key is caught before it produces a wrong key* (Matrix uses base58 + HKDF + a MAC check; the spike's `sha2(domain ‖ recovery)` is a placeholder, not a proposal), and the daemon's key-blind storage endpoints.
- **C — binding table + session gate.** D3's `principal ↔ fingerprint` table, D4's group-claim session authorization, and the enrolment flow that registers a fingerprint against a principal.
- **D — browser device enrolment, end to end.** OIDC login → fetch blob → recovery-key prompt → unlock → non-extractable import → act as the member. Including the browser-storage-eviction path (see Consequences).
- **E — per-device revocation via device certificates.** Deferred; own ADR. Only start once a real operational need appears, since it reopens the op-log's verification rules.

## Consequences

**Positive.** Multi-device works with **no change to the membership op-log, the signed-content-op
rules, or the content-key delivery model** — the three hardest things in the system to change
safely. Adding a device costs zero owner actions, which is the property that makes an AD-managed
deployment viable. Identity survives both backup restore and IdP migration by construction, because
the durable subject is cryptographic and the volatile mapping is a table. And the organisation gets
instant revocation through AD without the daemon being handed owner authority.

**Costs, stated honestly.**

- **A compromised device compromises the member.** Every device holds the member key, so there is no per-device blast radius until Phase E. This is the central trade of D2/D5, and it should be stated plainly in user-facing docs rather than discovered.
- **The recovery key is a real user burden.** Matrix's model is documented to produce confusing verification flows and "Unable to decrypt" states; MAE inherits that. A user who loses the recovery key and has no enrolled device loses access to E2E content that no owner action can restore.
- **Browser storage is evictable.** Safari ITP and storage-pressure eviction can silently delete a non-extractable key. Phase D must request `navigator.storage.persist()` *and* treat re-enrolment as a normal, cheap flow rather than an exceptional one.
- **~21% of users are on engines without WebCrypto Ed25519/X25519.** This design needs a stated minimum-browser requirement. A JS/WASM fallback would forfeit non-extractability, which is much of the point, so the honest choice is to require the capability rather than degrade quietly.
- **The daemon learns metadata** — which OIDC principal maps to which fingerprint, and when devices enrol. Not content, but not nothing, and it should be documented as part of the daemon's threat model.

**Downstream/bug-risk framing (principle #9).** Phases A and B are additive and well-isolated: A
changes only the caller's key handling (the op-set layer needs no change at all), and B adds a new
storage surface with no existing consumer. Phase C touches the OAuth listener's authorization path,
which is security-critical and must inherit that listener's existing rejection tests wholesale.
Phase E is the only phase that reopens the op-log's verification rules, which is why it is last and
separately gated.

## Alternatives rejected

- **Devices as members, via delegated invite.** Tested rather than reasoned about, and it partly works — which is why it needed testing. Rejected because every membership semantic then applies to devices and several are wrong: removal does not cascade to devices by default (and cascade is a *local per-peer* setting, so peers can legitimately disagree about whether a revoked user's devices still have access), devices pollute member lists and role tables, and the member still cannot revoke their own device.
- **An OIDC `sub` as the membership subject.** Rejected on disaster-recovery and migration grounds (D3). It is *technically* possible today — `kb/add_member` takes an unvalidated string — which is exactly why it needs rejecting explicitly rather than being left as an available mistake.
- **Daemon-side signing on the member's behalf.** Rejected: a daemon that can author membership or content ops for a principal can forge any member's ops, which destroys the peer-verifiable property ADR-026 exists to provide and voids E2E confidentiality against the host.
- **Per-device certificates in v1 (Phase E brought forward).** Rejected for sequencing, not on merit. It is the better long-run answer and is why Phase E exists; doing it first would mean changing the op-log's verification rules before the far cheaper D1/D2 path has demonstrated whether per-device revocation is needed in practice.
- **Extending `Rebind` to be additive rather than retiring.** Rejected: `Rebind`'s retirement is load-bearing for key rotation (a rotated-away key must stop acting), and overloading one op with two opposite semantics would make the derivation ambiguous. Note that the owner path already behaves additively (`owner_principal_chain` is a forward-closure set that never retires) — an undocumented asymmetry filed as issue #661, which should be resolved before anything here relies on that shape.

## Verification

Per principle #14, each phase is verified by trying to falsify it. The four spikes' tests stay in
the tree as regression guards — several assert current *limitations* and are written to fail loudly
if a change moves the wall, which is exactly what should happen when Phase E lands.

- **Phase A.** A member admitted before a rotation, re-deriving from scratch, opens the *complete* history; a removed device opens strictly what it had before and nothing more. Both are already expressed in `revocation_forward_secrecy.rs` and must keep passing with the retained-key implementation substituted for the loop the spike hand-rolled. Plus the negative control: with no rotation, one key opens the entire op-set — otherwise the stranding counts are harness artefacts.
- **Phase B.** The per-byte tamper sweep, the wrong-key set, and the key-blind leak scan from `device_secret_storage_and_revocation.rs`, retained against the real format. Added: a **mistyped recovery key must be rejected by the checksum before it produces a wrong key**, and the leak oracle's own negative control must keep firing on a deliberately-unencrypted blob.
- **Phase C.** Every new route inherits the OAuth listener's existing rejection behaviour identically — wrong, expired, forged and missing-claim tokens — plus a raw-response-byte scan proving no other principal's binding leaks, matching the gate `daemon/tests/oauth_e2e.rs` and `daemon/src/tests/webview_tests.rs` already establish. Adversarially: a token whose group claims were removed must lose session access *without* any CRDT write occurring, and a principal must never be able to bind a fingerprint that another principal already holds.
- **Phase D.** Two browsers, one member, editing concurrently and converging — reusing the Phase 0 interop harness. Adversarially: a device whose IndexedDB is cleared mid-session must fail closed and re-enrol rather than silently authoring unsigned or wrongly-signed ops.
- **Phase E.** Revoking one device must leave the member's other devices working and the member key untouched — the property this whole ADR defers. It must also *not* regress forward secrecy: the revoked device gains nothing from the retained-key mechanism Phase A introduces.

**Two gaps this ADR inherits and must close rather than assume**, both named by the spikes as
untested:

- **The partition window.** A peer that has not yet received a `Remove` will still accept a revoked principal's writes. Inherent to a distributed signed log, but its practical size is unmeasured, and a browser client that reconnects across partitions may widen it. Phase C must measure it, not reason about it.
- **Mesh relay parity.** `enforce_epoch_fence`'s doc claims (#157 N1) it is the one fence shared by the hub *and* the mesh dialer, so enforcement cannot be present on one and absent on the other. The write-fencing spike exercised the hub path only. That completeness claim must be tested directly before Phase C relies on it.

One further inherited uncertainty, stated rather than buried: **whether a non-owner member may
author a content-key rotation** was inferred from the wrap-authorship rule, not measured. D5 assumes
they cannot. If that inference is wrong, D5's cost table is wrong in the user's favour, and the
Phase E deferral should be revisited.
