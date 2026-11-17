# ADR-113: Publication review — readers see approved revisions, writers keep a live draft

**Status:** Proposed (design). Nothing here is built. Amendment 2 (2026-11-15): attachments are
write-once and retained; deletion only under a retention policy; an approval can require the
rendered PDF's hash and a second factor. Amendment 3 (2026-11-15): approval policy per document
type, separate approve and publish acts, and what the record shows.
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

## Amendment 2 (2026-11-15): a revision's attachments are its record

A deployment keeps the signed original of an approved revision in MAE as the official copy of a
controlled document: the exact bytes, never regenerated. D6 stores attachments, but leaves open
whether one can be replaced, how long it is kept and who may remove it. This amendment closes
those, and builds on ADR-115 for storage and on ADR-098 Amendment 1 C3 for who counts as a person.

### B1. An attachment is written once

A revision holds at most one attachment of each kind. A `PUT` for a kind the revision already holds
is refused with `attachment_exists` and changes nothing, whoever sends it and whatever the revision's
state. The one exception is a retry: the same bytes (same sha256) answer success, so an interrupted
upload can be repeated safely. A new signed copy is a new revision (D2), never a replacement.

**A wrong file is annulled, not replaced** (decided 2026-11-15). The owner, or the principal who
uploaded it, records an `annul_attachment` event with a mandatory reason. The annulled bytes stay
stored and readable, marked annulled with the reason, and the kind takes one new upload, under the
same rules as the first. The read path lists both, in order, so the record shows the mistake and its
correction; nothing is ever overwritten. An annulled attachment counts as referenced for retention
(B5), exactly as a current one.

Every `PUT` carries the sha256 the client computed (`expected_sha256`). The daemon hashes what it
received and refuses a mismatch with `upload_digest_mismatch`, storing nothing. That is an integrity
check on the transfer, separate from `attachment_exists`.

### B2. Two kinds, and the daemon stores bytes

- `rendered_pdf` — the client's rendering of the frozen source, unsigned, kept for the record,
  with its declared `source_sha256` checked as D6 already requires.
- `signed_pdf` — the signed original.

The list is closed: another kind is another amendment. Both kinds are PDFs: the content type must be
`application/pdf` and the bytes must begin with `%PDF-`, or the upload is refused
(`attachment_type`). That keeps the store to what it says it holds; it is not a judgement of the
document. Both kinds share one cap, `max_attachment_bytes` (D6), set high enough for a scanned
signed original.

The daemon assumes no signing method, handwritten-and-scanned or a digital signature embedded in the
PDF alike, and verifies no signature (as D6). It records what was uploaded and by whom; it does not
judge it.

*Future work, not in this amendment:* a digital signature embedded in a PDF is appended as an
incremental update, so the signed PDF begins with the exact bytes it signed. When a `signed_pdf`
begins with the revision's `rendered_pdf` byte for byte, the daemon could record that link; when it
does not (a scan never will), it records nothing.

### B3. Who may attach

- `signed_pdf`: a principal on the KB's approver list (D3) who counts as a **person** under
  ADR-098 Amendment 1 C3 (an IdP sign-in on a binding flagged `person`). Every other principal is
  refused: `not_approver` off the list, `agent_forbidden` for an agent even when listed. Until native
  clients sign in through the IdP (ADR-111 P4), this means a browser session.
