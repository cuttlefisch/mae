# ADR-067: Admin-enforced live-query-only KB access (no full local replication)

**Status:** In progress (Phases A/B/C/E landed and shipped; Phase D — self-pointing
`RemoteHub` + OAuth self-minting + mTLS `kb/query.*` wiring — remains, tracked as issue
#453).
**Extends:** ADR-018 (identity-anchored KB access control — this ADR adds a new axis to
the same `kb_access` chokepoint), ADR-026 (peer-verifiable signed membership op-log — the
new policy field lives in this log).
**Relates to:** ADR-037 (E2E content encryption — the threat model this design must not
weaken), ADR-038/039/041 (editor-authored membership, identity/authz hardening, key
separation — the key-delivery mechanism this ADR must not break), ADR-052 (OAuth 2.1
resource server — the transport a query-only member authenticates over), ADR-053 (live
scoped read-through KB query surface — the mechanism this ADR extends from non-members to
restricted members), ADR-062 (federation registry scaling + unified local/remote-hub
search — `RemoteHubQueryLayer` is the client-side vehicle this ADR reuses).
**Tracking:** issue #449 (epic tracker). Issue #448 was the original "an ADR is needed
for this gap" request issue — resolved by #449's creation, now closed; #449 is the real,
open tracking issue.

## Context

MAE's per-KB role model (ADR-018) grants a principal `owner | editor | viewer`, which
determines `Join | Read | Edit | Manage` capability via one chokepoint,
`kb_access(kb_id, principal, op)` (`daemon/src/collab_handler/mod.rs`). Today, **any**
principal with a role — even the least-privileged `viewer` — who calls `kb_join` gets full
local CRDT replication: their client becomes a complete, durable, offline-capable copy
holder of the entire KB. Confirmed directly in the running code
(`daemon/src/collab_handler/mod.rs:1183-1189`):

```rust
match role {
    Some(role) => {
        let allowed = match op {
            KbOp::Join | KbOp::Read => true,
            KbOp::Edit => role.includes(SyncRole::Editor),
            KbOp::Manage => role.includes(SyncRole::Owner),
        };
        ...
```

`KbOp::Join` and `KbOp::Read` are handled by the **identical match arm** — there is no
existing distinction between "may read this KB's content" and "may replicate a full local
copy of it." This is a real gap, not a policy an admin can already configure and simply
hasn't discovered.

ADR-053 already solved the adjacent problem for a different population: a **non-member**
thin client (e.g. an occasional VS Code session) can read/search a KB live over the
network via `kb/query.*`, with zero local replication, ever. But ADR-053's own Context
section is explicit about scope: "This ADR does not contradict [full-replication-for-
members]; it adds a distinct, explicitly-scoped capability for *non-member, read-only,
thin* clients." Nothing today lets a KB owner say "you may read this KB, but you may not
carry a durable offline copy of it home" to someone who otherwise has Read access —
relevant for sensitive KBs (HR, security review, legal, client-confidential work) where
granting read access should not automatically imply an unrevocable, undetectable local
export.

**Why this needs its own ADR, not a one-line policy flag.** Three structural properties
of MAE's existing architecture make this harder than "add a boolean," and glossing over
any of them would produce a design that either doesn't actually work for encrypted KBs or
reintroduces a spoofing vulnerability ADR-018/026/037 already closed:

1. **Membership is a signed, tamper-evident op-log, not a plain CRDT map.**
   `shared/sync/src/kb/collection_oplog.rs`'s own header describes it as "the ADR-026
   signed membership op-log — the append-only, CRDT *set* of signed membership ops...
   validity is *derived* by every peer replaying this log, never read as a trusted
   verdict." Each op is a `SignedMembershipOp` (`shared/sync/src/membership.rs:270`) with
   `.sign()`/`.chain_hash()`/`verify_signed()` binding signature and payload into a hash
   chain. For an *anchored* KB, `kb_access` derives role from this signed log via
   `doc_store.derived_membership` (`daemon/src/collab_handler/mod.rs:1158-1163`,
   ADR-042-memoized) — **not** from the legacy `member_roles` YMap, which remains
   authoritative only for un-anchored/legacy KBs
   (`daemon/src/collab_handler/mod.rs:1165`). A new replication-restriction flag placed
   anywhere *other* than this signed log — e.g. a plain unsigned field on the collection
   doc — would be exactly the kind of thing a key-blind relay or a compromised hub could
   flip, undermining the same trust model ADR-018 built the fingerprint-anchored,
   op-log-derived membership system to guarantee in the first place.

