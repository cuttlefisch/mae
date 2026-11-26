# ADR-098: Durable identity for network clients — devices, OIDC, and the binding table

**Status:** Proposed. Phased (A–E); Phase A is a prerequisite fix, not new design. Amendment 1
(2026-11-14) brings Phase C forward for the read path and defines who counts as a person.
Amendment 2 (2026-11-26) confirms D1 for Browser MAE's signing and drops a per-device-key detour.
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
**Blocked on:** issue #176 (Phase A), for Phases B, D and E. Phase C's read path is not (Amendment 1).
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
- **C — binding table + session gate** (built first, for the read path: Amendment 1). D3's `principal ↔ fingerprint` table, D4's group-claim session authorization, and the enrolment flow that registers a fingerprint against a principal.
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

## Amendment 1 (2026-11-14): Phase C first, for reads; who counts as a person; Phase B settled

Browser MAE's reader (ADR-097) must enforce per-person, per-KB access. Today it can only do so for a
person whose key is already a member and who presents a daemon-issued token: an IdP session names
nobody the daemon's membership knows. This amendment brings Phase C forward for that case, defines
the person/agent distinction ADR-113 already relies on but this ADR never stated, and records the
three Phase B questions decided on 2026-11-13.

### C0. Phase C is built before #176, for the read path

