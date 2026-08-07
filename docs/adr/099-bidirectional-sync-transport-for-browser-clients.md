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