2. **`kb_join` performs the access check and the replication in the same function, back
   to back.** `daemon/src/collab_handler/kb_membership.rs:85`, `handle_kb_join`: gates on
   `kb_access(..., KbOp::Join, ...)` around line 177, and — if allowed — immediately (same
   function, lines ~249-346) reads the `kbc:{kb_id}` collection doc, subscribes the
   session to `sync_update` events, enumerates every node in the collection, and pushes
   full state for each one, subscribing to every `kb:{node_id}` doc too. Splitting "may
   read" from "may join/replicate" at the `kb_access` decision point is therefore the
   actual, sufficient enforcement point — no separate replication-throttling mechanism is
   needed once `KbOp::Join` can be denied independently of `KbOp::Read`.

3. **E2E-encrypted KB key delivery is currently *bundled into* `kb_join`, not a separate
   step — this is the central design challenge, not a footnote.** For an E2E-encrypted
   KB, a member's wrapped content-key rides an `Admit` op's `wrapped_key` field
   (`shared/sync/src/membership.rs:135-141`). The client currently learns this via
   `kb_join`'s own response `collection_state` field
   (`daemon/src/collab_handler/kb_membership.rs:356-358`), then derives the usable key
   client-side (`crates/mae/src/collab_bridge/mod.rs:1687-1726`, `derive_content_key` →
   `find_wrapped_content_key`, `shared/sync/src/membership.rs:976,1010`). `kb/query.*`
   (`daemon/src/kb_query.rs`, dispatch at lines 53-97: only `capabilities`/`get`/`search`/
   `graph`) **never** exposes the collection doc, the op-log, or a wrapped key — for an
   E2E KB, `kb/query.get` returns raw ciphertext the caller is assumed to already be able
   to decrypt (`daemon/src/kb_query.rs:206-217`'s own comment: "a genuine KB member to
   decrypt client-side with a key only they hold"). **If this ADR simply denied `kb_join`
   for a restricted member, it would also cut off their only existing path to the
   decryption key — breaking E2E content access entirely, not just replication.** Any
   design that doesn't solve this explicitly does not actually work for the KBs (the
   sensitive ones) this ADR exists to protect.

**A fourth, load-bearing correction found while designing this ADR:** `kb/query.*` is
reachable **only** over the OAuth HTTPS listener
(`daemon/src/oauth.rs:452-491` → `kb_query::dispatch`) — confirmed by exhaustive grep:
zero call sites reach `kb_query::dispatch` from the mTLS collab handler. ADR-053's own
Decision-1 prose says the surface is "hosted on the network listener(s) established by
ADR-052 (OAuth) and/or existing mTLS (ADR-017)," but the mTLS half was never actually
wired up. This means, as of today, a query-only-restricted member on an mTLS-only
deployment (no OAuth listener enabled) would have **no path at all** to read the KB they
were just granted Read access to — a real, must-fix prerequisite for this ADR to be
usable outside OAuth-enabled deployments, addressed explicitly in Phase D below rather
than silently assumed away.

## Real-world grounding

"May read this content live" and "may take a durable, offline, bulk local copy of it" are
a well-established, separately-enforced permission axis across mature systems — this is
not a novel control MAE would be inventing from scratch:

