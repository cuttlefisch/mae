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
