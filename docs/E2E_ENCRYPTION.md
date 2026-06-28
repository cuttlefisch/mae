# End-to-End Content Encryption (ADR-037)

> **Status:** v1 design + in-progress implementation (issue #131). This document is the security
> design-of-record and the honest statement of what E2E encryption in MAE does — and does **not** —
> protect. It incorporates a prior-art-grounded cryptographic review (issue #155).

## 1. What this protects

A KB can be marked **`Encryption::E2e`**. Its node *content* is then encrypted at the **CRDT-payload layer**
under a per-KB symmetric **content key** held only by members. A daemon/hub/relay that hosts or forwards the
KB — but is **not a member** — carries only ciphertext and **cannot read the content**. The protection is
**transport-agnostic** (it rides the CRDT, identical over the mTLS hub and the P2P/iroh mesh) and holds for
**N members**.

**The property delivered is *confidentiality of node content from non-members (relays/hosts/the hub)*.** It
is *not* anonymity, not metadata privacy, and not protection from a member you admitted. See §7.

## 2. Threat model

- **Adversary:** a key-blind relay/host/hub (incl. one that terminates the mesh QUIC session), and a
  **removed** member. The adversary may reorder, withhold, or duplicate ops it relays.
- **Trusted:** current members (they hold the content key by definition); the owner's signing identity; the
  out-of-band channel that delivers the owner's identity (the mTLS trust store / the `mae://join` ticket
  node-id).
- **Goal:** the adversary learns no node *content* plaintext. (Membership, authorship, sizes, and timing are
  **not** hidden — §7.)

## 3. Cryptographic primitives

All in `shared/sync/src/content_crypto.rs` (reviewed: sound, no must-fix break inside the primitives).

- **Content AEAD:** XChaCha20-Poly1305, 256-bit key, 128-bit tag, **fresh random 24-byte nonce per op**
  (the X-variant exists precisely for random nonces; collision risk is negligible far beyond op volumes).
- **Key wrap (sealed box):** to wrap the content key to a member, generate an **ephemeral X25519** keypair,
  ECDH with the member's identity key, derive a one-time wrap key, AEAD-seal the content key. Output =
  `ephemeral_pub ‖ sealed`. This is a NaCl/libsodium-style sealed box, slightly **stronger** (fresh random
  nonce + fresh ephemeral, used once).
- **KDF:** `SHA-256("mae-content-key-wrap/v1" ‖ DH ‖ ephemeral_pub ‖ recipient_pub)` — binds both public
  keys + a versioned domain-separation tag (defeats unknown-key-share / cross-protocol confusion).
- **Identity reuse:** the member's **Ed25519** identity is converted to X25519 (the standard libsodium
  birational map) for the wrap. Dual-use is within the published joint-security result **because** the wrap
  KDF is domain-separated from the signing context — those tags are load-bearing and must never be removed.
- **Encrypt-then-sign:** content ops are encrypted, then the ciphertext-bearing op is signed (ADR-036). A
  receiver **verifies before decrypting**; a key-blind relay verifies integrity without the key.

## 4. Key lifecycle

1. **Enable (owner):** `(kb-set-encryption KB "e2e")` — the owner generates a `ContentKey`, persists it
   `0600` (`mae_mcp::content_key_store`, never on the daemon), **self-wraps** it (so the owner can decrypt),
   and authors a **signed** owner op asserting E2e + the genesis self-admit. The mode is recorded in the
   **signed membership op-log** (not an unsigned flag — see Finding F2), so it cannot be silently downgraded.
2. **Wrap-on-admit:** when the owner admits a member, it wraps the content key to that member's identity key
   and carries the `wrapped_key` in the member's **signed `Admit` op**. The member's pubkey is obtained from
   their join request (`PendingRequest`).
3. **Derive (member):** a member recovers its key from the signed op-log
   (`membership::derive_content_key`): the genesis self-admit is the trust anchor, and the **latest
   owner-authored** wrapped op targeting the member (in deterministic causal order) wins.
4. **Rotate-on-remove (§D3):** removing a member generates a **fresh** key re-wrapped only to the *remaining*
   members. The removed member has no new wrapped op → keeps only the old key → **cannot read content
   authored after the rotation**.

The daemon stores + relays all of this **key-blind** via the `kb/collection_op` RPC (it stores opaque
owner-signed bytes; it never holds the content key).

## 5. Distribution & trust