- `rendered_pdf`: as D6 (the requester's client renders and uploads it), in any state except
  `rejected` and `withdrawn`.
- `signed_pdf` only to a revision that is approved, in force, superseded, derogated or imported:
  never to one still pending, so no revision rejected later can carry a "signed original" of
  something never approved. A signature often comes after entry into force, which this allows.

### B4. What is recorded

Each attachment's sha256, size, content type, uploader principal, the uploader's kind, the IdP
identity bound to that principal at the time (`iss`, `sub`), and time go into the revision record
(returned by D5's read path) and into the event chain (D7) as an `attach` event. An exported trail
shows which bytes were attached and by which person, even if the binding changes later.

### B5. Retention, and the only deletion

- **A superseded revision keeps its attachments.** ADR-115's sweep never collects a blob a revision
  references (its D5), and purging a node leaves its revisions (D5 above). Nothing in this ADR or
  ADR-115 removes them as a side effect.
- **Deletion happens only under a retention policy**, a minimum period after a revision is
  superseded or derogated (decided 2026-11-15):
  - the operator sets a deployment-wide floor in the daemon's local configuration, never over the
    network;
  - an owner may set a KB's period longer than the floor, never shorter;
  - with no policy set, nothing is ever deletable;
  - every policy change is an event in the chain (D7);
  - a shorter period applies only to revisions superseded after the change, never retroactively.

  Until the period has passed, nothing deletes the attachment.
- **After it, deletion is an owner act**, one attachment at a time, recorded as a `delete_attachment`
  event carrying the deleted sha256 and size. The revision record stays, with the attachment marked
  deleted, so the trail still says what existed. An approved revision that is in force cannot lose
  its attachments by any means.
- **The evidence outlives nothing it proves.** The event chain (D7), and any validation data kept
  with an attachment, is never pruned or compacted while any revision it proves is retained, so a
  retained document can always be verified against its record.
- **The daemon offers no other path.** An operator with access to the disk can still remove files;
  the event chain then shows a referenced blob missing, which the read path reports rather than
  hides (ADR-115 D6's resolver states).

### B6. Existing signed originals come in with their history

A deployment that already keeps signed originals elsewhere must not start with an empty record.
An import creates past revisions in the `superseded` (or `derogated`) state, each with its frozen
source, its dates, its `signed_pdf` and, where one exists, its `rendered_pdf`, plus an `import`
event naming who imported it and from where. It extends A4, which seeds only the in-force revision.
It is an owner act, and it is refused for a node whose history already holds revisions past the
imported dates, so an import can fill the past but never rewrite it.

### B7. The approval is the signature

For a controlled document, the evidence that it was approved is the approval record itself, not an
image on the PDF (decided 2026-11-15). The record holds who approved, when, and the exact bytes they
approved, and the approver signed in with a second factor. Two per-KB switches, owner-set, off by
default (a personal KB wants neither):

- **`approval_requires_rendered_pdf`.** `approve` carries `rendered_pdf_sha256`, which must equal the
  revision's current `rendered_pdf` attachment (B1). The act records it, so the approval binds to
  bytes, not only to a source revision. With no rendered PDF attached, or a different hash, the act
  is refused (`rendered_pdf_required`, `rendered_pdf_mismatch`). Annulling that `rendered_pdf` after
  approval is refused: an approval's bytes stay what was approved.
- **`approval_requires_mfa`.** The approver's request must come from an IdP session whose sign-in
  used a second factor, as ADR-098 reports it (the token's `amr`, or an `acr` level). The accepted
  values are the operator's configuration, with no built-in default, because they differ by IdP and
  are measured, not assumed; with the switch on and nothing configured, every approval is refused
  (fail closed). The act records which claim and value satisfied it. Otherwise it is refused
  (`mfa_required`). A self-issued token or an editor session never
  approves anyway: it is not a person (B3).

`signed_pdf` stays as designed (B1–B6): the record of a signature collected by other means, and the
form imported history takes. The approval record is what proves approval.

ADR-098 owns reading the claims: the validator exposes how the person signed in (`amr`, `acr`, and
whether that meets the configured second-factor rule), and this ADR owns requiring it for `approve`.

### B8. Reading

An attachment is read under the KB's own access (`kb_access` Read, ADR-115 D2), as D6.

### B9. Tests this amendment adds

- A second `PUT` of different bytes for a kind the revision holds is refused and the stored
  attachment is unchanged; the same bytes again succeed.
- A `PUT` whose bytes do not match `expected_sha256` is refused and stores nothing.
- A non-PDF (wrong content type, or bytes not starting `%PDF-`) is refused for both kinds.
- `signed_pdf` on a pending revision is refused; on approved, in force, superseded, derogated and
  imported it is accepted.
- An import creates superseded revisions with their signed originals and an `import` event, and
  is refused when it would land before revisions the node already has.
- With `approval_requires_rendered_pdf`: approve without a rendered PDF, or with another hash, is
  refused; with the matching hash it records it; annulling that rendered PDF afterwards is refused.
- With `approval_requires_mfa` and no accepted values configured, every approval is refused.
- With `approval_requires_mfa`: an IdP token without a qualifying `amr`/`acr` is refused; one with
  it is accepted and the act records the claim and value; switching the requirement off restores
  the plain approval.
- `signed_pdf` from: an approver who is a person (accepted); an approver whose binding is
  unflagged, or a self-issued or editor session (agent, refused); a non-approver (refused);
  a rejected or withdrawn revision (refused).
- A sweep and a node purge leave every revision's attachments readable.
- Annulling needs a reason and the owner or the uploader; it leaves the annulled bytes readable and
  marked, takes exactly one new upload, and appends an `annul_attachment` event.
- A KB period below the operator's floor is refused; with no policy, no deletion succeeds; a
  shortened period does not apply to a revision superseded before the change.
- Deletion before the retention period, by a non-owner, or of an in-force revision's attachment
  is refused; an owner's deletion after it leaves the revision record, marks the attachment deleted
  and appends a verifiable `delete_attachment` event.

## Amendment 3 (2026-11-15): approval policy per document type, and what the record shows

Amendment 2 settled a revision's attachments. This one settles who must act on a controlled document
before it is approved and in force, and what its record shows afterwards. The owner decided each
point on 2026-11-15 with the deployment's document-control review; this amendment records them as
design. It needs ADR-098's follow-up that exposes how a person signed in (`amr`, `acr`,
`auth_time`) and which IdP groups their token carried.

### C1. The policy is per document type

A KB's review policy maps each **document type** to the acts a revision of that type needs. Each
KB picks **one** source for its types, named in its settings, never both: the node's kind
(ADR-114), matched by its match key, or a node property whose allowed values the KB declares
(`<KEY>_ALL`, C11), for KBs that already classify documents that way. A
type with no entry uses the KB-wide policy D3 already defines (`required_acts`, reviewer and
approver lists).

**The type cannot be a way around the policy.** The type is frozen into the revision when review
is requested, as the template is (C7). If a node's type changed since its last approved revision,
the revision is judged against the **stricter** of the old and the new type's policies: every act
either requires.

Each act names:
- its **capacity**: `review`, `approve`, `publish` (entry into force) or `accept` (C5);
- its **performer**: an IdP group, satisfied by a person whose token carries it, or a named member;
- optionally, its **order** relative to the others. With none given, every `review` precedes every
  `approve`, which precedes `publish`.

A type also sets `in_force_on_approval` (C2) and whether one person may perform two of a
revision's acts (C3). A single document may **add** acts to its type's policy, never remove one, and
a requester can only make the policy stricter (D3's rule, extended to types).

### C2. Approving and putting into force are separate acts

`approve` and `enter_into_force` stay two acts, each recorded as its own event. By default the
principal who approved may also perform `enter_into_force`: it is still a second, recorded act. A
type, or the KB, may instead require a distinct publisher (an IdP group or a named member); that is
the practical need where approval itself happens outside MAE, for instance on paper.

`in_force_on_approval` can be set per type as well as per KB. For a type that has it, the approval
is the entry into force: the daemon records both events at the same moment, the second naming the
first.

### C3. Distinct people

By default no principal performs two acts on the same revision, so a reviewer does not also
approve. A type may allow it (C1), and C2's default lets the approver also publish. D3's rules hold
whatever the type says: no author acts on their own revision, and an agent never acts.

**An agent's edit is its person's authorship.** An edit made by an agent session on a person's key
puts that person in the revision's author set (D3), so they cannot approve, accept or put into force
what their own agent wrote, even when they are on the approver list.

### C4. A fresh second factor at the moment of acting

B7's `approval_requires_mfa` gains a freshness window, and it covers every controlled act, not only
`approve`: `enter_into_force`, `accept`, attaching `signed_pdf` or `controlled_copy`, and the
owner's administrative acts on a review-enabled KB (membership, reviewer and approver lists, the
review policy). Each requires the actor's sign-in time (the token's `auth_time`) to fall within an
operator-set number of seconds before the act, so the client re-authenticates at the moment of approving rather than relying on a session
signed in hours earlier. The act records `auth_time` beside the `amr`/`acr` that satisfied B7. An
old sign-in is refused `reauthentication_required`, and its message tells the person to sign in
again.

### C5. Accepting an uploaded external document

A revision whose content is an uploaded PDF rather than a frozen source (an external document) has
no source for D6's check to compare. Its type requires one or more `accept` acts instead. The
principal who uploaded the PDF never counts towards them, whatever the type allows: an author may
upload, and the acceptance acts are the control.

### C6. The controlled copy

A third attachment kind, `controlled_copy`: the approved `rendered_pdf` with an approval sheet
appended as an incremental PDF update. At upload the daemon verifies that the revision's
`rendered_pdf` is, byte for byte, the beginning of the controlled copy, and refuses it otherwise
(`controlled_copy_mismatch`). It records both hashes. The copy follows Amendment 2's rules (B1 write
once, B2 a PDF, B5 retention). It can be attached only once the revision is **in force**, so the
approval sheet can show who put it into force and the effective date; for a type with
`in_force_on_approval` that is the moment of approval.

### C7. The template is part of the frozen source

A revision records the template its rendering was composed with (an identifier and its sha256),
frozen beside its source (D2). Two revisions with the same text but different templates are
different revisions.

### C8. What the record shows

For each revision, the record, built from its events (D7), shows:
- **authored by**: every editor (D3), each with their kind; an edit made by an agent session using
  a person's key shows as an agent edit on that person's behalf;
- **reviewed by**: each reviewer, when, and the role or group that satisfied the act;
- **approved by**: who, when, the role or group, and the sign-in details (C4). With several
  approvals, which act is the one the type's `approve` requires;
- **put into force by**: who and when, and the effective date;
- **what was approved**: the frozen source's hash, the template's (C7) and the rendered PDF's (B7);
- **what it replaced**: the revision it supersedes (`replaces`), and on that revision `superseded_by`;
- **why**: the reason given with the request.

Each act is an event in the chain, with its performer, time, capacity and the group that satisfied
it.

### C9. Agents are enabled once, after a recorded assessment

Before any review-enabled KB on a deployment accepts an agent's edits at all, the deployment
records a one-time act: a person the operator designates in local configuration records that the
agent feature was assessed, with a reference to the assessment. The act is chained like every
other (D7). Until it exists, agent sessions are refused writes to review-enabled KBs
(`agents_not_assessed`). It is recorded through the local operator CLI, never over the network,
because it governs what the agent may do (principle #16).

**Per KB, agents act within two owners' limits** (decided 2026-11-15). Under the deployment gate
above, two different people decide, and the text names them so they are not confused:

- **The KB owner** sets the KB's agent ceiling: `none`, `read_only` or `read_write`. A
  review-enabled KB starts at `none` until its owner raises it. Anything beyond read-only for an
  agent needs the KB owner's authorisation.
- **P, the person an agent acts for**, creates and renews their own agent's grant on a KB, up to
  read-only on their own authority, each time with a fresh second factor (C4). A grant lasts 7 days
  by default; the operator caps it, at most 30.
- **An agent never exceeds P.** Its rights are checked against P's current rights on that KB on
  every request, and all of P's agents lose access when P does.

Each ceiling change and each grant is an event in the chain (D7).

### C10. The list of documents in force

`kb/review.in_force {kb_id}` answers, under the KB's own read access (`kb_access` Read), one entry
per document with a revision in force: its code, title, revision number, state, in force since,
approved by and put into force by, its document type, and its classification marking (a property
the KB names). A deployment builds its master list of controlled documents from it. It is paged like `kb/query.list`.

### C11. Values come from the KB's vocabularies

Field values on a controlled document are chosen from the KB's vocabularies, never invented
(decided 2026-11-15).

- **Where the vocabularies live.** A **vocabulary node**, named by id in the KB's settings, declares
  them with Org's own mechanisms: `#+TAGS:`, `#+TODO:`, `#+PRIORITIES:`, and the `<KEY>_ALL`
  property convention for a property's allowed values (document type when C1's source is a
  property, classification, and the like). mae-org already reads all of these.
- **The vocabulary node is itself reviewed.** It is a document type whose policy (C1) makes the KB
  owner its approver. Anyone, an agent included, may draft a change to it in the working copy; the
  change takes effect only when the owner approves that revision, with C4's fresh second factor.
  There is no separate per-node write gate, and an agent's proposal is simply its draft awaiting the
  owner's approval.
- **The gate is review.** Org text and MCP can write any value, so the working copy may hold an
  undeclared one meanwhile. `kb/review.request`, and again `approve`, check every tag, TODO
  keyword, priority and declared property of the revision against the vocabularies of the KB's
  **approved** vocabulary revision. An undeclared value is refused `vocabulary_violation`, with the
  node, the field and the value.
- When C1's type source is ADR-114 kinds, the KB's declared kinds are that vocabulary, and no
  `<KEY>_ALL` declares types.

### C12. Tests this amendment adds

- A type requiring review then approve refuses an approve before the review; a document adds an act
  and cannot remove one; a requester cannot loosen the type.
- Changing a node's type to a lighter one does not drop an act: the revision is judged against the
  stricter policy, and the type it was judged against is frozen in it.
- An edit by an agent session on P's key, then P's approve → `author_cannot_approve`, also when P
  is on the approver list.
- A performer named by group is satisfied only by a token carrying that group, and the act records
  it; a named member by that principal only.
- Distinct people: the reviewer's approve is refused by default and accepted where the type allows;
  the approver may publish by default and is refused where the type requires a distinct publisher.
- `in_force_on_approval` per type records both events at once; per KB still works.
- A stale `auth_time` is refused `reauthentication_required` for approve, enter into force, accept,
  attaching a signed original or controlled copy, and an owner's administrative act; a fresh one is
  accepted and recorded.
- An external PDF's uploader never counts as an accept act.
- A controlled copy whose prefix is not the rendered PDF is refused; one attached before the
  revision is in force is refused; a matching one records both hashes and cannot be replaced.
- The record of a superseded revision names its successor, and the successor names it.
- Agent writes to a review-enabled KB are refused until the assessment act exists; that act is
  unreachable from the network.
- On a review-enabled KB with policy `none`, an agent's read and write are refused; with
  `read_only`, a write is refused; an expired grant is refused; renewing without a fresh second
  factor is refused; a grant longer than the operator cap is refused; P granting beyond read-only
  without the KB owner is refused; an agent of P is refused once P loses access to the KB.
- `kb/review.in_force` lists only documents in force, only to readers of the KB.
- A revision with an undeclared tag, TODO keyword, priority or property value is refused
  `vocabulary_violation` at request and at approve, naming node, field and value; declaring it in
  a vocabulary revision the owner approves lets the same revision through; a declared but
  unapproved vocabulary change does not.
- A KB's settings accept one type source; naming both is refused.

## Build sequence (2026-11-16)

This section orders the work of this ADR and of ADR-115 into pull requests that implementation can
start one at a time. It decides nothing new: where the ADRs leave a question open or disagree, the
question is listed under **Open before building** below, and the PR that depends on it says so.

### What exists to build on

Measured on `main` at the date above:

- **Built:** the per-document editor record (`daemon/src/doc_editors.rs`,
  `storage_editor_record.rs`), with the `Authors::Known | Unknown` verdict A1 and A3 require;
  `DocStore::state_with_high_water`, the A2 cutoff; ADR-116 P1 to P3 (`mae_sync::kb::history`,
  `storage_history.rs`, `kb/history.list | get | diff | verify | checkpoint` on the collab handler,
  signed heads with the daemon's Ed25519 key); the seed record that A4 reads (`kb_seed`,
  `kb_seed_nodes`); the identity binding table with the `person | agent` flag and the typed
  principal (`identity_binding.rs`, `oauth_self_issue.rs`: `ValidatedPrincipal.kind`); the
  maintenance tick; `mae-daemon backup` (a tar archive of `VACUUM INTO` snapshots).
- **Not built:** anything in this ADR's D1 to D8 or its amendments; anything in ADR-115; ADR-116 P4
  (rollback) and P5 (retention, holds); ADR-114's kind vocabulary and match key; a node bin or
  purge in the daemon (deletion is `KbCollectionDoc::remove_node`); a per-KB lock; streaming
  request or response bodies on the OAuth listener (every response is `Full<Bytes>`, every request
  body is read whole under `max_request_body_bytes`); dispatch of any namespace other than
  `kb/query.*` on that listener.
- **Assumed from the sign-in claims follow-up to ADR-098** (in progress): the validated principal
  carries `SignIn { amr, acr, auth_time, groups }`, with `second_factor()` and `fresh_within()`.
  PR-8 and PR-9 need it; nothing before them does.

### Rules every PR in this sequence follows

- **Mergeable and green alone.** Each PR leaves `main` releasable. New behaviour is off by default
  or unreachable until its switch is set: review stays off per KB (D3), `view` defaults to
  `"working"` (D5), and a route that is not configured falls through as an unknown path, as the
  binding routes do today.
- **Data is additive.** New tables are created with `CREATE TABLE IF NOT EXISTS` in the daemon's
  `state.db`, beside the editor record and history, so the existing backup covers them. No PR
  rewrites an existing row. A PR that needs a backfill says so and makes it a single-writer pass,
  never a read-triggered side effect (ADR-093).
- **Every test fails without its code.** Adversarial cases first: the refusal, the race, the forged
  or replayed input. Each PR commits mutant entries for the mechanisms it adds (file, exact text,
  replacement, the test that must fail), and runs only its own mutants between review rounds; the
  full list runs once before the PR merges. Where the list lives is open (see O14).
- **One independent review per PR at the end**, for each PR marked so below. Every PR in this
  sequence touches authorization, identity, retention or deletion, so every one is marked.
- **Benchmarks** are `#[ignore]`d tests that report numbers, as ADR-109's are. A PR that introduces
  a limit sets its default from its benchmark's first run and pins it there.

### PR-1. Blob store core

**Builds:** ADR-115 P1 without image processing: D1 (files and `kb_blob`), D2 (access), D3 (the
three routes, the cap, the transfer digest, response headers, `ETag`), D3a's typing by sniffing
only, the per-tenant storage quota, the blob directory in `mae-daemon backup`, and D8's E2E
refusal.

**Does not build:** image processing (PR-2). Until PR-2, an upload sniffed as an image is refused
`attachment_unsupported`, so no unprocessed image is ever given an address that processing would
later change. Also not: the `attachments` map, `blob_ref`, the sweep (PR-3; until then nothing
deletes a blob, and the quota bounds growth), revision attachments (PR-6).

**Prerequisites:** none beyond `main`. Settles O3 and O4 first, because the upload contract and the
stored media types are fixed here.

**Data:** `kb_blob(kb_id, sha256, size, media_type, width, height, stored_at)`, primary key
`(kb_id, sha256)`. Files at `blobs/<kb_id>/<first two hex>/<sha256>`, written to a temporary file in
the same filesystem, `fsync`ed, then renamed; leftover temporary files are removed at startup. Usage
per tenant is the sum of `kb_blob.size` over its KBs. New configuration: `max_attachment_bytes`,
the tenant storage quota, the blob directory. No migration: all tables are new.

**Routes and errors:** `POST /kb/<kb_id>/blobs`, `GET` and `HEAD /kb/<kb_id>/blobs/<sha256>`,
routed before the JSON-RPC body read. The listener gains a streaming request path and a streaming
response body for these routes only; the JSON-RPC path keeps its whole-body read and its limit. Each
route gets a `RequestCost` in `tenant.rs`. Refusals: `403 not_member` (identical for every
non-member case, including an unknown KB), `404 blob_missing` (members only), `413
quota_exceeded`, `507 tenant_quota_exceeded`, `400` with the transfer-digest refusal kind (O3),
`attachment_unsupported`, `e2e_unsupported`.

**Tests:**
- A non-member's `GET` and `HEAD` receive byte-identical status, headers and body for a blob this
  KB holds, one another KB holds, one nobody holds, and an unknown KB.
- A member of KB A cannot read KB B's blob by its hash. A viewer cannot upload.
- A transfer-digest mismatch leaves no file and no row. The answer's sha256 is that of the stored
  bytes, whatever the client sent.
- An upload over the cap stops reading at the cap (bytes read are counted); peak memory for a body
  ten times the cap is constant (benchmark).
- An upload interrupted mid-stream, or by a restart, leaves no file and no row; concurrent identical
  uploads both succeed and leave one file.
- The quota refuses the upload that would exceed it and accepts the one before it.
- The same bytes uploaded to two KBs give two files; deleting one KB leaves the other's file.
- Responses carry `nosniff`, the sandbox policy, and `Content-Disposition: attachment` off the
  allowlist; `If-None-Match` answers `304` without opening the file (instrumented).
- An E2E KB refuses; an image upload refuses until PR-2; a JSON-RPC request still hits its body
  limit.
- A backup contains the blob tree and a restore serves the same bytes.

**Mutants:** upload checked at `Read` instead of `Edit`; the non-member branch split by existence;
the cap compared with `>` for `>=`; the digest comparison skipped; rename before `fsync`; the KB
dropped from the file path (cross-KB sharing); the quota off by one; each security header removed;
the `304` path reading the file.

**Size:** L. **Independent review:** yes (new unauthenticated-reachable routes, a no-oracle rule,
streaming on the listener).

### PR-2. Image processing

**Builds:** ADR-115 D3a for images: dimensions read from the header and refused over
`max_attachment_pixels`; decoding in a bounded worker with memory, time and frame limits, never on
the request task; metadata stripped; orientation applied to the pixels; colour profile kept; SVG
sanitised by an element and attribute allowlist; processing idempotent. Enables the image
allowlist that PR-1 refused.

**Does not build:** thumbnails, client-side processing, anything for E2E KBs.

**Prerequisites:** PR-1. New decoder and sanitiser dependencies pass `deny.toml`.

**Data:** fills `kb_blob.width` and `height`. No migration: PR-1 stored no image.

**Errors:** `413 attachment_too_many_pixels`, `422 attachment_undecodable`,
`attachment_unsupported`.

**Tests:**
- A header declaring more than the pixel limit is refused without the decoder being called
  (instrumented).
- A malformed image, a decompression bomb and an animation over the frame limit each fail within
  the limits and are refused.
- The stored image has no EXIF, XMP or location data; a rotated photograph is stored upright
  (pixel comparison); a colour profile survives.
- An SVG with a script, an external reference or an event handler is stored without it.
- For every allowed format, processing a processed file returns identical bytes.
- Decoding saturating its worker pool queues or refuses further uploads; it never runs on the
  request task.
- Benchmark: decode, strip and rotate time and peak memory per format at 1, 12 and 40 megapixels;
  the limits' defaults come from it.

**Mutants:** the pixel check moved after decoding; stripping skipped; orientation ignored; one
dangerous element added to the SVG allowlist; a re-encoding parameter changed so processing is no
longer idempotent.

**Size:** L. **Independent review:** yes (untrusted input to parsers, new dependencies).

### PR-3. Attachments in the node, and collection

**Builds:** ADR-115 D4 on the node side and D5 (its P2): the `attachments` map in the node CRDT
(`NAME` to `{sha256, size, media_type}`), `MaterializedNode::attachments`, its entry in
`MaterializedNode::digest` and in the history content encoding (both per O1), its field-type check
(`invalid_field`); `blob_ref` derived from every holder that exists at this point (working nodes,
history versions, holds); a per-KB lock taken by every node-write path whose update changes
`attachments`, and by the sweep; the sweep on the maintenance tick, bounded per pass, with the grace
period; operator holds (O6).

**Does not build:** revision holders (PR-4 and PR-6 add theirs, each in the same PR that creates
the holder); the mae-org resolver, seeding and export, display in MAE's editor (the parallel track
below); fetching blobs from a hub for mesh peers.

