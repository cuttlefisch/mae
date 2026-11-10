# ADR-113: Publication review — readers see approved revisions, writers keep a live draft

**Status:** Proposed (design). Nothing here is built.
**Relates to:** ADR-106 (node version history is a local audit trail — revisions follow the same
placement), ADR-101 / ADR-103 (agent-made links held for human review — becomes a case of this
ADR), ADR-098 (durable identity; its service-account binding is where an agent principal is
marked), ADR-111 (remote clients over HTTPS; member-signed ops, #727), ADR-053 (the `kb/query.*`
read surface this extends), ADR-109 (resource budget; attachments get their own cap), ADR-018
(KB roles).

## Context

A hosted KB edited through a browser is about to carry controlled documents: procedures and work
instructions that a team must follow, where a reader has to see the version that was approved and
put into force, not whatever is being typed into it now. The same KB is co-edited live (ADR-099,
ADR-100), and agents write to it (ADR-103). Today a reader of `kb/query.get` sees the working copy,
which is the only copy.

The requirements, as the browser client's project states them (V1–V9):

- **V1** A document moves draft → in review → approved → in force → historical. Approval and entry
  into force are distinct, each a recorded act. Some sets of documents go into force on approval.
- **V2** A reader is served the in-force revision (or the approved one, for a set that goes into
  force on approval). Earlier revisions stay readable, marked historical, with their dates.
- **V3** The approver is never an author of the revision, and an agent never approves. The server
  enforces both.
- **V4** The approval unit is one document or a named set, which is published atomically: a set of
  three with one unapproved publishes nothing.
- **V5** Each approved revision carries a document code, a revision number, its frozen source, a
  rendered and optionally a signed PDF, and its approval record (who, when, in which role). All
  are reachable from the published page.
- **V6** Before entry into force, an approval can be withdrawn. After it, only a new revision
  supersedes it. Nothing published is edited in place.
- **V7** The PDF is rendered from the frozen revision; the signed PDF is stored on the revision.
- **V8** A writer sees, per document, where it is in the flow.
- **V9** A reader's structure (folders, kinds, tags) comes from published revisions only.

### Prior art

This is document control, and its rules are long settled:

- **ISO 9001:2015 §7.5.3** requires documented information to be approved for adequacy before
  issue, its changes controlled, and obsolete versions kept from unintended use while retained.
  That is V1, V2 and V6.
- **21 CFR Part 11** (US FDA, electronic records) requires computer-generated, time-stamped audit
  trails that do not obscure earlier entries (§11.10(e)), and that each signature show the
  signer's name, the date and time, and its *meaning* — review, approval, responsibility,
  authorship (§11.50). That is V5's "in which role", and the reason the event log below is
  append-only and hash-chained.
- **Separation of duties** (NIST SP 800-53 AC-5, the "four-eyes" rule) is V3.
- **SharePoint's content approval** (major/minor versions, drafts visible to authors and approvers
  only, readers see the last major version) is the editing model the client wants: the working
  copy is a minor version, an approved revision a major one.

## Decision

### D1. The working copy stays the CRDT; revisions live outside it

Co-editing does not change. The node's CRDT document is the working copy, and it is always the
draft. Revisions, the published pointer, review requests and their events are **daemon tables in
the daemon's SQLite database**, outside the CRDT — the same placement ADR-106 gives node history,
for the same reason: they record what happened on this hub, not shared state to converge. The
existing backup (`VACUUM INTO`) covers them with no change.

Not signed CRDT ops, for now, because the inputs to the decision are not peer-verifiable yet:
approvers authenticate by OIDC rather than by member keys (member-signed ops over HTTPS wait on
#727 and ADR-111 D3), and whether a principal is an agent is daemon state (ADR-098's
service-account binding). D3 below is enforceable by the daemon today and by no one else.

**v1 is hub-local**: mesh peers do not replicate revisions or the pointer. **v1 serves plaintext
KBs only**: for an E2E KB (ADR-037) the daemon cannot freeze readable source, so every method here
refuses with `e2e_unsupported`.

### D2. A revision is frozen when review is requested

`kb/review.request` captures each node's source at the `version` the requester names: an
immutable, content-addressed blob (sha256) of the normalized org text plus the materialized
fields — exactly the fields `KbNodeDoc::version` digests. If the working copy no longer has that
version, the request is refused (`stale_version`, with `data.current`), so a requester freezes
what they saw. The approver approves exactly what was submitted; the working copy keeps moving.

A revision is `(kb, node, rev_no, doc_code, source_sha256, version, state, created_by,
created_at)`, never edited. Its states are in_review → approved → in_force → historical, or
in_review → rejected | withdrawn. **Editing an in-force document only changes the working copy**,
which is the next draft (V6).