Phase A (#176) is a prerequisite for **D5**: rotation is the device-loss path, and without retained
keys every rotation strands history. Phase C (binding table + session gate) involves no rotation and
no content key, so #176 does not block it. The binding gives an IdP session exactly the authority
`kb/query.self_token` already gives whoever holds the same key — D3 calls that token "the same
binding in the other direction" — so it adds no capability over what exists, only a second way to
present it.

The scope is the read path: `kb/query.*`, including `kb/query.my_kbs`. Writes over an IdP session
wait for the hub to verify op signatures (#727, ADR-111 D4), as every HTTPS write does. On an E2E
KB a bound session sees what that member's self-issued token sees — ciphertext and its own wrapped
key — and cannot decrypt in a browser until Phase D.

A person with no key at all (browser only) is not served by C0; they wait for Phases B and D.

### C1. The principal is typed

An IdP token's subject is never compared with a member fingerprint. The listener resolves every
bearer token to one of:

- **a member fingerprint** — a self-issued token's principal, or an IdP token whose `(iss, sub)` is
  bound in the D3 table;
- **an unbound IdP subject** — any other IdP token, carried as `(iss, sub)` in a form no fingerprint
  can equal. It is a member of nothing, so `kb_access` denies it every KB, and `kb/query.my_kbs`
  answers it with an empty list.

This is ADR-052 D4's typed principal (ADR-111 P2), required here because C0 is the first place an
IdP subject and a fingerprint meet.

This supersedes one path ADR-067 Phase D left open: accepting an external token whose
`principal_claim` already carries the member's fingerprint. Whether that is safe depends on who can
influence the claim at the IdP, which the daemon cannot see. A deployment that relied on it binds
its subjects instead (C2), and the binding is the only way an IdP token reaches a fingerprint.

### C2. Binding is by proof of possession, one subject to one key

1. With an IdP bearer token, the person asks the listener for a challenge. A self-issued token is
   refused: its principal already is a fingerprint.
2. The daemon returns a single-use challenge, valid for minutes, covering a domain-separation tag,
   `iss`, `sub`, the listener's audience, the fingerprint being claimed and a random nonce.
3. The person signs it with the member key. The program that holds the IdP session (the browser,
   for Browser MAE) and the one that holds the key (native MAE, or a CLI) need not be the same; the
   challenge and the signature are short enough to carry between them. Native MAE signs with
   `mae --sign-identity-challenge` (decided 2026-11-15). It signs only the daemon's canonical
   challenge for its own key, and only when the person states the issuer and subject the challenge
   names, so nobody can be talked into binding another person's login to their key.
4. The daemon checks the signature against the claimed public key, that the key's fingerprint is the
   one in the challenge, that the challenge is unexpired and unused, and that the token presenting
   the signature has the same `(iss, sub)`.

Constraints, each tested: one `(iss, sub)` is bound to at most one fingerprint and one fingerprint
to at most one `(iss, sub)` (D3's "never bind a fingerprint another principal holds"); replacing a
binding takes an explicit unbind first; binding grants no membership — membership stays
owner-authored (D4).

The binding table, the person flag (C3) and unbinding are administered by a **local** daemon CLI,
like the token CLI, never by a network method, an MCP tool or Scheme (principle #16). That CLI may
also bind **without** proof of possession, for development, tests and recovery: a local operator can
already mint a token for any fingerprint, so this adds no power. Such a binding is recorded as made
by the administrator rather than by proof, and the binding list shows which is which (decided
2026-11-14).

The D4 session gate is part of C: an optional set of allowed groups, checked on every request
before the binding is looked up. A token carrying **any one** of them is admitted, one carrying
none is refused (decided 2026-11-15); with none configured there is no group gate.

### C3. Who counts as a person

A request counts as a **person** only when both hold:

1. its credential is an IdP token, from an interactive sign-in; and
2. the bound fingerprint is flagged `person` in the binding table.

Every other request counts as an **agent**: self-issued tokens, the native editor's mTLS session,
and IdP sessions whose binding is unflagged. A binding is `agent` until an administrator sets it.
This is the "whether a principal is an agent is daemon state" ADR-113 relies on, and it is what a
version history record (ADR-116) and a publication approval (ADR-113) read.

Both conditions are needed because an agent working in a person's editor signs with that person's
key (D1, D2): a flag on the key alone would make it a person. Requiring an IdP sign-in is what an
agent in the editor does not have.

Stated rather than discovered later:

- **A person's own edits from native MAE count as agent** until native clients sign in through the
  IdP (ADR-111 P4). A rule that "only a person may clear an agent's change" therefore applies to
  them too. Accepted as the interim state (2026-11-14), rather than adding a third kind; ADR-111 P4
  removes it.
- **An agent driving a browser that holds a person's live IdP session counts as that person.** The
  daemon cannot tell them apart; that boundary belongs to the IdP and the device.

### C4. Phase B decisions (2026-11-13)

- **Two secrets.** D2's secret-storage recovery key and ADR-040's offline recovery key `R` are
  different secrets with different jobs: the first unseals the member key on a new device, the
  second signs a `Rebind` when the member key is lost or burned and must stay offline and unused.
  The interface never presents them as one, and names them differently.
- **Owner recoverability.** An owner registers `R` at enrolment, enforced by the interface; no
  co-owner design now. A sole owner without `R` is not an offered state.
- **Browser cryptography.** The envelope, wrap and sealed-blob code in the browser is MAE's own Rust
  crypto compiled to WASM, published with the same signed provenance as the `mae-org` WASM. An
  audited JavaScript implementation serves only as a differential check in tests. A second, AES-GCM
  envelope for WebCrypto is rejected: it would fork the wire format that signed ops and wraps commit
  to.

### C5. Verification added

On top of Phase C's existing list:

- **Typed principal.** An IdP token whose `sub` is byte-identical to a member's fingerprint string
  is denied that member's KBs, and `kb/query.my_kbs` answers it with an empty list.
- **Proof of possession.** Refused: a signature by another key; a challenge for another fingerprint,
  subject or audience; an expired challenge; a replayed one; a self-issued token asking for a
  challenge; a second subject binding an already-bound fingerprint, and a second fingerprint for an
  already-bound subject.
- **Kind.** A self-issued token for a `person`-flagged fingerprint is an agent; an IdP session bound
  to an unflagged fingerprint is an agent; only an IdP session on a flagged binding is a person. No
  network method changes a flag (a raw-byte scan of every method's refusal, as Phase C requires).
- **Unbinding.** After an unbind, the next request with the same IdP token is an unbound subject.

## Amendment 2 (2026-11-26): the browser signs with the member key; no per-device keys

The browser client does not sign its content ops today, and the hub cannot enforce signed ops
(ADR-036 Amendment 1 F6, #727) until every writer signs. A plan of 2026-11-25 assumed the browser
would sign with a key of its own per device, bound to the person, with the binding table accepting
several device keys per person. That reverses D1, D5 and C2, and it was weighed against them before
any code was written. **Owner decision, 2026-11-26: D1 stands. The browser signs with the person's
member key.**

- **How.** The browser imports the member key once as a non-extractable WebCrypto Ed25519 key (D2)
  and never exports it again. mae-crypto builds the exact bytes ADR-036 D1 signs
  (`op_canonical_bytes`, the same Rust function the daemon verifies against), the browser signs
  them with `crypto.subtle.sign("Ed25519", key, bytes)`, and sends `author_pubkey`, `author` (its
  fingerprint) and `sig` (ADR-036 Amendment 1 F5). mae-crypto's `sign_op`, which signs with a key
  loaded into the module, stays for a client that holds the seed there; the browser does not, so
  the secret is never extractable.
- **Why not per-device keys now.** No protocol or binding change is needed, and the binding stays
  one `(iss, sub)` to one fingerprint (C2). With several device keys per person, an edit would name
  a device fingerprint while an approval names the person, so ADR-113's author rule could not see
  that the same person edited and approved without a device-to-person mapping it does not have, and
  every new device would need the owner to admit it as a member (D4 forbids the daemon doing so).
  Per-device keys stay Phase E, with its own ADR, as D5 says.
- **A second device** obtains the member key from Phase B's key-blind sealed storage, which the
  pilot does not need. Phase B's block on #176 applies only to end-to-end encrypted KBs, whose
  content a rotation could strand; a plaintext KB (every KB under publication review) is not
  affected.
- **A lost device** is D5's member-key rotation (ADR-040 `Rebind`), one owner act.

Enforcement on the hub is ADR-036 Amendment 1, already built behind
`require_signed_content_ops`; it is turned on once the browser signs and the unsigned-op count
stays at zero.