**Prerequisites:** PR-1. O1, O5 and O6 settled.

**Data:** CRDT key `attachments`, additive, tolerant reader, never written on read. Tables
`blob_ref(kb_id, sha256, holder)` and the hold table of O6. `blob_ref` is an index derived from the
documents and the history, rebuildable at any time; the sweep computes its live set from the
holders under the lock, never from a stale index. New configuration: the grace period (default 24
hours) and the per-pass bound. Migration: none for documents. The first sweep after upgrade builds
`blob_ref` from scratch.

**Errors:** `invalid_field` for a mistyped entry (the existing kind). Whether binding a hash this KB
does not hold is refused is O15.

**Tests:**
- A node written before this PR keeps its `version`, and every existing history entry still
  verifies (fixtures captured before the change).
- Replacing an image under the same name changes the node's `version`.
- A blob referenced only by an old history version survives a sweep; so does one under a hold, and
  one in the bin as defined by O5.
- A blob uploaded but unreferenced survives within the grace period and is collected after it
  (injected clock).
- A bind racing a sweep never loses a blob, over deterministic interleavings at each pause point of
  both, as in the existing race suite.
- Every node-write path (`sync/update`, `/ws`, `kb/node_update`, the dialer relay) takes the lock
  when it changes `attachments`: one test per path, run with the sweep paused inside its critical
  section.
