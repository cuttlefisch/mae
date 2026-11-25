# ADR-120: The daemon's activity record

**Status:** Accepted (design), 2026-11-25: the owner decided O1, O3 and O5 and the fail-closed rule with its alert. Nothing here is built. Phased (P1a, P1b, P2–P5, below); P1a, P1b and P2 come first.
**Amends:** ADR-117 D7 (its operator log becomes this record's administrative events; its binding
log stays, and its head is attested here) and D9 (each auditor call is an event here), and ADR-118
D7 (its "read requests are logged" is this record).
**Relates to:** ADR-113 D7 and Amendment 4 E10 (the review event chain and its export encoding, which
this record reuses), ADR-116 D4 (signed heads, and the limit of a head kept only on the host),
ADR-111 (remote clients over HTTPS), ADR-099 (the WebSocket session), ADR-098 (the binding table
and the typed principal), ADR-060 (tenants), ADR-109 (the resource budget), ADR-115 (blob routes),
ADR-119 (the live site), ADR-027 (observability), CLAUDE.md principles #8, #14 and #16.
**Evidence:** `docs/research/120-activity-record-prior-art.md`.

## Context

A deployment's security framework requires the daemon to keep a record of user activity:

1. **Events.** Every authenticated request, including reads of every document, not only of
   controlled ones. Writes: edits, review acts, uploads and downloads. Sign-ins, session starts, and
   refusals with their refusal kind. Administrative acts: membership, policy, approver lists.
2. **Fields, at least:**
   - the principal (for an agent, the agent and its person);
   - the KB, and the node or revision;
   - the method or route;
   - the outcome (success, or the refusal kind);
   - the time.

   The client's network address is personal data. It is recorded only on a sign-in, a session start
   or a refusal. A read or write carries a session id instead.
3. **Retention, documented.** Two years for acts, sign-ins and refusals. Twelve months for reads and
   connections. Disposal at the end of retention is itself recorded, and nobody deletes otherwise.
4. **Tamper evidence.** At least append-only storage with access control. Common control frameworks
   ask for logs protected against tampering (ISO/IEC 27001:2022 Annex A 8.15; NIST SP 800-92 and
   SP 800-53 AU-9, whose enhancement AU-9(3) is cryptographic protection).
5. **Readers.** A read-only auditor and the operator, with export and filters, so that a person can
   review the record periodically for abnormal patterns.
6. **Coverage.** What the daemon sees, with a stated boundary to what a relaying server sees, and a
   key to join the two.

**What exists, checked against `origin/main` at `61097ad3`:**

| Piece | Where | What it records |
|---|---|---|
| Per-document editor record | `daemon/src/doc_editors.rs`, `storage_editor_record.rs` | One coalesced row per document, attribution kind and editor, with the WAL range. Written in the same transaction as the update. **No reads, no times that order anything, no address.** |
| Node version history | ADR-116 P1–P3, `storage_history.rs` | Content versions, chained, with heads signed by the daemon's Ed25519 identity key. Not an activity record. |
| Review event chain | ADR-113 D7 and E10 | Designed, not built. Per KB, never deleted, JCS-encoded, exported as JSON Lines. |
| Binding log, operator log, auditor | ADR-117 D7 and D9 | Designed, not built. |
| HTTPS listener | `daemon/src/oauth.rs::handle_request` | One entry point. The live site, blobs, `/ws`, binding routes and JSON-RPC all branch from it, and authentication happens there. **Nothing is recorded.** |
| Collab sessions | `collab_handler::run_session` | mTLS, `/ws` (through an in-memory pipe) and mesh sessions share one dispatch loop. **Nothing is recorded.** |
| Diagnostics | `tracing` | A startup line on the listener. Not a record: not complete, not durable, not protected. |
| Separate SQLite files | `state.db` (one FIFO writer, `synchronous=NORMAL`), `identity-bindings.db`, `blobs/blobs.db` | Each is in `mae-daemon backup`. |
| Session identifiers | — | None. A self-issued token has no `jti`. An IdP token's `sid` is not read. |