The **document code** (PRO-n, IT-n, REG…) belongs to the node, not the revision, and is stable
across revisions. Each KB has a list of permitted prefixes; the number is unique per prefix
(`doc_code_taken`). Revision numbers (Rev00, Rev01…) count the node's approved revisions.

### D3. Who may act — enforced by the daemon

- **Authors.** A revision's author set is the requester plus every principal who edited the node
  since its **last revision that reached approved or in force** — not since the previous revision,
  or an editor whose work went into a rejected request would drop out of the next one built on the
  same text. This needs the daemon to record who applied each update, which it does not today
  (`DocStore::apply_update` takes no principal): v1 adds a per-document editor record written in
  `apply_update`. The same record is D's working-copy history (D7).
- **Review is a per-KB switch**, owner-only, and every change to it is an event in the same chain
  (D7). With it off (the default, and what a personal KB wants), the reader
  is served the working copy as today and `kb/review.*` refuses (`review_not_enabled`). With it on,
  everything below applies.
- **Reviewers and approvers** are explicit per-KB lists set by the owner, **fail-closed**: a
  principal not on the list cannot review, approve, put into force or withdraw an approval
  (`not_reviewer`, `not_approver`).
- **No author acts on their own revision**: `approve`, `act` and `enter_into_force` by anyone in the
  author set → `author_cannot_approve`.
- **No agent acts at all**: a principal marked as an agent is refused even when listed
  (`agent_forbidden`).
- **Policy cannot be loosened by a requester.** `in_force_on_approval` defaults from the KB policy.
  A request may override it only toward the stricter setting (false); loosening it is owner-only
  (`role_forbidden`).
- `required_acts` is per-KB policy and defaults to `approved` alone: one approver who is not an
  author is enough. An owner may require more acts (e.g. a separate `reviewed`), each by a
  principal who is not an author.

### D4. Sets publish atomically

A review request is the approval unit: one or more revisions, a name, and its
`in_force_on_approval` mode. The published pointer, `published[(kb, node)] = rev_id`, moves for
**every** member of a request in **one SQLite transaction**, and only when the whole set
qualifies: under publish-on-approval, when the last member is approved; otherwise when the set's
entry into force is recorded, which requires every member approved (`set_incomplete`). The
previous in-force revision of each node becomes historical, keeping its dates.

**Entry into force** is its own act (principal, time, `effective_at`), by an approver who is not
an author. **Derogation** ends a document's force without a replacement — an obsolete procedure:
an approver who is not an author records it as an act, the in-force revision becomes historical,
and the node leaves `view: "published"`. **Withdrawal**: the requester may withdraw a request before approval (the revision becomes
`withdrawn`). Before entry into force, any listed approver who is not an author — not only the one
who approved — may withdraw an approval, and the revision returns to `in_review`. After entry into
force only a new revision supersedes (`invalid_transition`).

### D5. The read path