- Opening a document without the map writes nothing to it.
- Benchmark: sweep time per pass for 1,000 and 10,000 nodes with three attachments each.

**Mutants:** one holder kind removed from the live set; the lock skipped on one write path; the
grace comparison reversed; `attachments` dropped from the digest or the encoding.

**Size:** L. **Independent review:** yes (deletion, and a lock on the write path).

**Parallel track, after PR-3 and outside this sequence:** ADR-115 P3 (the mae-org resolver and
placeholder states, released as a mae-org tag), P4 (seeding and export carrying attachments; needs
PR-2, because the dry run runs the same processing), and P5 (display in MAE's editor). None of
PR-4 to PR-10 depends on them.

### PR-4. The revision record, the request, and the event chain

**Builds:** D1 (tables, hub-local, plaintext only); D2 (the freeze at the requester's `version`,
with A2's cutoff from `state_with_high_water`; document codes with per-KB prefixes); D3's per-KB
switch (owner-only, an event) and its tighten-only rule for `in_force_on_approval`; D7's chain
(append-only, `prev_hash`, signed with the daemon's Ed25519 key, reserved `member_sig`, export and
offline verify through the CLI); the request side of D8; ADR-116 D2.3's version cut on freeze and
its D5 exemption for versions a revision references. Columns for the frozen document type (C1) and
template (C7) exist from here, nullable, so later PRs do not change the revision's identity. The
sweep's live set gains every revision's frozen `attachments` map, in every state.

**Does not build:** any act (approve, act, reject, enter into force, derogate), the reviewer and
approver lists, `view: "published"`, revision attachments. With the switch on, readers are served
exactly what they are today.

**Prerequisites:** PR-3 (a frozen map must hold its blobs). O7, O8 and O11 settled.

**Data:** per-KB review settings (switch, `in_force_on_approval`, `required_acts`, code prefixes);
review requests (name, mode, reason, requester, state); revisions (`rev_id`, KB, node, request,
`doc_code`, `rev_no` null until approved, `source_sha256`, `version`, cutoff WAL id, state,
document type, template id and sha256, creator, time, `replaces`); frozen sources, where O8 decides;
document codes, unique per `(kb, prefix, number)`; the event log (per KB: sequence, kind, actor and
kind of actor, time, payload, `prev_hash`, `entry_hash`, daemon signature, `member_sig`). No
migration.

**Methods and errors:** `kb/review.request`, `kb/review.withdraw` (by the requester, before
approval), `kb/review.status`, `kb/review.pending`, `kb/review.set_policy` (switch, mode,
`required_acts`), `kb/review.set_code`; `kb/query.revisions`; `kb/query.get` with `revision`.
Dispatched on the OAuth listener and wherever O9 decides. Refusals: `invalid_params`,
`role_forbidden`, `review_not_enabled`, `stale_version` (+`current`), `unknown_request`,
`node_not_in_kb`, `doc_code_taken`, `invalid_transition`, `e2e_unsupported`.

**Tests:**
- A request naming a superseded `version` is refused with the current one.
- A revision's body and `version` are unchanged after the working copy is edited.
- A share, a restore or an edit racing the freeze never yields content and cutoff that disagree
  (A2), over deterministic interleavings.
- A requester setting `in_force_on_approval: true` against a KB default of false is refused
  `role_forbidden`.
- A non-owner turning review on or off is refused; the owner's change is an event.
- With review off, `kb/review.request` is refused `review_not_enabled` and `kb/query.get` is
  unchanged.
- The chain verifies end to end and fails after any single column of any row is altered, a row is
  removed, or two rows are swapped; a forged signature fails.
- Two concurrent `set_code` calls for the same number: one wins, one is refused `doc_code_taken`;
  an unlisted prefix is refused.
- A blob held only by a withdrawn revision's frozen map survives a sweep.
- The history version cut at the freeze is not pruned by any retention evaluation.
- Every method refuses an E2E KB.

**Mutants:** the stale-version check removed; `prev_hash` left out of `entry_hash`; the signature
skipped; the owner check removed; the tighten-only check reversed; the revision holder left out of
the sweep.

**Size:** L. **Independent review:** yes (authorization, the daemon's signing key, an append-only
record).

### PR-5. Acts and publication

**Builds:** D3's reviewer and approver lists (owner-set, fail-closed), the author rule as A1 and A3
define it (`author_cannot_approve`, `authors_unknown`), and the agent rule from the principal's kind
(ADR-098 Amendment 1, C3); `required_acts`; D4's atomic publication of a set in one SQLite
transaction, entry into force with `effective_at`, derogation, withdrawal of an approval; D5's
`view: "published"` for `get` and `list` (summary fields from the revision), and its delete and
purge guards (as O5 defines delete and purge); `rev_no` assigned at approval; A4's imported,
in-force revision from the seed record, through the operator CLI only.

**Does not build:** revision attachments, sign-in requirements, per-type policy.

**Prerequisites:** PR-4. O5 settled.

**Data:** reviewer and approver lists per KB; the published pointer `(kb, node) -> rev_id`; acts
(revision, act, principal, kind of principal, time, `effective_at`). No migration.

**Methods and errors:** `kb/review.approve`, `kb/review.act` (`reviewed`), `kb/review.reject`,
`kb/review.withdraw` (extended to approvals), `kb/review.enter_into_force`, `kb/review.derogate`;
`kb/review.set_policy` gains the lists. Refusals add `not_reviewer`, `not_approver`,
`author_cannot_approve`, `authors_unknown`, `agent_forbidden`, `set_incomplete`,
`invalid_transition` (+`from`, `act`).

**Tests:** every case in "Tests that must exist" above, and in particular:
- X edits; Y requests; it is rejected; Y requests again; X approves: `author_cannot_approve`.
- A listed approver whose request counts as an agent (a self-issued token for a `person`-flagged
  fingerprint, or an IdP session on an unflagged binding) is refused `agent_forbidden`; an unlisted
  person `not_approver`.
- A set of three with one unapproved moves no pointer; a fault injected between the pointer writes
  leaves all three unmoved.
- Withdrawal after entry into force is refused; before it, a different listed approver returns the
  revision to `in_review`.
- Deleting a node with a published revision is refused (`act: "delete"`) and succeeds after
  derogation; purging a node with historical revisions leaves every revision and the chain
  verifiable.
- A1's credit cases and A3's three unknown cases, driven through real `sync/update` traffic.
- The imported act is unreachable from every `kb/review.*` method and from an agent principal: a
  scan of the method table and of every refusal.

**Mutants:** the author check removed; `Unknown` treated as an empty author set; the kind check
removed; an empty list treated as "anyone"; the pointer moved per member outside the transaction;
the delete guard removed.

**Size:** L. **Independent review:** yes (the authorization core of this ADR).

### PR-6. Revision attachments

**Builds:** D6, B1 to B4 and B8: `rendered_pdf` and `signed_pdf` stored in the blob store as
revision holders; write-once with the same-bytes retry; annulment with a reason by the owner or the
uploader, and exactly one new upload after it; the PDF check; `signed_pdf` only from a listed
approver who counts as a person, only in the states B3 lists; the declared `source_sha256` check
for `rendered_pdf`; the uploader, its kind, its bound `iss` and `sub` and the time, in the revision
record and as `attach` and `annul_attachment` events; D5's `attachments` list on a revision. B7's
`approval_requires_rendered_pdf` switch, with annulment of the approved rendered PDF refused.

**Does not build:** deletion and retention (PR-7), `controlled_copy` (PR-9), the second factor
(PR-8), signature verification.

**Prerequisites:** PR-1 and PR-5. O2, O3, O4 and O13 settled.

**Data:** revision attachments (revision, kind, sequence within the kind, sha256, size, content
type, declared source, uploader, uploader kind, `iss`, `sub`, time, state `current | annulled |
deleted`, reason). The sweep's live set gains them. No migration.

**Routes and errors:** the revision attachment routes as O3 decides. Refusals:
`attachment_exists`, the transfer-digest kind (O3), `attachment_type`, `not_approver`,
`agent_forbidden`, `invalid_transition`, `quota_exceeded`, `rendered_pdf_required`,
`rendered_pdf_mismatch`.

**Tests:** B9's attachment, annulment and `signed_pdf` cases, and:
- A rendered PDF declaring another revision's source is refused, and the declaration of an accepted
  one is recorded.
- A sweep and a node purge leave every revision's attachments readable.
- A concurrent second `PUT` of different bytes for the same kind: exactly one is stored.

**Mutants:** the write-once check removed; the `%PDF-` check removed; the person check removed from
`signed_pdf`; a pending state added to `signed_pdf`'s allowed states; the annulled-after-approval
check removed.

**Size:** M. **Independent review:** yes (who counts as a person, a write-once record).

### PR-7. Retention, deletion, and imported history

**Builds:** B5: the operator's floor in local configuration only; a KB period no shorter than the
floor; nothing deletable with no policy; non-retroactive shortening (each revision keeps the period
in force when it was superseded or derogated); owner deletion of one attachment after the period,
leaving the record marked and appending `delete_attachment`; no path that prunes the event chain.
B6: import of past revisions in `superseded` or `derogated` state with their attachments, refused
when it would land before revisions the node already has.

**Does not build:** any other deletion path. ADR-116 P5 (history retention) is separate.

**Prerequisites:** PR-6. O2, O10 and O12 settled.

**Data:** the KB's attachment retention period and its history of changes (also in the chain); the
period stamped on a revision at supersession or derogation. No migration.

**Methods and errors:** the period on `kb/review.set_policy`; deletion and import as O10 names them.
Refusal kinds as O12 names them.

**Tests:** B9's retention and import cases, and:
- No network method, MCP tool or Scheme call can set the operator's floor (a scan).
- Deleting an attachment whose bytes another holder references removes the revision's holder only;
  the file stays.

**Mutants:** the floor comparison reversed; the shortened period applied retroactively; the
in-force check removed; the owner check removed; the overlap check on import removed.

**Size:** M. **Independent review:** yes (retention and deletion).

### PR-8. Sign-in strength at the moment of acting

**Builds:** B7's `approval_requires_mfa` (accepted `amr` and `acr` values from operator
configuration, no built-in default, fail-closed when the switch is on and nothing is configured)
and C4's freshness window over every controlled act this sequence has built: approve, enter into
force, attaching `signed_pdf`, and the owner's administrative acts on a review-enabled KB (as O9
scopes them). Each act records the claim and value that satisfied it and the `auth_time`.

