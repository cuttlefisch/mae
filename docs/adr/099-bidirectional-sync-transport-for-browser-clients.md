# ADR-099: Bidirectional sync transport for browser clients

**Status:** Proposed.
**Depends on:** ADR-097 (Browser MAE is a KB surface — its D3 establishes that a one-directional
transport cannot serve a write surface), ADR-052 (OAuth 2.1 resource server — the listener this
extends), ADR-053 (the read-through query surface this sits beside), ADR-098 (the identity a
session carries).
**Supersedes:** ADR-074 D1 **for the write path only**. ADR-074's SSE decision remains correct and
unchanged for read-only views.
**Relates to:** ADR-006 (collaborative state engine), ADR-054 (daemon concurrency hardening —
whose `ConnLimiter` discipline binds here), ADR-091 (session handle for MCP dispatch).
**Evidence:** `docs/research/097-browser-crdt-interop-spike.md` — the Phase 0 spike that
established the payload bytes cross runtimes intact.
Amendment 1: `docs/research/099-websocket-authentication-prior-art.md`.
**Tracking:** issue TBD.

## Context

Browser MAE needs to *write*. ADR-053's `kb/query.*` surface is read-only by construction — its
dispatch has no create/update/delete arm at all — and ADR-074's SSE bridge is deliberately
one-directional. There is no bidirectional transport anywhere in the tree: no WebSocket, no SSE,
no `tokio-tungstenite`, `axum`, `warp` or `tower-http`. `hyper` v1 inside `daemon/src/oauth.rs` is
the only HTTP server MAE has.

**ADR-074 named this exact moment.** Its D1 chose SSE over WebSocket because the feed was *"strictly
one-directional"* and *"the browser never needs to push anything back"* — true for a viewer — while
recording that WebSocket *"remains available to reconsider if a future, genuinely bidirectional need
arises."* ADR-097 D1 created that need. This ADR is the reconsideration, and it is scoped narrowly:
ADR-074's reasoning was correct for what it covered and stays in force there.

**What the Phase 0 spike settled.** A browser `Y.Doc` running stock `yjs`, with no MAE code and no
shim, reads a real `KbNodeDoc` as live `Y.Text`/`Y.Array`/`Y.Map`, edits at UTF-16 offsets that land
where Rust expects, and converges byte-identically with two concurrent native writers across all six
apply orders. So the *payload* is already portable: yrs v1 updates, v1 state vectors, UTF-16 offset
kind.

**What it did not settle, and this ADR must.** Only the envelope differs. MAE's wire format is a
custom JSON-RPC 2.0 message with `Content-Length` framing over TCP (`shared/sync/src/wire.rs`), not
the y-protocols binary sync (`messageYjsSyncStep1/2/Update`) that `y-websocket` speaks. So `yjs` +
`y-websocket` cannot connect to MAE as-is, and the choice is between teaching the daemon
y-protocols or re-framing MAE's own envelope over a bidirectional socket.

## Decision

### D1 — WebSocket, not SSE, for the write path

A KB editing session is bidirectional by nature: the browser pushes updates as often as it receives
them. SSE would require a second, separate channel for every write, which means two authorization
paths, two failure modes, and no ordering relationship between a write and the notification of its
own effect.

ADR-074's SSE choice stays correct for the read-only HTML view (ADR-073), which genuinely never
pushes. Nothing in that decision is reversed; this ADR adds a second transport for a different
surface rather than replacing the first.

### D2 — Re-frame MAE's existing envelope over WebSocket; do **not** speak y-protocols natively

This is the load-bearing decision, and it is settled on operational grounds rather than taste.

> **Amended (Amendment 1, B0):** the multiplexing argument below is out of date. Hocuspocus, the
> de facto Yjs server, has multiplexed documents over one socket since 2.0. B0 re-evaluates D2 on
> the remaining grounds, and the decision stands for different reasons.

**Multiplexing decides it.** Browsing a KB opens many documents — each node is its own
`kb:{node_id}` yrs doc. `y-websocket` is one connection *per document*. The OAuth listener's
`max_connections` defaults to 256, so a handful of users browsing a KB of any size would exhaust
the listener. MAE's envelope is **already document-scoped**: every `sync/*` message carries
`params["doc"]` (`daemon/src/collab_handler/sync_methods.rs:18,44,210,280,312,359`) and every
`kb/*` message carries `kb_id`/`node_id` (`shared/sync/src/wire.rs`). So one connection carrying N
documents needs no new addressing scheme — the addressing already exists.

