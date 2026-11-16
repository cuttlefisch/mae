# ADR-036: Signed content operations (peer-verifiable node-edit authorship)

**Status:** Accepted (design). The explicit follow-on named in **ADR-026 §A5** ("Scope = membership;
**Amended:** Amendment 1 (2026-11-15, proposed): the hub verifies every content op it accepts, on
every KB and every path (#727).
signed *content* ops are a deferred follow-on") and the remaining half of P2P **Phase 4** (#91).
Tracks #91.
**Extends:** ADR-023 (epoch-fenced rebase — content write-access), ADR-026 (signed membership op-log
+ per-peer derivation), ADR-022 (SV-reconcile).
**Pairs with:** ADR-037 (E2E content encryption — same content-op substrate: integrity + confidentiality).
**Depends on:** #72 (unpredictable daemon-issued epoch token) — **met** (PR #99).
**Builds on:** ADR-025 (the daemon mesh this must secure).

## Context

ADR-026 made **membership** peer-verifiable: every membership mutation is an Ed25519-signed, hash-chained
op, and every peer *derives* validity from the log rather than trusting a relay. But ADR-026 §A5
deliberately scoped that to membership and left **content** (node-edit) ops governed only by ADR-023's
**epoch fence**:

- A content op (a `sync/update` carrying a yrs delta for a `kb:{node}` doc) is **not author-signed**. The
  daemon attributes it to a session/client_id, and ADR-023 fences ops authored under a stale (pre-grant /
  post-removal) epoch.
- The fence is a *write-access* control (was this lineage authored under the author's current epoch?), not
  an *authorship* proof. On a mesh (ADR-025), peer A receives content ops **relayed through** peer B's
  daemon, which A does not trust. A malicious relay can **mis-attribute** an edit (claim Bob authored
  Alice's change, or inject content under a member's identity) and a receiving peer has no cryptographic
  basis to reject it — only the epoch fence, which says nothing about *who* signed.

So the same "blockchain verification of KB **contents**" that ADR-026 gave membership must extend to
content: **every peer verifies who authored each edit, and that they were an authorized member at that
edit's epoch, without trusting the relay.**

## Decision

Adopt **signed content ops**, mirroring ADR-026's proven membership pattern on the content path. Authors
(editors) sign each content op; peers verify on apply against the ADR-026 derived membership at the op's
epoch. The daemon mesh only *transports* — it neither authors nor needs to be trusted for attribution.

### D1 — A content op is author-signed (Ed25519) + epoch-stamped

A content op is the existing `sync/update` envelope plus an **authorship header**:
`ContentOp{kb_id, node_id, base_sv, author, epoch, issued_at}` + `sig` + `author_pubkey`, where the
signature covers **deterministic canonical bytes of the header ‖ the yrs update payload** (version-tagged,
NUL-separated — the ADR-026 `canonical_bytes` discipline). `author = fingerprint_of(author_pubkey)`
(STANDARD_NO_PAD, identical to `mae_mcp::PublicKey::fingerprint()` and the membership layer). `epoch` is
the author's current ADR-023 epoch, **carried in the signed op** (a per-peer `fresh_epoch_token()` would
break cross-peer client_id agreement — the same rule ADR-026 §2b-3 established for membership). Pure-Rust
(`ed25519-dalek` + `sha2`, in-lockfile); O(1) per-op verify.

This lives in a new `shared/sync/src/content_ops.rs`, **sibling to `membership.rs`**, reusing its signing
discipline (`sign(secret)` / `verify(sig, pubkey)`, canonical-bytes versioning, fingerprint format).

### D2 — Editors author + sign; the daemon relays key-blind

**Only editors author + sign** content ops (the human or the AI peer hold the identity key, ADR-017). The
daemon **attaches nothing of its own** to authorship — it verifies + relays. A relay daemon that is not a
member, or that is hostile, can drop/delay (liveness) but cannot **forge** an attributed edit. This keeps
the daemon **honest-but-untrusted** for content, exactly as ADR-026 made it for membership.

> Implication for the hosted/headless path: a daemon that *hosts* a KB on a member's behalf would need a
> delegated content-signing capability (a member key, or a future capability grant). Hosting-without-an-
> editor-key is out of scope here and noted for the headless-host work (#111); the mesh case (editor →
> own daemon → peers) is fully covered.

### D3 — Verify on apply (peer-side), before merge

Before a peer applies a content op (`apply_doc` / `apply_update` in `daemon/src/dialer.rs` /
`collab_handler.rs`), it checks, **locally and trustlessly**:
1. `sig` verifies against `author_pubkey`; `fingerprint_of(author_pubkey) == author`.
2. `author ∈ derive_valid_members(...)` (ADR-026) **at the op's `epoch`** — the author was an authorized
   member when they authored it.
3. ADR-023's epoch fence holds: the op is authored under the author's current-epoch
   `derive_kb_client_id(author, epoch)` (now peer-side, ADR-026 §A5).
A failing op is **rejected and surfaced** via the ADR-024 notification bus (not silently dropped). A
passing op merges via the existing non-destructive `apply_update` / SV-reconcile (ADR-022) — signing
**does not change convergence**, only admission.

### D4 — Causal binding: reuse the yrs SV + epoch fence; do NOT add a content-DAG now

Each op already carries `base_sv` (the author's state-vector at authoring) and an epoch; together with the
membership DAG that establishes authority, this is sufficient to reject forged/replayed/mis-attributed
ops **without** a second causal hash-chain over content. A full **content auth-DAG** (every content op
referencing predecessor hashes, for fully-leaderless content authority) is the ADR-026 §B research
endgame — **deferred to ADR-039**, not built here. ADR-036 is the pragmatic, peer-verifiable slice; it
does not block on the research item.

## Adversarial exploit-path review

- **Forged / mis-attributed edit ("Bob wrote this")** → rejected: no valid `sig` for the claimed author,
  or `fingerprint_of(author_pubkey) ≠ author`. A relay cannot mint an attributed edit.
- **Non-member injects content** → rejected: `author ∉ derive_valid_members` at the op's epoch (ADR-026).
- **Removed-member writes / pre-grant cascade** → rejected at **every** peer: the op's epoch is stale vs the
  author's derived current epoch ⇒ ADR-023 fence (peer-side); the membership resolver already invalidated
  their grants. (#72 makes the epoch unpredictable, so the fence is not bypassable — the gate it satisfies.)
- **Replay an old signed op** → idempotent under yrs SV (already applied = no-op); a replay claiming a newer
  base_sv fails the fence (stale epoch / SV mismatch).
- **Tamper with the payload after signing** → the signature covers header ‖ payload bytes; any mutation
  breaks `verify`.
- **Relay reorders / drops ops** → liveness, not safety: yrs/CRDT convergence is order-independent for
  *applied* ops; an omitted op is simply absent (every honest peer that has it still applies it). Ordering
  cannot change *attribution* (each op is self-attesting).
- **Honest-server assumption removed for content attribution.** What remains unaddressed here:
  **confidentiality** (a member/relay still *reads* plaintext) → ADR-037; fully-leaderless content
  authority (no epoch sequencer at all) → ADR-039 (§B).

## Consequences

- Content authorship + write-authority become **peer-verifiable**; the ADR-025 mesh relays content freely
  without being trusted for attribution — closing the gap ADR-026 left for the content path.
- A signed-op header per content op: one Ed25519 signature (~64 B + pubkey) per edit. Edits are frequent,
  so cost matters more than for membership — but signing/verify is O(1) and ~µs; the wire overhead is small
  vs a yrs update. Batching multiple node deltas under one signature is a permitted optimization.
- **Migration:** unsigned content ops (v0.14 hub, psk/none auth) keep working on the legacy path; signed
  content requires daemon key-mode, which the mesh requires anyway. A peer that receives a *signed* op
  enforces it; a peer that receives an *unsigned* op on a key-mode KB treats it per policy (reject on the
  mesh; accept on a trusted single-hub during migration).
- Reviewer guardrail: any content op **applied without** passing signature + membership-at-epoch + fence
  checks on the receiving peer is a security regression. Transport auth (ADR-025) and the epoch fence
  (ADR-023) are each necessary but not sufficient for attribution.

## Verification

Unit (mirroring ADR-026's 29 membership tests): `ContentOp` sign/verify (tamper-any-field-breaks,
wrong-key, forged-fingerprint each rejected); author-not-member-at-epoch rejected; stale-epoch op fenced
peer-side; valid op applies + converges (SV advances). **Security-negative oracle on a real 2-daemon
mesh** (the harness in `daemon/src/dialer.rs` tests): a relay that forges/mis-attributes a content op is
rejected by the receiving peer; a removed member's content op doesn't cascade; a valid member's edit
applies + both peers converge. Cross-OS (principle #13). v0.14 single-hub content tests stay green.

## Amendment 1 (2026-11-15): the hub verifies every content op it accepts

D3 made verification peer-side, and the mesh requires it. The hub does not: it verifies only on a
KB that has a content anchor, consults `require_signed_content_ops` only on `sync/update`, never
checks that a session's own op is authored by that session, and applies the mesh join snapshot and
one internal sync path unverified (#727). Browser clients now write over the hub (ADR-099 Amendment
2), so the hub's authorship guarantee can no longer be opt-in. This amendment closes it.

### F1. What is stored, and what travels

- **A member's public key travels with every op.** A signed op carries `author_pubkey`, and
  `author` is its fingerprint (D1); the verifier checks that the two match before anything else.
  The fingerprint is self-certifying, so no per-member key registry is needed to verify. A binding
  (ADR-098) or a member list may show a key; nothing depends on it.
- **A KB's anchor is stored once.** Membership is derived from the owner-signed op-log, which needs
  the owner's public key: the KB's content anchor. A joined KB registers one today; a KB an editor
  shares to the hub usually has none, which is why the hub falls back to accepting unsigned ops
  there. From now on, the hub records the sharing session's public key as the KB's anchor when the
  KB is first shared, durably, in the same record a joined KB's anchor uses. A KB shared before this
  is anchored when its owner next connects with the key whose fingerprint the collection names as
  owner, or by the operator's local CLI.

### F2. One membership source for access and for verification

Verification admits an op when its author is a member at the op's epoch. It reads membership from
the **same** source `kb_access` reads (`member_role`): the signed op-log on a KB that has one, the
locally authoritative member list on one that does not. An anchored KB without a signed op-log
yet therefore keeps working; it does not lose every member to an empty derivation.

The anchor is stored with its **provenance**, and the rule depends on it:

- **An anchor this hub recorded itself** (the owner's session at first share, the owner's next
  share or write, or the operator's `kb anchor`): the hub is the authority for the KB's member
  list. Its log is the source only when rooted at a genesis that anchor signed — a
  cryptographically valid owner self-admit, with no predecessor, authored by the anchor key. Any
  other log (one that starts with a member's self-`Rebind` or the owner's key rotation, one whose
  genesis another key signed) derives no members, so membership comes from the member list, as on
  an un-anchored KB (verified genesis or fallback). No member op can therefore make such a KB lose
  its members.
- **An anchor registered from a mesh join** fails closed (ADR-026): the member list came from a
  relay, so the log is always the source, and without a genesis the anchor signed it yields no
  members. A relay can never substitute its own member list.

One decision makes this choice for access, the epoch fence, op admission and the member set alike.
The owner authoring a genesis later, to move a hub-recorded KB onto signed membership, is a separate
step, not part of this amendment.

### F3. Every write path verifies

- `sync/update` and `kb/node_update` (the browser's path), over mTLS, `/ws`, and every other
  transport that reaches them, run the same verification, and both consult the switch.
- The internal sync path #727 names (`__mcp_sync_update`) gets the same cap and verification, or is
  removed if nothing needs it.
- **Collection documents** (`kbc:`) are written the same way: a manifest change (a node added,
  renamed or removed) is a signed op on the collection document, admitted only from an author with
  the role that change needs. Membership and policy stay in the signed op-log (ADR-026).
- **The mesh join snapshot** (#727 §2) is the one path that carries merged state rather than
  individual ops. Its collection op-log is verified against the anchor as now; whether node states
  in a snapshot need an owner signature, or are re-requested as signed ops, is decided in its own
  amendment before the switch is turned on for the mesh's join path. It is tracked here so the
  switch does not go on with it open.

Open, not covered by the switch yet (as built; each needs its own decision):

- **Collection manifest signing.** No signed-manifest op format exists, and
  `kb/collection_node_add`/`kb/collection_node_remove` build their collection update on the
  daemon, so there are no client bytes to sign. A raw `kbc:` write stays owner-only
  (`deny_collection_smuggling`); the dialer applies the owner's pushed collection unverified.
- **The mesh join snapshot**, as above.
- **`kb/share`'s node states.** A share carries merged node state, not individual ops, like the
  join snapshot; it is the owner's own session (re-share of another owner's id is refused), and is
  not verified as content ops.
- `__mcp_sync_update` needed nothing: the daemon already refuses it on that path.

### F4. An op a session originates is its own

On an editor session (mTLS, a self-issued token, a bound IdP login on `/ws`), the op's `author`
must equal the session's principal (`not_author`). A session cannot author for another member.
Ops a mesh peer relays (`Transport::P2p`) are checked for signature and membership only: their
author is legitimately not the connection.

### F5. Every writer signs

Native MAE signs with its identity key (D2). Browser MAE signs with the member key it holds, through
mae-crypto's `sign_op`: domain-separated, over exactly D1's canonical bytes, sharing the Rust code
that produces them so the browser, the editor and the verifier cannot diverge. A daemon never signs
on a member's behalf (ADR-098 rejected it): an agent writes with a key of its own, or through a
client that holds its person's key.

### F6. Turning it on

`require_signed_content_ops` stays the switch, daemon-wide. With it off, unsigned ops are accepted
and counted (the existing metric). It is turned on once every client signs and the count stays at
zero over the pilot. Then an unsigned op, an op from an author who is not a member at its epoch, a
mis-attributed op, or an originated op not authored by its session is refused, on every KB and
every path.

### F7. Tests

- An unsigned op is refused on `sync/update` and on `kb/node_update`, over mTLS and `/ws`, on an
  anchored KB and on a KB anchored at first share; accepted and counted with the switch off.
- A KB shared after this change has its sharer's key as anchor; a KB shared before is anchored at
  the owner's next connection, and not by a key whose fingerprint is not the owner's.
- An anchored KB without a signed op-log admits its members' signed ops (F2).
- An op whose `author_pubkey` does not hash to `author`, or whose signature does not verify, is
  refused; an op authored by another member on a session is refused `not_author`; the same op
  relayed by a mesh peer is admitted.
- An unsigned manifest change to a collection document is refused with the switch on.
- A browser-signed op (mae-crypto) and an editor-signed op for the same bytes are both accepted,
  and their canonical bytes are identical.