- Keys travel **inside the signed, hash-chained membership op-log** (ADR-026), so a non-owner cannot inject,
  forge, or substitute a key (`verify_signed` binds `author_pubkey ↔ fingerprint ↔ signature`; only the
  anchored owner's wrapped ops count).
- **Causal order is relay-independent** (topological by `prev_hash`, `chain_hash` tiebreak) — a relay
  **cannot reorder** to change which key a member derives; it can only *withhold* (a detectable
  liveness/DoS issue, not a confidentiality break).
- **Trust anchor:** the genesis owner self-admit. The anchor pubkey **must be pinned to the
  out-of-band-verified owner identity** (the mTLS-bound `COLL_OWNER_KEY` on the hub; the ticket node-id on
  the mesh) — see Finding F1.

## 6. Prior-art positioning

MAE v1 = a **single per-KB symmetric content key, sealed-box-wrapped to each member, distributed via a
signed membership CRDT log, rotated on removal**. This is the **Jazz/cojson** model almost feature-for-feature,
and the efficient cousin of **Signal Sender Keys** (O(N) re-wrap on removal vs Sender Keys' O(N²)). It
deliberately omits forward secrecy / post-compromise security — the documented trade-off every CRDT-native
E2E system makes (CRDTs need the full causal history).

| Scheme | Key model | Rekey on remove | FS | PCS | Local-first fit |
|---|---|---|---|---|---|
| **MAE v1 (ADR-037)** | one per-KB symmetric key, sealed-box-wrapped per member | **O(N)** re-wrap | ✗ | ✗ | **High** (key-blind op-set relay, no central server) |
| MLS / TreeKEM (RFC 9420) | ratchet tree, per-epoch secret | **O(log N)** | ✓ | ✓ | Medium (needs a linear epoch order) |
| Signal Sender Keys | per-sender key over pairwise channels | **O(N²)** | partial | ✗ | Low (online pairwise channels) |
| Matrix Megolm | per-sender ratcheted session | O(N) | partial (history-sharing undermines) | ✗ | Low/Medium (homeserver-centric) |
| Keyhive / BeeKEM (Ink & Switch) | CGKA DH ratchet tree | **O(log N)** | ✓ | ✓ | **Highest** (CRDT-native, pre-alpha) |
| Jazz / cojson | per-group read key sealed to members | O(N) | ✗ | ✗ | High (key-blind cloud) |

**Verdict:** a sound, defensible v1 that sits squarely in the shipping CRDT-native-E2E cluster — **provided
the docs are blunt about what it does not protect (§7)**. The named evolution for FS/PCS + O(log N) is
**BeeKEM** (the local-first-correct TreeKEM; ADR-037 §D4), kept flagged as research-maturity; MLS RFC 9420 is
the audited fallback reference.

Sources: [RFC 9420 (MLS)](https://www.rfc-editor.org/rfc/rfc9420.html) ·
[Signal Sender Keys analysis (Balbás et al.)](https://arxiv.org/pdf/2301.07045) ·
[Matrix/Megolm breaks (Albrecht et al.)](https://nebuchadnezzar-megolm.github.io/) ·
[Keyhive/BeeKEM (Ink & Switch)](https://www.inkandswitch.com/keyhive/notebook/02/) ·
[Jazz encryption](https://jazz.tools/docs/react/reference/encryption) ·
[secsync (key-blind Yjs)](https://github.com/serenity-kit/secsync).

## 7. What we do NOT protect (read this)

1. **No forward secrecy.** A leaked or retained content key decrypts all content under that key until the
   next rotation. Rotation denies *future* content only; already-delivered plaintext stays readable to
   whoever had it.
2. **No post-compromise security.** A leaked identity key exposes every content key ever wrapped to it (the
   wraps sit in the immutable op-log); the group does not self-heal until the owner rotates.
3. **O(N) rekey on removal** — not O(log N). Fine for KB-scale teams; not for thousands of churning members.
4. **Metadata is exposed to the relay/host/hub by design** (the membership log must stay peer-verifiable in
   cleartext): per-op **edit sizes** (±40 B — XChaCha is a stream cipher, ciphertext length = plaintext
   length), **edit timing**, **author attribution** (the ADR-036 signed header is cleartext, and the
   epoch-derived client-id is precomputable), and the **full membership / social graph** (fingerprints,
   roles, who-admitted-whom, pending requests). **Node ids and titles are currently cleartext in the
   collection manifest** even for an E2e KB (Finding F5). Traffic-analysis resistance is out of scope.
5. **A malicious *member* is not constrained by encryption** — members hold the key. Confidentiality is from
   non-members/relays/hosts; *membership control* (ADR-026) is the only defense against an admitted peer.
6. **Operational guardrail (load-bearing):** the daemon must **never** log/persist plaintext for an E2e KB
   and must never require the content key to relay; a key-blind relay can only set-union + relay the signed
   encrypted op-set (it cannot state-vector-diff/merge ciphertext). Encrypt-then-sign (verify-before-decrypt)
   ordering is load-bearing.
7. **Unbounded growth:** the op-set + membership log are grow-only (no compaction yet — ADR-028); long-lived
   heavily-edited KBs accumulate all historical ciphertext.
8. **Enabling E2e is forward-only — it does NOT retroactively un-send already-transmitted plaintext (#171).**
   The natural flow — share a KB with content, *then* `kb-set-encryption e2e` — transmits that content as
   plaintext while the KB is still unencrypted, so the daemon already holds plaintext `kb:{node}` docs. Enabling
   now **re-seals** every already-shared node (#171, shipped): the owner seals each node's current content as
   op 0 of a sealed op-set that continues the node's lineage, so a member who *joins after enable* reads the
   sealed content (and all subsequent edits seal) — the join-decrypt path works. **What enabling cannot do is
   retract the plaintext copy the daemon received before enable:** that pre-enable `kb:{node}` snapshot remains
   on the key-blind daemon at rest. So confidentiality for shared-then-encrypted content is *forward* (future
   reads + edits are sealed), not *retroactive*. Workaround for full confidentiality: enable E2e on an empty KB
   **before** adding content. Purging the residual pre-enable plaintext base (needs a fresh on-daemon node
   lineage) is the remaining work tracked under #171.

> **Fail-closed seal (the operational guardrail, §7.6, is now enforced in code — #168/#170).** Both write paths
> refuse to emit plaintext on an E2e KB: `kb/node_update` (`build_kb_node_update_request` returns *refuse* and
> requeues when it holds no content key or sealing fails — #168) and the `kb/share` / reconnect-re-share path
> (`select_share_node_states` ships the sealed op-set or skips the node, never the plaintext snapshot — #170).
> The E2e decision reads the **signed** `derive_encryption` (anchor-pinned), not the relay-flippable unsigned
> flag, so a downgrade (F2) cannot trick the editor into plaintext. This closes the F2 downgrade *and* the
> deletion-fence gap for E2e KBs (a sealed delete rides a client-id-stamped outer op the ADR-023 fence catches;
> see #167).

## 8. Security review findings (issue #155)

The core primitives are sound (no must-fix break inside the crypto). Actionable findings, with disposition:

| # | Finding | Severity | Disposition |
|---|---|---|---|
| F1 | **Anchor TOFU** — `derive_kb_content_key` re-derives the anchor from the relay-supplied collection instead of pinning to the OOB-verified owner identity (mTLS `COLL_OWNER_KEY` / ticket node-id) → genesis-substitution / key-injection by a malicious relay | WEAKNESS→BUG | **Fix in 3b** (cross-check genesis author == authenticated owner) + mesh node-id pinning follow-up |
| F2 | **Encryption mode is an unsigned collection-map flag** → a relay can flip `e2e→none` → victim emits plaintext (downgrade attack) | WEAKNESS→BUG | **Fix in 3b**: assert E2e in the **signed** owner op-log; seal path **fail-closed** (never author plaintext once the signed log asserts E2e) |
| F3 | Owner-side key-gen / wrap-on-admit / **rotate-on-remove not wired** (design + tests only) | BUG (gap) | **This is 3b (#151) + 3c (#152)** — the work in progress |
| F4 | Content-key authority **undefined under `Governance::Quorum`** (frozen key if the genesis owner is quorum-removed) | WEAKNESS | **Fix in 3b**: restrict E2e to `SingleOwner` (reject `SetGovernance(Quorum)` on an E2e KB) for v1 |
| F5 | **Node titles cleartext** in the collection manifest even on an E2e KB (redundant — titles are also encrypted inside the node op-set) | WEAKNESS | Document now (§7.4); omit/encrypt manifest title — hardening follow-up |
| F6 | No **all-zero / low-order DH** rejection in wrap/unwrap → attacker-chosen ephemeral could force a known wrap key (caught only because wraps are signature-covered) | WEAKNESS | **Fix in 3b** (cheap: reject all-zero shared secret) |
| F7 | AEAD carries **no AAD** binding ciphertext to op/kb/node/author/epoch; signed header carries **no content-key-id** | WEAKNESS (defense-in-depth) | Hardening follow-up (mitigated today by encrypt-then-sign + wraps-in-signed-ops) |
| F8 | Op-set + membership-log **unbounded growth**; `derive_*` is O(ops) per update | WEAKNESS (scale) | Document (§7.7); compaction + incremental derivation = ADR-028 follow-up |
| F9 | `ContentKey` zeroization best-effort; intermediates not zeroized; `Clone` present | LIMITATION | Hardening follow-up (`zeroize` crate) |

**Two reviews:** this is the **design pass** (before building 3b). An **implementation pass** re-runs after
3b/3c land — verifying the code matches this design — before 3d's docker confidentiality gate is declared
meaningful.

## 9. Roadmap

- **3b (#151):** enable surface (signed mode), owner key gen/persist/self-wrap, wrap-on-admit. Folds in F1,
  F2, F4, F6.
- **3c (#152):** removal rotation (§D3) — the O(N) re-wrap to remaining members.
- **3d (#153):** docker e2e encrypted-lifecycle + required CI gate (the confidentiality oracle).
- **Hardening follow-ups:** F5 (manifest titles), F7 (AAD + content-key-id), F8 (compaction), F9 (zeroize),
  mesh anchor node-id pinning.
- **FS/PCS evolution (deferred, ADR-037 §D4):** BeeKEM CGKA ratchet (O(log N) + forward/post-compromise
  secrecy), kept flagged research-maturity.
