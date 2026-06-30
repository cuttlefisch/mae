# Security Review — Identity & Authorization for E2E KB Sharing

> **Status:** design-pass review (companion to `E2E_ENCRYPTION.md`, which covers the content-encryption
> primitives). This document audits the **identity** and **authorization** foundations under MAE's
> multi-user encrypted KB sharing. Every finding is grounded in primary-source prior art and cites
> `file:line`. Findings are tracked as GitHub issues; fixes land per the cadence in §5.

## 1. Scope & verdict

Reviewed: `shared/mcp/src/{identity,tls,auth,keystore}.rs`, `shared/sync/src/{membership,kb,content_ops,
content_crypto}.rs`, `daemon/src/{collab_handler,dialer,ticket,main}.rs`. Adversary model: a network MITM, a
malicious/compromised **key-blind relay/host daemon**, an unauthorized peer attempting to join/write, a
**removed** member, and concurrent-op races.

**Verdict.** The identity scheme is a coherent, prior-art-faithful **SSH-style asymmetric trust model** and
the authorization scheme is a well-constructed **capability + ReBAC hybrid** (UCAN-style attenuation,
Keybase-style sigchains, p2panda strong-removal, an external-anchored signed membership op-log derived
identically by every peer). **No authentication-bypass and no privilege-escalation path was found in the
membership layer.** The defects are concentrated at **(a) the boundary between the new signed op-log and the
legacy `member_roles` store, and (b) enforcement-point coverage** — and the two HIGH bugs (A1, N1) share one
root cause and one fix: *derive role and epoch from the single signed op-log, via one shared fence helper, on
every write path* (the project's own principle #8) — **shipped in #157**. The MEDIUM A2 turned out to be a
*missing feature* (the local-blocklist deny-list is designed but unwired everywhere — not a live bypass) and
is split into its own tracked issue. The remaining items are defense-in-depth + lifecycle (key rotation,
at-rest protection, single-key separation) — none an architectural dead-end.

## 2. Findings summary

| # | Area | Finding | Severity | Status |
|---|---|---|---|---|
| **A1** | Authz | **Epoch source asymmetry** — `kb_access` reads role from the op-log (anchored), but the epoch fence reads epoch from legacy `member_roles` (`epoch_of`→0 for a mesh-admitted member) → valid edits by any non-epoch-0 member are wrongly rejected | **HIGH (bug)** | fix issue |
| **N1-authz** | Authz | **Epoch fence absent on the mesh dialer path** — the ADR-023 fence runs only in hub `kb/node_update`, not `dialer::apply_doc` → incomplete mediation | **HIGH (bug)** | fix issue |
| **A2** | Authz | **Local blocklist mechanism unimplemented** — designed in `membership.rs` but unwired everywhere (`MembershipView::default()` at all derive sites, no storage, no setter). NOT a live bypass (no block can be set); the real fix is a feature (durable deny-list + block/unblock RPC + thread all sites + editor surface) | **MEDIUM** | feature issue |
| **N2-authz** | Authz | **Content-key authority frozen at genesis owner** — a quorum can remove the owner from governance but cannot rekey (only the genesis owner authors honored `wrapped_key` ops) | **MEDIUM** | restrict E2e⇒SingleOwner (3b/#151) or generalize |
| **N3-authz** | Authz | `expires_at` enforced against local wall-clock, not a log-derived logical time → peers can disagree on a time-boxed member | LOW | document/harden |
| **N4-authz** | Authz | Permissive auto-join races a concurrent `set_policy(restrictive)` (TOCTOU) | LOW | harden (re-check in crit. section) |
| **N5-authz** | Authz | Capability attenuation, owner-only governance, transport gate, smuggling defense — **reviewed sound** | OK | — |
| **I1** | Identity | **Single-key reuse** — one Ed25519 seed signs membership+content ops, backs TLS, is the mesh node-id, AND (formerly) was the X25519 wrap key; ran against signing/key-exchange separation guidance | ~~WEAKNESS~~ → **ADDRESSED (#198)** | **ADR-041 shipped** — signing and key-exchange now use distinct keys: each identity derives a dedicated X25519 wrap key (`SHA-512("mae-x25519-wrap/v1" ‖ seed)`) and **publishes** the wrap pubkey in the signed log (a sender cannot derive it from the ed25519 pubkey). The seed still backs TLS + the mesh node-id (those are signing/identity uses, correctly separated from key-exchange) |
| **I2** | Identity | ~~**No key rotation / rebind** — rotating a key = a new principal → lose every KB membership + every content-key wrap~~ | **RESOLVED** | **SHIPPED (ADR-040)** — cross-signed `Rebind` op (old key endorses new, history-preserving): owner + member self-rotation + owner reactive re-wrap (PR2a–PR2c). **Recovery:** pre-registered offline **recovery key** v2 — `collab-register-recovery-key`/`collab-recover-identity` + daemon accept-gate (PR3/PR3b). Owner-mediated re-join remains the path for a *compromised* key (Remove + §D3 + ADR-039 block). No owner co-sign; event-driven (no expiry) |
| **I3** | Identity | **At-rest plaintext** — identity key, PSK keystore, and per-KB content keys are `0600` hex, no passphrase/OS-keychain | WEAKNESS | document; opt-in passphrase/keychain follow-up |
| **I4** | Identity | **Non-unix chmod not enforced + no warning** (Windows exposure) — tension with principle #13 | WEAKNESS | **Partly fixed (#158):** the single secret-file write path (`keystore::write_secure`) now **warns once** on non-unix instead of silently no-opping (the "no warning" gap); native ACL tightening (owner-only DACL) remains a follow-up before a Windows release |
| **I5** | Identity | **Revocation propagation** — `authorized_keys` (transport) ≠ membership op-log ≠ content-key epoch; removal must rotate the content key to exclude an already-joined ex-member | LIMITATION | document; verify rotation-on-removal (3c) |
| **I6** | Identity | TOFU `accept-new` first-contact window (`known_hosts` path); mesh ticket closes it in-band | LIMITATION | document; recommend OOB fingerprint / `strict` |
| **I7** | Identity | Fingerprint (full SHA-256, no truncation); self-signed cert expiry ignored (RFC-7250 raw-public-key style); label is display-only — **all sound** | OK | — |

Content-crypto findings (F1–F9) are in `E2E_ENCRYPTION.md §8`; **A3 = content-review F1** (anchor TOFU — pin
to the OOB-verified owner), folded into 3b.

## 3. Identity & Trust

### Trust model
MAE uses an SSH-style asymmetric identity model, not a CA/WebPKI one. Every participant — editor and daemon
alike — has a long-lived **Ed25519 keypair** ("identity"). The *public key is the identity*; there is no
certificate authority and no name-to-key PKI. Trust is established as in OpenSSH:
- **Daemon side (`authorized_keys`).** A daemon trusts a peer iff its Ed25519 public key is listed
  (`$XDG_DATA_HOME/mae/collab/authorized_keys`). The file is **re-read on every handshake**, so
  `mae-daemon authorize`/`revoke` take effect with no restart. Access control compares the **full 32-byte
  key**; the human label is display-only and is never a trust input.
- **Client side (`known_hosts`, TOFU).** An editor pins a daemon's key on first connection; on a *changed*
  key it **aborts and does not re-pin** (OpenSSH `StrictHostKeyChecking=accept-new` semantics). Policy:
  `strict` / `accept-new` (headless default) / `prompt`.
- **Transport.** TLS 1.3, but the X.509 cert is only a carrier for the Ed25519 key (RFC 7250 "raw public
  key"); custom verifiers check the key (OID `1.3.101.112` + 32-byte length) and intentionally ignore cert
  validity/expiry/CA chains — the key, not the cert, is the identity.
- **Fingerprints.** `SHA256:<base64(sha256(pubkey))>`, full 256-bit, untruncated (modern OpenSSH format).
  Compare out-of-band (`mae-daemon identity`) to verify a TOFU prompt, like a Signal "safety number."
- **Mesh (P2P).** A `mae://join/<…>` ticket carries the owner's node-id (its Ed25519 key); the dialer
  asserts the live connection's identity matches the ticket **before** trusting anything (addresses are
  routing hints only) — the out-of-band binding RFC 7250 / TOFU require, closing the first-contact window
  on the mesh path. KB membership is then derived from the signed op-log anchored on the owner key.

### Trust roots
(1) each peer's Ed25519 identity key; (2) the daemon's `authorized_keys`; (3) the client's `known_hosts`
pins; (4) a KB's owner key (the membership op-log anchor); (5) the per-KB content key, wrapped to each
member's identity key through the membership op-log (the daemon stays key-blind).

### Honest weaknesses
- **One identity key serves several roles** (I1 — key-exchange now split out, #198) — signs membership +
  content ops, backs TLS, and is the mesh node-id. The E2E key-wrap key is a **separate published X25519
  subkey** (ADR-041), no longer the Ed25519→X25519 map. Signing and key-exchange are therefore now on
  **separate** keys (#198), closing the original concern; the remaining reuse — signing membership/content
  ops, TLS, and the mesh node-id — is all *signing/identity* context, domain-separated and standard.
  *(Sources: [filippo.io](https://words.filippo.io/using-ed25519-keys-for-encryption/),
  [EdDSA double-pubkey oracle](https://arxiv.org/pdf/2308.15009),
  [libsodium ed25519→curve25519](https://libsodium.gitbook.io/doc/advanced/ed25519-curve25519).)*
- **No key rotation/rebind** (I2) — rotating = a new principal (lose memberships + wraps). The fix is a
  signed rebind op (old key endorses new key), replayed through the membership op-log so peers transfer
  membership + re-wrap — Matrix **cross-signing** solves exactly this.
  *([MSC1680](https://github.com/matrix-org/matrix-spec-proposals/issues/1680).)*
- **Keys are plaintext at rest** (I3) — identity key, PSK keystore, per-KB content keys (`0600`, no
  passphrase). The local-first posture (assumes full-disk encryption); an opt-in passphrase/keychain wrap is
  the documented next step — the import path already rejects *encrypted* SSH keys, so half the precedent
  exists. *([age #252](https://github.com/FiloSottile/age/discussions/252).)*
- **Windows** (I4) — file permissions are **not** enforced and no warning is emitted on non-unix. Must be
  addressed before any Windows release (principle #13).
- **First-contact TOFU window** (I6) on the `known_hosts` path; mitigate via OOB fingerprint comparison or
  `strict` policy, or use the mesh ticket path (binds the key in-band).
  *([RFC 7250](https://www.rfc-editor.org/rfc/rfc7250.html), [SSH accept-new](https://linux-audit.com/ssh/config/client/option-stricthostkeychecking/).)*
- **PSK auth is coarse** — all peers sharing a PSK collapse to one `psk:<keyid>` principal (no per-peer
  attribution/revoke). Use `key` mode for real multi-user access control.
- **Two-layer revocation** (I5) — removing from `authorized_keys` blocks new connections but does not remove
  a member from a KB's op-log; an E2E ex-member who already holds the content key retains read access until
  the key is **rotated + re-wrapped** to the reduced set (3c).

## 4. Authorization & Membership

### Model
Access to a shared KB is governed by a signed, hash-chained **membership operation log** (ADR-026). Each
mutation — admit, remove, role-change, revoke, governance — is an Ed25519-signed `MembershipOp` whose
validity *every peer derives locally and identically* (`derive_valid_members_governed`), without trusting the
relaying daemon. It composes established prior art: **UCAN**-style capability delegation with attenuation (a
grant never exceeds the granter's role; `can_invite` is a delegable capability); **Keybase**-style sigchains
(ops hash-chained via `prev_hash`); and **p2panda-auth** "strong removal" for deterministic
concurrent-conflict resolution (mutual removals both apply; a removed member's concurrent actions are
transitively invalidated). Roles are hierarchical (Owner ⊇ Editor ⊇ Viewer). Governance is `SingleOwner`
(default) or `Quorum{m}` (removing an admin needs *m* distinct admin co-signatures).

### Trust anchor
Every valid log is rooted at a **genesis owner self-admit** signed by an *external* anchor: the owner's own
key for a hosted KB, or the join-ticket node-id for a KB joined from an untrusted relay (ADR-025). A log
whose genesis is not signed by the registered anchor yields **zero** members — a relay cannot self-attest a
collection into existence.

### Enforcement points
1. **`kb_access`** (the RBAC chokepoint, `collab_handler.rs:898`): resolves the caller's role from its
   cryptographic principal (key fingerprint, never a label; ADR-018 strict binding) via the op-log for
   anchored KBs or `member_roles` for owned KBs, then applies hierarchical RBAC × join policy × transport
   policy (owner-bypass). Every lifecycle handler routes through it at `Manage`.
2. **Epoch fence** (ADR-023): a granted member must author content under their current-epoch yrs client_id;
   pre-grant divergent lineage is rejected. *(See A1/N1 — currently split source + hub-only.)*
3. **Signed content ops** (ADR-036): a content edit's author (from the *signed header*, not the connection)
   must be a current Editor+ member at the op's epoch.
4. **Collection-smuggling defense**: raw writes to `kbc:` are owner-only — membership/policy cannot be
   mutated outside the gated handlers.

### Honest gaps (and fix status)
- **Split epoch source (A1, HIGH).** Role from op-log, epoch from the legacy `member_roles` (empty for a
  mesh member) → non-epoch-0 members wrongly fenced. Fix: derive the epoch from the same op-log
  (one source of authority — a basic ReBAC tenet,
  [Zanzibar](https://www.usenix.org/system/files/atc19-pang.pdf)).
- **Fence missing on the mesh path (N1, HIGH).** The fence runs only on hub `kb/node_update`, not
  `dialer::apply_doc`. Fix: one shared fence helper on both write paths (complete mediation).
- **Local blocklist mechanism unimplemented (A2, MEDIUM — NOT a live bypass).** Re-scoped on closer reading:
  the `MembershipView.blocklist` self-protection deny-list is *designed* in `membership.rs` (step 7 of
  `derive_valid_members_governed`) but **entirely unwired** — every daemon derive site (`kb_access`,
  `verify_content_op`, `verify_relayed_content_op`, the strong-removal path) passes `MembershipView::default()`
  (empty), there is no DocStore storage for blocked principals, and no RPC/command to set one. Because no block
  can be set, the empty default is *consistent* everywhere — there is no admit-a-blocked-principal hole today.
  The real fix is therefore a **feature**, not a one-line wiring change: local durable per-KB deny-list +
  operator-local block/unblock RPC + thread the loaded view into *every* enforcement point (deny-lists must
  apply at all of them — [SPKI/SDSI RFC 2692](https://www.rfc-editor.org/rfc/rfc2692),
  [UCAN revocation](https://ucan.xyz/revocation/)) + editor command/Scheme/MCP surface for human↔AI parity +
  adversarial tests. Tracked as a dedicated issue (see §5). (Global op-log removal — `Revoke`/`Remove` — already
  works and propagates; A2 is the *local* self-protection override for principals you cannot get globally
  removed.)
- **Content-key authority frozen at genesis owner (N2, MEDIUM).** A quorum can remove the owner from
  governance but cannot rekey. Unlike MLS, removal does not force re-encryption
  ([RFC 9420 §12](https://www.rfc-editor.org/rfc/rfc9420.html)). v1 fix: restrict E2e to `SingleOwner`;
  later: let current owners author rotation ops under the active governance.
- **Local-clock expiry (N3) + permissive-join/policy-flip TOCTOU (N4)** — LOW hardening: derive a logical
  "now" from causally-prior ops; re-check policy inside the membership-write critical section.

Capability attenuation, owner-only governance, the strong-removal/quorum resolver, transport gating, and
smuggling defenses were reviewed and found **sound — no privilege-escalation path identified**.

## 5. Fix priority & cadence

1. **The unified fence (A1 + N1)** — DONE (#157): one `enforce_epoch_fence` helper derives role + **epoch**
   from the signed op-log and is called on **both** the hub `kb/node_update` and the mesh dialer relay paths
   (complete mediation). Closes both HIGH bugs. (3b's dual-write was the interim hub mitigation for A1.)
   **A2 split out** as its own MEDIUM feature issue (local-blocklist mechanism is unimplemented, not a live
   bypass — see §4): durable deny-list + block/unblock RPC + thread every derive site + editor surface +
   adversarial tests. Picks up after the core encryption story (3c/3d) closes.
2. **N2 / E2e governance** — restrict E2e KBs to `SingleOwner` for v1 (folds into 3b/#151, F4); generalize
   later.
3. **I-series identity hardening** — document I1/I3/I5/I6 now (this doc); I4 (Windows) before any Windows
   release; I2 (key rotation/rebind) + I1 (HKDF subkeys) as ADR follow-ups before E2E GA.
4. **Implementation-pass review** re-runs after the fixes + 3b/3c land, gating 3d's confidentiality claim.

Every fix above is security-relevant and ships with an **adversarial test** exercising the failure mode
(malicious relay, removed member, stale/forged epoch, blocked principal) — per the testing-rigor principle.

## 6. Maintainability & wild-usability review (v0.15 release gate)

A three-lens adversarial review of whether the E2E story holds up **in the wild** over time — not just
demo-green. Lenses: **key lifecycle & loss**, **scaling & operational longevity**, **confidentiality
honesty & failure modes**. Verdict: **no confidentiality-bypass or correctness BLOCKER** — seal/share/
genesis/DH/open all fail *closed* (Lens-3 F-L3-7) and convergence/fencing/at-rest-scrub are well-tested. The
release-relevant gaps are (a) one **key-loss UX blocker**, (b) a **scaling wall** that is honestly documented
but unmitigated, and (c) **doc/code drift + caveats invisible at the point of action**.

### 6.1 Key lifecycle & loss (Lens 1)

| ID | Finding | Severity | Disposition |
|----|---------|----------|-------------|
| KL1 | Identity-seed loss is **silent, total, unrecoverable** — no backup prompt at key creation, no onboarding doc. The Ed25519 seed is the single root: the X25519 wrap key is *derived* from it, and the content-key file is a recoverable cache (re-derivable from the self-wrap in the op-log). So the seed is the sole SPOF. | **BLOCKER (UX)** | Release fix: backup prompt at key creation + "back up your identity" in the user guide. → issue |
| KL2 | ~~ADR-040 rotation/rebind is design-only (0 code).~~ | **RESOLVED** | **SHIPPED** (this release): cross-signed `Rebind` — owner + member self-rotation, owner reactive re-wrap (PR2a–PR2c, #210/#216/#218/#221). |
| KL3 | ~~Solo-owner key loss has **no recovery actor** (no second owner / recovery key) → KB unrecoverable.~~ | **RESOLVED** | **SHIPPED** (this release): pre-registered offline **recovery key** v2 — `collab-register-recovery-key` + `collab-recover-identity`, daemon accept-gate (PR3 #222 + PR3b #223). Register *before* loss; latest-registration-wins revokes a leaked key. |
| KL4 | At-rest keys are `0600` plaintext (no passphrase/keychain) — local theft reads everything (I3). | documented-limitation | → issue (I3, post-release). |
| KL5 | No backup/restore tooling or documented recipe for the collab dir (keys + KB). | SHOULD-FIX | → issue + DAEMON_ADMIN.md recipe (Phase 5). |

### 6.2 Scaling & operational longevity (Lens 2)

All four scaling findings roll up to one fix — **ADR-028 op-set / membership-log compaction + checkpointing**
(already tracked as #156 F8). The codebase is honest about it (`docs/E2E_ENCRYPTION.md` F8) but it is
currently *unmitigated*. Cost tracks **total-edits-ever**, not live-content-size.

| ID | Finding | Severity |
|----|---------|----------|
| SC1 | Op-set is grow-only per node; `merge` re-encodes full state on every inbound op ⇒ **O(M²)** over a node's life; a 1-edit change costs ~150–300 B forever (100×+ blowup on hot nodes). | SHOULD-FIX |
| SC2 | Membership op-log is grow-only; each removal appends O(N) permanent re-key ops; every `derive_*` replays the whole log. | SHOULD-FIX |
| SC3 | `derive_valid_members` is super-linear (~O(M³)–O(M⁴)) and re-run on **every content op** the daemon fences (no memoization, re-parses `oplog_ops()` each call). Per-write hot path. | SHOULD-FIX (blocker-adjacent at high churn) |
| SC4 | A cold joiner pulls + AEAD-decrypts the **entire op history** of every node (no "since op X"). | SHOULD-FIX |
| SC5 | Daemon `PRAGMA synchronous=NORMAL` under WAL is **not power-loss-safe** to last-ack (only last-checkpoint); ~60 s/500-update window. Heals from peers if any exist; solo daemon = real loss (#77). | SHOULD-FIX |
| SC6 | No metric/command surfaces op-set/log size or growth; naive `cp` of the live SQLite DB (with `-wal`/`-shm` sidecars + secure_delete churn) captures inconsistent state — no safe backup recipe. | SHOULD-FIX (ops) |

**Mitigation for release:** memoize the derived member-set per collection-doc version (SC3 — content edits
never change membership, so this is a safe, high-leverage cache) is a candidate quick win; the rest are
tracked for v0.16 compaction. Surface op-set/log growth in `collab_doctor` + document the `VACUUM INTO`
backup recipe and the solo-daemon durability caveat (Phase 5).

### 6.3 Confidentiality honesty & failure modes (Lens 3)

The *primitives* are sound; the exposure is **honesty reaching the user** and **doc/code drift**.

| ID | Finding | Severity | Disposition |
|----|---------|----------|-------------|
| CF1 | `kb-set-encryption` oversells: every FS/PCS/metadata caveat lives only in a doc the user never sees. Shipping an "E2E" label whose caveats are invisible at enable time. | **SHOULD-FIX (product)** | Emit a one-time advisory on enable + in the `*KB Sharing*` buffer + the primitive doc (Phase 4). |
| CF2 | Docs describe the **superseded** Ed25519→X25519 birational-map wrap; code uses the dedicated **published** X25519 wrap key (ADR-041/I1). Docs misdescribe live crypto *and* undersell I1. | ✓ RECONCILED | E2E §3 rewritten to the published-wrap-key model; I1 downgraded to ADDRESSED (#198). |
| CF3 | E2E doc contradicts itself on the F5 manifest-title leak (§7.4 "cleartext" vs §9 "shipped"). Node **ids** are cleartext (true); node **titles** are blanked (shipped #156). | ✓ RECONCILED | §7.4 (ids cleartext, titles blanked) + §9 (enable-time scrub shipped) now agree. |
| CF4 | "Withholding is detectable" overclaims: only the **membership** log is hash-chained. Content ops are an unordered set with **no completeness proof** — a withheld content op is indistinguishable from "not authored yet." | SHOULD-FIX-DOCS | Scope the claim; true fix = op-set completeness/key-id (F7/#176). |
| CF5 | Silent-skip decrypt: wrong key / rotated key / tampered op / not-yet-received all surface identically as "content absent." Fail-*safe* for secrecy, fail-*silent* for the user. | documented-limitation | Surface an "N ops undecryptable — wrong/rotated key?" indicator (→ issue). |
| CF6 | #176 re-sync loss: one key per KB, no key-history → a from-scratch re-sync **after** a rotation permanently drops pre-rotation content, even for a legitimate current member. Undocumented user-facing failure. | documented-limitation (#176) | Document in E2E §7; fix = `HashMap<String, Vec<ContentKey>>` + op-set key-id (F7). |
| CF7 | Metadata surface understated at the *user* surface: the host learns the **full social graph, per-edit author/timing/size, node-ids** — "E2E" connotes Signal-like minimization it does not provide. | documented-limitation | Promote the quantified "what the host learns" to the enable surface (CF1, Phase 4). |

### 6.4 Release dispositions

- **BLOCKER (must fix before v0.15):** KL1 (identity-seed backup prompt + doc), CF1 (E2E-enable advisory at
  point of action). Both are small, high-leverage UX fixes.
- **In this release by plan:** KL2/KL3 (Phase 2 identity arc), CF2/CF3 (Phase 5 doc reconciliation),
  CF4/CF7 wording (Phase 5), KL5 backup recipe (Phase 5).
- **Tracked, not release-blocking (v0.16):** SC1–SC6 (ADR-028 compaction + durability + ops observability,
  with SC3 memoization a candidate quick win), KL4 (I3 at-rest passphrase), CF5/CF6 (key-history + undecrypt
  indicator, #176/F7).

No finding reopens a confidentiality bypass. The system is **correct because it fails closed**; the wild-
usability work is making that correctness *survivable* (key loss), *scalable* (compaction), and *honest at
the point of use* (caveats at enable, not buried in a doc).
