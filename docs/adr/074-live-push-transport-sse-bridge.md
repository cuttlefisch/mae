# ADR-074: Live push transport for the HTML KB view (SSE bridge into the OAuth listener)

**Status:** Proposed.
**Depends on:** ADR-073 (the live HTML KB view this ADR adds push to), ADR-025/ADR-026/
ADR-027 (the P2P daemon-mesh transport/integrity/observability trio whose event plumbing,
`EventBroadcaster`/`SharedBroadcaster`, this ADR reuses rather than reinventing).
**Relates to:** ADR-054 (daemon concurrency hardening — the `ConnLimiter` discipline every
existing listener already applies; this ADR's new SSE connections must too).
**Tracking:** tracker issue TBD (see ADR-070's header).

## Context

ADR-073's v1 is poll-based: the browser page's JS re-fetches `kb/query.get`/`.graph` on an
interval. A genuinely live view should instead be told when to re-fetch, not guess on a
timer. MAE already has a real, working push mechanism —
`EventBroadcaster`/`SharedBroadcaster` (`shared/mcp/src/broadcast.rs`) — but it is wired
**only** to the mTLS TCP collab listener and the P2P mesh (confirmed:
`daemon/src/main.rs`'s `run_oauth_listener` call, line 360, passes `doc_store` and a
connection limiter but never `broadcaster`; `run_oauth_listener`'s own signature has no
broadcaster parameter). There is zero WebSocket/SSE infrastructure anywhere in the
codebase today.

## Decision

### D1 — Wire protocol: Server-Sent Events, not WebSocket

The feed this view needs is strictly one-directional: server → browser ("this KB's
content changed, re-fetch it"). The browser never needs to push anything back over this
specific channel — any real mutation a user makes goes through its own separately
authenticated request, not this channel. Given that:

- SSE is plain HTTP — it reuses the exact same bearer-token validation and TLS handshake
  already terminated for every other request on this listener, with no protocol-upgrade
  dance.
- `hyper` v1 (already the listener's HTTP implementation, per ADR-052) supports
  chunked/streaming responses natively — no new dependency (e.g. `tokio-tungstenite`,
  never previously needed in this codebase) is required.
- WebSocket's bidirectional capability would be unused overhead for a read-only push feed.

WebSocket is rejected for this ADR on those grounds; it remains available to reconsider if
a future, genuinely bidirectional need arises.

### D2 — Bridge design

Thread `broadcaster: Option<SharedBroadcaster>` into `run_oauth_listener`'s signature
(currently `doc_store`-only) and its `daemon/src/main.rs` call site (line 360), mirroring
exactly how the collab TCP listener already receives it (`main.rs`, the
`run_collab_server`/`accept` path). Add a new SSE route (e.g.
`GET /kb/{kb_id}/view/events`) that:

1. Authenticates identically to every other route on this listener (bearer token → 
   `kb_access` principal resolution — no new auth logic).
2. Subscribes to `sync_update` (and any future KB-diagram-relevant event types) via the
   broadcaster's existing `subscribe`/`add_event_sub`/`subscribe_doc` API
   (`shared/mcp/src/broadcast.rs`), **filtered to exactly the requesting principal's
   accessible KB/node scope** — this filtering is the single most security-critical piece
   of this ADR (see Consequences).
3. Forwards a thin "changed, re-fetch `{kb_id}`/`{node_id}`" signal as an SSE `data:`
   frame — never the raw changed content itself. The browser's existing `kb/query.get`
   call (unchanged from ADR-073) does the actual authenticated, encryption-aware fetch.
   This keeps the push channel free of any encryption/redaction logic duplication — that
   logic lives in exactly one place (`kb_query.rs`), never two.
4. Client-side JS (`daemon/src/webview.rs`) replaces `setInterval` polling with an
   `EventSource` listener, falling back to polling if the `EventSource` connection drops
   (gate G1: no silent capability degradation — a client that loses push must still work,
   degraded but honest, not silently stale).

### D3 — Backpressure and lifecycle

Reuses `ConnLimiter` (ADR-054) for SSE connection caps, identically to every other
listener on this daemon. Defines an idle-connection reap policy for abandoned SSE streams
— explicitly not repeating the KB Unix socket's own previously-identified lack of one.

## Consequences

**Cross-KB event leakage is the primary risk this ADR introduces.** A principal
authenticated for KB A's SSE stream must never receive an event for KB B, even one they
also happen to have access to under a *different* bearer token session, let alone one they
have no access to at all. This is not a theoretical concern — `EventBroadcaster` today
filters by event type and doc id, and the new bridge must apply the SAME `kb_access` scope
check per-event as `kb_query.*` already applies per-request, not merely at
subscribe-time. This is a hard gate on this ADR's own adversarial test (below), not an
optional nice-to-have.

## Verification

Follows the existing three-tier daemon convention (per ADR-073's own verification
section), with this ADR's tier-2 dispatch-adversarial test specifically targeting the
cross-KB leakage risk: subscribe as a principal with access to KB A only, trigger a real
mutation on KB B via a separate authenticated path, assert **zero bytes** referencing KB B
ever reach the SSE stream (not merely that the client-side code ignores them — the
server must never send them). A real-binary e2e test confirms an `EventSource` connection
over real TLS actually receives a real event after a real KB mutation, and that dropping
the connection doesn't leak a resource or crash the listener. A comparative test confirms
the polling fallback (D2.4) actually activates when the SSE connection is severed
mid-session.

## Amendment 1 (proposed, 2026-11-12)

**Change notices for many consumers: one per-KB change index, read over SSE and over ADR-099's
socket.**

**Status of this amendment:** Proposed. A draft for the owner's decision; nothing here is built. The
header above is unchanged. Evidence: `docs/research/074a-change-feed-prior-art.md`.

### Why amend

ADR-074 was written for one page watching one KB. What is needed now is a feed that many readers
consume at once: ADR-073's page, browser readers like it, and read-only services. Several readers
may watch several KBs, and readers disconnect and come back. D2 planned to subscribe each stream to
`EventBroadcaster`. Checked against the code and the prior art, that plan has four problems:

- **No durable position.** The broadcaster's sequence (`next_seq`, `shared/mcp/src/broadcast.rs`) is
  in memory and restarts at 1, so a reader that reconnects has nothing to resume from. ADR-074 says
  nothing about resumption at all.
- **It carries content.** A `sync_update` holds the update bytes. D2.3 then had to strip them, so
  every stream task would handle content it must never send.
- **The wrong scope.** A collab session subscribes per document. A KB-wide reader would have to
  subscribe to every node document of the KB, and to new ones as they appear.
- **Its overflow rule fits sessions, not readers.** On overflow it closes a document's data and
  drops other events (ADR-099 B4). That is right for a session that resyncs, and has no meaning
  for a reader.

ADR-099 has since moved the write path to a WebSocket, and its Amendment 1 says ADR-074 D1 stands
for read-only views. This amendment keeps D1. It replaces D2's bridge, extends D3, and settles five
questions: what a notice says, how a reader resumes, how each notice is authorized, what fan-out
costs, and how SSE and the WebSocket share one source.

**What the code has today, checked 2026-11-12:**
- `DocStore::apply_update` and the replace path behind `share_doc` and `restore_doc` already call
  `emit_change`. It feeds the projector and covers hub, socket and mesh writes alike.
- The WAL append is one `BEGIN IMMEDIATE` transaction, which already writes the editor record
  (ADR-113 D3).
- `kb/query.*` reads CRDT state from `DocStore`, not the Cozo projection.
- `kb/query.list` already returns a per-node `version` "so a reader can poll for what changed".
- ADR-073's page polls `kb/query.graph` and `.get` every 5 s (`daemon/src/webview.rs`).
- The listener's TLS configuration advertises no ALPN protocol, so a browser that connects to the
  daemon directly speaks HTTP/1.1.

### C1. One notice source: a per-KB change index written in the commit's own transaction

Every durable KB write records a notice in the same storage transaction as its WAL entry. Two new
tables hold the notices:

- **`kb_feed(kb_id PRIMARY KEY, epoch, head, floor)`**:
  - `head` is the KB's last sequence;
  - `floor` is the lowest cursor from which replay is complete;
  - `epoch` is a random 32-bit value, created with the row.
- **`kb_changes(kb_id, subject, seq, op, PRIMARY KEY (kb_id, subject))`**, indexed on
  `(kb_id, seq)`. `subject` is a node id, or the reserved subject for the KB itself. There is **one
  row per subject, holding its latest change**.

Each subject a write records increments `head` once and upserts that subject's row with the new
`seq`. This happens inside the transaction `wal_append` already opens. What is recorded:

- **A node document write:** `(node, upsert)`.
- **A collection document write** (`kbc:{kb_id}`): the manifest is compared before and after the
  update, under the document lock `apply_update` already holds.
  - A node the manifest gained is recorded as `(node, upsert)`.
  - A node it lost is recorded as `(node, delete)`.
  - Any other change, such as membership, the op-log or settings, is recorded as
    `(kb, changed)`.
- **A KB blocklist change** (`add_kb_block` / `remove_kb_block`): `(kb, changed)` in its own
  transaction. The blocklist changes access without touching any document.
- **Ephemeral documents** (`is_durable_doc` false) record nothing.

After commit, the only in-memory signal is a per-KB `tokio::sync::watch` holding `head`. It is a
wakeup, never the data.

**Why this shape:**
- **Commit order is sequence order.** SQLite has a single writer and the increment happens inside
  the transaction, so a reader that sees `seq` *n* has also seen every *m* < *n*. Delivering
  straight from the broadcaster would lose this. Concurrent writers broadcast after commit in
  either order, so a reader could receive 8, disconnect, resume from 8 and never see 7. (Postgres
  NOTIFY: the notification is a signal; data goes in a table.)
- **A notice exists if and only if its change does.** A failed append rolls both back, which is the
  transactional outbox.
- **One row per node** (CouchDB's "only the most recent change for a given document", Kafka's
  compacted topics) makes the replay window unbounded for everything except deletes. Storage grows
  with the number of nodes, not with the edit rate.
- **A sequence per KB, not per store.** A per-store sequence would let a reader of one KB count
  every other KB's writes, from other tenants included, by the gaps in its own cursor. It would
  also need Kubernetes-style bookmarks to keep a quiet stream's cursor current.

**Rejected:**
- **`EventBroadcaster` as the source** (D2 as written), for the four reasons above.
- **Deriving the feed from the `wal` table.** Compaction trims it, it has no `kb_id` or operation,
  and its rows hold update bytes.
- **An append-only event log with a time window** (Kubernetes' 5 minutes, GitHub's 300 events,
  Stripe's 30 days). These make the long-away reader the usual case for a reset.
- **Emitting after projection.** The projector can be disabled. And `kb/query.*` does not read the
  projection, so waiting for it buys nothing.
- **Buffering notices in memory and flushing them in batches.** A crash would lose the latest row of
  a node, and a reader whose cursor had already passed it would never be told.

### C2. The event schema: thin notices only

```
id: <cursor>
event: change
data: {"kb":"<kb_id>","node":"<node_id>","op":"upsert","seq":1042}
```

- **`op` is `upsert` or `delete`.** A `kb_changed` notice has no `node` and means "re-read what you
  hold about this KB" (`kb/query.capabilities`, then whatever it shows). There is no "created": a
  reader that does not know a node fetches it.
- **The fetch is the truth.**
  - A `delete` may later be followed by an `upsert` for the same node, because content can arrive
    before the manifest lists it (ADR-105).
  - A fetch that finds no node means the node is absent.
  - Delivery is at least once, and a reader handles a notice by re-fetching, which is idempotent.
- **`seq` lets a reader skip a fetch.** `kb/query.get` and each `kb/query.list` entry gain a `seq`
  field: the node's change-index sequence, read **before** its state. Because `apply_update`
  applies in memory before it commits, the returned state includes at least that change. A reader
  holding `seq` ≥ the notice's has nothing to fetch.
- **`kb/query.list` returns the KB's `cursor`.** It is read before the first page's scan, and the
  reader keeps it from the first page. This is Kubernetes' list-then-watch: subscribe from that
  cursor and nothing between the list and the subscription is missed.
- **Control events:**
  - `reset {kb, reason, cursor}`, described in C3;
  - `access_lost {kb}`, described in C4;
  - `renew_needed {deadline}` and `closing {reason}`, described in C4;
  - a `: keepalive` comment, described in C5.

  These events carry no `id`, except `reset`, which carries the new cursor.

**A notice never carries:**
- title, body or tags, even for a plaintext KB;
- the author or editor of the change. **Decided (2026-11-13):** version history (ADR-116) is the
  single source of truth for who changed what. A view of authorship reads from there, behind
  history's own gate, never from a notice;
- membership details;
- timestamps;
- update bytes;
- anything `kb/query.list` would not return to the same principal.

That last rule is the invariant the leak test checks.

For an E2E KB, the node ids and the fact of a change are metadata the daemon already sees (ADR-037,
"which node changed"), and `kb/query.list` already returns them to members. The feed reveals
nothing beyond that.

**Rejected:**
- **Snapshot notices with content** (Stripe's snapshot events). A notice with content would need
  the encryption and redaction logic of `kb_query.rs` a second time, and Stripe itself warns that
  the snapshot "can be stale".
- **The node's content `version` digest in the notice.** It would mean materializing the node on
  every update under the lock.
- **The yrs state vector.** It is large, and it exposes client ids and per-client edit counts.

### C3. Resumption

**The cursor is the event id, and it covers every KB in the stream.** Its form is
`1:<tag>.<epoch>.<seq>,<tag>.<epoch>.<seq>,…`:
- `tag` is the first 12 hex digits of SHA-256 of the `kb_id`;
- `epoch` is the KB's feed epoch;
- `seq` is the KB's last delivered sequence.

It contains no CR, LF or NUL (the SSE id rules). It is not a credential: the cursor is a position,
not a grant, and access is checked again on every open (C4).

**Giving and reading the cursor:**
- The client sends the cursor back as `Last-Event-ID`.
- A client starting from `kb/query.list` cursors may give them in the URL instead, as
  `GET /events?kb=<kb_id>@<epoch>.<seq>&kb=…`. When both are present, `Last-Event-ID` wins per KB,
  as in CouchDB.
- The stream accepts a set of KBs because of the browser's six-connection limit on HTTP/1.1, shared
  across every tab of an origin (MDN). One stream per KB would leave a reader page almost no
  connections for its own `kb/query` requests.
- Two KBs in one stream whose tags collide are refused at open (`kb_tag_collision`). At 48 bits
  this is improbable, and the check is deterministic.

**For each KB at open:**
- **The cursor is honoured** when its epoch matches, `floor` ≤ `seq` ≤ `head`, and at most
  `events_max_replay` rows lie past it. The stream replays those rows in `seq` order, one row per
  subject, then goes live.
- **Otherwise the stream sends `reset {kb, reason, cursor}` for that KB and carries on.** The
  reasons are:
  - `no_cursor`;
  - `epoch_changed`;
  - `ahead_of_head`;
  - `below_floor`;
  - `too_many_changes`.

  The `reason` is always present, so a reader can tell a gap past the replay limit from a restore
  or a bad cursor in its logs. `too_many_changes` also carries the limit it exceeded
  (`{kb, reason: "too_many_changes", limit, cursor}`), and the limit is documented with the route.

  The reader relists that KB (`kb/query.list`, paged, with its cursor) and continues from the
  `reset`'s cursor. This is Kubernetes' 410 Gone, sent per KB, and the other KBs in the stream are
  unaffected.

**When the gap is too old.** Because the index holds one row per node, a cursor stays valid
indefinitely, with two exceptions:
- **Tombstone pruning.** `delete` rows older than `events_tombstone_retention_days` are deleted, and
  `floor` rises to the highest pruned `seq`. Kafka's compacted topics prune deletes the same way.
- **Epoch rotation.** The epoch is replaced whenever the KB's history may have gone backwards:
  - a whole-instance `backup restore`;
  - a per-KB checkpoint restore (ADR-032);
  - recreating a KB under the same id.

  A restored store reuses sequence numbers. A reader holding a pre-restore cursor would otherwise
  skip every change until `head` passed its cursor. `ahead_of_head` catches only the case where the
  restore went further back than the reader.

  The restore commands rotate the epochs in the store they write. A store file copied into place by
  hand bypasses them. That residual risk goes in `docs/DAEMON_ADMIN.md`'s restore section, and it is
  the reason restore must go through the commands.

`too_many_changes` is Matrix's "limited" choice. Past the cap, one paged list is cheaper for both
sides than thousands of notices, each followed by a fetch.

**Rejected:**
- **Positions held by the daemon** (Kafka consumer groups, Postgres replication slots). Postgres
  documents that a slot "will prevent removal of required resources even when there is no
  connection using them". Every reader that never comes back would hold state on the daemon.
  A client-held cursor costs the daemon nothing.
- **Jumping to the head on a bad cursor** (Kafka's `latest`). It hides the changes that were
  missed.
- **Replaying from zero** (`earliest`). It can be as large as the KB.

### C4. Authorization

**Opening a stream:**
- The only credential is a bearer token in the `Authorization` header. The listener already
  refuses `?access_token=` (ADR-111 P1), and the cursor in the URL is not a credential.
- The browser's `EventSource` constructor cannot set headers. So the page reads the
  `text/event-stream` response through `fetch()`, as `fetch-event-source` does and as ADR-073's
  page already does for `kb/query`, and it implements reconnection and `Last-Event-ID` itself.
- ADR-099's connect ticket is not reused here: `fetch()` can set the header, so no ticket is
  needed.
- Every KB named in the stream goes through the **same gate `kb/query` uses** (`load_gated` →
  `check_kb_read_access`, `Transport::Hub`). It is one gate, not a second copy.
- A bad token gets the listener's own `401` and `WWW-Authenticate`, the same as on every other
  route.
- **A KB the token cannot read (proposed; pending the owner's decision).** The stream opens for
  the KBs it can read and sends `access_lost {kb}` for each refused KB as its first events, before
  any notice. A refused KB is never silently absent, and "never had access" takes the same path
  as "lost access mid-stream", so a reader has one code path. Only a bad token (`401`) or every
  KB refused fails the open.

  The prior art is one-sided: systems that serve several separately authorized resources over one
  connection report per resource and keep the connection. A failed Phoenix channel `join` errors
  that topic only. Ably and Pusher authorize each channel on its own, and a failed attach puts
  that channel in an error state. Matrix `/sync` omits rooms the user cannot see and moves rooms
  they lose to `leave`. Failing the whole open would let one revoked KB take down a multi-KB
  reader.

  The probing concern (one request reveals which of many KB ids a token can read) adds nothing:
  `kb/query` already answers that per KB, and the stream only batches it. The 16-KB-per-stream cap
  bounds the batch.
- ADR-067 `QueryOnly` members are admitted, because this is the query surface.
- ADR-099 B6's daemon-only-token gate does not apply. The stream is read-only, like `kb/query`, so
  it accepts the tokens `kb/query` accepts.

**Every batch is authorized:**
- Before writing any batch for a KB, the stream evaluates the gate again for (principal, KB) and
  sends nothing for that KB unless it allows.
- The decision is not cached. Batches are coalesced (C5), so the cost scales with streams × wakeups,
  not with writes. A cache is added only if measurement shows it is needed.
- Membership changes and blocklist changes are themselves `kb_changed` notices on the same feed, so
  they wake every stream on that KB, and the re-check runs before anything that follows them.
- A revoked reader receives nothing after the change that revoked it. It may also lose notices
  committed just before the revocation that it had not yet received, which is the safe direction.

**When access is lost:**
- The stream sends `access_lost {kb}` and stops serving that KB. Its cursor component is dropped
  from later ids, and the other KBs continue.
- This is Matrix's explicit `leave`, not silence.
- Because every batch is re-checked, this meets ADR-074's original Consequences requirement
  ("per-event, not merely at subscribe-time"): no event for a KB is written without the gate
  allowing it at the time of writing.

**Expiry ends the stream, and the cursor makes reconnection lossless.** SSE has no client-to-server
frame, so ADR-099 B3's in-band `session/renew` does not fit. What carries over is the deadline:
- The stream's deadline is the token's `exp`.
- At `deadline − events_renew_lead_secs` (default 60), the stream sends
  `renew_needed {deadline}`.
- At the deadline it sends `closing {reason: "token_expired"}` and ends the response. The 4401 of
  B3 becomes an event, because an HTTP response that has started cannot change its status.
- The client reconnects with a fresh token and its last id, and loses nothing.
- Offboarding takes effect at the next batch after the membership change, and at token expiry at
  the latest.
- No `principal_changed` check is needed. A reconnect is a new authorization, and a different
  principal presenting someone else's cursor gains a position, not access.
- The daemon's own shutdown sends `closing {reason: "shutting_down"}`.

### C5. Fan-out and slow consumers

**The daemon holds no per-consumer queue.**
- For each KB with at least one stream, the daemon keeps one `watch` channel (its `head`) and a
  shared ring of the last `events_tail_rows` rows (default 256). One reader per KB refills the ring
  on each wakeup.
- Each stream holds only its cursors. On wakeup it takes the rows past its cursor, from the ring if
  they are still there and from SQLite if not.
- Wakeups are coalesced: a stream writes at most one batch per `events_min_interval_ms` (default
  250). A burst of keystrokes on one node becomes one notice, because the index keeps only the
  node's latest row.
- In the steady state, store reads are O(KBs watched) per interval, not O(streams).

**A slow consumer falls behind without costing anything, and it never loses data:**
- A consumer that reads slowly simply has an older cursor. When it reads again, it gets the latest
  row per node.
- A consumer that stops reading fills its TCP send buffer. A write that cannot complete within 5 s
  ends the stream, the same bound the collab session and ADR-099's socket use (D4). The client
  reconnects from its last id.
- Falling behind is therefore at most a reconnect, never a missed change. ADR-099 B4 has to close a
  document on overflow, because a dropped update would silently leave the client's copy short. A
  notice needs no such rule, because the index can produce it again.

**Caps and lifetime** (all in `OAuthConfig`, beside `ws_*`):
- `events_enabled`, default `false`, as for `webview_enabled` and `ws_enabled`.
- `events_max_streams`, default `max_connections / 4`. On the proxy's HTTP/1.1 upstream leg, every
  stream holds a `ConnLimiter` slot for its whole life. Streams must never be able to take every
  slot, so three quarters stay for requests and sockets.
- `events_max_streams_per_principal`, default 4. A reader needs one stream, plus one more briefly
  while it reconnects. The rest leaves room for a few tabs.
- `events_max_kbs_per_stream`, default 16. This bounds the cursor at about 500 bytes.
- `events_max_replay`, default 1000.
- `events_tombstone_retention_days`, default 30 (Stripe's 30-day window).
- A keepalive comment every `events_keepalive_secs`, default 15, as the SSE spec advises. It also
  sits well under the 60 s proxy read timeout that ADR-111 D6's ping was chosen against. A failed
  keepalive write ends a dead stream. This is D3's idle-reaping policy: no stream outlives its token
  or a dead peer.
- Response headers:
  - `Content-Type: text/event-stream`;
  - `Cache-Control: no-store`;
  - `X-Accel-Buffering: no`, so a buffering proxy does not hold notices;
  - `retry: 3000` on open, as a hint only.

**Cost on the write path:** one counter update and one upsert inside the existing WAL transaction.
On `kbc:` writes only, a manifest comparison that is O(manifest). This is a hot path (principle #9),
so C7 makes it a measured gate rather than an assumption.

### C6. One notice source for SSE and the WebSocket

Two things are kept apart here: data and notices.

- **Data** is update bytes for documents a session has opened. It stays where ADR-099 put it: the
  broadcaster, doc-scoped, on the socket only. It never goes on SSE.
- **Notices** come from the change index (C1) and nowhere else. Both transports render the same
  thing:
  - **SSE:** `GET /events` (C3).
  - **WebSocket:** a `kb/changes.subscribe {kbs: [{kb, cursor}]}` method on ADR-099's envelope.
    Notices arrive as `kb/changes.notice` frames with the same JSON body, the same cursor, the same
    `reset` and `access_lost`, through the same gate.

On the socket, B3's in-band renewal replaces C4's close-and-reconnect, because the socket can carry
it. A native client watching a KB therefore needs no second connection. A browser reader needs no
socket. Neither transport has its own idea of what changed.

ADR-074's route `GET /kb/{kb_id}/view/events` becomes `GET /events`, because a stream covers several
KBs. D2.3 (thin "re-fetch" signals) and D2.4 (fall back to polling when the stream fails) stand. The
fallback is `kb/query.list`, comparing `seq`, and the page states plainly when it is polling (gate
G1).

### C7. Verification added

Each test must fail without its mechanism.

**The source:**
- **Commit order:** two concurrent writers to different nodes of one KB. A reader that disconnects
  after any notice and resumes misses nothing. The test fails when notices are delivered from the
  broadcast path instead of the index.
- **Atomicity:** a WAL append forced to fail leaves no notice row. A notice row is never visible
  without its WAL entry.
- **Coverage:** every mutating `DocStore` path yields a notice, or the test shows it leaves no
  readable state changed:
  - `apply_update`;
  - the replace path (`share_doc`, `restore_doc`, rebirth);
  - mesh relay;
  - seed apply;
  - `delete_doc`, which today does not call `emit_change`.

  Another check proves that no new write path bypasses the index (principle #15: the projector feed
  had zero callers for months).
- **Manifest operations:** a node removed from the manifest yields `delete`; one added yields
  `upsert`. A membership op yields `kb_changed`, and so does a blocklist change.

**Resumption:**
- Resuming from each cursor taken in a random walk returns exactly the set of nodes changed since
  it, each once.
- Resuming from the `kb/query.list` cursor misses no change made while the list was paged.
- After a backup restore and after a checkpoint restore, an old cursor gets `reset
  {epoch_changed}`. The test fails without epoch rotation, because the reader silently misses the
  reused sequences.
- Each other reset reason is produced on demand: `below_floor`, `ahead_of_head`,
  `too_many_changes` and `no_cursor`. A reset for one KB leaves the stream's other KBs running.
- A reconnect with `Last-Event-ID` and a URL cursor uses the header.

**Authorization:**
- ADR-074's original test: a stream for KB A while B changes. Zero bytes about B reach the stream.
- A wrong, expired, forged or missing-claim token is refused at open exactly as on every other
  route.
- A KB refusal at open carries the same error object `kb/query` returns for that token and KB.
- A member removed mid-stream gets `access_lost`, and no notice with a `seq` after the removal.
- A blocklisted principal is cut off without any document write.
- The raw stream bytes contain no title, body or tag value of the watched KB, and no id from any
  other KB. This is checked for a plaintext KB and for an E2E KB.
- A credential in the URL is refused.

**Lifetime:**
- `renew_needed` arrives at the lead time, and `closing {token_expired}` arrives at `exp`.
- Nothing is written after the deadline.
- Reconnecting with a new token loses nothing.

**Fan-out:**
- A consumer that stops reading is dropped within the write timeout, and the daemon's memory does
  not grow with its backlog.
- 100 streams on one hot KB cause O(1) store reads per interval, not 100.
- The per-principal and global caps refuse the stream past the cap, and never refuse a `kb/query`
  request.

**Cost:**
- The `apply_update` latency benchmark is run before and after the change, on a KB the size of
  ADR-060's 20,000-node store.
- The proposed bar: p99 grows by no more than 10%.
- The `kbc:` manifest comparison is measured separately on the same store.

**Not regressed:** ADR-099's socket works with `events_enabled = false`, and the SSE route works with
`ws_enabled = false`.

### Open questions for the owner

1. **Defaults.**
   - streams: a quarter of `max_connections`, and 4 per principal;
   - 16 KBs per stream;
   - a replay cap of 1000;
   - 30-day tombstone retention;
   - a 250 ms coalescing interval;
   - a 15 s keepalive.

   Each has a reason above. None is measured yet.
2. **The write-path cost bar: measured** (`docs/research/074a-write-cost-measurement.md`, #905). At
   the ADR-109 hosted scale the index is affordable: 76 bytes per node, and p50 goes from 0.62 to
   0.75 ms at 162 writes/s. "+10% at p99" is the wrong bar, because any in-transaction index adds
   1–2 pages per commit and pulls SQLite's automatic checkpoint (an fsync) onto p99. The proposed
   bar is added p50 plus throughput, on an idle machine. Two prerequisites come with it: a single
   in-process writer lock, so shard connections queue instead of sleeping (with it, 32 writers run
   at 1,280/s with p99 58 ms), and checkpoints off the commit path. The in-memory coalescer is not
   needed. Accepting the bar is the owner's decision.
3. **HTTP/2 on the listener.** Should the listener advertise `h2` through ALPN? That lifts the
   browser's six-connection limit for direct connections. It does not remove C3's multi-KB stream,
   because the proxy's upstream leg still costs a slot per stream.
4. **The WebSocket method.** *Decided (2026-11-13):* it ships in the same release as the SSE
   route: one index, one gate, two transports.
5. **A partial open.** C4 now proposes it, from the prior art there: the open succeeds for the
   readable KBs and sends `access_lost` first for each refused one. Still the owner's decision.
6. **Authorship.** *Decided (2026-11-13):* never in a notice. Version history is the single source
   of truth for authorship (C2).