**Does not build:** group performers (PR-9). `accept` and `controlled_copy` take the check in PR-9,
where they are built.

**Prerequisites:** PR-6 and the sign-in claims follow-up to ADR-098. O9 settled.

**Data:** the switch and the operator's accepted values and window; the recorded claims on acts.
No migration.

**Errors:** `mfa_required`, `reauthentication_required`.

**Tests:** B9's second-factor cases and C12's stale `auth_time` cases for each act above; a
self-issued token never satisfies either rule; turning the switch off restores the plain approval.

**Mutants:** the empty-configuration case allowed; the freshness comparison reversed; each covered
act's check removed, one mutant per act.

**Size:** M. **Independent review:** yes (authentication strength).

### PR-9. Policy per document type, and the controlled record

**Builds:** C1 (one type source per KB; the type frozen at request; the stricter of the old and new
type's policies; acts with capacity, performer and order; a document may add acts, never remove
them); C2 (separate publish act, optional distinct publisher, `in_force_on_approval` per type with
both events recorded at once); C3 (distinct people by default); C5 (an external document revision
and its `accept` acts, the uploader never counting); C6 (`controlled_copy`, with the prefix check,
only once in force); C8 (the record assembled from events, `replaces` and `superseded_by`); C10
(`kb/review.in_force`). C4's freshness extends to `accept` and `controlled_copy`.

