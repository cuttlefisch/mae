# ADR-038: Editor-authored membership for owned KBs (key-blind collection writes)

**Status:** Accepted (design + in-progress implementation). Tracks #131 (Phase 3b, #150/#151). Surfaced by
building ADR-037 — the daemon must stay key-blind, so the owner (an editor) must author the key-distribution
channel itself.
**Extends:** ADR-026 (peer-verifiable signed membership op-log), ADR-018 (identity-anchored access control).
**Pairs with:** ADR-037 (E2E content encryption — this is how the per-KB key is distributed without the
daemon ever holding it) and ADR-039 (identity & authorization hardening).

## Context

ADR-037 requires the per-KB content key to ride the **signed membership op-log**, wrapped to each member, so
a hosting/relaying daemon carries content it cannot read. That only works if the op-log is authored by a
principal the daemon does **not** have to be trusted to read keys for.

Building it surfaced a foundation gap. In the hub flow (an editor shares a KB to `mae-daemon`; peers join):

- The **editor is the KB owner** — the daemon binds `COLL_OWNER_KEY` to the authenticated principal (ADR-018
  strict binding, `collab_handler.rs:1641-1648`).
- But the daemon's `append_signed_membership` early-returns unless **its own** signer is the owner
  (`collab_handler.rs:771-777`). For an editor-owned KB it is not, so **no signed membership op-log is ever
  authored** — only the legacy unsigned `member_roles` CRDT map is mutated.
- The editor is a **read-only collection mirror** — it has no way to write the collection.

Therefore ADR-037's `derive_content_key` (which needs the signed op-log: a genesis anchor + `wrapped_key`
ops) **never engages for editor-owned KBs**. We need the owner editor to author the signed membership op-log
for KBs it owns, while the daemon — which must not hold the content key — only stores and relays it.

## Decision

**For an editor-owned KB, the OWNER EDITOR authors the signed membership op-log; the daemon stores the
opaque, owner-signed bytes and stays key-blind.**

### D1 — `kb/collection_op`: a key-blind collection-write RPC (landed, #154)

A new daemon RPC `kb/collection_op { kb_id, update }`: it confirms owner authority via the **same**
`kb_access(…, KbOp::Manage)` gate the legacy member RPCs use, then stores + rebroadcasts the **opaque
owner-signed collection delta** via the existing `persist_and_broadcast_collection`. The daemon never parses
the op's semantics and never holds a content key. `KbCollectionDoc::append_signed_op` (which produced the
bytes) intentionally does **not** validate — every peer *derives* validity from the signed log (ADR-026), so
the daemon storing un-inspected owner-signed bytes is sound. This is the editor's only authoring path for an
editor-owned KB's signed op-log.

### D2 — Network-task authoring (the owner holds the secret + the key)

All authoring + content-key crypto runs in the editor's **network task** (`run_collab_task`) — the only
place holding both the identity secret (`signing_identity`) and the content key. The main thread keeps
editing the plaintext `KbNodeDoc` unchanged. The owner authors, in the network task:
- the **genesis self-admit** (anchor) on enable, with the content key self-wrapped to the owner;
- an **`Admit`** op per member carrying `wrapped_key = wrap_to_member(content_key, member_pubkey)`;
- a **`Remove`** + rotation re-wrap on removal (ADR-037 §D3).
Each is shipped via `kb/collection_op`; every peer (including the owner's own main thread) relearns through
the existing `kbc:` broadcast path.

### D3 — Member pubkey via the membership channel

`wrap_to_member` needs the member's 32-byte Ed25519 **public** key, but the owner has only the fingerprint
(a one-way hash). The daemon — which holds the joiner's pubkey from the authenticated session — records it in
the **`PendingRequest`**, which rides the `kbc:` broadcast the owner already mirrors. The owner reads
`pending.pubkey` → wraps. A bare-fingerprint add with no pending request authors a **keyless** `Admit` and
defers the wrap; a later wrap-only op supersedes it (latest-owner-wrap-wins). No new editor-side key store.

### D4 — Dual-write op-log + `member_roles` (epoch-fence consistency)

The ADR-023 epoch fence reads a member's epoch from the legacy `member_roles` map. So the owner's
`kb/collection_op` delta mutates **both** the signed op-log (`append_signed_op`) **and**
`member_roles`/epoch (`upsert_member`) in **one** combined collection update (captured via a state-vector
diff), using the **same** epoch in both. This keeps the security-critical fence's source consistent without
touching the fence. *(The deeper fix — deriving the epoch from the op-log directly — is ADR-039 / #157; the
dual-write is the correct interim for the hub flow.)*

## Threat model — what it protects / what it doesn't

**Protects:** the daemon never holds the content key (key-blind hosting); a non-owner cannot author
collection ops (`kb_access(Manage)`); a forged op (signature ≠ claimed author pubkey) is stored but is **not
a valid member** — store-blind ≠ trust-blind, since every peer derives validity from the signed log.
**Does NOT protect:** this ADR is the *authoring channel*; the encryption guarantees + their limits are in
ADR-037, and the anchor-pinning / unified-fence hardening is in ADR-039. The daemon is still trusted for
availability + ordering (it can withhold, not forge or read).

## Consequences

- Editor-owned KBs gain a peer-verifiable signed membership op-log + the ADR-037 key-distribution channel,
  with the daemon key-blind — the headline local-first property (host you don't trust can't read).
- Unencrypted KBs are unchanged: they keep using the daemon-authored legacy member RPCs; `kb/collection_op`
  is only used for E2e KBs.
- The owner is the sole author for an E2e KB (the daemon's semantic member RPCs are bypassed for it),
  avoiding dual-genesis.
- New surface area: the `kb/collection_op` RPC + the network-task authoring path (each adversarially tested).

## Verification

- Daemon integration test (landed): the owner ships a signed genesis + admit; the daemon stores them
  key-blind; a peer derives `owner=Owner` + `member=Editor` from the relayed op-log; **a forged op is not a
  valid member**.
- Editor tests: enable → genesis authored + content key self-wrapped + persisted (0600); a second identity
  derives nothing; a member admitted with its pending pubkey unwraps the same key; epoch consistent across
  the op-log + `member_roles`.
- `make ci-all` green; unencrypted path byte-identical; the daemon never logs/persists plaintext or a key.