- **Salesforce**: distinct **"Run Reports"** (view live, in the UI) and **"Export
  Reports"** (download to Excel/CSV) permissions. Export has a *dependency* on Run but is
  independently revocable — a profile can grant one without the other. (Salesforce Help,
  report-permission documentation; CloudAnswers, "Salesforce Report Permissions Deep
  Dive.")
- **Atlassian Confluence**: page-level **View** is separate from space-level **"Export
  Space"** — bulk-exporting the *entire* space (the closest real analog to "full local
  replication" of a KB) requires a distinct permission, revocable independent of View.
  (Atlassian Confluence KB, "How to restrict the ability to export a Space in
  Confluence.")
- **Google Workspace Drive DLP**: an admin-configurable rule action, **"Disable download,
  print, and copy,"** strips the ability to obtain a durable local copy from users who
  otherwise have read (Viewer/Commenter) access, while live viewing remains fully intact.
  Notably scoped to viewers/commenters only — editors retain copy ability, the same shape
  as an Editor-vs-Viewer role split. (Google Workspace Admin Help, "Prevent users from
  downloading, printing, or copying files.")
- **Microsoft Purview / SharePoint**: the **"Restricted View"** permission level lets a
  user open and read a document in-browser with download/print/copy-paste blocked;
  Purview sensitivity labels extend this with **"Do Not Forward"**-style encryption-based
  rights that persist even off the originating system. (Microsoft Learn, "Apply
  encryption using sensitivity labels.")
- **NIST SP 800-53 Rev. 5, control AC-4, "Information Flow Enforcement"** — the strongest
  formal citation available. AC-4's own discussion text draws exactly this axis:
  information flow control "regulates where information can travel... in contrast to who
  is allowed to access the information, and without regard to subsequent accesses to that
  information," and explicitly names "prohibiting information transfers between connected
  systems (i.e., allowing access only)" as a recognized enforcement pattern. This gives a
  standards-body-level name for the distinction this ADR needs: **access control** (who
  may read — ADR-018's existing role table) is orthogonal to **flow control** (whether the
  content may leave the trust boundary as a durable copy — this ADR's new axis).
- **Honest counter-example, cited so this ADR doesn't overclaim (Signal "View Once" /
  disappearing messages)**: a weaker, purely *temporal* analog rather than a role-based
  permission, and Signal's own issue tracker documents real client-side bypasses (a
  desktop client forwarding unopened view-once media before it is ever displayed). Cited
  here deliberately, not omitted, because it is the honest limit of what any client-
  enforced "no durable copy" control can guarantee: it stops the *casual*, default path to
  a full offline copy and defends against the realistic risk (an unencrypted CRDT dump
  sitting on a lost or stolen laptop, or a scripted bulk export), not against a fully
  malicious client screen-recording or manually transcribing content it legitimately read
  live. See "Limitations" below.

## Decision

Five phases, matching this project's established ADR-phase/issue convention (see
ADR-050's epic-with-lettered-phases precedent and ADR-062's own recent phased execution).

### Phase A — signed op-log field: `ReplicationPolicy`

Add `pub replication: ReplicationPolicy` (`enum { Full, QueryOnly }`, `#[default] Full`)
to `MembershipOp` (`shared/sync/src/membership.rs`), written only on `Admit`/`SetRole` —
**per-subject**, the same way role itself is assigned, not a KB-wide governance op (a
KB-wide "no one may replicate" toggle is a different, coarser control this ADR does not
propose; per-subject is the precise fit for "this specific viewer, not everyone"). Emit a
new canonical-bytes signing format (`"maememb/v5"`) *only* when `replication == QueryOnly`
is present, mirroring the existing disjoint-field version precedent already used for the
`v1→v4` evolution (`shared/sync/src/membership.rs:161-221`) — every existing `Full`-
implying op stays byte-identical `v1` and every existing signature keeps verifying
unmodified; this is a strictly additive schema change, not a breaking one. Thread the
field through `ValidMember` and `build_members`'s existing "latest valid op wins" overlay
(the same causal-order rule role assignment already uses), and through
`collection_oplog.rs`'s op-record codec (a new optional field, same pattern as the
existing `wrapped_key` field).

**Verification (adversarial).** (1) A pre-ADR-067 `v1` op with no `replication` field
must still verify under the unmodified signature it was originally signed with — a
genuine backward-compatibility regression test, not just a forward-compatibility default.
(2) A forged op flipping a member from `QueryOnly` back to `Full` **without the KB
owner's signature** must be rejected by `verify_signed`/the existing crypto-validity
filter — an attacker (including a compromised or malicious relay) cannot restore their
own replication rights by fabricating an op; only a genuinely owner-signed op can change
the policy. (3) **Replay resistance**: a stale, previously-valid `Admit(Full)` op for a
member, re-appended to the log *after* a later, causally subsequent, owner-signed
`SetRole(QueryOnly)` op for that same member, must lose under `build_members`'s causal-
order overlay — constructed as a real 3-op DAG (`Admit(Full)` → `SetRole(QueryOnly)` →
replayed stale `Admit(Full)`), asserting the final derived state is `QueryOnly`, not a
hand-picked 2-op case that could pass by only exercising the common path.

## Implementation note (Phase A, principle #15)

`ReplicationPolicy` (`Full`/`QueryOnly`, `#[default] Full`) added to `shared/sync/src/
membership.rs`'s `MembershipOp` as a plain field (not `Option`) — the same shape as
`can_invite: bool`, meaningful only on `Admit`/`SetRole`, harmlessly `Full` everywhere else.
Threaded through `ValidMember` and `build_members`'s existing causal-order "latest op wins"
overlay with the exact same semantics as `role`/`epoch`: a `SetRole` for an unrelated reason
must still carry the subject's intended replication value explicitly (no independent
"preserve unless mentioned" behavior — matches how `role` already works). `collection_oplog.rs`'s
encode/decode threads the field through storage (`OP_REPLICATION_KEY`, new const in `kb/mod.rs`);
`build_membership_op`'s constructor defaults it to `Full`, so all ~40 existing call sites across
the daemon/editor needed zero changes.

**A real design subtlety resolved during implementation, not glossed over**: the Decision text's
"mirroring the existing disjoint-field version precedent" framing undersells one real case —
`wrapped_key` (v2) and `replication` (v5) are **not** disjoint the way v3/v4 are. An `Admit` onto
an E2E-encrypted KB can carry BOTH a `wrapped_key` (so a `QueryOnly` member can still decrypt
what they're allowed to read live, via the future Phase C `kb/query.my_wrapped_key`) AND
`replication: QueryOnly` at once. `canonical_bytes()`'s v5 arm is therefore a strict **superset**
of v2's field layout (wrapped_key bytes first, in the same position v2 would put them, then the
replication marker) rather than a sibling version — confirmed by a dedicated test
(`adr_067_query_only_admit_can_still_carry_a_wrapped_key_bound_into_v5`) that tampering with
*either* field independently breaks verification, not just one.

