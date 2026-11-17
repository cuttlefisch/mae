# ADR-117: Access administration — caller-scoped reads, signed origins, and a local directory sync

**Status:** Proposed (design). Nothing here is built. Phased (P0–P9, below).
**Depends on:** ADR-098 and its Amendment 1 (the binding table, the typed principal, `person` /
`agent`, the session group gate, the local `binding` CLI), ADR-026 (the signed, hash-chained
membership op-log), ADR-018 (roles, join policy, `kb_access`), ADR-040 (`Rebind`), ADR-113 with its
Amendments 2 and 3 (review lists, the event chain, `kb/review.*`, the second factor at the moment of
acting).
**Extends:** ADR-026 (an origin on every grant, `maememb/v7`), ADR-113 (D8 below: a review request
freezes its policy; lists accept only people).
**Relates to:** ADR-074 Amendment 1 (`kb_changed` / `access_lost` come from membership writes),
ADR-099 Amendment 2 (bound IdP sessions on `/ws`), ADR-036 Amendment 1 (the hub verifies what it
accepts; mae-crypto signs typed ops), ADR-111 (remote clients over HTTPS), ADR-116 (history and
event chains), CLAUDE.md principle #16.
**Source of the requirements:** a browser client's administration design (its views, its account
lifecycle and its list of what it needs from MAE). The owner's decisions on it are marked
*(owner, 2026-11-15)*; its recommendations adopted here are marked *(adopted)*.

## Context

A browser client is about to let people see and administer access to KBs on a hosted daemon. It
needs three views: **my pending work** (what I must review or approve, in every KB), **my access**
(what I hold and why), and an **owner's administration page** for one KB. It also needs a way for
an owner to change membership from the browser, and an organisation needs access to follow its
directory, where people join and leave groups, without anyone re-typing membership.

What MAE has, checked against the code on `origin/main` (2026-11-16):

| Piece | Where | State |
|---|---|---|
| `(iss, sub)` ↔ fingerprint, one to one, with `kind` and `bound_by` (`proof` / `admin`) and `bound_at` | `daemon/src/identity_binding.rs` (own SQLite file, read on every request, writable while the daemon serves) | Built. Holds **current state only**: an unbind deletes the row and leaves no trace. |
| Typed principal; an unbound IdP subject is a member of nothing | `oauth_self_issue::ValidatedPrincipal`, `oauth::unbound_answer` | Built. The principal carries `issuer`, `subject`, `credential`, `kind`; **not** the token's groups, `amr`, `acr` or `auth_time`. |
| Session group gate | `oauth::check_allowed_groups` | Built. One list for the whole daemon. |
| `kb/query.my_kbs` | `daemon/src/kb_query.rs` | Built: the caller's KBs and role, nothing about anyone else. `kb_query::dispatch` receives the caller as a fingerprint string, not as the validated principal. |
| Membership writes | `kb/add_member`, `kb/remove_member`, `kb/approve_member` (owner, mTLS); `kb/collection_op` (an owner-signed CRDT update, mTLS, opaque to the daemon); `mae-daemon kb member` (daemon-owned KBs, **daemon stopped**) | Built. None reachable over HTTPS. |
| Op-log fields | `mae_sync::membership::MembershipOp` | Has `author`, `issued_at`, `expires_at` (the derivation already drops an op past it), versioned canonical bytes `maememb/v1`–`v6`. **No origin, no reason.** |
| Gate source | `collab_handler::access::member_role` | The signed op-log for an anchored KB; the unsigned `member_roles` map otherwise, which has **no expiry**. |
| mae-crypto | `shared/crypto` v0.1.0 | Member key, identity challenge, content AEAD and wrap. **No typed membership op**: the browser cannot sign one without signing raw bytes. |
| Review lists, `kb/review.*`, the event chain | ADR-113 | Designed, not built. `kb/review.pending` is per KB. |

Two constraints decide most of what follows:

- **Controls that bound authority are not on the agent's surface** (principle #16). Membership,
  review lists, the person flag and the directory mapping are such controls. Over the network they
  are reachable only by a *person* session (ADR-098 Amendment 1 C3), and the operator's parts only
  locally.
- **The daemon never signs membership for someone else** (ADR-098 D4). It signs only for KBs whose
  owner *is* the daemon's identity, as `kb member` already does.