So the record is new work. The question is how to add it to every request without a second, third or
fourth log beside the ones above (principle #8), and without making the hot path wait behind the
`state.db` writer.

## Decision

### D1. What is recorded, and where the daemon's view ends

**One event per request, at each of the daemon's entry points:**

- **The HTTPS listener**, one event for every request that reaches `handle_request`:
  - JSON-RPC, including the bare bearer probe;
  - live-site pages, the index and the explorer data;
  - blob upload, download and `HEAD`;
  - `/ws/ticket`, the `/ws` upgrade, and the binding routes.

  Public routes (discovery metadata, static site assets) carry no principal and no KB content, and
  are not recorded.
- **Collab sessions** (mTLS, `/ws`, mesh), at `run_session`'s dispatch:
  - each method that is not a sync message is a request;
  - opening or fetching a document is a read;
  - a subscription covers the broadcasts the session then receives, until it ends;
  - edits are recorded as **write spans** (D6).
- **The KB Unix socket.** Its principal is the peer's OS user, where the platform reports one
  (`SO_PEERCRED` on Linux, `getpeereid` on macOS), and `unknown` otherwise (principle #13).
- **Local operator commands** that read the record or change permissions (`token mint`,
  `kb member`, `binding *`, `directory *`, `review list *`, `activity export`). Their principal is
  `operator:<os user>`, and their transport is `cli`.

**A read of many nodes names them.** A search or listing lists the node ids it returned, bounded by
the method's own result cap. A response that exposes a whole KB's listing (the site's `index.json` and
`explorer.json`) records `["*"]`.

**A sign-in, from where the daemon stands**, is either of two things:
- an authentication exchange the daemon performs itself: the mTLS or PSK `session/hello`, a binding
  proof, or a ticket redeemed at the `/ws` upgrade;
- the first request bearing an IdP token whose `(iss, sub, auth_time)` the daemon has not seen
  before, recorded with that token's `amr` and `acr`.

**The boundary.** The daemon records what reaches it. The following are not its to record:
- the IdP's own sign-in, and its failed sign-ins;
- the browser client backend's sessions, and its refusals made before it calls the daemon;
- reads of a static export (ADR-119 X1), which never reach the daemon.

These belong to the server that sees them. Two records are joined by the daemon's session id (D4),
which the daemon returns on every authenticated response so that the relaying server can log it
beside its own session. The IdP's `sid` is recorded too, when present, as a second key. A mesh peer
keeps its own record of its own users. This daemon records the peer as a relay (D2's `mesh`
principal kind) until the hub verifies every signed content op (#727).

### D2. The fields

Each event is a JSON object (its **body**). A field that does not apply is `null`, never omitted.

| Field | Content |
|---|---|
| `v` | `1` |
| `at` | RFC 3339 UTC, with exactly six fractional digits (microseconds), from the host clock (D10) |
| `kind` | `request`, `read`, `subscribe`, `write_span_open`, `write_span_close`, `write_reconciled`, `upload_intent`, `upload`, `download`, `review_act`, `admin`, `sign_in`, `session_start`, `session_end`, `disposal`, `daemon_start`, `daemon_stop`, `record_disabled`, `config_changed`, `clock_step`, `head_attest` |
| `principal` | `{id, kind, iss, sub, on_behalf_of, grant}`. The kinds are `person`, `agent`, `service`, `unbound`, `fingerprint`, `psk`, `mesh`, `local`, `operator` and `anonymous`. For an agent (ADR-118 D1), `id` is `agent:<id>`, `on_behalf_of` is P's fingerprint, and `grant` is the grant id. |
| `session` | The daemon's session id (D4) |
| `idp_sid` | The IdP token's `sid`, when present |
| `transport` | `https`, `ws`, `mtls`, `mesh`, `kb_socket` or `cli` |
| `route` | The JSON-RPC method, or the HTTP method with the route's template (`GET /kb/{kb}/blobs/{sha256}`): never the raw path, which can carry text a client chose |
| `kb_id`, `node_ids`, `revision`, `blob` | The objects touched. `node_ids` is an array of at most 100 distinct ids, in the order the request named them, each truncated to 256 bytes. |
| `node_ids_total` | `null`, unless the request named more than 100 distinct nodes: then their number, and `node_ids` holds the first 100. An index or a large listing can name thousands, and an event is written inside the group commit under `synchronous=FULL`, so an uncapped list would make one large result stall every other request's commit. The cap is the encoder's, like the address rule. |
| `wal` | `{from, to}` for a write span, linking to the editor record |
| `outcome` | `{result: "ok" | "refused" | "error", kind}`, where `kind` is the refusal or error kind the caller received |
| `count`, `first_at`, `last_at` | For a coalesced refusal (D6); otherwise `1`, `at`, `at` |
| `peer` | `{addr, forwarded}`, **only** on `sign_in`, `session_start` and refused outcomes. `forwarded` is taken only from a peer listed in `activity.trusted_proxies`, and is kept apart from `addr`, so where it came from stays visible. |
| `unverified` | For a refusal before authentication only: the token's claimed `iss` and `sub`, each truncated to 256 bytes, labelled as unverified |
| `ref` | A pointer into another record: for a `review_act`, the ADR-113 `(kb_id, seq, entry_hash)`; for `head_attest`, the attested log and its head |
| `detail` | A small object for each kind, defined beside the kind: for example, a sign-in's `amr`, `acr` and `auth_time` |
| `salt` | 16 random bytes, base64 (D7) |

**The address rule is a property of the encoder, not a habit of the callers.** The writer clears
`peer` on every event that is not a sign-in, a session start or a refusal, whatever the caller put
there. P1a's test pins this.

### D3. Where it lives: its own SQLite file, with one writer

The record is a separate database, `activity.db`, beside `state.db` in the collab data directory. It
is one file per daemon, not one per tenant. In ADR-060 a tenant is a quota grouping of principals,
set in configuration, which can change. A KB can have members from several tenants, and a sign-in has
no KB at all, so no partition by tenant is well defined. `tenant` is recorded as a column instead.

Not tables in `state.db`, for three reasons:
- **`state.db` has one FIFO write slot for the whole store** (`storage_writer.rs`), and compactions,
  seed inserts and checkpoints hold it. Every read on the daemon would queue behind them, and the
  record would slow every collab write.
- **The durability settings differ.** `state.db` runs `synchronous=NORMAL`. The record needs `FULL`
  (D5).
- **Its retention and disposal are independent.** Deletes, `secure_delete` and the checkpointer's
  truncation are best kept away from the KB store's writer.

What `state.db` would have given is atomicity with the edit. D6 obtains the same result from the
editor record that already lives there.

**Layout.** The `events` table holds, per row:
- the link and the body, as their canonical bytes (D7), which are the record;
- indexed columns derived from the body: `gseq`, `chain`, `seq`, `at`, `principal_id`, `kb_id`,
  `kind`, `outcome_kind`, `session` and `tenant`.

The columns are an index and never a source. Export filters on them, and verification recomputes each
from the body and fails on a mismatch, so editing a column to hide a row from a filter is detected.

Further tables:
- `heads`, the signed heads;
- `anchors`, one row per chain;
- `sessions` (`session`, `started_gseq`) and `sign_ins` (a hash of `(iss, sub, auth_time)`, with its
  `gseq`): only to know whether a session or a sign-in is new. They are disposed of with the chain
  that holds their events.

The file is mode `0600`, owned by the daemon's user.

**Append-only in the database itself.** A trigger refuses every `UPDATE`. A second trigger refuses a
`DELETE` of any row whose `seq` is past its chain's current anchor, and D9 moves the anchor only in
the transaction that disposes. Triggers do not stop the operator, who can edit the file; the chain
makes that visible (D7). They stop a stray code path.

`activity.db` joins the `mae-daemon backup` set as an SQLite source (`VACUUM INTO`).

### D4. The session id

The daemon mints its own session id for every principal kind:

| Transport | The session is | Its id |
|---|---|---|
| HTTPS with a bearer token | the token | `HMAC-SHA256(session_key, SHA-256(token))`, truncated to 128 bits, base32 |
| `/ws`, mTLS, mesh, KB socket | the connection | 128 random bits, minted when it opens |
| `cli` | the command's run | 128 random bits |

`session_key` is 32 random bytes in the daemon's state directory. The id says nothing about the token
and cannot be reversed to it.

**How each principal kind maps:**
- a **person or unbound IdP token** gets a new id at each refresh, because the token changes. Its
  `idp_sid` is recorded whenever the token carries one. Its `(iss, sub, auth_time)` ties successive
  ids to one sign-in, even when a refreshed token lacks `sid`;
- an **agent token** (ADR-118) gets a new id for each access token. The grant id ties them together;
- a **socket renewal** (ADR-099 B3) keeps the connection's id. The renewal is an event of its own.

**Every authenticated response carries `Mae-Activity-Session: <id>`.** This is the join key D1
promises.
- **Format.** The id is 26 characters: the 128 bits in RFC 4648 base32, lowercase, unpadded
  (`^[a-z2-7]{26}$`).
- **Where.** Every response of the HTTPS listener once the bearer token has been validated: `kb/query.*`,
  `kb/review.*`, the blob routes, the site, the `/ws` ticket mint, and the `/ws` upgrade (which then
  carries the connection's id, the one its events use). It is on refusals too, whenever a principal was
  established: a `403`, a `404` from an access gate, and every JSON-RPC error. A request whose token
  does not validate (`401`) has no session and carries no header. Its refusal event records the
  address instead (D2), so a relaying server joins it by time and address.
- **It is an identifier, never a credential.** No route, method or socket accepts it as
  authentication, or resumes anything by it, and it cannot be turned back into the token. A relaying
  server may log it as it is. A test scans every entry point's authentication for any read of the
  header or the id. It does not depend on the IdP sending `sid`, which is optional in the OpenID logout specs,
undefined in OIDC Core, and unspecified on refresh (research).

**A session start** is the first event the writer sees with a given id. The writer inserts into
`sessions` with `INSERT OR IGNORE`. If the row is new, it writes a `session_start` event carrying the
address before the request's own event, in the same transaction. There is no in-memory cache whose
eviction could produce a second start. A connection's end is a `session_end` event. A bearer session
has no end the daemon can see; its token's expiry is in the start event's `detail`.

### D5. The write path: an event is committed before the answer, and a full queue refuses

**The rule.** No response, and no byte of a response, leaves the daemon until the request's event has
committed. The same applies to the first frame of a document sent to a session.

**How:**
- Each entry point builds the event in an `ActivityScope`. Inner layers fill it as they decide: the
  principal after authentication, then the KB and node, then the outcome.
- The outer layer submits the event to a bounded in-process queue, with a one-shot acknowledgement,
  and awaits it before sending.
- The dispatch and the commit run in a task of their own, which the connection only awaits. A
  client that hangs up mid-request makes the HTTP server drop its side, and an effect already
  handed to another thread (a blob commit) would then land with no event. The task holds the
  connection's slot until its commit is done, so requests whose clients have gone stay bounded by
  the connection cap.
- **One connection holds at most 100 such requests** (`MAX_REQUESTS_PER_CONNECTION`). A request
  runs on to its commit after its HTTP/2 stream is reset, so without a limit of its own a client
  could open a stream, reset it and repeat, piling up commit waits on one connection. The same
  number is the listener's advertised `SETTINGS_MAX_CONCURRENT_STREAMS`, so a client that keeps to
  the stream limit never meets this one: only a client resetting streams does. Past it a request is
  refused `429 too_many_requests_on_connection` at once and counted into that kind's aggregate
  (D6). 100 is the smallest value RFC 9113 §6.5.2 recommends advertising.
- **A clean stop drains them first.** It stops taking requests, then waits for those in flight to
  commit their events, then flushes the coalesced windows and commits `daemon_stop` (D10). A request
  arriving once the stop has begun is refused `503 activity_record_unavailable` and counted into an
  aggregate that lands before `daemon_stop`. The wait is bounded by `STOP_DRAIN_TIMEOUT`, 10 s: each
  request's commit is already bounded by the commit timeout once its dispatch ends, so this bounds
  only a dispatch that does not end (a stalled upload), and ten seconds plus the writer's own stop
  stays well inside systemd's default 90-second stop timeout. A stuck request therefore cannot turn
  a clean stop into a kill that loses `daemon_stop`. If the wait runs out, the daemon logs
  `activity_record_stop_drain_timeout` (D13) and stops anyway: those requests' events may land after
  `daemon_stop`, or not at all.
- One writer task drains the queue. Each batch is everything waiting, up to 512 events, in one
  `BEGIN IMMEDIATE` transaction. While one commit is in flight, the next batch gathers, which is
  group commit with no timer.
- The writer reads each chain's tip **inside** the transaction, never from a cache, so an operator
  command appending from another process cannot fork the chain.

**`synchronous=FULL`**, so a committed event survives a power loss as well as a crash. An answered
read whose event was lost in a power cut would be a gap.

**When the record cannot take the event:**
- A full queue makes the request wait for space, within `activity.commit_timeout_ms` (default
  2,000), which also bounds the wait for the commit.
- If the timeout passes, or the writer has failed (a full disk, an I/O error), the request is refused
  `503 activity_record_unavailable` with `Retry-After`, and nothing of its answer is sent. On a
  session, the message is refused with the same kind. Yrs updates are idempotent, so the client
  resends.
- The daemon logs an error, raises the ADR-027 counters (`activity_unavailable_total`, queue depth,
  commit latency, age of the last head), and fails its health check until a commit succeeds.
- **The unhealthy state must reach a person** (owner decision, 2026-11-25). A deployment wires the
  health check and `activity_unavailable_total` into its monitoring with an alert. A fail-closed stop
  that nobody hears about is not acceptable. The deployment guide states this as a requirement of
  enabling the record, and `activity status` prints whether the last commit is older than the
  alert threshold.

**Why refuse rather than count drops.**
- The requirement makes a dropped read a gap. Counting proves the gap afterwards and does not close
  it. auditd's default does exactly that, and a production case counted 126 million lost records.
- Halting the host, auditd's `-f 2`, turns one slow disk into an outage for everyone. Refusing the
  affected request is the per-request form of SP 800-53 AU-5(4)'s degraded mode.

The cost is availability. When the record's disk fails, the daemon stops serving. That is the
requirement working as intended, and the alarm says why.

**What it costs: an estimate, not a measurement.**
- *CPU:* encoding and two SHA-256 hashes over about 600 bytes, a few microseconds per event.
- *Insert:* tens of microseconds per row within a batch. ADR-074a measured a whole single-row
  `wal_append` transaction at about 75 µs p50 on this hardware.
- *Commit:* one fsync per batch. A checkpoint's fsync moved `state.db`'s p99 to about 4 ms in that
  measurement.
- *Added latency:* a request therefore waits for one to two commits, roughly 2 to 10 ms on local
  disk, and more on network block storage.
- *Throughput:* bounded by batch size over commit time, which is thousands of events per second, far
  above ADR-109's 20-user target.

P1a's benchmark measures all of this. If the measured cost is unacceptable, O5 is the owner's
decision, not a quiet change to `NORMAL`.

### D6. Effects that commit before their event, write spans, and coalesced refusals

A read has no effect, so D5 makes it exact. A write's effect lands in another database, so the record
cannot share its transaction. Three rules close the gap.

1. **Reconcile from the effect's own attributed record.** An effect whose store already records who
   did it, in the same transaction, is recorded after it:
   - an edit, through the editor record;
   - a review act, through ADR-113's chain;
   - a membership change, through the signed op-log;
   - a binding change, through ADR-117's binding log.

   A crash between the effect and its event leaves the effect attributed in its own store. At
   startup, the writer compares each source's high-water mark with the last position the record
   references, and records what is missing as `write_reconciled` (or the matching kind), with
   `detail.recovered = true`.
2. **Write ahead when the effect has no record of its own.** A blob commit, `token mint` and a ticket
   mint each record an event before the effect: an `upload_intent`, or the act with outcome
   `pending`. The outcome follows as a second event. A failed write-ahead means no effect, as ADR-117
   D7 already rules for `token mint`.
3. **Write spans for sync sessions.** Recording every yrs update would be one event per keystroke.
   - A session's first update to a document opens a span (`write_span_open`, with the starting WAL
     id). It is committed **before** that update is applied.
   - The span closes at session end, at document close, or every `activity.span_close_secs` (60), as
     `write_span_close` with the WAL range and the update count.
   - A crash leaves a span open. Startup closes it from the editor record's `last_wal_id` for that
     editor and document.

**Refusals are coalesced, and never dropped.** Repeated refusals of the same `(principal or address,
route, refusal kind)` within `activity.refusal_window_secs` (60) are handled as follows:
- the first is an event committed before its answer, like any request;
- the rest increment an in-memory count, flushed at the window's end as one event with `count`,
  `first_at` and `last_at`.

This bounds what an unauthenticated flood can make the daemon write, so the fail-closed queue cannot
be turned into a denial of service by bad tokens. A flood spread across many addresses is bounded
too: refusals made before authentication never hold more than half the queue. Past that point they
are counted into one aggregate per refusal kind for the window, without addresses. The accepted
limits:
- a crash loses the count of the current window, never the fact that refusals of that kind from that
  source began;
- under a spread flood, the addresses beyond that half of the queue are not recorded, only their
  number.

**During an outage, refusals are not queued.** While the record is unhealthy (D5), a refusal is
answered `503 activity_record_unavailable` at once and counted, as that answer, into the aggregate
of that kind for the window. Queued instead, each would wait out the commit timeout and then land as
its own row once the writer recovers, so a bad-token flood during an outage would hold the queue and
leave a row per request. Counted, it is still never dropped: after recovery the aggregate's count
includes every one. A request refused at its connection's limit (D5) and one arriving during a clean
stop are counted the same way.

### D7. Tamper evidence: two chains, a hiding body hash, and signed heads at an interval

**Two chains, one for each retention class (D9): `long` and `short`.** Each event belongs to exactly
one chain, by a fixed function of its kind and outcome:

| Chain | Events | Retention (D9) |
|---|---|---|
| `long` | every refused outcome; `sign_in`; `session_start` and `session_end`; write spans and reconciled writes; `upload_intent`, `upload` and `download`; `review_act`; `admin`; `disposal`, `daemon_start`, `daemon_stop`, `record_disabled`, `config_changed`, `clock_step`, `head_attest` | 2 years |
| `short` | successful `request`, `read` and `subscribe` | 12 months |

Session starts and ends are in the long chain (owner decision, 2026-11-25, closing O1): a write is kept
for 2 years, and its session must resolve to an address for as long as the write is kept. The
retention record states this reason.

The function is an exhaustive `match` with no default arm, so a new kind does not compile until it is
classed. Downloads are in the long chain because the requirement lists them with writes.

**The encoding is ADR-113 E10's, with one change.** Bytes are RFC 8785 (JCS), under E10's value
restrictions. Hashes are lowercase hex, and keys and signatures are base64. The change is that the
body enters the chain only through its hash:

```
body_hash  = SHA-256("maeactivity-body/v1\n" ‖ JCS(body))
link       = {v: 1, chain, seq, gseq, prev_hash, body_hash}
entry_hash = SHA-256("maeactivity/v1\n" ‖ JCS(link))
```

`seq` counts from 1 in each chain. `gseq` counts across both, assigned by the one writer, so the
union of the two chains is gapless. `prev_hash` is the previous `entry_hash` in the same chain, or 64
zeros.

**Why the body is separate.**
- A filtered export can leave an event's content out and still verify. It ships the omitted event's
  link only, from which the verifier recomputes `entry_hash` (ADR-116 D3 separates content for the
  same reason).
- The body's 16-byte `salt` makes `body_hash` hiding. Without it, a stub's hash would confirm a guess
  at a low-entropy body (who, which node, which second), the attack ADR-116 keys its content hash
  against.

**Signed heads at an interval, not a signature per event.** A head is:

```
{format: "maeactivity-head/v1", gseq,
 long:  {seq, entry_hash},
 short: {seq, entry_hash},
 signed_at}
```

It is signed as `Ed25519(identity key, "maeactivity-head-sig/v1\n" ‖ JCS(head))` with the daemon's
identity key, as ADR-116 D4 and ADR-113 E10 sign. The writer appends a head in the first batch after
`activity.head_interval_secs` (10) has passed. It also appends one on the maintenance tick when any
event is newer than the last head (so an idle daemon does not leave its last events unsigned), at
clean shutdown, and before every export.

- *Cost:* a signature per event would add a few tens of microseconds in the single writer, and 64
  bytes on each of millions of rows a year, which is an estimate.
- *What it buys:* each event could be verified alone in a filtered export. The stubs above give the
  same assurance for the whole range, and say whether anything is missing from it, which a signature
  per event cannot.
- *Precedent:* CloudTrail signs a digest once an hour.
- *The accepted window:* events younger than the last head are protected by the chain only, for up to
  one interval plus one batch.

**The honest limit (ADR-116 D4, CT).** Someone with the daemon's key and its files can rewrite the
record and sign it again. That shows only against a head kept off the host. So:
- `mae-daemon activity head` prints the latest head, for the operator to ship elsewhere;
- every export and every backup includes one;
- `verify --against <head-file>` checks the record against a head kept outside.

The same comparison is the only way to see a restore from an old backup, which rolls the record back.

Forward-secure sealing is rejected (Rejected, below).

### D8. Readers, and who never reads it

- **The operator's export:**

  ```
  mae-daemon activity export [--principal ID] [--kb ID] [--kind K] [--outcome ok|refused|KIND]
                             [--session ID] [--since T] [--until T] [--no-proof] --out FILE
  ```

  It opens the file read-only, and records its own run first, as a `read` by `operator:<os user>`.
  The output is JSON Lines:
  - `{type: "header", format: "maeactivity-export/v1", daemon_key, filter, from_gseq, to_gseq}`;
  - one `{type: "anchor", chain, through_seq, through_gseq, through_entry_hash, disposal_gseq}` line
    for each chain that has been disposed of (D9);
  - for each event in range, in `gseq` order: `{type: "event", link, body}` if it matches the filter,
    or `{type: "omitted", link}` if not, where `link` and `body` are base64 of the canonical bytes;
  - `{type: "head", canonical, sig}` lines where heads fall, and a fresh head last.

  `--no-proof` leaves the omitted lines and heads out: a smaller file for a person reviewing patterns,
  which cannot be verified.
- **Offline verification**, with no daemon configuration or database:

  ```
  mae-daemon activity verify --in FILE --key KEY [--against HEAD]
  ```

  1. Decode each `link`.
  2. Check that each chain's `seq` is consecutive from its anchor, and that `gseq` strictly
     increases. A `gseq` missing above both chains' anchored `through_gseq` is a gap; below that, it
     is a disposed event of the other chain, whose own links show any removal.
  3. Check `prev_hash` against the previous `entry_hash` in the chain, or the anchor for the first.
  4. Recompute `entry_hash`.
  5. For an `event` line, recompute `body_hash` from `body`.
  6. Verify every head's signature, and that it names the `seq` and `entry_hash` at its position.
  7. For each anchor, check that the disposal event at `disposal_gseq`, which is in the file, states
     the same values.

  A failure names the first bad `gseq`. A missing range is reported as a range. `--key` and
  `--against` come from outside the file, never from its header (E10).
- **The auditor** (ADR-117 D9): `activity/query` on the HTTPS listener takes the export's filters,
  pages through the same objects, and is read-only. Only a person session that holds the auditor
  role reaches it. Each call is itself an event.
- **No agent reads it** (principle #16):
  - `activity/*` refuses an `agent` principal at the route, before anything else;
  - it is not served on `/mcp` (ADR-118 D9);
  - no editor MCP tool, Scheme primitive or command reads or configures it;
  - its settings exist only in `daemon.toml`.

  A record of what agents did that an agent could read would show it which of its acts were noticed.
  One it could configure would not be a control.
- **No other reader.** A KB owner does not see who read their KB (O2).

### D9. Retention and disposal: a signed prefix, never a hole

**Periods.** `activity.retention_long_days` (default and floor: 730) and
`activity.retention_short_days` (default and floor: 365). Configuration may lengthen them, never
shorten them: a value under the floor stops the daemon at startup. Shortening would be deleting early
by another name. A changed value is a `config_changed` event at the next start.

**Disposal** runs on the maintenance tick, once a day. For each chain, it takes the **longest prefix**
whose every event has `at` older than the chain's period, stopping at the first that is not. In
passes of at most 10,000 rows, each pass is one transaction that:
1. appends a `disposal` event to the `long` chain, with the chain, `through_seq`, `through_gseq`,
   the `through_entry_hash`, the count, the oldest and newest `at` disposed, and the period applied;
2. moves the chain's row in `anchors` to that position;
3. deletes the prefix, which the delete trigger now permits;
4. deletes the matching `sessions` rows, for the `short` chain.

The `long` chain disposes of its own prefix the same way. A disposal event that is a chain's current
anchor is never itself disposed.

**Why the record stays verifiable.**
- The first remaining event's `prev_hash` equals the anchor's `through_entry_hash`.
- The anchor is stated by a `disposal` event that is itself in the chain and covered by a head.

Verification therefore starts at the anchor, as CloudTrail's starts at a stated digest. Nothing is
ever removed from the middle of a chain. Deleting old rows from a single chain would break it, and
keeping their hashes as tombstones would keep a guessable trace of personal data. Two chains, each
disposed of from the front, avoid both.

**The clock cannot dispose early.** A backwards step makes a later event look older. Disposal stops
at the first event that is not old enough, so an out-of-order `at` can only keep events longer.

**Nobody deletes otherwise.** There is no method, command or option that removes events, and no
switch that disposes faster. A hold suspending disposal (an investigation) is not designed (O6).

### D10. The clock and the order

- **Time.** `at` comes from the host's clock, which the deployment keeps on NTP. Precision is
  microseconds. It is stamped when the event is built, when the daemon decides, not when it commits.
- **Order.** `gseq` is the order, assigned by the one writer in commit order. Two events' `at` can
  invert by up to one batch.
- **Steps.** When an event's `at` is more than one second before the latest committed `at`, the
  writer records `clock_step` with both values, so a step back is visible rather than silent.
- **Restarts.** `daemon_start` (with version, configuration digest, retention and tips) and
  `daemon_stop` bracket each run. A start without a stop is a crash.

### D11. Switching it on and off

`[activity] enabled` defaults to `false`. The requirement belongs to one deployment's framework, and a
local daemon serving one editor (principle #12) has no reason to keep a record of its user.
`mae-daemon --check-config` warns when the HTTPS listener is on and the record is off.

Turning it off is visible. A daemon that starts with the record off, while an `activity.db` exists,
appends one `record_disabled` event, signs a head, and writes nothing more. There is no switch that
turns off part of the record: no per-route opt-out and no sampling.

**The settings, all in `daemon.toml`:**

| Setting | Default |
|---|---|
| `enabled` | `false` |
| `queue_capacity` | 4096 |
| `commit_timeout_ms` | 2000 |
| `head_interval_secs` | 10 |
| `span_close_secs` | 60 |
| `refusal_window_secs` | 60 |
| `retention_long_days` | 730 |
| `retention_short_days` | 365 |
| `trusted_proxies` | `[]` |

### D12. One record: what it replaces and what it reuses

| Existing or designed | Becomes |
|---|---|
| `doc_editors` (built) | Unchanged. It is the source D6 reconciles from, and write spans carry its WAL range. It stays where it is because the author rule needs it in the edit's own transaction. |
| ADR-116 history (built, P1–P3) | Unchanged: content versions, a different purpose, a different retention. |
| ADR-113 review chain (designed) | Unchanged: per KB, never deleted, exported per KB. A `review_act` event references its `(kb_id, seq, entry_hash)` and does not copy it. |
| ADR-117 D7 operator log (designed) | **Replaced** by `admin` events with transport `cli`. The write-before-effect rule moves into D6. |
| ADR-117 D7 binding log (designed) | Kept in `identity-bindings.db`, because it must share the binding change's transaction. Its head is attested here (`head_attest`), in place of "the daemon's own chain". |
| ADR-117 D9 auditor-call logging | **Is** this record. |
| ADR-118 D7 read logging with agent, person and grant | **Is** this record's `principal`. |
| `tracing` | Unchanged and not a record. Nothing that needs to be complete or durable goes only there. |
| ADR-027 metrics | Gains the writer's counters (D5), which carry the AU-5 alert. |

### D13. The operator contract: health, status, and the head line

A deployment's monitoring and log collector are built against exactly this, so it is part of the
decision, and a test pins each line of it.

**Health.** `GET /api/health` on the OAuth listener (the address in `oauth.bind`), unauthenticated,
status only, as ADR-111 P1 defines it:
- `200 {"status":"ok"}` when the record is disabled, or enabled and taking events;
- `503 {"status":"unavailable"}` when the record is enabled and the writer has failed, or an event has
  waited past `activity.commit_timeout_ms` without a successful commit since. Nothing else is said:
  the public surface carries no detail (`health_opens_nothing_else`).
- It recovers by itself. The writer retries the failed batch, and the first successful commit returns
  the health check to `200`. No restart is needed, and a restart does not skip the alert: a daemon
  that cannot open `activity.db` at start-up answers `503` from its first request.

**Status, with the counters.** `mae-daemon activity status [--json]` on the host. Exit 0 healthy, 1
unhealthy, 2 when it cannot reach the running daemon. `--json` prints one object:
`{"healthy", "unavailable_total", "queue_depth", "last_commit_age_ms", "last_head_age_s",
"chains": {"long": {"seq", "entry_hash"}, "short": {"seq", "entry_hash"}}}`. mae-daemon has no
metrics endpoint; this command is where the counters are read.

**Log lines.** Through the daemon's ordinary log (stdout): one `ERROR` line with the stable message
`activity_record_unavailable` on the transition to unhealthy, and one `INFO` line
`activity_record_recovered` on recovery. A collector may alert on either signal; the health check is
the authoritative one. One `ERROR` line `activity_record_stop_drain_timeout`, with the number still
in flight, when a clean stop's wait for requests in flight (D5) runs out.

**The head line.** Each signed head is written to **stdout** as one line holding one JSON object,
outside the log formatter, so it is never wrapped in a log prefix. The line begins with exactly
`{"mae_activity_head":1,`, which is how a collector selects it. Its fields:
- `mae_activity_head`: the line format's version, `1`;
- `chain`: `"long"` or `"short"`;
- `seq`: the last event the head covers;
- `entry_hash`: that event's `entry_hash`, lowercase hex;
- `at`: the head's time, RFC 3339 UTC with milliseconds;
- `key_id`: the SHA-256 of the daemon's Ed25519 public key, lowercase hex;
- `sig`: the Ed25519 signature, base64, over the head's canonical bytes (D7's domain prefix and the
  RFC 8785 encoding of the object without `sig`).

**Cadence:** a head for each chain at start-up and at shutdown, and every `activity.head_interval_secs`
(default 10) for a chain that advanced since its last head, never for an idle one. So an idle daemon
writes nothing between its start and stop lines, and `last_head_age_s` grows; that is not an alarm by
itself.

**The external copy.** `mae-daemon activity verify --key <public-key-file> --against <file>` reads
`<file>` as lines, ignores every line that does not begin with the head prefix, and checks the record
against the latest head of each chain that verifies under `--key`. The log host's copy is therefore
simply the collected lines, one file or a concatenation, in any order. A head with a bad signature
fails verification; it is never skipped silently.

## Consequences

- Every request to a daemon with the record on is either recorded or refused. A read is never
  answered unrecorded. A write is recorded or reconciled from its own attributed store.
- **Availability now depends on the record's disk.** A full or failing disk stops service, loudly.
  That is the requirement's choice, and the alarm states it.
- **Latency.** Each request waits for a group commit with fsync, estimated at 2 to 10 ms on local
  disk. P1a's benchmark measures it.
- **Storage.** About 0.8 KB per event with its indexes, an estimate. At ADR-109's target of 20 users,
  and an assumed 1,000 events per user per day, the 12-month class holds about 7 million events,
  roughly 6 GB at steady state. ADR-109's budget must count it, and the benchmark replaces the
  estimate.
- **The operator remains the trust anchor.** The record makes tampering evident, as far as a head is
  kept elsewhere. It cannot make it impossible.
- A relaying server keeps its own record. The daemon's records the relay's address, not the user's,
  and the session header is how the two are joined.
- ADR-117 has one fewer log to build, and ADR-118 has nowhere else to put its read logging.

## Rejected

- **Tables in `state.db`.** Every read would queue in the KB store's single FIFO write slot, behind
  compactions and checkpoints, and the record's durability would be `state.db`'s `NORMAL` (D3).
- **One database per tenant.** Tenants are quota groupings of principals, and a KB or a sign-in does
  not belong to exactly one of them (D3).
- **Counting drops and raising an alarm.** It proves the gap and does not prevent it. Halting the host
  is the other extreme, and it was rejected too (D5; auditd).
- **Answering before the event commits** (enqueue, then answer). A crash or power cut loses the batch
  in flight, and those are answered reads with no record. pgaudit has the same window between commit
  and log.
- **A signature per event.** Its cost scales with traffic, and it does not show what is missing from
  a filtered export; stubs and heads do (D7).
- **One chain, with old rows deleted.** It breaks the chain. Keeping hash-only tombstones keeps a
  guessable trace of the personal data that retention exists to remove (D9).
- **Temporal sharding** (CT's annual shards, adapted to months). Retention becomes as coarse as the
  shard, and the heads to keep multiply. A signed prefix disposal is as verifiable, at a day's
  granularity.
- **Forward-secure sealing.** journald's FSS, the widely deployed implementation, carried a flaw for
  about a decade that let past entries be forged undetected, and it never prevented deletion. MAE does
  not write its own. A head kept off the host covers the stolen-key case with standard Ed25519.
- **The IdP's `sid` as the join key.** It is optional, defined only by the logout specs, and
  unspecified on refresh (D4).
- **Recording each sync update.** That is one event per keystroke. Spans plus the editor record give
  the same accountability (D6).
- **`tracing` as the record.** It is not complete, not durable, and not protected.

## Build sequence

Each phase is one PR, mergeable and green alone, off by default until `enabled` is set. Every test
fails without its code. Every PR touches an append-only record with personal data or the daemon's
signing key, so each gets one independent review at the end. Mutants are declared per PR, run
diff-scoped between rounds and in full before merge (ADR-113's rules).

### P1a. The record, the writer, and the HTTPS listener (L)

**Builds:**
- `mae_daemon::activity`: the body and link types, JCS encoding, the chain, the class function, and
  the address rule;
- `activity.db` with its triggers;
- the writer, its queue, group commit, `FULL`, and tip-in-transaction;
- `ActivityScope` on every branch of `handle_request`;
- session ids, the `sessions` table and the response header;
- sign-in observation from `(iss, sub, auth_time)`;
- coalesced refusals;
- `daemon_start`, `daemon_stop` and `clock_step`;
- `activity export` with every filter, the stubs, and `--no-proof`;
- `activity verify`, chain only until P2;
- the backup source;
- the configuration and its floors.

**Does not build:** collab sessions, the KB socket, reconciliation (P1b); heads (P2); disposal (P3);
the auditor (P4).

**Tests:**
- **Completeness.** A table test walks every route and every JSON-RPC method the listener dispatches,
  taken from the dispatcher's own list, not a copy. Each yields exactly one event with a classified
  outcome. A method added without a classification fails it.
- **Fail closed.**
  - With the writer held by a test hook, a `kb/query.get` gets `503 activity_record_unavailable`,
    and its body holds no node content.
  - With capacity N and the writer held, request N+1 is refused at the timeout. Releasing the writer
    lets the next request through.
  - A writer I/O error fails the health check.
- **The address.** Over a seeded random mix of events, no event outside `sign_in`, `session_start`
  and refused outcomes has a non-null `peer`, whatever the caller set. A forwarded address from an
  untrusted peer is never recorded.
- **Session.**
  - Two requests with one token share an id and one `session_start`.
  - A refreshed token gets a new id and a new start.
  - The header equals the recorded id.
  - A token without `sid` still has a session, with `idp_sid` null.
- **Chain.**
  - Flipping any byte of any stored link or body fails verify.
  - Removing, swapping or reordering a row fails it.
  - Editing an indexed column fails it.
  - `UPDATE` and an unanchored `DELETE` are refused by the triggers.
- **Concurrency.** An operator command appending while the daemon writes leaves both chains linear,
  over deterministic interleavings.
- **Export.**
  - A principal filter returns only that principal's events, and the file verifies.
  - Altering a stub fails it.
  - A filter matching nothing still verifies over its range.
  - A `--no-proof` file is refused by verify, as unverifiable.
- **Coalescing.** 10,000 bad-token requests from one address write a bounded number of rows. At a
  clean shutdown their counts sum to 10,000, and the first of each window is committed before its
  answer.
- **Clock.** An injected backward step records `clock_step`, and `gseq` order holds.
- **Detached requests stay bounded.** Over one HTTP/2 connection, requests reset after they began
  never hold more than the connection's limit, and the next are refused `429` and counted. A clean
  stop with an upload paused after its effect waits for it, and its event commits before
  `daemon_stop`. A bad-token flood during an outage queues nothing past the first refusal, and its
  counts sum to the flood.

**Mutants:**
- the acknowledgement await removed, so the answer goes before the commit;
- the address rule skipped for reads;
- `prev_hash` or `gseq` left out of the link;
- `salt` left out of the body hash;
- the column-against-body check removed from verify;
- each trigger dropped;
- the session-start insert inverted;
- the 503 path sending the prepared body;
- a read classed into `long`;
- the tip read from a cache;
- the per-connection limit ignored, or its refusals not counted;
- a clean stop not waiting for requests in flight, or still taking new ones;
- refusals queued, or not counted, during an outage.

**Benchmark** (`#[ignore]`): added latency p50 and p99 at 50, 200 and 1,000 events per second, bytes
per event, and batch sizes. Budgets are set from the first run (O5).

### P1b. Sessions, the KB socket, and reconciliation (M)

**Builds:**
- events at `run_session`'s dispatch for mTLS, `/ws` and mesh;
- reads on fetch and subscribe, committed before the first frame;
- write spans;
- the KB socket's peer credentials;
- the `cli` principal for the operator commands that exist;
- D6's reconciliation at startup;
- write-ahead for blob commit and token and ticket mint.

**Tests:**
- A subscription's first frame is not sent while the writer is held.
- A span opens before its first update is applied, and closes with the editor record's WAL range.
- Killing the daemon between an edit's commit and its span close leaves, after restart, a closed span
  equal to the editor record. The same kill between a blob commit and its `upload` event leaves
  `upload_intent` and a reconciled outcome.
- A mesh session's edits are recorded as the relay's, never as a person's.
- A failed write-ahead means no blob and no token.

**Mutants:** the span opened after the update; reconciliation skipping one source; write-ahead
moved after the effect; the mesh peer recorded as `fingerprint`.

**P1 (P1a + P1b) is the first production use:** every request recorded, append-only,
access-controlled, classed for retention, and exported with filters.

### P2. Signed heads (M)

**Builds:** heads at the interval, at shutdown and at export; each head also written to the daemon's
log as one structured line (`activity_head`, the signed head as JSON) for the deployment's collector
(O3); `activity head`; `verify --key` and `--against`, which accepts a head taken from that log;
heads in backups; `head_attest` for ADR-117's binding log when that is built. It reuses
ADR-116's signing code.

**Tests:**
- A forged head fails.
- A record rewritten and re-signed with the right key passes `--key` and fails `--against` an older
  external head.
- A restored older backup fails `--against` the latest external head.
- A head copied from the daemon's log line verifies with `--against`, and one with a byte changed does
  not.

**Mutants:** the domain prefix dropped; the head not naming `short`; `--against` reading the file's
header key.

**When.** Deferring this phase to after P1 loses nothing that P1's chain does not already give against
someone who cannot recompute it. The first head signs everything before it as it stands.

### P3. Disposal (M)

**Builds:** D9's daily pass, anchors, `disposal` events, the trigger's anchor bound, the floors at
startup, anchors in export and verify.

**Deadline.** This must ship before the first production event is 365 days old. Until then nothing is
due, so deferring it loses no property. P1a's startup logs an error once the oldest event is 330 days
old, a check that P3 replaces with disposal itself.

**Tests:**
- Over seeded random event mixes and clock steps, nothing younger than its period is ever disposed.
- After disposal, the export verifies from the anchor.
- A deletion past the anchor is refused.
- A floor violation stops startup.
- Disposal of the `long` chain anchors itself.
- The current anchor's disposal event survives.

**Mutants:** the prefix scan not stopping at the first young event; the anchor moved after the delete;
the floor check removed; a `short` period applied to `long`.

### P4. The auditor's read (M)

**Depends on** ADR-117 P0 (the caller context and second factor) and P8 (the auditor role).

**Builds:** `activity/query` with D8's filters and paging; agents refused at the route; each call
recorded.

**Tests:**
- An agent token, a non-auditor person, and an auditor without the second factor are refused. Each
  refusal is recorded.
- The method is absent from `/mcp`.
- A grep test finds no editor tool or Scheme primitive naming `activity/`.

**Mutants:** the agent check moved after the role check; the call not recorded.

### P5. Agents and alerts (S)

**Builds:**
- the `principal` fields filled for ADR-118's agent tokens, as part of its phases;
- a storage-capacity warning, at a configurable free-space floor on the record's filesystem
  (AU-5(1)).

**Tests:**
- An agent's read names the agent, P and the grant.
- The warning fires before the disk fills.

## Open items

- **Decided 2026-11-25 (O1):** session starts and ends are kept 2 years, in the long chain, so every
  kept write resolves to an address (D7's class table).
- **O2. KB owners reading their KB's activity.** Not designed. It is accountability against the
  readers' privacy, and needs its own decision.
- **Decided 2026-11-25 (O3):** every signed head is also written to the daemon's log as one
  structured line (P2). The deployment's log collector carries it to its log host, which keeps it with
  the same restricted access and retention as the record. The external head that `verify --against`
  checks is the one on that host.
- **O4. Filtered exports grow with the range**, at one stub per omitted event: about 200 bytes each,
  or about 1.4 GB for a year at the storage estimate. A Merkle tree per batch (Crosby and Wallach's
  history tree) would replace stubs with logarithmic inclusion proofs. Adopt it if the size proves to
  matter.
- **Decided 2026-11-25 (O5):** `FULL` stays, and P1a measures the added latency. Relaxing it to
  `NORMAL` (safe against a crash, but losing the last commits on a power cut) is the owner's decision,
  brought back with the measurement.
- **O6. A hold on disposal**, for an investigation. Not designed. It would suspend D9 for a chain or
  range, and be recorded like any act.
- **O7. Unverified claims in refusals.** D2 records a refused token's claimed `iss` and `sub`,
  labelled. If the deployment treats that as recording personal data about people who did not make
  the request, drop the field.