- `kb/query.get` gains `view`: `"working"` (the default — today's behaviour, unchanged) or
  `"published"` (the in-force revision, or the approved one under publish-on-approval); or
  `revision: "<rev_id>"` for any revision, historical included. A revision response adds
  `revision: {rev_id, doc_code, rev_no, state, approved_at, in_force_at, superseded_at,
  acts: [{act, principal, at}], attachments: [{kind, sha256, size, content_type, url}]}` — V5's
  five items from one call.
- `version` keeps its meaning: a digest of the fields returned. For a revision it is the frozen
  digest and never changes.
- `kb/query.list` gains `view: "published"`: only nodes with a published revision, each with the
  same summary fields as the working view (title, kind, tags, properties, …) taken from that
  revision, its `version`, and its revision summary, with the same paging, byte order and parameter
  rules. A reader builds its structure from published revisions alone.
- `kb/query.revisions {kb_id, node_id}` lists a node's revisions with their dates.
- **Deleting** a node (to the trash) is refused while it has a published revision — in force, or
  approved under publish-on-approval — with `invalid_transition` (`data.act: "delete"`) until it is
  derogated; otherwise deletion would let an editor who is not an approver end a document's force.
  A node with only historical revisions, or none, is deleted as usual and leaves
  `view: "published"`; its revisions stay readable by `rev_id`, for the record. **Purging** a node removes its working copy and leaves its revisions — approved, in force
  or historical — with their attachments and events: they are the document-control record, and the
  event chain (D7) could not verify without them. A node with an in-force revision cannot be
  purged (`invalid_transition`) until it is derogated, so ending a document's force stays an
  approver's recorded act and never a side effect of deletion.

### D6. Attachments

A revision holds content-addressed attachments: `rendered_pdf` and `signed_pdf`. They are uploaded
and downloaded over HTTPS on the OAuth listener (`PUT`/`GET
/kb/<kb>/revisions/<rev_id>/attachments/<kind>`), not as base64 in JSON-RPC, under their own cap,
`max_attachment_bytes`, separate from ADR-109 D6's node cap (`quota_exceeded`). The client renders
the PDF from the frozen source and uploads it with the `source_sha256` it declares it rendered; the
daemon refuses a declared source that is not the revision's, records the declaration for audit, and
does not inspect the PDF's content. Only an approver may upload the
signed PDF, including after entry into force (the signature often comes later). The daemon records
who uploaded each attachment and when, and does not verify PDF signatures in v1.

### D7. The record is append-only and tamper-evident

Every request, act, transition and upload is a row in an append-only event log, hash-chained
(`prev_hash`) and signed with the daemon's identity key, so the trail can be exported and checked
offline (21 CFR Part 11 §11.10(e)). A reserved `member_sig` column takes the actor's own signature
once members sign their own operations (#727).

Together with the per-document editor record (D3), this **is** the version-record work the browser
client calls D: revisions and events for published versions, the editor record for the working
copy. ADR-106's local node history is unchanged.

**Agent edits held for review (ADR-101/103's "applied, pending review") become a case of this
ADR.** An agent's edit changes only the working copy; readers never see it until a human approves
a revision containing it. "The change-maker cannot clear the flag" is D3's author rule; "an agent
never clears it" is `agent_forbidden`.

### D8. Methods and errors

Writer methods, on the OAuth listener: `kb/review.request`, `kb/review.act`
(`act: "reviewed"`), `kb/review.approve`, `kb/review.reject`, `kb/review.withdraw`,
`kb/review.enter_into_force`, `kb/review.derogate`, `kb/review.status`, `kb/review.pending`, and the owner-only
`kb/review.set_policy` and `kb/review.set_code`.

`kb/review.status` reports per document: **draft** — defined exactly as *the working copy's
`version` differs from the latest revision's frozen `version`* — in review (waiting for whom),
approved, in force (since when). `kb/review.pending` lists open requests; the client diffs the
frozen source against the published one.

Refusals carry `error.data.kind`: `invalid_params`, `role_forbidden`, `not_reviewer`, `authors_unknown`,
`not_approver`, `author_cannot_approve`, `agent_forbidden`, `stale_version` (+`current`),
`invalid_transition` (+`from`, `act`), `set_incomplete`, `doc_code_taken`, `unknown_request`,
`node_not_in_kb`, `quota_exceeded` (+`size`, `limit`), `e2e_unsupported`, `review_not_enabled`.

## Tests that must exist

Each fails without the rule it guards:

- X edits; Y requests; it is rejected; Y requests again; X approves → `author_cannot_approve`.
- A listed approver marked as an agent approves → `agent_forbidden`; an unlisted human →
  `not_approver`.
- A set of three with one unapproved: no pointer moves, and `view: "published"` is unchanged for
  all three.
- Editing an in-force document: `view: "published"` returns the same body and `version` before and
  after; `status` turns to draft.
- A requester sets `in_force_on_approval: true` against a KB default of false → `role_forbidden`.
- A rendered PDF whose `source_sha256` is another revision's → refused.
- Withdraw after entry into force → `invalid_transition`; an approval withdrawn before it, by a
  different listed approver, returns the revision to `in_review`.
- Turning review on or off by a non-owner → `role_forbidden`; by the owner, an event in the chain.
- Derogation: the node leaves `view: "published"` and its revision stays readable by `rev_id`.
- Deleting an in-force node → `invalid_transition` with `act: "delete"`, and it stays in
  `view: "published"`; after derogation the delete succeeds.
- Purging a node with historical revisions leaves every revision and the event chain verifiable.
- With review off, `view: "published"` is the working copy and `kb/review.request` →
  `review_not_enabled`.
- The event chain verifies end to end, and fails to verify after any row is altered.
- (A1) Text submitted ahead of another member's edit, and integrated by it, is credited to both
  submitters, whether it integrates through an edit or through a share.
- (A3) Approval is refused with `authors_unknown` for a document whose record starts after the
  cutoff, for a cutoff past the document's high-water mark, and when an unsigned relay or the daemon
  wrote in the window.
- (A2) A share, a restore or an edit racing the freeze never yields content and cutoff that disagree.
- (A4) The imported act is unreachable from `kb/review.*` and from an agent principal.

## Policy decisions

1. Approvers are an explicit per-KB list set by the owner, fail-closed.
2. An approver who is not an author is enough; three distinct people are not required. Review
   itself is a per-KB switch, because a personal KB needs none of this.
3. The first KB to use this is plaintext, so v1's `e2e_unsupported` costs nothing yet.

## Amendment 1 (2026-11-09): authorship as built, and seeding

Building the editor record (D3's prerequisite) and the native-KB seeder, and three rounds of
independent review against them, settled details D3 left open. They are recorded here so the review
methods are built against what exists.

### A1. Who counts as an author

The daemon records, per document, every party that applied a change to it (`DocStore`'s editor
record), atomically with the write. Every content write path must say who it is; there is no default.

- **A submitter is an author** of everything in its update. The record names who *submitted* an
  update, not who wrote each piece of content it carries: an update can bring another client's text
  the daemon never saw. Until the hub verifies content-op signatures on every write (#727), the
  submitter is accountable for all of it. This is a stated limit, not a guarantee.
- **A relay is recorded as a relay.** An edit forwarded by another daemon is recorded as that daemon
  (a mesh peer), and, when the edit carries a content-op signature the daemon verified, the signer
  is recorded as an author too. Recording a replayed old signature can only add an author, which
  blocks an approval rather than permitting one; that is the safe direction.
- **Content held back is credited when it appears.** A CRDT update that depends on content not yet
  received is stored pending and becomes visible later, when the missing content arrives. Its
  submitter is recorded again at that moment, including when a share or restore installs it, so text
  planted ahead of someone else's edit cannot appear under only their name.
- **Unauthenticated sessions** share one record per document, since they have no stable identity.

### A2. The review cutoff is a WAL position, not a time

A revision's author window is everything after the moment it was frozen. That moment is the
document's WAL high-water mark, read together with the content under the same lock as every write
and every share or restore (`state_with_high_water`). A wall-clock cutoff was rejected: a clock that
steps back, or two edits in the same millisecond, would hide an author. Timestamps on the record are
for display only.

### A3. When authors are unknown, approval is refused

`authors_since(revision cutoff)` answers either the set of authors or **unknown**. Unknown when:

- the record for the document is incomplete before the cutoff (documents that existed before the
  record did, or whose content was replaced by a restore or migration, start a new record there);
- the cutoff lies beyond the document's high-water mark (a cutoff from another document or another
  store);
- any change in the window came from a writer that is not an identified person: the daemon itself,
  an unsigned relay, or an unauthenticated session.

`approve`, `act` and `enter_into_force` refuse with `author_cannot_approve` when the approver is a
known author, and with **`authors_unknown`** when the set is unknown. Approval is never decided on
an incomplete record.

### A4. Seeded content enters as an imported, in-force revision

A KB created by seeding from org files (the operator's `mae-daemon kb seed`) is published at seed:
each seeded page becomes its first revision, **in force**, through an **imported** act. The act is
distinct from approval, because no approver acts. It is recorded in the chain with:

- the operator who ran it and when;
- the source repository, revision, commit and tree, and the seed plan's hash;
- per page, its content hash, its creation date (the first commit in which a file carrying its
  `:ID:` appeared) and its last modification at the source revision;
- `effective_at` = the source revision's commit time.

Only an operator can perform it, through the CLI; it is not a `kb/review.*` method, and no agent
principal can reach it. Every change after the seed goes through review. The source files are input
only: the server's KB is the truth from then on.

## Consequences

- A reader's view of a controlled KB stops being the live working copy. A KB with no review policy
  is unaffected: `view` defaults to `"working"`.
- The daemon gains durable state that mesh peers do not have. Replicating it waits on
  member-signed operations; until then a published revision exists on the hub that approved it.
- `apply_update` gains a principal, which every caller must supply. That is a change to the write
  path, reviewed as one.
- Attachments add a binary store with its own cap to the resource budget (ADR-109).

## Rejected

- **Revisions as CRDT state.** A revision would converge like any other edit, which is the opposite
  of frozen, and V3's checks would rest on data a peer cannot verify yet.
- **Approval at the moment of approve, not at request.** The approver would sign off on text that
  can change between reading it and approving it.
- **"Author" as the requester only.** A co-author who did not file the request could approve their
  own work, which V3 forbids.
- **A wall-clock author window** (Amendment 1, A2). A clock stepping back, or an edit in the same
  millisecond as the freeze, would drop an author.
- **Seeded pages as unreviewed drafts** (Amendment 1, A4). The reader would serve nothing until
  every page of an already-reviewed source was approved again.