## Decision

### D1. Three caller-scoped reads, and nothing about another principal to anyone without authority

All three run on the HTTPS listener under the caller's own token. Each answers about the caller, or
about a KB the caller owns; none takes a principal as a parameter.

**`kb/review.my_pending {cursor?, limit?}`** — the caller's review work across every KB where
`kb_access` allows them Read and review is enabled (ADR-113 D3), in one paged list:

- `to_act`: each open request where the caller may act now: KB, node, document code, revision,
  title, the capacity (`review`, `approve`, `publish`, `accept`), **why it is theirs** (`named`, or
  `group` with the group the current token carries), `requested_at` and `turn_since` (when the
  previous required act completed);
- `blocked`: requests where the caller is a performer but cannot act yet, each with a reason code
  (`author_of_revision`, `awaiting_prior_act`, `reauthentication_required`, `agent_forbidden`) —
  shown, never hidden;
- `waiting_on_others`: requests the caller filed or is an author of, with the outstanding acts as
  the frozen policy states them (a group, or a named member as the policy names them) and since
  when. **Never the members of a group**: the daemon does not know them, and they are the
  directory's.

Group eligibility is read from the token of the call (ADR-113 C1): a group gained or lost shows on
the next token. An `agent` caller gets `to_act: []` and `kind: "agent"`, so a client can say why.
It replaces nothing in ADR-113: `kb/review.pending` stays the per-KB list for the KB's own view.

**`kb/query.my_access {}`** — the caller's access with its sources:

- per KB in `my_kbs`'s set: the role, its **origin** (D2), and which source the gate read it from
  (`oplog` or `roster`); an entry whose expiry has passed is absent, as it is from the gate;
- the review lists the caller is on, by name; the group-performed capacities the current token
  satisfies in each KB's policy; whether that KB's acts require a second factor, and the window;
- join requests the caller made that are still pending (only KBs the caller asked to join);
- the **binding**: `iss`, `sub`, `bound_by`, `bound_at`, `kind`, and the current session's
  sign-in (`amr`/`acr`/`auth_time`, and whether it meets the configured second-factor rule);
- for an unbound IdP subject: `binding: null` and the count of KBs the last directory pass found it
  entitled to but could not admit for want of a key — a count, no ids.

A KB where the caller is blocked or not a member is absent, as from `my_kbs`: one rule, not two.