Four adversarial tests added to `shared/sync/src/membership.rs`, matching this section's own
Verification bullets: `adr_067_full_replication_op_stays_v1_byte_identical` (backward-compat —
a `Full`-replication op, the default every pre-ADR-067 signature was created under, stays
byte-identical `v1` and verifies unmodified), `adr_067_forged_replication_downgrade_without_
owner_signature_is_rejected` (an op flipping `QueryOnly`→`Full` without a fresh owner signature
fails `verify_signed`), `adr_067_replayed_stale_full_admit_after_query_only_setrole_does_not_win`
(the 3-op replay-resistance DAG — verified as a genuinely meaningful test, not a vacuous one, by
confirming it would fail under a plausible bug class: if `build_members` trusted raw input-array
order instead of the causal `prev_hash` DAG, the duplicate admit re-processed last would
incorrectly restore `Full`), and the wrapped_key/v5-interaction test above.

`cargo test`/`cargo clippy --all-targets -- -D warnings`/`cargo fmt --check` clean across both
the editor and daemon workspaces; `cargo build --workspace --features gui` clean.

### Phase B — `kb_access` Join/Read split + `kb_join` enforcement

Split the identical match arm at `daemon/src/collab_handler/mod.rs:1184`. `KbOp::Read`
stays unconditional for any role, as today. `KbOp::Join` becomes conditional on the
member's `ReplicationPolicy`: denied when `QueryOnly`. Legacy/un-anchored KBs (no signed
op-log — role read from the plain `member_roles` YMap) have no `ReplicationPolicy` field
to consult at all; they fall back to today's `Full` behavior unconditionally — an
explicit, named scope boundary (see "Limitations"), not a silent gap, since retrofitting
tamper-evidence onto an unsigned map would be the exact spoofing risk Phase A's design
avoids.

