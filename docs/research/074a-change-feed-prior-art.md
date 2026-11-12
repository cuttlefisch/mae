# ADR-074 Amendment 1 prior art: change feeds for many consumers

Date: 2026-11-12. Each item records what was checked and what it decides for the amendment. Where a
source did not say something, that is stated rather than inferred. The brief was to find evidence
*against* a design, not for one: each source is read for the failure mode it documents.

## The question

A read-only reader (ADR-073's view, and browser readers like it) polls `kb/query.*`. It should
instead be told what changed, over Server-Sent Events, by a daemon serving many such readers. The
design has to say: what a notice contains, how a reconnecting reader avoids missing changes, how
each notice is authorized, what a slow reader costs, and how this relates to ADR-099's WebSocket.

## HTML Standard: Server-Sent Events

<https://html.spec.whatwg.org/multipage/server-sent-events.html>

- **Resumption is client-held.** On reconnect the user agent sends `Last-Event-ID` with the last
  `id` it received, and only when that string is not empty. The last event ID buffer "does not get
  reset, so the last event ID string of the event source remains set to this value until the next
  time it is set by the server": an event without an `id` keeps the previous one.
- **`id` constraints.** An `id` value containing U+0000 is ignored. The header's value space is "any
  UTF-8 encoded string, that does not contain U+0000 NULL, U+000A LF, or U+000D CR."
- **`retry`.** A field of ASCII digits sets the reconnection delay; anything else is ignored. The
  initial delay is implementation-defined.
- **Ending a stream.** "If res's status is not 200, or if res's Content-Type is not
  text/event-stream, then fail the connection", and a failed connection is not retried. The
  non-normative introduction adds that HTTP 204 tells a client to stop reconnecting.
- **Proxies.** "Legacy proxy servers are known to, in certain cases, drop HTTP connections after a
  short timeout. To protect against such proxy servers, authors can include a comment line (one
  starting with a ':' character) every 15 seconds or so."
- **Connection limits.** "Clients that support HTTP's per-server connection limitation might run
  into trouble"; the suggested workaround is one `EventSource` shared through a shared worker.
- **No request headers.** `EventSource(url, eventSourceInitDict)`; the only init member is
  `withCredentials`, which changes the credentials mode (cookies). There is no way to set
  `Authorization`.

**Decides:**
- the event id is the whole resumption state, because it is all the client sends back;
- an id is a printable string with no CR, LF or NUL;
- a keepalive comment every 15 seconds;
- a stream may cover several KBs, because the connection limit makes one stream per KB expensive
  (see MDN below);
- the browser EventSource constructor cannot carry the bearer token. This is the same problem
  ADR-099 Amendment 1 B1 solved for WebSocket, and it needs its own answer (see fetch-event-source).

## MDN: the six-connection limit

<https://developer.mozilla.org/en-US/docs/Web/API/EventSource>: "When **not used over HTTP/2**, SSE
suffers from a limitation to the maximum number of open connections, which can be specially painful
when opening various tabs as the limit is _per browser_ and set to a very low number (6). The issue
has been marked as "Won't fix" in Chrome and Firefox." Over HTTP/2 the limit is the negotiated
number of concurrent streams, by default 100.

**Checked in MAE:** `load_tls_config` in `daemon/src/oauth.rs` builds the rustls `ServerConfig`
without setting ALPN protocols, so a browser connecting directly to the daemon cannot negotiate
HTTP/2 and gets HTTP/1.1, even though `hyper_util`'s auto builder would serve HTTP/2. Behind a
TLS-terminating proxy the browser-to-proxy leg may be HTTP/2, but the proxy-to-daemon leg is
typically HTTP/1.1, and there every stream is one upstream connection and one `ConnLimiter` slot.

**Decides:** one stream per KB is rejected. A reader watching a handful of KBs in one tab would
consume most of the six connections the browser allows for the whole origin, `kb/query` requests
included. A stream names a set of KBs. Whether the listener should advertise `h2` is left to the
owner; it would not remove the need, because the proxy's upstream leg still costs a slot per stream.

## Azure fetch-event-source

<https://github.com/Azure/fetch-event-source>: written because the browser API cannot pass custom
request headers ("You cannot pass in custom request headers"), can only make GET requests, and
"the browser will silently retry for you a few times and then stop". The library parses
`text/event-stream` from a `fetch()` response, so headers can be set, and it resends the last event
ID when it reconnects.

**Decides:** the wire format stays standard `text/event-stream`, and the browser consumes it with
`fetch()` and the `Authorization` header, as ADR-073's page already does for `kb/query`. The
credential never goes in the URL (ADR-111 P1 already refuses `?access_token=`). Reconnection and
`Last-Event-ID` are then the client's job, which the amendment specifies instead of inheriting
EventSource's undocumented retry policy.

## CouchDB `_changes`

<https://docs.couchdb.org/en/stable/api/database/changes.html>

- "Only the most recent change for a given document is guaranteed to be provided." Intermediate
  states may be skipped.
- `since` starts "from the change immediately after the given update sequence"; `since=now` starts
  at the current position.
- `feed=eventsource` "Works the same as Continuous Mode, but sends the events in EventSource
  format", and "CouchDB also honours the Last-Event-ID parameter", which overrides `since`.
- `heartbeat` sends an empty line periodically on longpoll, continuous and eventsource feeds.
- Deleted documents carry `"deleted": true`; there is no separate "created" kind.
- Against: "The results returned by _changes are partially ordered", "the order is not guaranteed to
  be preserved for multiple calls", and when shard replicas are unavailable "you might see changes
  again that you have previously seen", so applications should be idempotent. Sequences "can be any
  valid json object".
- Filters exist (`_doc_ids`, `_selector`, `_design`), evaluated per change.

**Decides:**
- **Latest change per subject.** The change index keeps one row per node, holding its latest
  sequence. A reader that was away for a week replays at most one row per node changed, not every
  edit. Replay is therefore not bounded by a retention window.
- **Two operations, `upsert` and `delete`**, not created/updated/deleted. A reader that does not
  know a node learns it is new by fetching it.
- **At-least-once delivery.** A reader treats a notice as "re-fetch", which is idempotent.
- **Opaque cursors.** The cursor is a string the client stores and returns, never parses.
- `Last-Event-ID` overrides a cursor given in the URL, as CouchDB does.

## Kubernetes list-then-watch and 410 Gone

<https://kubernetes.io/docs/reference/using-api/api-concepts/>

- A list returns a `resourceVersion`, and a watch from it lets a client "fetch the current state
  and then subscribe to subsequent changes, without missing any events."
- When "the historical version of that resource is not available, clients must handle the case by
  recognizing the status code `410 Gone`, clearing their local cache, performing a new **get** or
  **list** operation, and starting the **watch** from the `resourceVersion` that was returned."
- "Clusters using etcd 3 preserve changes in the last 5 minutes by default."
- `BOOKMARK` events exist "to mitigate the impact of short history window": they advance a client's
  `resourceVersion` without a change to a resource it watches. Clients may not assume they arrive.
- "Resource versions must be passed unmodified back to the server."

**Decides:**
- `kb/query.list` returns the KB's cursor read **before** its scan. A reader lists, then subscribes
  from that cursor, and misses nothing in between. This is the one change the amendment makes to
  ADR-053's surface, and it adds fields only.
- A cursor that cannot be honoured gets a `reset` for that KB, the equivalent of 410: the reader
  relists that KB and resubscribes from the cursor the list returned.
- Five minutes of history is the failure mode to avoid. With one row per node (CouchDB), the
  replay window is limited only by deleted-node tombstones, so `reset` is rare.
- Bookmarks are not needed. They exist because Kubernetes' version is shared by every resource
  type, so a quiet watch's position goes stale against the global counter. A per-KB sequence only
  moves when that KB changes.

## Matrix `/sync`

<https://spec.matrix.org/latest/client-server-api/> (Syncing; `GET /_matrix/client/v3/sync`)

- `since` is "A point in time to continue a sync from"; `next_batch` is "The batch token to supply
  in the `since` param of the next `/sync` request." One token covers every room the user is in.
- When "a large number of events arrive between calls to `/sync`, a "limited" timeline is returned",
  with a `prev_batch` token to page back. The spec's Timeline table defines `limited` slightly
  differently ("limited by the `limit` on the filter"); the page does not reconcile the two.
- A room the user has left appears under `rooms.leave` (only when the filter asks for it).

**Decides:**
- **One cursor for many subjects.** One token for many rooms is the model for one stream covering
  several KBs: the event id carries a position for every KB in the stream.
- **A large gap is answered with a reset, not a flood.** Past a cap, the stream sends `reset` for
  that KB instead of replaying thousands of notices. Matrix's `limited` makes the same choice: a
  reader that missed that much should resync, not replay.
- **Losing access is an explicit signal** (`access_lost`), not silence.

## Kafka: consumer positions, log compaction, offset reset

- Design, consumer position (<https://kafka.apache.org/42/design/design/>): "the position of a
  consumer in each partition is just a single integer, the offset of the next message to consume",
  in contrast to systems that "keep metadata about what messages have been consumed on the broker."
- Log compaction (same page): it "ensures that Kafka will always retain at least the last known
  value for each message key"; a record with a null payload is a delete (a tombstone); delete
  markers "will themselves be cleaned out of the log after a period of time"
  (`delete.retention.ms`), so a consumer that reaches the head of the log later than that may miss
  a delete.
- `auto.offset.reset` (<https://kafka.apache.org/42/configuration/consumer-configs/>): "What to do
  when there is no initial offset in Kafka or if the current offset does not exist any more on the
  server": `earliest`, `latest`, or `none` ("throw exception to the consumer").

**Decides:**
- **The consumer holds its position; the daemon holds no per-consumer state.** A slow or absent
  reader costs the daemon nothing. This is what makes slow readers cheap (see Postgres slots for
  the cost of the alternative).
- **Tombstones are kept for a retention period, then pruned.** Pruning raises a per-KB floor. A
  cursor below the floor gets `reset`, because it may have missed a delete. Kafka's compacted-topic
  guarantee has exactly this shape.
- **A cursor the daemon cannot honour gets `reset`**, the equivalent of `none`. Silently jumping
  to the head (`latest`) would hide missed changes, and replaying from the start (`earliest`) can be
  very large.

## PostgreSQL: LISTEN/NOTIFY and replication slots

- NOTIFY (<https://www.postgresql.org/docs/current/sql-notify.html>): notifications go to sessions
  that have executed `LISTEN`; inside a transaction they "are not delivered until and unless the
  transaction is committed", and "messages from different transactions are delivered in the order
  in which the transactions committed". Identical payloads within one transaction are folded into
  one. The payload "must be shorter than 8000 bytes". "If this queue becomes full, transactions
  calling `NOTIFY` will fail at commit." And: "If binary data or large amounts of information need
  to be communicated, it's best to put it in a database table and send the key of the record."
- Logical decoding slots (<https://www.postgresql.org/docs/current/logicaldecoding-explanation.html>):
  "Replication slots persist across crashes and know nothing about the state of their
  consumer(s). They will prevent removal of required resources even when there is no connection
  using them," which "consumes storage", and "In extreme cases this could cause the database to
  shut down to prevent transaction ID wraparound". After a crash a slot "might return to an earlier
  LSN", and clients "are responsible for avoiding ill effects from handling the same message more
  than once."

**Decides:**
- **The notification is a signal; the table is the truth.** The in-memory wakeup carries only "KB
  K moved". A stream reads committed rows from the change index.
- **Notices are written in the transaction that commits the change**, so their sequence order is
  commit order. A notice exists if and only if its change does.
- **A shared queue that can fill is a failure mode, not a design.** NOTIFY's full queue fails
  writers. The amendment keeps no queue at all.
- **No server-side consumer positions.** A slot pins resources for a consumer that may never come
  back, which is the cost a daemon serving tens of readers must not take on.

## Transactional outbox

Debezium, Outbox Event Router
(<https://debezium.io/documentation/reference/stable/transformations/outbox-event-router.html>): the
pattern "avoids inconsistencies between a service's internal state (as typically persisted in its
database) and state in events consumed by services that need the same data." The page does not
spell out the mechanism. The pattern's mechanism is the one NOTIFY documents above: write the event
in the same transaction as the state.

**Decides:** the change index is written inside the existing WAL-append transaction, which already
carries the editor record (ADR-113 D3). A crash between "content committed" and "notice recorded"
cannot happen, because there is no "between".

## Stripe events

<https://docs.stripe.com/events/how-events-work> and <https://docs.stripe.com/api/events>

- Thin events: "a lightweight notification containing the event type and resource ID. Your code can
  then optionally fetch the complete event or the latest state of the related resource from the
  API." Stripe recommends them for new integrations because they "let your integration fetch the
  latest resource state", including when "Data integrity is critical". Even a snapshot event "can
  be stale by the time your application processes it".
- Ordering: Stripe does not guarantee delivery in generation order (the delivery-behaviour section;
  the fetched page rendered this section in Spanish: "Stripe no garantiza la entrega de eventos en
  el orden en que se han generado"), and says to track event ids to detect duplicate deliveries.
- Events are retrievable through the API for 30 days.

**Decides:**
- **Thin notices.** A notice names the KB, the node, the operation and a sequence; the reader
  fetches through `kb/query.*`, which applies access and E2E rules in one place (ADR-074 D2.3,
  kept). This also keeps the feed's payload fixed as node content evolves (ADR-093, ADR-114).
- The 30-day figure is the reference for the tombstone retention default.

## GitHub webhooks and Events API

- Webhook best practices
  (<https://docs.github.com/en/webhooks/using-webhooks/best-practices-for-using-webhooks>): respond
  "with a 2XX response within 10 seconds"; use `X-GitHub-Delivery` to detect duplicates, and a
  redelivery keeps the same value; "If your server goes down, you should redeliver missed webhooks
  once your server is back up." The page does not discuss ordering.
- Events API (<https://docs.github.com/en/rest/activity/events>): "This API is not built to serve
  real-time use cases", latency "can be anywhere from 30s to 6h", polling uses ETag/304 and
  `X-Poll-Interval`, and a timeline holds "up to 300 events" from "the past 30 days".

**Decides:**
- **Push to a consumer's endpoint (webhooks) is rejected for readers.** A browser has no endpoint,
  and recovery after downtime would fall back on manual redelivery.
- **A bounded event list read by polling (the Events API) is what the reader does today.** Its
  documented latency and window are the case against keeping it.

## nginx in front of an SSE stream

<https://nginx.org/en/docs/http/ngx_http_proxy_module.html>: `proxy_buffering` is on by default, so
nginx collects the upstream response into buffers; "Buffering can also be enabled or disabled by
passing "`yes`" or "`no`" in the "X-Accel-Buffering" response header field." `proxy_read_timeout`
defaults to 60 s and applies "between two successive read operations"; if the upstream sends
nothing for that long, the connection is closed.

**Decides:**
- the stream response sends `X-Accel-Buffering: no`, so a buffering proxy does not hold notices
  back;
- the 15-second keepalive (above) sits well under the 60-second read timeout that ADR-111 D6's
  WebSocket ping was already chosen against.

## What the evidence argued against

- **Reusing `EventBroadcaster`, as ADR-074 D2 planned.** Postgres and Kafka both separate a durable
  position from a transient signal. The broadcaster has only the signal. Its sequence counter
  (`next_seq`) is in-memory and restarts at 1. On overflow it drops non-data events (by design;
  ADR-099 B4 closes only a document's data). And it carries update bytes, which a notice must
  never hold.
- **A time-windowed event log** (Kubernetes' 5 minutes, GitHub's 300 events, Stripe's 30 days).
  Every one of them makes the long-away reader the common case for a reset. CouchDB's
  latest-per-document index, and Kafka's compacted topics, keep the window open for everything but
  deletes.
- **A store-wide sequence.** CouchDB (one sequence per database) and Matrix (one token per user,
  across rooms) both use one position across many subjects. Here
  that would let a reader of one KB count the writes made to every other KB on the daemon from the
  gaps in its own cursor, and it would need Kubernetes-style bookmarks to keep quiet streams
  current. A per-KB sequence has neither problem.