**`kb/members.list {kb_id}`** — owner only (`kb_access` Manage) **and** a person session
(`person_required` otherwise; principle #16 — it is the read side of a control, and it carries other
people's identities):

- per member: fingerprint, label, role, origin, expiry, and — where bound — the `iss`, `sub`,
  display name, `kind`, `bound_by` and `bound_at` of its binding; members whose expiry has passed,
  listed as expired;
- pending join requests with the requester's binding and fingerprint;
- open directory proposals for this KB (D5) and the time of the last directory pass that covered it;
- open review requests blocked on a named performer who no longer has access (for D8's reassignment);
- the KB's membership and review history are read through D7 and ADR-113 D7, not duplicated here.

The binding table gains a `display_name` column, written from a configured claim (default `name`)
when a binding is made by proof, refreshed at each sign-in that carries the claim and by the operator
CLI *(owner, 2026-11-16)*. It is personal data the daemon did not hold before, so it is shown only to
the KB's owner (this list) and the auditor (D9), never to other members, and it is removed with the
binding. It is a label, never an identity: nothing authorizes on it.

### D2. Every grant carries its origin in the signed op-log, and an expired grant does not count

An `Admit` or `SetRole` op gains an **origin**:

- `directory`: the stable group id that grants the role, and the directory pass that produced it
  (D5);
- `manual`: who granted it is already the op's `author` and when is its `issued_at`; the op adds
  the **sha256 of a mandatory reason**. The reason's text (at most 500 bytes, NUL-free) is kept in a
  daemon table outside the CRDT, keyed by that hash *(owner, 2026-11-16)*. An optional expiry is the
  op's existing `expires_at`.

The origin is part of the signed bytes, as a new canonical version **`maememb/v7`**: the fields
`v5`/`v2` would emit, then the origin fields. It is emitted only when an origin is present, so every
existing op stays byte-identical and every existing signature verifies — the strictly additive rule
`v2`–`v6` each followed. An op without one reads as `origin: unrecorded`. Every new write path
(D4, D5, the operator CLI) records one, and refuses a `manual` grant without a reason. The existing
mTLS `kb/add_member` takes an optional reason and records `manual` with or without it, so native
editors that do not send one yet keep working; the owner's page shows "no reason recorded". The
roster mirrors origin and expiry beside the role, so an un-anchored KB, which has no signed op to
carry them, still reports and enforces them.

**One entry, one origin per fingerprint per KB.** A later op replaces the entry's role and origin;
there is no second entry to compare. A directory entry never "extends" a manual one, or the reverse.

**Expiry is honoured on both sources.** The derivation already drops an op past `expires_at`. The
unsigned roster an un-anchored KB is gated on gains the same expiry, mirrored from the op, and
`member_role` treats a passed expiry as no membership on either branch, so `kb_access`,
`my_kbs`, `my_access` and content verification (ADR-036 F2) agree. Once ADR-074 Amendment 1's change
index exists, the maintenance scheduler writes a `(kb, changed)` notice when an expiry passes, so
open streams send `access_lost` at the expiry, not at the next unrelated write.

**Visibility, stated:** the op-log is replicated to every member peer, so it carries only the
reason's hash: a reason can be sensitive and must not reach every member's copy. The text is read
only by the KB's owner (D1) and the auditor (D9), and the signed hash proves the text the daemon
shows is the one the grant was made with. A daemon that lost the text still has a verifiable record
that a reason existed. A replica that is not the hub sees the hash only.

**Directory-managed KBs take no manual grants** *(owner, 2026-11-15)*. A KB the mapping file names
(D5) whose owner is the daemon's identity is directory-managed: `kb member add` refuses with
`directory_managed`, and so does every network path. Temporary access is a directory account with
its own expiry, placed in the KB's group. Removing a manual entry (one predating the mapping) only
subtracts, and stays allowed. A personal KB the mapping names is not directory-managed: its owner
remains its authority, and the origin is provenance there, not a lock.

### D3. Nobody grants anything to themselves

*(The owner's requirement, 2026-11-15, generalised.)* Every method that changes a permission refuses,
with `self_grant`, a change whose beneficiary is the caller:

- membership: `kb/membership.submit` (D4), `kb/add_member`, `kb/approve_member`, and the ops inside a
  `kb/collection_op` update (the daemon already scans them for `Rebind`s);
- review: adding oneself to a reviewer or approver list, naming oneself as a performer, reassigning
  an act to oneself (D8), and an agent ceiling or grant (ADR-113 C9) whose beneficiary is the
  caller's own principal;
- the beneficiary is compared as a principal, and a principal's `Rebind` predecessors and successors
  count as the same principal. A `Rebind` or a recovery-key registration is not a grant: it moves
  what the principal already holds and stays allowed.

**Removing something from oneself is allowed** (`kb/leave`, demoting oneself, leaving a list): it only
subtracts.

What this adds over the op-log's own attenuation, stated honestly: a member already cannot grant
above their own role, so on membership the check is mostly defence in depth with an explicit refusal.
It is load-bearing where attenuation says nothing: replication policy (ADR-067), review lists,
performers, reassignment and agent grants. A person who controls a second, unbound key can still
have it admitted by someone else; that key is an `agent` (ADR-098 C3) and D8 keeps it off every list.

**An owner approves in their own KB only by a directory group or because the operator put them on
the list** *(adopted)*, and never what they authored (ADR-113 D3). The operator's path is a local
command (`mae-daemon review list add`), recorded in D7's operator log.

### D4. A browser owner changes membership by signing the op; the daemon verifies and appends it

New HTTPS method **`kb/membership.submit {kb_id, op, sig, author_pubkey}`**: `op` is a typed
`MembershipOp` (Admit, Remove, SetRole) with its origin; the caller's browser built it and signed its
canonical bytes with the member key it holds. The daemon:

1. requires a **person** session (`person_required`) with a fresh second factor (D6);
2. checks the signature, that `author_pubkey` hashes to `author`, and that `author` is the session's
   principal (`not_author`, ADR-036 F4's rule applied to membership);
3. requires the KB to have an op-log and anchor (`oplog_required` otherwise: an un-anchored KB's
   owner anchors it first, ADR-036 F1), `prev_hash` to be the current head (`stale_head`, with the
   head, so the client re-signs), and `issued_at` within the listener's clock-skew bound;
4. runs the **same derivation** that gates access with the op appended and refuses an op that would
   not take effect (`op_not_effective`): capability, attenuation and timebox are judged by the one
   implementation, never a second copy;
5. applies D2 (origin present; `manual` with a reason; `directory` only against an open proposal
   for this KB, subject and role — D5) and D3;
6. on an E2E KB, refuses an `Admit` without a `wrapped_key` (`wrap_required`); the browser holds the
   content key as owner and wraps it in WASM (ADR-098 C4); the daemon cannot check the wrap and does
   not try;
7. appends the op with `append_signed_op` — the owner's signature, never the daemon's — mirrors it
   into the roster in the same write, and the collection write records `(kb, changed)` (ADR-074 C1).

Pending joins are approved the same way: the owner signs the `Admit`.

**mae-crypto needs a typed op; v0.1.0 has none.** The canonical encoding of `MembershipOp` moves
into mae-crypto (`membership_op`), and mae-sync re-exports it, as it already does `content_crypto`,
so the browser, the editor and the verifier run one encoder. The WASM API signs only a parsed op,
never caller-supplied bytes, and returns the op's fields for the client to show before signing —
the rule `identity_challenge` follows. This is mae-crypto **0.2.0**, released with the same signed
provenance as 0.1.0. ADR-036 F5's `sign_op` for content ops is its sibling, not this.

### D5. The directory sync is a local operator command

*(Owner, 2026-11-15:)* membership that follows a group is produced by a **sync** into the signed
op-log, not by checking groups at each request; review and approval capacities are the group in the
token at the moment of the act (ADR-113 C1); the deployment-wide session gate stays as it is.
*(Owner, 2026-11-15:)* the sync **applies** in KBs owned by the daemon's identity and **proposes** in
personal KBs, for their owner to sign.

`mae-daemon directory sync [--apply]`:

- **Inputs, all local.** A mapping file in the daemon's configuration (`kb_id` → `[{group_id,
  group_label, role}]`), versioned by the operator; a **snapshot** of the mapped groups' members as
  `(iss, sub)`, printed as JSON by a command the operator configures. That command holds the
  directory's **read-only** credential; MAE holds none, embeds no directory API, and never puts a
  secret in argv. Groups are named by stable id, never by display name (ADR-098 D4).
- **Dry run by default.** Without `--apply` it prints and records the diff and changes nothing.
- **Resolution.** A subject in several mapped groups of one KB gets the highest role; the origin
  names the group that grants it (ties: the smallest group id). A subject is resolved to a member
  through the binding table: unbound → not admitted, counted as "pending key"; bound `agent` →
  never receives anything from the directory.
- **Daemon-owned KB, `--apply`:** Admit, SetRole and Remove ops, origin `directory`, signed by the
  daemon as owner, as `kb member` already signs. Only `directory` entries are changed. A legacy
  `manual` entry for a subject a group now covers is replaced by the directory entry (D2: one
  origin); the replaced op stays in the log. A daemon-owned KB that is E2E-encrypted is refused and
  the refusal recorded: a sync that wraps a content key for every new member makes the host the
  key's distributor, and directory-managed access and confidentiality from the host do not combine.
- **Personal KB:** proposals only — rows in a daemon table `(kb, subject fingerprint, action, role,
  group, pass)`, outside the CRDT, shown to the owner by `kb/members.list` and signed by them through
  D4. The next pass supersedes a proposal it no longer makes. The daemon never signs here.
- **Fails closed.** If the snapshot lacks any group the mapping names, the pass applies nothing in
  any KB and records why.
- **Risky changes are held for a person's approval, in the browser** *(owner, 2026-11-16: no
  arbitrary threshold and no CLI flag to override; configurable, with strict baselines, and approved
  in the GUI)*. Each pass splits a KB's diff into changes it applies and changes it **holds**. A held
  change set is a daemon-table row `(kb, pass, changes, why held)`, recorded in D7's operator log,
  superseded by the next pass that no longer makes it.
  - **What is held** is a per-KB policy the owner sets (D6: a person, fresh second factor), within a
    deployment baseline the operator sets locally that no KB can loosen. The **built-in baseline**,
    which applies until anyone sets one, is strict: hold every removal and every role change, and
    apply only additions. Stricter rules an owner may add: hold additions too; hold any change that
    touches a member who holds a review or approval capacity (ADR-113 C1) or the owner role. Looser
    rules (apply removals automatically up to a count or share of the KB's directory entries) are
    available only where the operator's baseline permits them.
  - **Who approves.** In a personal KB, its owner (who signs anyway, D4). In a daemon-owned KB, the
    principals the operator names in a local `directory.approvers` group. An approval or rejection
    is `kb/directory.decide {kb, pass, decision, reason}`, a person act under D3 and D6, recorded
    with its sign-in details; the daemon then applies an approved set exactly as computed, after
    recomputing it under the collection lock (a set that no longer matches the current state is
    refused, `held_set_stale`, and the next pass re-proposes it). Agents never approve.
  - **What the GUI shows.** `kb/members.list` returns the KB's held sets with each change and why it
    was held; the operator CLI shows the same for a deployment, and can approve only where the
    operator is also a named approver (no operator override path).
- **Runs while the daemon serves.** Membership writes need the daemon's store and signer, which
  the serving daemon holds under the store lock. The command therefore enqueues the pass (mapping,
  snapshot and flags, with their sha256) in its own SQLite file beside the binding table, and the
  daemon executes it on its maintenance tick — the binding table's precedent: a separate file both
  may write. The file has the binding table's permissions and boundary: no network method, MCP tool
  or Scheme primitive writes it (principle #16), and whoever can write it can already run the
  operator CLI. The daemon recomputes the diff under each collection's lock at execution, so nothing decided
  against stale state is written; the command waits for and prints the result. With the daemon
  stopped, the command runs the same function itself under the store lock (`open_exclusive`). One
  function, two callers.
- **Every pass is recorded** in D7's operator log: who ran it, when, the mapping and snapshot hashes,
  per-KB counts, each op's `chain_hash`, each proposal, and what could not be done.
- **Cadence** is the operator's (a timer). The target is a pass interval no longer than the token
  lifetime, so a removal by sync is no slower than one by the session gate; it is measured in a
  pilot, not enforced. The owner's page shows the last pass time and flags it past the operator's
  `directory.max_pass_age_secs`.

**The `person` flag, in batch** *(adopted)*: `mae-daemon directory mark-people [--apply]` sets
`kind = person` on `proof` bindings whose subject is in the configured people group of the snapshot,
and back to `agent` for those no longer in it. Local, dry-run by default, each change in D7's binding
log. Never over the network.

**A subject that changes** *(adopted)*: when accounts move to a new directory and their `sub`
changes, `mae-daemon binding move-subject` rewrites one binding's `(iss, sub)` atomically, keeping
its fingerprint, kind and history, so memberships are untouched (ADR-098 D3's whole point).

### D6. Administrative acts need a person, and a fresh second factor

Every administrative act over HTTPS — `kb/membership.submit`, a review list or policy change,
reassignment (D8), an agent ceiling or grant (ADR-113 C9) — requires a **person** session whose
sign-in meets the second-factor rule **within the freshness window**, and refuses otherwise
(`reauthentication_required`, `mfa_required`). This is ADR-113 C4's window and B7's accepted values,
one configuration and **one checker** for approving and for administering:
`oauth.second_factor.{accepted_amr, accepted_acr, max_age_secs}`. With none configured, every
administrative act is refused (`second_factor_unconfigured`), as B7 fails closed. There is no
per-KB switch to turn it off for administration: the network administration surface is new, and a
stolen browser session that could add a key without a second factor is the attack it exists to stop.
mTLS methods are unchanged except as ADR-113 C4 already rules for review-enabled KBs.

**This needs ADR-098's sign-in follow-up, and states its minimum:** `ValidatedPrincipal` carries the
token's groups (`groups_claim`), `amr`, `acr` and `auth_time` for an IdP credential, and
`kb_query::dispatch` and the new methods receive the whole validated principal rather than a
fingerprint string. ADR-113 Amendment 3 needs the same thing; it is built once (P0).

### D7. The record: a binding log, an operator log, and binding that follows `Rebind`

**The binding log.** `identity-bindings.db` gains an append-only, hash-chained table written **in the
same transaction** as each change: `bind` (proof or admin), `unbind`, `set_kind` (before, after),
`move_subject`, `follow_rebind`, each with the time, the fingerprint(s), and who: the proof route,
the daemon, or the local CLI with its OS user and the command. Without it nobody can later say who was
a person when they approved; an act's own record (ADR-113 B4) is no substitute for the binding's
history.

**The operator log.** Local commands that change permissions — `token mint`, `kb member`,
`binding *`, blocklist changes, `directory *`, `review list *` — append an event (who, when, the
command, its effect) to an append-only, hash-chained operator log beside the binding table. Where the
effect lives in the same database the event is in the same transaction; where it has no store of its
own (`token mint`), **the event is written before the effect** and a failed write means no token.
The daemon signs both logs' heads into its own chain on a schedule, as ADR-116 D4 signs history
heads. The operator remains the deployment's trust anchor and can still edit files on disk; the logs
make that visible, not impossible.

**Binding follows a verified `Rebind`.** When the daemon accepts a `Rebind(old → new)` that the
derivation honours, on any path (`kb/collection_op` already computes the pairs), and `old` is
bound, the binding
moves to `new` without the operator: same subject, same kind, a `follow_rebind` event. It does
**not** move, and logs `rebind_unresolved` for the operator, when `new` is already bound to another
subject, or when the KBs the daemon holds derive different successors for `old` (ADR-040's
compromise fork: a stolen old key racing the recovery key). Following the derivation, never a second
signature check, keeps one judge of what a `Rebind` means.

### D8. Review lists accept people, members may read the policy, and a request freezes it

These extend ADR-113 and are to be cited by its next amendment.

- **Lists accept people only.** `kb/review.set_policy` refuses a named reviewer, approver or
  performer whose fingerprint is not a member of the KB or is not bound with `kind = person`
  (`performer_not_person`). A binding later set back to `agent` keeps its place but cannot act
  (`agent_forbidden` at the act, as now) and is flagged on the owner's page.
- **Members read the policy.** `kb/review.policy {kb_id}` under `kb_access` Read returns the KB's
  review switch, per-type acts, performers, ordering, distinct-people rule, and the second-factor and
  rendered-PDF switches. Members see performers as roles, groups and labels only; named members'
  fingerprints and bound IdP identities are returned only to the owner and the auditor
  *(owner, 2026-11-16)*.
- **A request freezes its policy.** `kb/review.request` stores the resolved requirements — acts,
  performers, order, distinct-people, `in_force_on_approval`, the second-factor and PDF switches —
  with the revision, beside the frozen type (ADR-113 C1). Each act is judged against the **stricter**
  of the frozen and the current requirements, so a later change can tighten an open request and can
  never cheapen it. *Who* is eligible — list membership, groups in the token, KB membership, kind — is
  read at the act, so a removal from a list takes effect at once.
- **Reassignment** *(adopted)*: when a named performer can no longer act (gone from the KB, unbound,
  or `agent`), the owner names another for that act with a reason (`kb/review.reassign`), under D3
  and D6, as an event in the chain. It is the only change a frozen request accepts. Acts already
  performed stand. Nothing is reassigned automatically, and a group-performed act never blocks.

### D9. A read-only auditor, by group, in the daemon's configuration

*(Adopted.)* The browser client has no administrator of its own. Someone who must see everything is a
MAE auditor: a person session (a binding flagged `person`) whose token carries one of
`audit.groups`, set only in the daemon's local configuration, with a sign-in meeting the
second-factor rule. The auditor may read, for every
KB: `kb/members.list`'s content, the membership op-log with origins, the review event chains, the
binding log, the operator log and the directory pass records — never node content. Every auditor
method is a read; the auditor role grants nothing in `kb_access` and cannot be combined into a write.
Each auditor call is itself an event in the operator log.

### D10. A notification outbox, and a credential that can only read it

*(Owner, 2026-11-15: notifications appear inside the browser client for now; e-mail and chat come
later.)* `my_pending` is the in-client inbox. The outbox is designed now so the later phase needs no
new daemon design:

- When a review event makes an act someone's turn, the daemon appends `(seq, iss, sub, reason:
  "review_turn", at)` to an outbox table **in the same transaction** as the event (ADR-074 C1's
  outbox shape), once per request, step and subject. It carries no KB, title or document code.
- Named performers resolve through the binding table; group performers through the last directory
  snapshot that covered the group. No snapshot, no notification — `my_pending` still shows the work.
- `notify/outbox.read {after, limit}` is the only method a **notify credential** reaches: a token the
  operator mints locally whose principal is a typed `service:notify`, which `kb_access` never
  matches and every other method refuses. The reader keeps its own cursor; the outbox is append-only
  and trimmed by an operator retention period.

## Consequences

- The browser client can build its three views and its administration page on methods that answer
  only about the caller, or about a KB the caller owns, under the caller's own token. It stores no
  permission of its own.
- Membership on a hosted daemon is changed over HTTPS only by an op its owner signed, so ADR-026's
  peer-verifiability and ADR-098 D4 both hold. Organisation KBs are owned by the daemon and follow the
  directory; personal KBs stay their owners'.
- The op-log gains `maememb/v7`. Old peers that do not know `v7` cannot verify new grants; the
  version is introduced the way `v2`–`v6` were, and peers are upgraded before an owner writes one.
- The operator gains local work (the mapping, the sync timer, the people flag) and two logs to keep.
  That is where it belongs.
- A group change still writes nothing in MAE by itself: a removal by directory reaches a KB at the
  next pass, and the session gate cuts whoever leaves the organisation at the next token.

## Rejected

- **Group → role checked at each request for membership** (the alternative to D5's sync). A role that
  is not in the signed log is invisible to peers, cannot verify a signed edit (ADR-036), cannot wrap
  an E2E key, leaves no record in MAE of who had access when, and its removal waits for the token.
  Kept for review capacities, where the act happens on a fresh token and lives only in the daemon.
- **The daemon signing membership in any KB from the directory.** A daemon that signs for an owner is
  indistinguishable from the owner (ADR-098 D4).
- **Explicit per-KB denials.** A second list to maintain and to forget someone on. Urgent removal is
  the operator's existing blocklist.
- **A browser-client administrator with powers.** It would hold a credential able to change
  membership for everyone: the confused deputy the client's design refused.
- **Sending a raw CRDT update from the browser (`kb/collection_op`).** It would put yrs and the
  collection document into the browser, and the daemon cannot judge an opaque update; a typed op it
  verifies and appends is smaller and checkable.
- **Unbind-then-bind for a key rotation.** It needs the operator for every lost laptop, and loses the
  kind; D7's follow is what ADR-098's "add a device costs nothing" needs for rotation too.

## Verification

Each test fails without the rule it names; each is listed in the repo's mutation list with the text
whose removal must make it fail.

- **No cross-principal leak.** For each D1 method, a raw-response-byte scan finds no fingerprint,
  `sub` or label of any principal except the caller — for a member, a non-owner, an unbound subject
  and an agent; `kb/members.list` by a non-owner is refused, and by an `agent` owner session
  `person_required`.
- **my_pending.** A request the caller authored is `blocked: author_of_revision`, never `to_act`; a
  group-performed act is `to_act` with a token carrying the group and absent with one that does not;
  a KB the caller cannot read contributes nothing.
- **Origin.** A `v7` op round-trips and verifies; every pre-`v7` fixture op's bytes are unchanged; an
  op whose origin was altered after signing fails verification; a manual grant without a reason is
  refused on every new write path; the legacy mTLS `kb/add_member` without one records "no reason".
- **Expiry.** A manual grant past its expiry is denied by `kb_access`, absent from `my_kbs` and
  `my_access`, refused by content verification, on an anchored **and** an un-anchored KB; an open
  stream gets `access_lost` at the expiry with no other write.
- **Self-grant.** Each method in D3 refuses the caller as beneficiary, including through a `Rebind`
  successor; leaving and demoting oneself succeed; a QueryOnly member cannot make itself Full.
- **Submit.** Refused: a signature by another key; `author` ≠ session (`not_author`); a stale head; an
  op the derivation would not honour (over-attenuated, non-owner remove); an E2E Admit without a wrap;
  a `directory` origin with no matching proposal; an `agent` session; a stale `auth_time`. Accepted:
  an owner's signed Admit, which then appears in the derived membership of a second daemon holding
  the same log, and produces `kb_changed`. A browser-signed (mae-crypto WASM) and an editor-signed op
  for the same fields have identical canonical bytes.
- **Sync.** A snapshot missing one mapped group applies nothing anywhere; an empty group trips the
  hold policy under the built-in baseline (every removal and role change held, additions applied);
  a KB policy can tighten but not loosen the operator's baseline; an approved held set applies
  exactly; a stale one is refused `held_set_stale`; a rejection applies nothing; an agent or a stale
  sign-in cannot decide; an unbound subject is not admitted; an `agent` binding never is; a personal KB
  gets proposals and no op; an E2E daemon-owned KB is refused and recorded, an E2E personal KB gets proposals; `--apply` absent changes nothing;
  a pass enqueued while the daemon serves is applied by the daemon and recorded with its hashes; a
  diff computed against state that changed before execution is recomputed, not replayed.
- **Directory-managed.** `kb member add` and `kb/membership.submit` with a manual origin are refused
  in a directory-managed KB; removing a legacy manual entry succeeds.
- **Logs.** Each permission-changing command or method, run once, appends exactly one event; a
  binding change whose event write fails leaves the table unchanged; `token mint` with an unwritable
  operator log prints no token; altering any log row breaks verification.
- **Rebind follow.** An honoured `Rebind` moves the binding with its kind; a successor already bound
  elsewhere, or two KBs deriving different successors, moves nothing and logs `rebind_unresolved`.
- **Lists and policy.** An `agent`-bound or non-member performer is refused at `set_policy`; members
  read the policy, non-members are refused; lowering the required acts after a request does not let
  the request pass with fewer, and raising them does apply; a performer removed from the list cannot
  act on a request frozen before the removal.
- **Auditor.** A token with the audit group reads every KB's member list and logs and is refused
  every write; without the group, or without the second factor, it reads nothing; its reads are
  logged.
- **Outbox.** A turn change appends exactly one entry and a rolled-back event appends none; the notify
  credential reads the outbox and is refused every other method, raw-byte-scanned like D1.
- **Agent surface.** No MCP tool, Scheme primitive or network method reaches `directory *`,
  `binding *`, `review list *` or the operator log's writes (principle #16), checked against the
  generated tool and method lists.

## Open for the owner

1. **Outbox retention** (D10): proposed default 30 days, operator-configurable.
2. **The auditor's second factor** (D9): proposed a second factor at sign-in, not a fresh one, since
   the auditor only reads.

Decided 2026-11-16 and written in above: the sync's hold policy (D5), reasons as a hash with the text
in the daemon (D2), display names stored for the owner and auditor only (D1), members see labels
only in the policy (D8).

## Phases

Each is one PR, built on the previous, each with its tests from Verification.

- **P0 — the caller context.** `ValidatedPrincipal` carries groups, `amr`, `acr`, `auth_time`;
  dispatch receives it whole; one second-factor checker and its configuration (D6). Shared with
  ADR-113 Amendment 3.
- **P1 — the logs.** The binding log, `move-subject`, `display_name`, the operator log and its events
  for existing commands, signed heads (D7, D5's last paragraph).
- **P2 — origins.** mae-sync `maememb/v7`, reason and origin on every write path, expiry on both
  sources and its scheduled notice, `directory_managed` refusal (D2).
- **P3 — the reads.** `kb/query.my_access`, `kb/members.list` (D1).
- **P4 — browser membership.** mae-crypto 0.2.0 typed op and WASM signer; `kb/membership.submit`;
  D3's self-grant rule on every existing membership method (D3, D4, D6).
- **P5 — the directory sync.** Mapping, snapshot command, the pass queue, apply/propose, proposals on
  the owner's page, the pass record, `mark-people` (D5).
- **P6 — binding follows `Rebind`** (D7).
- **P7 — review side**, built with ADR-113's methods: `kb/review.my_pending`, `kb/review.policy`, the
  frozen policy, people-only lists, reassignment, `review list add` (D1, D3, D8).
- **P8 — the auditor** (D9), then **P9 — the outbox and the notify credential** (D10), when the later
  notification phase is scheduled.
