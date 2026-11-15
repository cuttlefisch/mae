# End-to-End Content Encryption (ADR-037)

> **Status:** v1 shipped (issue #131, closed — §9 Roadmap below for the phase-by-phase
> breakdown). This document is the security design-of-record and the honest statement of
> what E2E encryption in MAE does — and does **not** — protect. It incorporates a
> prior-art-grounded cryptographic review (issue #155).

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

All in `shared/crypto/src/content_crypto.rs`, re-exported as `mae_sync::content_crypto` (reviewed: sound, no
must-fix break inside the primitives). It lives in the no-I/O `mae-crypto` crate so the browser runs the same
code (§10).

- **Content AEAD:** XChaCha20-Poly1305, 256-bit key, 128-bit tag, **fresh random 24-byte nonce per op**
  (the X-variant exists precisely for random nonces; collision risk is negligible far beyond op volumes).
- **Key wrap (sealed box):** to wrap the content key to a member, generate an **ephemeral X25519** keypair,
  ECDH with the member's identity key, derive a one-time wrap key, AEAD-seal the content key. Output =
  `ephemeral_pub ‖ sealed`. This is a NaCl/libsodium-style sealed box, slightly **stronger** (fresh random
  nonce + fresh ephemeral, used once).
- **KDF:** `SHA-256("mae-content-key-wrap/v1" ‖ DH ‖ ephemeral_pub ‖ recipient_pub)` — binds both public
  keys + a versioned domain-separation tag (defeats unknown-key-share / cross-protocol confusion).
- **Separate published wrap key (ADR-041 / I1, #198):** signing and key-exchange use **distinct** keys.
  Each member derives a dedicated X25519 **wrap key** from its identity seed —
  `SHA-512("mae-x25519-wrap/v1" ‖ seed)[..32]` — and **publishes** the wrap *public* key in the signed
  membership op-log (a joiner ships it on join; an admit/rebind records it). The owner seals the content key
  to that *published* wrap key, **not** to a key derived from the Ed25519 identity. (The earlier design reused
  the Ed25519→X25519 birational map; I1 replaced it so a signing-key compromise does not imply a
  key-exchange-key compromise.) The wrap KDF stays domain-separated from the signing context — those tags are
  load-bearing and must never be removed.
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
   roles, who-admitted-whom, pending requests). **Node ids are cleartext in the collection manifest** even
   for an E2e KB; node **titles** are blanked there (forward + enable-time scrub, Finding F5 / #156).
   Traffic-analysis resistance is out of scope.
5. **A malicious *member* is not constrained by encryption** — members hold the key. Confidentiality is from
   non-members/relays/hosts; *membership control* (ADR-026) is the only defense against an admitted peer.
6. **Operational guardrail (load-bearing):** the daemon must **never** log/persist plaintext for an E2e KB
   and must never require the content key to relay; a key-blind relay can only set-union + relay the signed
   encrypted op-set (it cannot state-vector-diff/merge ciphertext). Encrypt-then-sign (verify-before-decrypt)
   ordering is load-bearing.
7. **Unbounded growth:** the op-set + membership log are grow-only (no compaction yet — ADR-028); long-lived
   heavily-edited KBs accumulate all historical ciphertext.
8. **Enabling E2e re-seals AND purges the pre-enable plaintext base (#171, shipped — reseal-as-replace).**
   The natural flow — share a KB with content, *then* `kb-set-encryption e2e` — transmits that content as
   plaintext while the KB is still unencrypted, so the daemon momentarily holds plaintext `kb:{node}` docs.
   Enabling **re-seals** every already-shared node: the owner seals each node's current content as op 0 of a
   FRESH sealed op-set (clock 0, self-contained) and ships it `reseal:true`, so the daemon `share_doc`-**REPLACES**
   the node doc — owner-gated (Manage), epoch-fence-skipped — **deleting the pre-enable plaintext snapshot+WAL**
   rather than stacking the op-set on top of it. Combined with `PRAGMA secure_delete=ON` + a TRUNCATE checkpoint
   on delete, the pre-enable plaintext bytes are **scrubbed from the DB file at rest**, not just unlinked. A
   member who *joins after enable* reads only ciphertext (the join-decrypt path), and the key-blind daemon
   retains no recoverable plaintext. Guarded by the encrypted e2e gate (a pre-enable `PRECANARY` must be ABSENT
   from the daemon store after enable — teeth-checked: it LEAKS on the old merge path) + the storage unit test
   `delete_document_scrubs_plaintext_from_the_db_file_at_rest`. *Residual limitation:* a member who legitimately
   pulled the plaintext **before** enable keeps their own local copy (like §D3 history) — enable protects the
   relay + future joiners, not peers who already read in the clear; for that, enable E2e **before** sharing.

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
| F3 | Owner-side key-gen / wrap-on-admit / **rotate-on-remove not wired** (design + tests only) | BUG (gap) | **FIXED — 3b (#151) + 3c (#152), both shipped** (§9 Roadmap) |
| F4 | Content-key authority **undefined under `Governance::Quorum`** (frozen key if the genesis owner is quorum-removed) | WEAKNESS | **Fix in 3b**: restrict E2e to `SingleOwner` (reject `SetGovernance(Quorum)` on an E2e KB) for v1 |
| F5 | **Node titles cleartext** in the collection manifest even on an E2e KB (redundant — titles are also encrypted inside the node op-set) | WEAKNESS | **FIXED at the CRDT-state level (#156):** (a) *forward* — the editor blanks the manifest title for an E2e KB (downgrade-resistant `kb_collection_is_e2e`), so a node added on an E2e KB never writes a cleartext title; (b) *enable-time* — enabling E2e on an existing KB ships a `blank_node_titles_delta` (alongside the genesis + #171 node reseal) that scrubs the titles already in the manifest. The blanked title does **not** survive in the `kbc:` doc's compacted **snapshot** state (verified at-rest: `blank_node_titles_delta_purges_old_title_from_state_bytes`). (c) *at-rest WAL* — the enable-time blank is shipped with `scrub:true`, so the daemon **force-compacts** the `kbc:` doc after applying: re-snapshot the title-blanked state, trim + TRUNCATE-checkpoint the WAL, `secure_delete` zeroing the freed pages — so the superseded cleartext title cannot linger in the `kbc:` WAL at rest either (verified: `compact_scrubs_superseded_wal_content_from_the_db_file_at_rest`). The key-blind daemon keeps only `node_id`s, at rest and in memory |
| F6 | No **all-zero / low-order DH** rejection in wrap/unwrap → attacker-chosen ephemeral could force a known wrap key (caught only because wraps are signature-covered) | WEAKNESS | **Fix in 3b** (cheap: reject all-zero shared secret) |
| F7 | AEAD carries **no AAD** binding ciphertext to op/kb/node/author/epoch; signed header carries **no content-key-id** | WEAKNESS (defense-in-depth) | **Parked (design note on #156):** in the op-set model the map is `{op_id: ciphertext}` with no per-op metadata, so op-set AAD can only bind `kb_id‖node_id` (already signature-bound) and a content-key-id needs an op-set schema change or header plumbing — fold into the **#176 rotation key-history** design (where the key-id earns its keep) rather than a standalone PR. Mitigated today by encrypt-then-sign + wraps-in-signed-ops |
| F8 | Op-set + membership-log **unbounded growth**; `derive_*` is O(ops) per update | WEAKNESS (scale) | Document (§7.7); compaction + incremental derivation = ADR-028 follow-up |
| F9 | `ContentKey` zeroization best-effort; intermediates not zeroized; `Clone` present | LIMITATION | **FIXED (#156, PR #190):** `ContentKey` derives `Zeroize + ZeroizeOnDrop`; the unwrap key bytes + the X25519 scalar/SHA-512 expansion are wiped |

**Two reviews:** this is the **design pass** (before building 3b). An **implementation pass** re-runs after
3b/3c land — verifying the code matches this design — before 3d's docker confidentiality gate is declared
meaningful.

## 9. Roadmap

- **3b (#151):** enable surface (signed mode), owner key gen/persist/self-wrap, wrap-on-admit. Folds in F1,
  F2, F4, F6.
- **3c (#152) — SHIPPED:** removal rotation (§D3) — the O(N) re-wrap to remaining members, authored on the
  network task against its own `kb_collections` replica (the kb/approve #179 fail-closed base rule).
- **3d (#153) — SHIPPED:** docker e2e encrypted-lifecycle + required CI gate (the confidentiality oracle).
  The §D3 path has its own e2e gate (`MAE_E2E_REMOVAL=1`): after a member reads CANARY1, the owner removes
  them + rotates + authors CANARY2 — the removed-but-still-subscribed member RECEIVES the post-rotation
  ciphertext over its live subscription (the relay does not re-filter broadcasts by membership) but, stranded
  on the old key, CANNOT decrypt it, while RETAINING the CANARY1 history; the relay stays key-blind throughout.
- **Hardening follow-ups:** F9 (zeroize) **shipped (#190)**; F5 (manifest titles) **shipped (#156)** — both
  the *forward* case (a node added on an E2e KB never writes a cleartext title) and the *enable-time*
  retroactive scrub (blanking titles already in the manifest, incl. the at-rest WAL); F7 (AAD + content-key-id)
  **parked** → folded into the
  #176 rotation key-history design; F8 (compaction) + mesh anchor node-id pinning still open.
- **FS/PCS evolution (deferred, ADR-037 §D4):** BeeKEM CGKA ratchet (O(log N) + forward/post-compromise
  secrecy), kept flagged research-maturity.

## 10. Browser key material: the mae-crypto WASM module

A browser client generates and uses MAE member keys with MAE's own Rust code, never a reimplementation:
`mae-crypto` (`shared/crypto`, pure, no I/O) compiled to `wasm32-unknown-unknown` behind the facade
`mae-crypto-wasm` (`shared/crypto-wasm`). The daemon, the editor and the browser run one implementation of:
the member key's public forms (`member.rs`, cross-checked against `mae_mcp::identity` by
`shared/mcp/tests/member_key_parity.rs`), the identity-binding challenge rules (`identity_challenge.rs`;
`mae_mcp::identity_challenge` wraps it), the key backup (`backup.rs`) and §3's content crypto.

**Build and pin.** `make crypto-wasm` prints the artifact's size and sha256; `make crypto-wasm-repro` builds
twice and requires identical bytes; `make crypto-wasm-test` runs the Node checks (size budget, ABI, a
native-vs-WASM differential over every deterministic response, interop both ways, entropy failure,
leak and memory residue). CI's "mae-crypto WASM" job runs all three on a pinned rustc and records size and
sha256. The contract version is `mae-crypto-wasm`'s own (`version` below), not MAE's; a consumer pins a
`mae-crypto-vX.Y.Z` tag. Size budget: `SIZE_BUDGET` in `shared/crypto-wasm/js/test.mjs`. First set at
400,000 bytes, ~15% over the first measurement (346,994 bytes, rustc 1.98.0, `--profile wasm`, 2026-11-15;
most of it Argon2/BLAKE2b, the two curve crates and serde_json). Raised to 550,000 in the first review
round, ~16% over 474,145 bytes: moving NFC into Rust (so a native caller derives the same key) added
unicode-normalization's tables, about 127 KB. Raising it again is recorded here.

**Loader.** `js/mae-crypto.mjs` (types: `js/mae-crypto.d.ts`): `instantiate(bytes)` is synchronous and
backs the module's one import, `mae_crypto_host.random_fill(ptr, len) -> u32` (0 = filled), with
`crypto.getRandomValues`; without it, loading throws. Exports: `memory`, `mae_crypto_alloc`,
`mae_crypto_call`, `mae_crypto_result_ptr`, `mae_crypto_result_free`, `mae_crypto_live_bytes`. A call that
traps retires the instance: the loader zeroes its whole linear memory (every held key with it) and every
later call throws; it is never restarted silently, since a fresh instance holds none of the keys.
`dispose()` does the same on purpose. A request over `MAX_REQUEST_BYTES` = 15,029,592 UTF-8 bytes (the
daemon's default `max_message_bytes`: base64 of a 10 MiB document plus 1 MiB) is refused with a
`RangeError` before the module sees it; a `content_seal` of a node at the daemon's 10 MiB document limit
fits. The entropy source can be replaced only by passing `randomFill` together with
`dangerouslyReplaceEntropyForTests: true`; that is for tests, and a production loader sets neither.

**Requests and responses.** A request is a JSON object with exactly the keys its operation lists (anything
else is `bad-request`). Every response is `{"ok":true,"result":R}` or
`{"ok":false,"code":C,"error":"message"}`. Bytes are standard base64 with padding. A member key lives in the
instance and is named by a handle `key` (a number, per instance, never reused); its 32-byte seed leaves only
through `export_seed` or inside a backup envelope. `KeyInfo` is
`{"key":n,"public_key":B64,"fingerprint":"SHA256:…","wrap_public":B64}`: the Ed25519 public key as
`PublicKey::encoded` writes it, the fingerprint as `PublicKey::fingerprint` does (`SHA256:` + unpadded
base64 of SHA-256 over the 32 bytes), and the ADR-041 wrap key.

| op | name | request | result |
|----|------|---------|--------|
| 1 | `version` | `{}` | `{"version":"0.1.0","backup_version":1,"challenge_domain":"mae-identity-binding-v1"}` |
| 2 | `generate` | `{}` | `KeyInfo` (seed: 32 bytes from the host) |
| 3 | `import_seed` | `{"seed":B64}` | `KeyInfo` |
| 4 | `export_seed` | `{"key":n}` | `{"seed":B64}` |
| 5 | `forget` | `{"key":n}` | `{"forgotten":bool}` (the key is wiped) |
| 6 | `key_info` | `{"key":n}` | `KeyInfo` |
| 7 | `fingerprint` | `{"public_key":B64}` | `{"fingerprint":"SHA256:…"}` |
| 8 | `check_challenge` | `{"key":n,"issuer":S,"subject":S,"audience":S,"challenge":S,"now":unix}` | `{"challenge":{"domain","iss","sub","aud","fingerprint","nonce","expires_at"}}` |
| 9 | `sign_challenge` | as 8 | `{"challenge":{…},"body":{"challenge_id","public_key","signature"}}` |
| 10 | `backup_seal` | `{"key":n,"passphrase":S,"kdf"?:{"m","t","p"}}` | `{"envelope":S}` |
| 11 | `backup_open` | `{"envelope":S,"passphrase":S}` | `KeyInfo` (a new handle) |
| 12 | `backup_params` | `{"envelope":S}` | `{"fingerprint","m","t","p"}` (checked, not opened) |
| 13 | `content_key_generate` | `{}` | `{"content_key":B64}` |
| 14 | `content_seal` | `{"content_key":B64,"plaintext":B64}` | `{"sealed":B64}` (`nonce ‖ ct+tag`) |
| 15 | `content_open` | `{"content_key":B64,"sealed":B64}` | `{"plaintext":B64}` |
| 16 | `content_wrap` | `{"content_key":B64,"wrap_public":B64}` | `{"wrapped":B64}` |
| 17 | `content_unwrap` | `{"key":n,"wrapped":B64}` | `{"content_key":B64}` |

The loader's method names are the camelCase forms (`signChallenge`, `backupSeal`, …). `challenge` (8, 9) is
the challenge text or the daemon's whole `/identity/challenge` answer; it is signed only if it is
byte-for-byte canonical, carries the domain tag, names this key, is issued to exactly the `issuer` and
`subject` given and by exactly the daemon `audience` names (its canonical resource URI, the challenge's
`aud`, so a challenge one daemon issued cannot be relayed through another), is unexpired at `now` (the
host's clock: unix seconds), and expires at most `MAX_CHALLENGE_LIFETIME_SECS` = 3600 s after `now`. The
daemon's `identity_challenge_ttl_secs` defaults to 300 (its tests keep the default within 120..=900 and at
most half the cap), so an hour admits any reasonable TTL plus clock skew and refuses a challenge that would
stay signable for days. `body` is what `POST /identity/bind` takes.

**Error messages never echo input.** Every `error` text is fixed per refusal kind (a malformed request
names the kind of problem and its line and column, never serde's message, which quotes values); no
seed, passphrase, key, envelope or challenge field ever appears in a response but a successful one's.

**Error codes.** General: `bad-request`, `unknown-op`, `no-key`, `keystore-full` (16 keys per instance),
`no-entropy`, `bad-public-key`. Challenge: `too-long`, `not-a-challenge`, `wrong-domain`, `empty-field`,
`other-key`, `other-issuer`, `other-subject`, `other-audience`, `expired`, `expiry-too-far`. Backup:
`too-long`, `not-an-envelope`, `unknown-version`, `unknown-kdf`, `kdf-params`, `passphrase`,
`passphrase-weak` (sealing only), `open` (a wrong passphrase and an altered envelope are deliberately
indistinguishable), `fingerprint-mismatch`. Content: `decrypt`, `malformed`, `bad-key`. Without entropy,
`generate`, `backup_seal` and `content_key_generate` answer `no-entropy`; `content_seal` and `content_wrap`
trap (fail closed, instance retired and wiped). A host fill of 16 bytes or more that is all zeros, or
identical to the previous such fill, is refused as no entropy.

**Backup envelope v1** (the format phase B imports; normative text in `shared/crypto/src/backup.rs`). One
line of JSON, exactly
`{"v":1,"fingerprint":FP,"kdf":"argon2id","m":M,"t":T,"p":P,"salt":SALT,"nonce":NONCE,"ct":CT}` (this key
order, no whitespace; surrounding ASCII whitespace is ignored). FP is the sealed key's fingerprint
(`SHA256:` + 43 unpadded base64 characters), readable without the passphrase so a person can tell which key a
file holds. SALT is 16 random bytes, NONCE 24, CT 48, each standard padded base64 in quotes. The passphrase
is NFC-normalised (by mae-crypto itself, on sealing and opening; any other implementation must do the same)
and must then be 1..=1024 UTF-8 bytes. key = Argon2id v0x13 (the NFC passphrase's UTF-8 bytes, SALT, M KiB,
T passes, P lanes, 32-byte output). HEADER = the same text without `,"ct":…`, i.e.
`{"v":1,"fingerprint":FP,"kdf":"argon2id","m":M,"t":T,"p":P,"salt":SALT,"nonce":NONCE}`. CT =
XChaCha20-Poly1305(key, NONCE, the 32-byte seed, associated data = HEADER's bytes, the fingerprint
included). Written with M=65536, T=3, P=4 (RFC 9106 §4); opened only with M in 19456..=262144, T in
2..=10, P in 1..=4. Opening refuses an unknown `v` before anything else (`unknown-version`), then any other
text, kdf, out-of-range costs or malformed field, then a failed tag (`open`: a relabelled fingerprint fails
here, as associated data), and last a seed whose fingerprint is not FP (`fingerprint-mismatch`). Sealing
also needs at least 12 characters after NFC (`passphrase-weak`): OWASP ASVS 4.0.3 V2.1.1's minimum for a
user-chosen secret. The envelope is the offline-attackable copy of the member's whole access, so length is
the one lever the person controls, and Argon2id's cost multiplies the entropy those characters carry rather
than replacing it (NIST SP 800-63B-4 asks 15 for a single-factor password; twelve plus a memory-hard KDF is
the trade made here, and the floor can only rise). Opening applies no floor, so an envelope made elsewhere
still opens.

**Secrets in the module.** Freed heap blocks are zeroed by the module's allocator and the stack an operation
used is zeroed when it returns, so after a call no copy of a seed, passphrase, content key or plaintext is
left in linear memory except the keys still held (until `forget` or `dispose`), nor the keys derived from a
seed (the X25519 wrap secret, the Ed25519 signing hash prefix) once the call that used them returns.
`js/test.mjs` searches the whole memory for each, raw and base64. JavaScript strings the caller holds (a
passphrase, an exported seed) are outside the module's reach.

**Reserved: op signing (#727), planned for the next minor version; no code yet.** A future op
`sign_op {key, kb_id, doc, update_sha256, …}` will sign a content or membership op with a held member key.
Like `sign_challenge` it will sign only a canonical, domain-separated message the module builds itself from
those typed fields (its own domain tag, distinct from `mae-identity-binding-v1`), never caller-supplied
bytes: there is, and will be, no generic `sign` operation, because the member key's signatures are its
authority over every KB it belongs to.