Three further reasons, each sufficient on its own:

- **One wire format across every transport (principle #8).** `mae_mcp::{read_message, write_framed, handle_request}` is already transport-generic, and the same `kb/*` dispatch serves the Unix socket, mTLS TCP, and the OAuth HTTPS listener. A y-protocols path would be a second, parallel wire format that every future protocol change must be applied to twice.
- **Authorization keys off the envelope.** `kb_access`, the ADR-023 epoch fence, and ADR-036 signed-content-op verification all read fields the MAE envelope carries and y-protocols has no place for. Speaking y-protocols would mean inventing a parallel authorization channel for exactly the checks that must never be bypassed.
- **Awareness is already MAE-shaped.** `shared/sync/src/awareness.rs` is MAE's own notification form, not the y-protocols binary awareness encoding, so it rides the same envelope with no adapter.

The browser side is therefore a **thin adapter**, not a provider: it maps MAE envelope messages to
`Y.applyUpdate` / `Y.encodeStateAsUpdate` and back. The Phase 0 spike already demonstrated exactly
this shape working against real `KbNodeDoc` state.

### D3 — The socket lives on the existing OAuth HTTPS listener, not a new one

A WebSocket upgrade on the existing listener inherits, unchanged: TLS termination, bearer-token
validation against JWKS, the `principal_claim` mapping, `ConnLimiter`, the handshake timeout, and —
most importantly — the **auth-rejection behaviour every other route on that listener is already
tested for**.

A second listener would mean a second place to get token validation right, on the surface where
getting it wrong is worst. Adding a route is the smaller change and the more auditable one.

The bearer token is presented on the upgrade request. The query-string token fallback that
ADR-073's view route carries (`oauth.rs:297-326`) is **not** extended here — a WebSocket client is
script-driven and can set headers, so it has no need for the concession a plain browser navigation
required.

> **Amended (Amendment 1, B1):** the last sentence is wrong for browsers. The WebSocket API cannot
> set `Authorization`. Native clients present the bearer header; browsers present a single-use
> ticket in the subprotocol list.

### D4 — Backpressure and caps follow the posture MAE already has

Per-session bounded queues, write timeouts, and slow clients **dropped rather than blocked** —
matching the existing MCP multi-client posture, and `ConnLimiter` checked immediately after accept,
before the handshake, as every other listener on this port already does (ADR-054).

Because D2 multiplexes, a single slow document must not stall a session's other documents. That is a
new failure mode multiplexing introduces and it is named here so it becomes a test rather than a
surprise.

### D5 — The dependency question is decided explicitly, not by drift

`hyper` v1 handles the HTTP upgrade but not WebSocket framing, so this needs one new dependency
(`tokio-tungstenite`, or `hyper-tungstenite` bridging it to hyper v1). That is a new dependency **on
a security-critical listener**, which is the same category of decision where ADR-052 evaluated and
explicitly rejected `rmcp-server-kit` as a single-maintainer third-party dependency.

The distinction to make, and to record rather than assume: `tokio-tungstenite` is the de-facto
standard Rust WebSocket implementation with broad production use, not a single-maintainer project —
but the implementer must confirm that at the time of adoption rather than inheriting this
sentence. If it cannot be confirmed, the fallback is implementing RFC 6455 framing directly over
hyper's upgrade, which is more code but no new trust.

## Consequences

**Positive.** One wire format, one authorization path, one set of rejection tests. A browser session
is one connection regardless of how many nodes it opens, so listener capacity scales with *users*
rather than with users × open documents. And because the envelope is unchanged, a browser client and
a native MAE editor are the same kind of peer to the daemon — the convergence the Phase 0 spike
demonstrated is the convergence production gets.

**Costs, stated honestly.**

- **A custom protocol forfeits the Yjs ecosystem.** No `y-websocket`, no `y-indexeddb` provider wiring for free, no off-the-shelf awareness UI. MAE must maintain its own thin client adapter. This is a real, recurring cost and it is accepted for the multiplexing and authorization reasons in D2 — but it should be revisited if a future need arises that the ecosystem serves and MAE's adapter does not.
- **One new dependency on the security-critical listener** (D5).
- **Multiplexing introduces head-of-line failure modes** that per-document connections do not have (D4).
- **`max_connections` semantics change meaning** — it becomes a cap on sessions rather than on documents. Operators reading `daemon.toml` need that stated, and `docs/DAEMON_ADMIN.md` must say so.

**Downstream/bug-risk framing (principle #9).** This adds a route to an existing listener rather
than changing any existing path, so the blast radius on current behaviour is small — but the route
is on the daemon's only network-facing HTTP surface, and it is the first *write* path ever exposed
there. The authorization inheritance in D3 is what keeps that safe, and it must be verified rather
than assumed (below).

## Alternatives rejected

- **Speak y-protocols natively so `y-websocket` connects unmodified.** Rejected on multiplexing first — one connection per document exhausts a 256-connection listener with a handful of browsing users — and on authorization second: y-protocols has no place to carry the fields `kb_access`, the epoch fence, and signed-content-op verification read, so it would need a parallel authorization channel around exactly the checks that must never be bypassed. The ecosystem benefit is real and is recorded as a cost above, not dismissed.
- **Extend ADR-074's SSE with a separate HTTP POST write path.** Rejected: two channels means two authorization paths and no ordering relationship between a write and the notification of its effect, for no gain over a single socket that already has both.
- **A second, dedicated listener for sync.** Rejected: a second place to get bearer validation right, on the surface where errors are worst, to avoid adding one route to a listener that already terminates TLS and validates tokens correctly.
- **Reuse the existing mTLS collab TCP listener (9473) from the browser.** Rejected because a browser cannot present a client certificate in a way this model can use, which is the whole reason ADR-098 exists; and exposing 9473 to browsers would widen a listener deliberately scoped to trusted peers.

## Verification

Per principle #14, verified by trying to falsify it.

- **Authorization inheritance is the primary gate.** The upgrade endpoint must reject wrong, expired, forged, and missing-claim tokens **identically** to every other route on the listener — asserted against the same cases `daemon/tests/oauth_e2e.rs` already covers, not a fresh and possibly weaker set. Adversarially: a token valid for KB *A* must not be able to subscribe to KB *B* on an already-open socket, and the raw frames must be scanned for any other KB's content, matching the leak oracle `daemon/src/tests/webview_tests.rs` establishes.
- **Authorization is re-checked per document, not per connection.** The one failure mode multiplexing creates that per-document connections cannot have: a session authorized for one KB must not reach another by naming it in a later frame. This must be a named test, not a property assumed from D3.
- **Multiplexing actually multiplexes.** Open N documents in one session and assert the daemon's connection count stays at 1 — the criterion the Phase 0 spike defined and could not test without a transport. `ConnLimiter`'s newly-exposed accessors (PR #646) make this directly observable.
- **Convergence through the real transport.** The Phase 0 spike's ≥3-writer, all-apply-orders, shuffled convergence test, re-run end-to-end over the socket rather than over files — including the negative control that must fail when the binding is disabled. A transport that silently drops or reorders updates would pass a naive "both edits arrived" check and fail this one.
- **Backpressure.** A deliberately slow client must be dropped, not allowed to stall the daemon, and a slow *document* must not stall the other documents in its own session (D4).
- **ADR-074 is not regressed.** The read-only HTML view must continue to work over SSE/polling with this route disabled, proving the two transports are genuinely independent rather than one having quietly become load-bearing for the other.

## Amendment 1 (2026-11-11): connecting from a browser, staying authenticated, and the first cut

**Status of this amendment:** Proposed. It corrects D3, fills in what D2–D4 left open for a real
client, and bounds what may ship before hub signature verification (ADR-111 D4) and the identity
binding (ADR-098). Evidence: `docs/research/099-websocket-authentication-prior-art.md`.

### B0. D2 re-evaluated: does a standard Yjs protocol serve MAE better than its own envelope?

D2's first reason was that `y-websocket` needs one connection per document. That is true of
`y-websocket`, but Hocuspocus 2.0 changed its message layout to
`[documentName][HocuspocusMessageType][YjsMessageType][yjs binary]`, so that "multiplexing allows
you to synchronize multiple documents over the same websocket connection". There is also an early
Rust port, `hocuspocus-rs` 0.1.x. The rejection is re-run here on what each protocol can carry.
The bar: adopt the standard unless MAE's envelope exists out of necessity and buys concrete
advantages without tradeoffs that matter.

**What the standard protocol gives** (y-protocols sync, and Hocuspocus on top of it):
- a maintained browser provider, with the handshake, reconnection and resync;
- multiplexing;
- per-document authentication (`onAuthenticate`) and read-only connections;
- in-session token refresh (`onTokenSync`), which corrects ADR-111 D6's remark that Hocuspocus
  leaves that gap open;
- string "stateless" messages;
- y-protocols awareness, which every Yjs editor binding (CodeMirror, ProseMirror, Tiptap, Monaco)
  speaks.

**What MAE must carry, and whether the standard can carry it:**

| MAE requirement | Standard protocol |
|---|---|
| **Per-update signed authorship** (ADR-036): each node update carries author, epoch, issue time, base state vector, signature and public key. The daemon verifies it and relays the header with the update, so a downstream peer re-verifies the same op (ADR-026's peer-verifiable integrity; the mesh relay depends on it). | No. An update is bare Yjs bytes (`varUint(2) • varBuffer(update)`), and the spec says nothing about authenticating or signing updates. The server merges each update into its own document and forwards it, so the signature has nowhere to travel. A side channel of stateless messages would pair signature and update by convention, with no ordering guarantee. (MAE's own `kb/join` also answers a plaintext KB from server state, as SyncStep2 does. The difference is the live path and the mesh relay, where MAE forwards the signed op with its header.) |
| **E2E KBs** (ADR-037): the daemon stores an opaque op-set it cannot decrypt. | No. The responder must hold the document to compute SyncStep2, so a server that cannot read the content cannot answer the handshake. |
| **Refusals per document, with a kind**: a multiplexed session refuses one document's write (`epoch_fenced`, `not_member`, `update_too_large`) and keeps its other documents. | Partly. Authentication failure is per document, but `beforeHandleMessage` refuses by throwing, which closes the whole connection. |
| **KB-level operations on the same channel**: join with per-node state vectors for a whole KB in one round trip, signed membership ops on the collection document, typed errors. | No equivalent. It would need a second protocol beside the socket. |
| **One wire format and one authorization path** across Unix socket, mTLS TCP, the HTTPS listener and the mesh (principle #8). | A second wire format for remote clients, with every authorization check (`kb_access`, the epoch fence, signature verification) re-implemented against it. |
| **A server in Rust, inside the daemon.** | Hocuspocus is a Node server. Its Rust port is at 0.1.x. Adopting it means either a Node sidecar on the security-critical path, or MAE maintaining a young port. |

**Decision: D2 stands, now on necessity rather than connection counts.** The envelope exists
because MAE's integrity model, signed ops relayed for peers to re-verify, and its E2E model, an
opaque op-set on the server, cannot be expressed in a protocol whose server merges plaintext and
forwards bare updates. Those are not preferences; they are ADR-026/036/037 properties MAE would
lose.

**The standard is adopted where it costs nothing and buys compatibility:**
- **Payloads stay standard Yjs:** v1 updates and v1 state vectors, as today.
- **Presence moves to y-protocols awareness** (B5). MAE's own awareness carries row and column
  cursors, which drift under concurrent edits. Yjs bindings anchor cursors with relative positions,
  the established pattern, and expect an `Awareness` object.
- **Compatibility is defined at the binding layer, not the wire.** The browser adapter must expose
  exactly a `Y.Doc` per node and a y-protocols `Awareness`. Then any Yjs editor binding works
  unmodified, and the adapter is the only MAE-specific client code.

**What is given up, stated plainly:**
- MAE maintains the client adapter: handshake, reconnect, resync, the offline queue. A maintained
  provider would otherwise supply these.
- A third-party app that speaks only Hocuspocus cannot connect to MAE unmodified.

**Who connects, as targeted (2026-11-11).** The clients MAE is building for are:
- **mae-web in the browser**, on this transport, with B1's ticket;
- **MAE's native editor and headless instances**, on this transport, with the bearer header
  (ADR-111 D6);
- **AI coding tools over MCP** (Codex, VS Code and similar).

MCP clients do not speak a sync protocol at all. They call MAE's tools: through a local MAE
reached by the MCP shim, which is itself a native or headless client of the daemon, or through an
MCP endpoint on the daemon's HTTPS listener under ADR-052. Neither path needs a Yjs provider.

User identity comes from an OIDC provider backed by the organisation's directory. The first one
supported is authentik in front of Active Directory. B3's issuer-and-subject check, B6's end
state and ADR-098's binding are written for exactly that issuer.

No off-the-shelf Yjs or Hocuspocus application is a target. That removes the one tradeoff B0 had
left open.

**When to revisit:** if such third-party apps become a target, the answer is a separate gateway
that speaks Hocuspocus to them. It would serve non-E2E KBs only, attribute their writes to the
gateway's own principal, and be unable to carry signed ops. It must never replace the envelope.

### B1. Two credentials on the upgrade; neither is ever in the URL

The browser WebSocket API takes only a URL and a list of subprotocols. It cannot set
`Authorization`, so D3 as written cannot serve the client this ADR exists for. The upgrade to
`GET /ws` accepts exactly one of:

- **`Authorization: Bearer <token>`**, for native and headless clients (ADR-111 D6). This is D3 as
  written.
- **A connect ticket**, for browsers. The client first calls `POST /ws/ticket` with its bearer
  header. The daemon answers `{ticket, expires_in}`, and the browser opens the socket with
  `new WebSocket(url, ["mae.v1", "mae.ticket." + ticket])`.

A credential in the query string is refused (`credential_in_url`), as the listener already does for
`?access_token=` (ADR-111 P1). Authentication is decided **before** the upgrade completes. So a
bad credential gets the same `401` and `WWW-Authenticate` as every other route on the listener
(D3's inheritance argument), and no unauthenticated socket ever exists.

**The ticket:**
- 256 random bits.
- Single use: it is consumed on its first presentation, whether or not the upgrade then succeeds.
- Valid for 30 seconds.
- Held in memory only, so a restart voids every ticket.
- Bound to the minting token's principal, its expiry and its tenant, and to the `Origin` of the mint
  request when there was one.
- Each principal may hold only a bounded number of unused tickets.

An unknown, used or expired ticket all give one refusal (`ticket_invalid`), so a probe learns
nothing. The ticket is deliberately **not** bound to the client's IP address, which the ticket
pattern often suggests. Behind ADR-111's TLS-terminating proxy every client arrives from the
proxy's address, so an IP binding would bind nothing and would only look like a control.
The daemon's response selects `mae.v1` and never echoes the ticket entry.

**Origin.** When a handshake carries `Origin`, it must exactly match an entry in
`oauth.ws_allowed_origins`, with no wildcards and no substring matching. It must also equal the
ticket's bound origin. MAE uses no cookies, so cross-site hijacking has no ambient credential to
ride on; the check is defence in depth and it pins the ticket to the page that minted it.

### B2. The protocol version is negotiated at the upgrade

The version is the `mae.v<N>` subprotocol. If the client offers no version the daemon speaks, the
upgrade is refused with `400`, `protocol_unsupported`, listing the supported versions. This happens
before the upgrade, not as a close after it.

The first frame the daemon sends is
`session/hello {protocol, principal, auth_deadline, limits: {max_frame_bytes, max_open_docs}}`.
It doubles as `session/whoami` for a socket that has not joined anything.

### B3. Staying authenticated: an in-socket renewal deadline

This is the server-enforced refresh ADR-111 D6 requires.

- The session's `auth_deadline` is the presenting token's expiry. A ticket carries its minting
  token's expiry.
- At `auth_deadline − renew_lead`, the daemon sends `session/renew_needed` (`renew_lead` is
  configurable, default 60 seconds).
- The client answers with a `session/renew {token}` frame. The token travels inside TLS, in a frame,
  never in a URL.
- The new token must validate and carry the **same identity: the same issuer and the same
  subject**, compared before any claim mapping. A different subject, or the same subject from a
  different issuer, closes the socket (`principal_changed`). This is SignalR's refresh rule. Once
  external issuers are accepted (after B6), the subject string alone is not an identity.
- On success the deadline moves to the new token's expiry, and every open subscription is
  re-authorized. A subscription the principal has lost is closed (`doc_closed`, reason
  `not_member`) without closing the session.
- A missed deadline closes the socket with code `4401`, reason `token_expired`. In-flight frames are
  not applied after the deadline.

Offboarding (ADR-111 D3) therefore takes effect at the next renewal at the latest.

### B4. Multiplexing, made concrete

- Frames are D2's envelope: one JSON-RPC message per text frame, every one naming its document.
- **Every frame is authorized against its own document's KB.** A connection holds no grant, and the
  same order applies as on every collab path: access, then signature and author, then the epoch
  fence. A session subscribes to a document only after its first authorized operation on it.
- Each document has its own bounded send queue. A document whose queue overflows is closed for that
  session (`doc_closed`, reason `slow_consumer`) and must be re-opened, which resyncs it. The
  session's other documents are not affected (D4's head-of-line case).
- Caps:
  - `max_frame_bytes` defaults to the listener's existing request-body cap, and a larger frame closes
    the socket (`1009`);
  - `max_open_docs` bounds the documents per session;
  - an open document counts against the tenant's existing quotas, exactly like the same request over
    the HTTPS listener.

### B5. Presence

Presence on this transport is **y-protocols awareness** (B0): the standard encoding, with cursors
as Yjs relative positions. It is carried per document as the payload of the envelope's
`sync/awareness`. MAE's native awareness (`shared/sync/src/awareness.rs`, row and column cursors)
stays for existing native peers until they move. A native client converts relative positions with
yrs' sticky indices.
- It is delivered only to sessions authorized for that document.
- The y-protocols spec says awareness is unauthenticated: any peer can claim any state. So the
  daemon decodes each update and refuses entries for client ids the session does not own.
- It attaches the sender's principal beside each entry. A client's own `user` fields (name,
  colour) are display hints, and a UI must label a cursor by the stamped principal.
- It is rate-limited per session and document.
- It is never persisted, and a session's presence is cleared when it closes.

### B6. The first cut, before hub signature verification and identity binding

ADR-111 D4 makes signature verification on the hub (#727) a prerequisite for the HTTPS listener to
carry writes from untrusted callers, and ADR-098's binding is what turns an IdP subject into a KB
member. Neither exists yet. Until both do:

- The route is **off by default** (`oauth.ws_enabled = false`).
- With it on, **only daemon-issued tokens** (`kid = "self"`, minted by the operator, ADR-111 P1) are
  accepted on `/ws` and `/ws/ticket`. A token from an external issuer is refused there
  (`issuer_not_supported`), even though the listener accepts it for `kb/query`.
- The session's principal is the token's subject, taken literally as the membership principal. The
  operator mints a token for a member's existing principal. That is the trust an operator-minted
  token already carries on this listener. It makes every writer in this cut a caller the operator
  vouched for, which is the only condition under which ADR-111 permits a channel-authenticated
  write.
- `docs/DAEMON_ADMIN.md` states that this cut is for development and for single-operator
  deployments, and that it ends when #727 and ADR-098 land. At that point external tokens are
  accepted and the principal comes from the binding.

### B7. Tests this amendment adds to Verification

Each test must fail without its mechanism.

**Tickets and the upgrade:**
- A ticket is refused on its second use, after 30 seconds, and after a restart.
- An unknown, used and expired ticket give identical responses.
- A ticket in the query string is refused.
- A ticket minted for one origin is refused from another.
- An origin outside the allowlist is refused, including one that merely contains an allowed origin.
- The selected subprotocol never contains the ticket.
- No version in common gives `400` before the upgrade.

**Renewal:**
- A missed deadline closes the socket with `4401`, and a frame sent after it is not applied.
- Renewal with a different principal closes the socket.
- A member removed between renewals loses that KB's documents at the next renewal, and keeps the
  others.

**Authorization:**
- An external-issuer token is refused on `/ws` while B6 holds.
- A frame naming a document of a KB the session cannot read is refused, and its raw response
  contains nothing from that KB (D3's leak oracle).

**Presence:**
- An awareness update for a client id the session does not own is refused.
- Every relayed entry carries the sender's stamped principal, whatever `user` fields the client
  set.
- A remote cursor stays on the same character under a concurrent insert before it (relative
  positions), where a row/column cursor would move.
- Presence never reaches a session not authorized for the document.

**Multiplexing:**
- A slow document is closed for its session while a second document in the same session keeps
  converging.
- N documents use one connection.

### Rejected in this amendment

- **Adopting Hocuspocus' protocol in place of the envelope** (B0). It cannot carry per-update
  signed authorship to peers, or serve E2E KBs whose content the server cannot read.

- **The bearer token in the subprotocol** (Kubernetes' scheme). It works, but it puts the long-lived
  credential in a header every proxy on the path can see and may log. The ticket carries 30 seconds
  of single-use authority instead.
- **The token in the first frame** (OWASP's preference). It requires an unauthenticated open socket,
  with its own timeout, cap and pre-auth message rules. It also moves rejection off the listener's
  tested pre-upgrade path, which is the property D3 chose this listener for.
- **A ticket in the query string** (Slack's shape). A URL is logged by proxies and servers, and
  OWASP says plainly to keep tokens out of it. Single use narrows the damage but does not remove the
  log entry.
- **Cookies.** They bring cross-site request rules, a CSRF surface and a session store, to carry a
  credential the client already holds as a bearer token.