**Does not build:** vocabularies and the agent policy (PR-10).

**Prerequisites:** PR-8, groups in the sign-in claims, and ADR-114's match key if a KB takes its
types from kinds (a KB taking them from a property does not need it). O11 and O12 settled.

**Data:** per-KB type source and per-type policies; per-document added acts; the revision's source
kind (`frozen | external`). The type and template columns from PR-4 are filled. No migration.

**Methods and errors:** `kb/review.set_policy` gains type policies; `kb/review.accept`;
`kb/review.in_force`. Refusals add `controlled_copy_mismatch` and the kinds O12 names for an unmet
performer or order.

**Tests:** C12's type, distinct-people, performer, `in_force_on_approval`, external document,
controlled copy, record and `in_force` cases, and: an edit by an agent session on P's key puts P in
the author set, so P's approval is refused even when P is listed.

**Mutants:** the stricter-of-two rule replaced by the new type alone; a per-document act removable;
the group performer satisfied by a named member; the uploader counted towards `accept`; the
controlled copy accepted before entry into force.

**Size:** L. **Independent review:** yes (the approval policy).

### PR-10. Vocabularies and the agent policy

**Builds:** C11 (the vocabulary node named in settings, read with mae-org; its approved revision as
the source of allowed values; the check at request and at approve; the vocabulary node's type with
the owner as approver) and C9 (the deployment's assessment act, recorded through the local operator
CLI by the person local configuration designates, and chained; `agents_not_assessed` on every agent
write path to a review-enabled KB; the KB owner's ceiling `none | read_only | read_write`, starting
at `none`; P's grants with a fresh second factor, 7 days by default, capped by the operator at most
30; every agent request checked against P's current rights).

**Does not build:** anything for E2E KBs.

**Prerequisites:** PR-9 (the vocabulary node is a reviewed type) and PR-8 (grants need freshness).
O9 and O16 settled.

**Data:** the vocabulary node id; the assessment act; ceilings and grants with their expiry. No
migration.

**Errors:** `vocabulary_violation` (+node, field, value), `agents_not_assessed`, and the kinds O12
names for a ceiling or grant refusal.

**Tests:** C12's vocabulary and agent cases, and: the assessment act is unreachable from every
network method, MCP tool and Scheme call (a scan); every agent write path (`sync/update`, `/ws`,
`kb/node_update`, the dialer relay) is refused before the act, one test per path.

**Mutants:** the vocabulary check skipped at approve; an unapproved vocabulary revision used; one
agent write path left ungated; the ceiling or the grant expiry not checked; P's current rights not
re-read.

**Size:** M. **Independent review:** yes (the agent's own authority, principle #16).

### Open before building

Each item is a gap or a disagreement between ADRs. It is not resolved here; the PR it blocks is
named.

- **O1. The `attachments` map and existing digests (PR-3).** ADR-115 D4 adds the map to
  `MaterializedNode::digest`, to the history content and to the seed `plan_hash`. The history
  content encoding (`maenode/v1`) writes a presence flag for every optional field, so it cannot take
  a new field without changing every encoding, and stored content is accepted only in its one
  encoding (ADR-116 D3). Decide whether an empty map leaves the digest unchanged (as `org_header`
  does) and how the history encoding is versioned so existing entries still verify.
- **Decided 2026-11-16 (O2):** ADR-113 B5 governs. A revision's attachments are held until B5's
  owner-only, audited deletion after the retention floor, and that deletion releases the revision's
  hold on the bytes; nothing else releases it. ADR-115 D5 is amended to say so in PR-6.
- **O2. Revision attachments: kept for ever, or deletable (PR-6, PR-7).** ADR-115 D5 says a
  revision's references are never released because a revision record is never deleted. ADR-113 B5
  lets an owner delete a revision's attachment after a retention period. Decide which governs, and
  that B5 deletion releases the revision's hold on the bytes rather than the file itself.
- **Decided 2026-11-16 (O3):** one upload path. Every file streams through ADR-115's blob route
  (`POST /kb/<kb>/blobs`, its digest check, quota and sniffing); attaching it to a revision is a
  separate call naming the hash and the kind, where ADR-113's rules apply (write-once, approver,
  person, state). One digest carrier and one refusal kind: ADR-115's. A node can hold any number of
  attachments by name and a revision up to its kinds; both attach by hash.
- **O3. Two upload contracts (PR-1, PR-6).** ADR-115: `POST /kb/<kb>/blobs`, a `Content-Digest`
  header, refusal `blob_hash_mismatch`. ADR-113: `PUT /kb/<kb>/revisions/<rev>/attachments/<kind>`,
  an `expected_sha256` field, refusal `upload_digest_mismatch`. Decide one digest carrier and one
  refusal kind, and whether a revision attachment is uploaded as a blob and then bound, or streamed
  to the revision route.
- **O4. The PDF media type (PR-1, PR-6).** ADR-115 stores and serves anything off its image
  allowlist as `application/octet-stream`; B2 requires `application/pdf`. Decide whether
  `application/pdf` becomes a stored type, served only as a download.
- **O5. The bin, and purge (PR-3, PR-5).** D5 (delete to the trash, purge) and ADR-115 D5 (nodes in
  the bin) refer to a node bin and a purge the daemon does not have; deletion today is
  `remove_node` on the collection. A design, or a mapping onto what exists, is needed.
- **O6. Holds (PR-3).** ADR-115 D5 and ADR-116 D5 each name operator holds; neither is built.
  Decide that they are one table and one CLI, and which PR builds it (this sequence assumes PR-3).
- **O7. The ADR-115 test on cross-KB bytes (PR-1).** "An upload of bytes another KB holds is read
  and verified in full before binding" describes a cross-KB deduplication path that ADR-115 D1
  rejects. Restate or drop it.
- **O8. Where the frozen source lives, and its hash (PR-4).** ADR-116 D2.3 cuts a history version at
  a freeze but defines no version kind for it, and its content hash is keyed while D2's
  `source_sha256` is not. Decide whether a revision references the history entry `(node, seq)` or
  holds its own copy, and the kind of that version.
- **Decided 2026-11-16 (O9):** native MAE sessions stay agents; native IdP sign-in (ADR-111 P4)
  is built **before** PR-4, so a person reaches their review-enabled KBs from native MAE as a person.
- **O9. Native sessions are agents (PR-4, PR-8, PR-10).** Under ADR-098 Amendment 1 C3, a person
  using native MAE counts as an agent. With C9, a review-enabled KB at the default ceiling `none`
  then refuses that person's reads and writes from native MAE. C4 requires a fresh `auth_time` for
  an owner's membership and list changes, which today go over the mutual-TLS collab path, which
  has no `auth_time`. D8 places `kb/review.*` on the OAuth listener, while `kb/history.*` is on the
  collab handler only. Confirm these consequences, and name the path for owner administration of
  a review-enabled KB.
- **O10. Unnamed methods (PR-7).** B5's attachment deletion and retention period, and B6's import,
  have no method in D8. B6 is "an owner act", while A4's import is operator-only, through the CLI.
  Name them, and say whether B6 is a network method (then C4 applies) or a CLI one.
- **O11. Writes over IdP sessions (PR-1, PR-4).** ADR-098 Amendment 1 C0 says HTTPS writes wait for
  hub signature verification (#727). This ADR needs IdP sessions to act (B3, B7, C4), and ADR-115
  uploads need `Edit` on the same listener. State that review acts and blob uploads, which are not
  CRDT content writes, are allowed before #727, or that they wait.
- **O12. Unnamed refusal kinds (PR-7, PR-9, PR-10).** None is defined for: a KB retention period
  below the floor, deletion before the period, an import overlapping existing revisions, an unmet
  performer or order on `publish` or `accept`, a ceiling or a grant refusal.
- **O13. Who uploads `rendered_pdf` (PR-6).** D6 says the requester's client renders and uploads
  it; B3 says "as D6". Decide whether only the requester may upload it, or any principal with
  `Edit`.
- **Decided 2026-11-16 (O14):** mutant lists move into the repository only after a confidentiality
  review of each list, and after a review of whether the harness should become proper tooling (a
  Rust runner over declared lists, or an existing tool) rather than the provisional scripts. That
  review is its own item before PR-1. The review is done: no list leaks anything, after small edits.
  The harness becomes `tools/mutate`, a standalone Rust crate in the style of `tools/audit-metrics`,
  reading declared lists from `mutants/<area>.toml`. Each entry carries an id, a runner, the test
  filter that must fail, and one or more exact find/replace edits. `--since <ref>` selects by
  files touched. The tool restores files left mutated by a killed run, and counts a killed build as
  no result. CI runs `--check` (anchors still match) on every PR, and the full run nightly or
  on a label. Each PR commits its own entries. Cost is a design requirement, not a tuning detail:
  the provisional runners spent about 3.5 minutes per mutant on work no verdict needed. So the tool
  prints and records each result the moment it is known, and resumes from that record. Each mutant
  names only the test targets and the filter that hold its guarding tests. The first failing test
  ends that mutant's run. Compile errors and killed builds are reported apart from survivors. Each
  run reports its time per mutant, so a slow list is visible.
- **O14. Where mutants are listed.** The repository has no committed mutant list or runner. One is
  needed before PR-1, so each PR can add its entries.
- **O15. Binding an unknown hash (PR-3).** ADR-115 D5 says binding takes the per-KB lock, and
  binding happens inside ordinary CRDT updates on several paths. Decide whether an update that binds
  a hash this KB does not hold is refused or accepted and rendered `Missing`, and whether a
  hub-local check on a CRDT update is acceptable for a map mesh peers also replicate (ADR-115 D8).
- **O16. One agent or all of P's agents (PR-10).** An agent signs with P's key, so the daemon cannot
  tell one of P's agents from another: a C9 grant can only be per `(KB, P)`. Confirm.
- **O17. A review-enabled KB switched to E2E.** Revisions and their frozen sources are plaintext,
  and ADR-116 Amendment 1 carries only history across the switch. Decide whether the switch is
  refused while revisions exist, as it is while history holds plaintext. No PR here builds the
  switch, but the first revision makes the question live, so it blocks PR-4.
- **O18. Incremental backup.** ADR-115 D1 says an incremental copy of the immutable blob files is
  exact; `mae-daemon backup` writes one archive each run. Either extend the format or narrow the
  claim (PR-1).

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
