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
- **Nothing replaces an anchor.** Not the owner's next connection, not the operator, and not a mesh
  join: a join ticket whose `kb_id` names a KB this hub recorded an anchor for, or a joined KB under
  another key, is refused and the dial ends (a redial with the same key is a no-op). A ticket is not
  the owner; letting one swap the key would re-root the KB's membership and flip a hosted KB to
  fail-closed `Joined`.

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

**Log wins, made visible** (the owner's ruling). Where the log governs, a member-list entry the log
does not admit has no access, and is never lost silently:

- when the anchor is recorded (the owner's connection or `kb anchor`), the daemon warns with the
  number of such entries, and `kb anchor` prints them;
- `mae-daemon kb anchor` with no arguments lists them for every anchored KB, and
  `kb anchor --kb-id X` with no key shows which source governs and lists them (both offline; not
  `doctor`, which never opens the store as a writer and points at the listing instead);
- no writer changes membership on the member list alone where the log governs: `kb/add_member`,
  `kb/remove_member`, `kb/approve_member` and the operator's `kb member` are refused
  (`signer_unavailable`) unless this daemon signs the change into the log as the owner whose key is
  the anchor, and a permissive `kb/join`'s auto-admit, which never signs, is refused there. The
  owner admits or removes the member with a signed membership op.

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
- **The control socket's mesh share** (`p2p/share_kb`, `handler.rs`): on a fresh share it writes
  the collection and each node's state as this daemon (`CONTROL_SOCKET`), not as verified content
  ops. Low risk: only the local control socket reaches it (filesystem permissions, the daemon's own
  user), it creates a KB this daemon owns and signs the genesis for, and the node states come from
  this daemon's own KB store.
- `__mcp_sync_update` needed nothing: the daemon already refuses it on that path.

### F4. An op a session originates is its own

On an editor session (mTLS, a self-issued token, a bound IdP login on `/ws`), the op's `author`
must equal the session's principal (`not_author`). A session cannot author for another member.
Ops a mesh peer relays (`Transport::P2p`) are checked for signature and membership only: their
author is legitimately not the connection.

Only a session with no identity at all has no principal, and skips this check: `auth.mode = none`.
It is already trusted for every KB, so the check would bound nothing; signature and membership
still apply. Every identified session has a principal: a key or mTLS peer its fingerprint, a `/ws`
bearer its token subject, psk the shared `psk:<keyid>` (so a psk session's signed ops are always
`not_author`; psk clients do not sign). A key peer whose public key is the all-zero encoding (a
valid point an operator could authorize) used to have none, and so was trusted for every KB; it now
has its fingerprint.

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
- A member-list entry the governing log does not admit is reported at anchoring and by
  `kb anchor` (the per-KB show and the no-argument listing); a member-list membership write is refused where the log governs and this daemon
  cannot sign into it, and works where the member list governs.
- A join ticket never replaces an anchor; a dial naming a KB this hub anchored is refused.
- A session with no identity relays a member's op, checked for signature and membership; a psk
  session cannot.
- On the anchored path, re-admission moves the epoch and an op under the old one is fenced; an
  owner's key rotation keeps the anchor and the new key writes.
- A browser-signed op (mae-crypto) and an editor-signed op for the same bytes are both accepted,
  and their canonical bytes are identical.

### Open items (as built)

- **A squatted `kb_id` keeps its anchor.** The first session to share an id owns it, as before
  (ADR-105 D5), and its key is now that KB's durable anchor. There is no command to clear an anchor,
  deliberately. Recovery today: the legitimate owner shares under a fresh id (an editor does this on
  `kb_id_owned_by_another`), and the operator revokes the squatter's access (`mae-daemon revoke` for
  a key). The id itself is not reclaimed. See `docs/DAEMON_ADMIN.md`.
- **The upgrade window for joined KBs.** Joined anchors were memory-only before this amendment, so
  on the first start after the upgrade a mesh-joined KB has no anchor until its dialer next dials,
  and until then reads the relay-supplied member list. The daemon cannot tell such a KB from one it
  hosts at load: join tickets are not persisted, the collection is a copy of the owner's, and the
  document editor record (ADR-113 D3) began after many joins and records mesh-peer writes on hosted
  KBs too. Closing it needs a persisted record of joined KBs; until then, an operator upgrading a
  daemon with mesh-joined KBs re-submits their tickets before serving clients.
- **A mesh owner that rotates its daemon key.** A joined KB keeps the key it was joined under, so a
  fresh ticket naming the rotated key is refused (F1). The owner's log carries the rotation, so
  accepting a successor the log proves is the fix; it is not built.
- **A genesis authored after anchoring.** The warning fires when the anchor is recorded. A KB whose
  owner authors its genesis later moves to the log then, without a warning; `kb anchor` (show, or
  the no-argument listing) reports it.

## Amendment 2 (2026-11-26): a change to a KB's node list is a signed op too

Amendment 1 F3 left one write path without an author's signature: the node list of a KB's
collection document (`kbc:<kb>`). The hub enforces signed content ops (#727 part 1, behind
`require_signed_content_ops`), but any editor can still add or remove a node, and the daemon writes
that change itself, recorded only against the session. This amendment closes it (#727 part 2).

**What writes the node list today** (surveyed 2026-11-26):

- `kb/collection_node_add` and `kb/collection_node_remove`, at `Edit`: the native editor's only way
  to create or delete a node. The daemon builds the collection update, so there are no client bytes
  to sign.
- `kb/collection_op` and a raw `sync/update` or `sync/share` of `kbc:<kb>`: owner only (`Manage`,
  `deny_collection_smuggling`), plus a member's own identity ops (ADR-040). Their membership and
  encryption changes are already signed op-log entries every peer verifies; nothing signs a node-list
  change made through them.
- `kb/share` (the owner's own first share, which creates the KB) and the mesh dialer's pushed
  collection (#727 part 3, after the pilot).
- The browser client writes none of these. It never writes the collection document; a node's title
  lives in its own node document, so a rename is a content op (mae-web, 2026-11-26).

### G1. A node-list change carries a signed manifest op

`kb/collection_node_add` and `kb/collection_node_remove` take the same header a content op carries
(`author`, `author_pubkey`, `epoch`, `issued_at`, `sig`), over a new canonical message:

```
maemanifest/v1 \0 kb_id \0 node_id \0 action \0 title \0 author \0 epoch \0 issued_at \0
```

`action` is `add` or `remove`; `title` is empty for a removal. Fields are NUL-terminated exactly as
D1's (empty or NUL-bearing ids refused), and a new domain tag, so a content-op signature can never be
presented as a manifest op or the reverse. It lives in `mae_crypto::manifest_op` beside
`content_op`, one encoding for every signer and the verifier.

The hub verifies it as it verifies a content op, through the same steps: the key's fingerprint is
the author, the signature holds, the author is a member with `Edit` at the op's epoch (the role the
method already requires) under F2's membership source, the epoch fence, and F4: on a session the
author is the session's principal. The daemon then builds and applies the collection update as it
does today, and the editor record names the author.

### G2. The switch covers it

The same `require_signed_content_ops`. Off: an unsigned manifest op is accepted and counted with the
unsigned content ops, so one number says whether every writer signs. On: refused
`signature_required`, whatever the caller's role. A signed op that fails verification is refused at
any setting.

### G3. Raw collection writes may not change the node list once the switch is on

With the switch on, a raw collection write (`kb/collection_op`, `sync/update`, `sync/share` of
`kbc:<kb>`) that adds, removes or retitles a node is refused `signature_required`: the node list
changes only through G1, so every change to it has an author's signature. The owner's signed
membership and encryption ops through `kb/collection_op` are unaffected. The check compares the node
list before and after the write, as the E7 delete guard does, in the store, under the same gate.
`kb/share`, which creates the KB with its node list in the owner's own session, is not a change to an
existing list and stays as it is (F3's open item); the mesh join and push stay with part 3.

### G4. Signers

- The native editor signs with its identity key (D2), in the network task that sends the request.
- mae-crypto gains the canonical bytes for both kinds of signer. A WASM `sign_manifest_op` is not
  built until a browser client needs one; mae-web does not write the node list.
- The browser client's relay refuses methods its editor never sends (mae-web, agreed 2026-11-26), so
  a same-origin script cannot reach `kb/collection_node_*` through it even before the switch is on.

### G5. Tests

- A signed add and remove by an editor apply, attributed to the author; the same op from another
  session is `not_author`; a non-member's or a viewer's signed op is refused; a removed member's op is
  fenced.
- A content-op signature presented as a manifest op (and the reverse) fails; a changed `node_id`,
  `action` or `title` fails.
- With the switch off an unsigned call is accepted and counted; on, refused.
- With the switch on, a raw collection write by the owner that adds, removes or retitles a node is
  refused and changes nothing; one that only appends a signed membership op is accepted.
- The editor's network task sends a signed header on both methods, and the daemon accepts it.