**Verification (adversarial, N-way not 2-way).** (1) A single test constructs four
principals on one KB — Owner, Editor, `Full`-policy Viewer, `QueryOnly`-policy Viewer —
and has all four call `kb_join` in the same run: only the `QueryOnly` Viewer is denied,
with an error message distinguishable from a non-member's `join_policy`-driven denial (a
restricted member should never be told "you are not a member," which would be actively
misleading). (2) A `QueryOnly` member holding a **stale, locally-cached** `collection_
state`/session token obtained *before* the policy was set must not be able to resume
`sync_update` delivery by presenting it — the session-subscription step
(`kb_membership.rs`'s `add_event_sub`/doc subscription) must never fire for a request
that fails the Phase B gate, regardless of what the client claims to already hold. (3) A
**mid-session** policy change (the owner sets `QueryOnly` while the member already has a
live, subscribed session from an earlier, permitted `Full`-era join) does **not**
retroactively tear down that live subscription — stated and tested as an explicit,
named limitation of this phase (session revocation is a distinct mechanism this ADR does
not build; the policy governs future `kb_join` calls, not already-established sessions),
not silently assumed to be handled by the same code path.

## Implementation note (Phase B, principle #15)

`kb_access_with_coll` (`daemon/src/collab_handler/mod.rs`) now derives `(role, replication)`
from the **same** lookup rather than a second `derived_membership` call — the anchored/
op-log branch reads `m.replication` off the `ValidMember` Phase A already populates; the
legacy/un-anchored branch (plain `member_roles`, no signed op-log) defaults to `Full`
unconditionally, matching the ADR's own named scope boundary. The `QueryOnly`-Join denial
is checked *before* the general hierarchical RBAC match, specifically so its message
(`"member is restricted to live-query-only access for KB '...' and may not replicate it
locally (ADR-067)"`) is distinguishable both from a role-insufficiency denial and from the
non-member `"not a member of KB '...'"` denial `kb_join`'s callers already see — telling a
genuine, restricted member they're "not a member" would be actively misleading.

`handle_kb_join` (`daemon/src/collab_handler/kb_membership.rs`) needed no structural change
for the "no subscription on deny" property: its existing `Deny`/`Err` match arm already
returns early, before the `session_docs.insert`/`track_client_connect`/`bc.subscribe_doc`
steps further down the function — Phase B's gate reuses that same early return, so a
denied `QueryOnly` join was already guaranteed to never reach the subscription code, no new
plumbing required.

Three adversarial tests added (`daemon/src/collab_handler/tests/
collab_handler_replication_policy_tests.rs`), matching this section's own Verification
bullets and each independently confirmed to fail against the pre-fix `kb_access_with_coll`
(re-ran the suite with the fix `git stash`ed — all three failed as expected, then passed
again once restored): `query_only_viewer_denied_join_others_allowed_with_distinguishable_
message` (the 4-principal N-way case — Owner/Editor/Full-Viewer allowed, QueryOnly-Viewer
denied with a message distinguishable from a genuine non-member's, constructed by
temporarily setting the KB's join policy to `restrictive` since the default `Invite` policy
gives a non-member `Pending`, not `Deny`, which would make the two cases trivially
distinguishable by variant alone rather than by message content), `query_only_member_kb_
join_never_subscribes_the_session` (registers two real sessions via `EventBroadcaster::
subscribe` — required before `subscribe_doc` has any observable effect at all — then proves
via a real `broadcast()` + `try_recv()` that only the allowed session's channel receives the
event), and `mid_session_restriction_does_not_tear_down_an_already_live_session_but_blocks_
future_joins` (the named limitation from this section's Verification bullet 3, both halves:
an already-subscribed session survives a mid-session restriction, but a fresh `kb_join`
attempt by the same principal afterward is correctly denied). No RPC surface exists yet to
set `replication` on a member (out of this phase's scope — Phase B is the gate, not the
admin command surface), so the `QueryOnly` fixture member's op is built and signed directly
via `KbCollectionDoc::build_membership_op` + `append_signed_op`, mirroring what `kb/
add_member`'s handler does internally for every other field.

`cargo test`/`cargo clippy --all-targets -- -D warnings`/`cargo fmt --check` clean in the
daemon workspace (159 tests passing, up from 156).

### Phase C — `kb/query.my_wrapped_key`: narrow key delivery for `QueryOnly` E2E members

A new `kb_query.rs` dispatch method, gated by the **same, unmodified** `Read`-only
`check_kb_read_access` gate every other `kb/query.*` method already uses (a `QueryOnly`
member already passes `KbOp::Read` under Phase B, so no new access-control logic is
needed here — only a new, narrowly-scoped read). Server-side, it loads the collection and
calls the existing `find_wrapped_content_key(ops, anchor_owner_pubkey, my_fingerprint)`
(`shared/sync/src/membership.rs:1010`) — a function already pure, requiring no secret
key material server-side, and fingerprint-scoped *by construction* (it returns only the
wrapped blob from the latest owner-authored op targeting the given fingerprint). The
response carries only `{wrapped_key, epoch}` for the caller's own principal — never the
op-log, never any other member's entry, never the collection doc. This is a direct,
principle-#8-compliant reuse of existing, already-tested crypto plumbing, not new crypto
logic: the only genuinely new code is the dispatch wiring and response shaping.

**Verification (adversarial).** (1) A two-member fixture (both E2E members of the same
KB) confirms member A's request returns *only* A's wrapped key, never B's — proving the
fingerprint-scoping is enforced at this new endpoint, not merely assumed from the
underlying function's contract. (2) A non-member (never admitted to the KB at all)
requesting this endpoint gets a clean empty/`None` result, not an error whose message
would leak whether the KB or a given fingerprint exists — matching ADR-053's existing
non-member-facing error-shape discipline. (3) The same call against a genuinely
*unencrypted* KB returns an explicit "not applicable," never a null/garbage value that a
naive client might mistake for "here is your key, it's empty." (4) A member whose key was
**rotated** (the owner re-wrapped content after a `Rebind`, ADR-040) receives the latest
wrap, not the original stale one — reusing the existing rotation-test pattern already
proven at `derive_content_key_delivers_to_members_excludes_others_and_rotates`
(`shared/sync/src/membership.rs:1507`) rather than writing a parallel, potentially-
diverging test for the same property.

## Implementation note (Phase C, principle #15)

`my_wrapped_key` (`daemon/src/kb_query.rs`) was added as a new `kb/query.*` dispatch
method exactly as designed — gated by the same `load_gated`/`check_kb_read_access` prefix
every sibling method already uses, with zero new access-control logic. It returns
`{applicable: false}` (no `wrapped_key` field at all) for a genuinely unencrypted KB,
`{applicable: true, wrapped_key: <hex or null>, epoch}` for an E2E KB — `null` (not an
error) for a real member with no wrapped-key op targeting them yet, reusing
`find_wrapped_content_key` unmodified. A non-member's request is denied with the exact
same `McpError` message every other `kb/query.*` method already produces for the identical
`check_kb_read_access` failure — deliberately not a special-cased shape, so this new
endpoint doesn't let a stranger learn anything by diffing its denial against a sibling's.

Three tests added (`daemon/src/tests/kb_query_tests.rs`): a two-member E2E fixture proving
each member's response never contains the other's wrapped key (checked both at the
top-level field and via a raw-wire substring scan, not just field equality — the same
`hostile_hub_operator_cannot_search_an_e2e_kb_for_plaintext` discipline this file's other
tests already use); the unencrypted-KB `applicable: false` case; and the non-member
denial-shape-parity case (asserting `my_wrapped_key`'s error message is byte-identical to
`kb/query.get`'s for the same non-member). The rotation case (item 4 of this section's own
Verification) was deliberately NOT re-tested here — `find_wrapped_content_key` already has
a dedicated rotation test (`derive_content_key_delivers_to_members_excludes_others_and_
rotates`) and this endpoint adds no new logic on that axis, so a parallel test would only
duplicate coverage, not add any (principle #8).

`cargo test`/`cargo clippy --all-targets -- -D warnings`/`cargo fmt --check` clean in the
daemon workspace.

### Phase D — client-side path: self-pointing `RemoteHub` + OAuth self-scoped tokens, and closing the mTLS gap

Two real prerequisites make Phase D genuinely new work, not free reuse:

1. **`RemoteHubQueryLayer`/`RemoteHubConfig` (`shared/kb/src/remote_hub.rs:65-276`,
   shipped this session as ADR-062 Phase D) has no baked-in assumption that `base_url`
   points at a distinct, external hub.** A `QueryOnly`-restricted member's own client can
   register the *same* KB as a `RemoteHub`-kind instance pointed at their **own** daemon's
   OAuth listener (`hub_kb_id` = the real KB id), reusing the exact "always live-query,
   never locally replicate" mechanism ADR-062 already built and adversarially tested,
   rather than inventing a second client-side code path for the same guarantee. The
   editor's `kb_join` command, on receiving Phase B's new denial, should surface a clear,
   actionable message and offer to auto-register this self-pointing instance instead of a
   bare failure.
2. **This requires a JWT whose `principal_claim` resolves to the member's own
   `SHA256:...` Ed25519 fingerprint** — real new work: extend `daemon/src/oauth.rs` (or a
   minimal, narrowly-scoped new token-issuance path) so a daemon can mint or accept a
   token bound to a principal already present in its own signed membership log, rather
   than assuming an external, pre-existing IdP relationship for every member. **This
   bridging is a deliberate, bounded translation at the one boundary that needs it — not
   a step toward replacing Ed25519/mTLS peer identity with OAuth as MAE's primary auth
   model.** Ed25519 remains the load-bearing identity substrate across the collaboration
   architecture (the signed op-log's tamper-evidence *is* the signing key; E2E content
   keys, ADR-037/041, wrap directly to member Ed25519 public keys with no OAuth-native
   equivalent; the P2P mesh, ADR-025, uses Ed25519 node IDs as the transport identity with
   no central server involved at all) — real, established precedent for exactly this
   split confirms it is the robust pattern, not a stopgap: **Tailscale** uses WireGuard
   keys for the actual peer/tunnel identity and OAuth/SSO only for its admin/coordination
   plane; **Teleport** and **HashiCorp Vault's SSH secrets engine** use OAuth/SSO to
   *issue short-lived SSH certificates*, never to replace key-based peer authentication
   itself. Phase D's token issuance is the same shape: OAuth authorizes *who gets a
   credential* for this one external-facing surface; the Ed25519 key remains the actual
   peer identity everywhere else.

Phase D also **wires `kb_query::dispatch` into the mTLS collab handler**, closing the gap
found during this ADR's own research (ADR-053's Decision-1 prose claims mTLS reachability
that was never actually implemented) — without this, a `QueryOnly` member on an
mTLS-only, no-OAuth deployment would have literally no path to read the KB they were
granted Read access to, which would make this ADR's whole feature unusable for exactly
the local-first, no-central-IdP deployments principle #12 treats as the floor.

**Verification (adversarial).** (1) An expired or revoked token must produce a real,
observable 401/auth-failure over the wire — reusing `remote_hub.rs`'s existing
`last_outcome()`/`LastOutcome::AuthFailed` pattern and its real-daemon e2e test harness
(`daemon/tests/remote_hub_query_layer_e2e.rs`) rather than inventing a parallel
verification style for what is structurally the same property. (2) A token whose `sub`
does **not** match any fingerprint in the KB's own signed membership log must be denied at
`kb_access`, never silently treated as a permissive non-member case (conflating "unknown
token subject" with "non-member, apply join_policy" would be a real access-control bug,
not a cosmetic one). (3) **The ADR-062 Hard Rule, applied specifically to the self-
pointing case** — the scenario most tempting to silently special-case into a local Cozo
read instead of a genuine live query, since "it's my own daemon" invites exactly that
shortcut: force the KB's live content to change between two `kb/query.search` calls made
through the self-pointing `RemoteHub` instance, and assert the second call's result
reflects the new content, never a value cached from the first — proving the self-pointing
integration inherits ADR-062's "never mirror, always query live" guarantee rather than
quietly regressing it because the hub happens to be reachable locally.

### Phase E — the retroactive-restriction limitation, made visible, not silently accepted

No code in this ADR can delete an already-replicated member's local disk copy — this is
structurally impossible (MAE has no remote-wipe mechanism over a peer's local storage, and
building one is explicitly out of scope: it would be a different, much larger and more
invasive feature with its own threat model) and not attempted. Instead: an owner-visible
signal, surfaced through the existing `kb_audit`/`kb_health` introspection surface,
reporting how many members hold a pre-restriction full replica — distinguished, using the
signed op-log's own timestamps against the member's actual join event, from members who
were restricted *before* ever joining (no residual-copy risk) versus those restricted
*after* already replicating (a real, named residual risk the owner should be able to see,
even though this ADR cannot eliminate it).

**Verification (adversarial).** The signal must correctly distinguish the two cases
against a constructed real timeline fixture — a member who joined, then was later
restricted (residual risk = true), interleaved with a member who was restricted before
ever attempting to join (residual risk = false) — not a single hand-picked case that
happens to be unambiguous.

## Implementation note (Phase E, principle #15)

A real gap surfaced during implementation: the signed op-log records **granted policy
over time** (Admit/SetRole ops), not **replication events** — there is no `kb/join`
success ledger anywhere in the system (`DocStore::track_client_connect` is a liveness
heartbeat with no history). So "the member's actual join event," as this section's
Decision text originally phrased it, isn't literally derivable. The honest, implementable
signal actually built is a sound **bound** on that question rather than a direct answer:
`had_full_replication_window` (`shared/sync/src/membership.rs`) walks a member's own
Admit/SetRole history in causal order and asks "was this member ever granted `Full`
before their current `QueryOnly` restriction?" — `Some(true)` means a real window existed
during which their client had permission to `kb_join` (a residual local copy is a live
possibility, even though this can't confirm one exists), `Some(false)` means they were
restricted from their very first `Admit` and could never have legitimately replicated,
`None` means not applicable (currently `Full`, or no signed op-log at all — the same named
scope boundary as Phase B's legacy-KB fallback). A convenience wrapper,
`had_full_replication_window_self_anchored`, resolves the anchor from the op-log's own
self-consistent genesis rather than requiring an externally-supplied anchor pubkey — every
other caller in this module threads a securely pre-established anchor specifically to
defend against a malicious relay's forged genesis, but this signal is a soft,
owner-facing UI hint computed from the owner's OWN locally-held collection replica, not an
access-control decision, so that defense doesn't apply here.

Wired into `KbSharingSnapshot`/`MemberView` (`crates/core/src/kb_sharing.rs`) as a new
`residual_replica_risk: Option<bool>` field, computed once per KB (not per member) from
the collection's own `oplog_ops()`. Because the `*KB Sharing*` buffer, the
`kb_sharing_status` MCP tool, and the `(kb-sharing-status)` Scheme primitive all already
share one `build_snapshot` (CLAUDE.md #3/#8), this single change gives the signal parity
across the human buffer, the AI peer, and user Scheme scripts for free — no per-surface
plumbing needed. The buffer's member row appends a short annotation ONLY for the
real-risk case (`Some(true)`); a restricted-but-never-at-risk member or a currently-`Full`
member gets no extra text at all, so the signal can never read as a false alarm.

One adversarial test (`crates/core/src/kb_sharing_tests.rs`) builds a real signed op-log
via `KbCollectionDoc::build_membership_op`/`append_signed_op` with four members
interleaved on one timeline — Alice (Admit(Full) → SetRole(QueryOnly), residual risk =
true), Bob (Admit(QueryOnly) directly, residual risk = false), and Carol (plain Full
editor, not applicable at all) — asserting all three outcomes simultaneously (not a single
hand-picked case) at both the snapshot-field level and the rendered buffer-text level (the
annotation appears for Alice, and is absent from both Bob's and Carol's rows).
`crates/core/Cargo.toml` gained a new dev-dependency on `mae-mcp` (for `Identity::generate`
— constructing a real signed op-log needs a real Ed25519 identity, the same reason the
daemon crate's own test suite already depends on it).

`cargo test --workspace`/`cargo clippy --all-targets -- -D warnings`/`cargo fmt --check`
clean across the whole editor workspace.

## Consequences

**Positive.** Closes a real, currently-uncovered gap (issue #448) using an established,
industry-precedented permission axis (access vs. flow control, NIST AC-4) rather than an
ad hoc invention. Reuses ADR-053's existing live-query surface and ADR-062's existing
`RemoteHubQueryLayer` rather than building a second "read without replicating" mechanism
— the query-only client path a restricted member ends up on is *literally the same code*
a non-member thin client already uses, just self-pointed. Reuses `find_wrapped_content_
key`'s already-correct, already-tested crypto logic rather than adding new key-handling
code. Closes a real, independently-found gap in ADR-053's own mTLS-reachability claim as
a side effect of Phase D.

**Costs (honest).** This is new, security-relevant surface (a new signed op type, a new
key-delivery endpoint, new token issuance) — every phase above carries adversarial tests
specifically because a subtle bug here (e.g. Phase A's replay case, Phase C's fingerprint
scoping) would be a real access-control or key-leak vulnerability, not a minor bug. Phase
D's OAuth-token-issuance work is genuinely new scope, not a reuse of existing machinery.

**Explicit named limitations (stated here, not left implicit or discovered later):**

- **Un-revocable local replica.** Once a member has replicated a KB, `SetRole(QueryOnly)`
  cannot delete their disk copy — this is a *forward-looking* access control (governs
  future `kb_join` attempts), not retroactive DLP. Same fundamental limitation every real
  precedent cited above shares (Salesforce/Confluence/Google DLP also cannot un-export an
  already-downloaded file).
- **Legacy/un-anchored KBs cannot enforce this at all** — they have no signed op-log to
  carry the policy in, and retrofitting one is out of this ADR's scope (would need its
  own migration design). An explicit, stated scope boundary: this feature is anchored-KB-
  only.
- **mTLS-only deployments need Phase D's wiring to use this feature at all** — until
  Phase D lands, a `QueryOnly` restriction on an OAuth-disabled deployment leaves the
  restricted member with no read path whatsoever. Phase D is not optional polish; it is
  required for the feature to be usable outside OAuth-enabled deployments.
- **Op-log growth.** Every `SetRole` toggling a member's `ReplicationPolicy` is a
  permanent, never-pruned entry in the signed op-log (`collection_oplog.rs`'s own
  existing PERF/DOGFOOD note on unbounded log growth). A flip-flopping admin adds to an
  already-tracked concern; this ADR does not introduce a new pruning mechanism, it adds
  to the load on an existing, already-acknowledged one.
- **Client-side enforcement is not tamper-proof against a fully malicious client**
  (the honest Signal "View Once" caveat above) — a determined, modified client could
  still screen-record or manually transcribe content it legitimately reads live. This
  control's real, defensible value is preventing the *casual*, default path to a durable
  offline copy (an unencrypted CRDT dump on a lost laptop, a scripted bulk export via the
  ordinary client) — the same honest scope every cited real-world precedent operates
  under, not an overclaimed guarantee this ADR cannot actually deliver.

## Alternatives rejected

- **An unsigned flag on the collection doc instead of the signed op-log.** Rejected —
  spoofable by a key-blind relay or compromised hub, breaking exactly the trust guarantee
  ADR-018/026/037 exist to provide for precisely the sensitive KBs this ADR is meant to
  protect. The whole point of a query-only restriction is defeated if an untrusted
  intermediary can silently strip it.
- **Retroactively revoking/wiping an already-replicated member's local CRDT state.**
  Rejected — technically infeasible without a remote-wipe mechanism MAE does not have and
  this ADR does not propose building, and arguably inappropriate: the content was
  legitimately readable at the time of replication. This is forward-looking access
  control, not DRM: named as a limitation (Phase E), not solved by force.
- **Requiring every deployment to be OAuth-reachable as a permanent precondition**,
  instead of also wiring `kb/query.*` into the mTLS path. Rejected — Phase D closes this
  gap rather than accepting an OAuth-only deployment requirement as permanent, which
  would make the feature unusable for exactly the local-first, no-central-IdP
  deployments MAE's own architecture treats as the default floor (principle #12).
- **Switching MAE's primary identity model from Ed25519/mTLS to OAuth**, considered and
  explicitly rejected during this ADR's design (not merely unconsidered): Ed25519 is the
  load-bearing substrate for the signed membership op-log, E2E content-key wrapping, and
  the P2P mesh's peer identity, none of which OAuth natively provides equivalents for.
  Phase D's OAuth-token bridging is a bounded translation at one surface, following the
  same pattern real, mature key-based-auth systems (Tailscale, Teleport, Vault's SSH
  secrets engine) already use — OAuth issues scoped credentials at a boundary, it does
  not replace the underlying peer-identity system.

## Verification

Per-phase adversarial tests are specified above and are the actual DoD for each tracked
GitHub issue (epic + phases A–E). Cross-cutting, checked once all five phases have
landed: an end-to-end scenario — an owner sets a viewer to `QueryOnly` on a fresh,
anchored, E2E-encrypted KB; the restricted viewer's `kb_join` attempt is denied with a
distinguishable message; their client auto-registers a self-pointing `RemoteHub` instance;
`kb/query.my_wrapped_key` delivers their wrapped key; `kb/query.get`/`search` against real
content succeeds and reflects live edits made by the owner after the restricted viewer's
last query, with no local replica ever created on the restricted viewer's machine —
verified by inspecting their local KB registry/data directory for the absence of any
locally-materialized copy of this KB's content, not merely by the client's own self-report
that it didn't replicate.
