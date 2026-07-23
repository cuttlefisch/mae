# ADR-053: Live scoped read-through KB query surface (no full replication required)

**Status:** Proposed.
**Extends:** ADR-035 (editor↔daemon boundary — the federation query layer is already named
as daemon-owned SHARED value in ADR-035's own table; this ADR extends that value across the
network boundary, it does not invent a new capability class), ADR-018 (identity-anchored
access control — every new query call is gated by the existing `kb_access` chokepoint),
ADR-023 (epoch-fenced writes — unaffected by this read-only surface, but the same principal
model applies).
**Complements:** ADR-052 (the auth layer this surface is gated behind), ADR-037 (E2E
content encryption — defines the hard boundary this design must respect).
**Tracking:** issue #375 (epic tracker); phase issue #382. **Depends on:** ADR-054 (this surface must not reintroduce the
global-lock bottleneck ADR-054 removes), ADR-052 (auth).

## Context

It is not reasonable to require a thin client (a VS Code session doing occasional ad hoc
search) to fully join and locally replicate every hub-hosted KB it might want to search —
for both storage cost and, especially for E2E-encrypted KBs, security reasons (a
casual/read-mostly participant shouldn't need a durable, cumulative local copy of
everything they've ever glanced at).

Research corrected an initial assumption: the daemon's local Unix-socket handler
(`daemon/src/handler.rs:106`, `"kb/search"` and siblings, backed by
`query_layer: Arc<dyn KbQueryLayer>`) **already implements this query logic** — the gap is
purely one of **network reachability**. The only network-facing listener today (the TCP
collab handler) exposes CRDT sync/membership RPCs (`kb/share`, `kb/node_fetch`, `kb/join`,
`kb/node_update`, `sync/*`) but nothing search-shaped. A remote peer must fully join and
replicate a shared KB locally before any MCP tool can search it — `docs/COLLABORATION.md`
states this design intent explicitly ("the daemon is a document hub, not the source of
truth... clients hold the authoritative CRDT state"). This ADR does not contradict that
philosophy for *members*; it adds a distinct, explicitly-scoped capability for
*non-member, read-only, thin* clients.

The E2E-encryption boundary (ADR-037) makes this a two-path problem, not one: for an
unencrypted KB, the daemon already legitimately holds plaintext (as host/member), so
server-side search is a straightforward authenticated query. For an E2E-encrypted KB, the
daemon is deliberately **key-blind** unless it is itself an editing member — server-side
plaintext search there would silently break the "key-blind relay" threat model ADR-037
exists to guarantee. These two cases cannot share one implementation strategy.

## Decision

1. **New RPC family** `kb/query.search` / `kb/query.get` / `kb/query.graph`, deliberately
   namespaced apart from the existing local-only `kb/*` methods so the trust/reachability
   distinction is visible in the wire protocol itself. Hosted on the network listener(s)
   established by ADR-052 (OAuth) and/or existing mTLS (ADR-017), never on the
   unauthenticated local socket.
2. **Every call passes through `kb_access(kb_id, principal, Read)`** before reusing
   `handler.rs`'s existing query-layer execution logic (principle #8 — no
   reimplementation of already-correct query code).
3. **Unencrypted KBs:** server-side search/get, capped per-request (result count,
   node-body size) to bound cost and prevent "search" from being used as a disguised
   full-dump vector.
4. **E2E-encrypted KBs:** server-side plaintext search is **structurally impossible by
   design**, not a policy toggle. Instead: a **capped, evictable, lazy-fetch primitive** —
   the client requests specific node IDs (or a small bounded candidate set drawn from
   metadata the daemon can honestly compute without seeing content — ciphertext size,
   timestamp, link-graph shape, never body text), fetches only that ciphertext, decrypts
   client-side with its own wrapped key, and serves subsequent local search from an
   LRU-evictable cache. **Never** full-upfront replication, **never** server-side plaintext
   search — the two extremes this ADR exists to avoid.
5. **Capability negotiation:** the surface tells a client up front whether a given KB is
   plaintext-searchable or lazy-fetch-only, so a generic MCP client doesn't need
   out-of-band knowledge of MAE's encryption model to use it correctly.
6. **Gated strictly behind `daemon_mode=shared`** plus the network listener being
   explicitly enabled — never on by default, consistent with principle #12 (daemon value
   is earned by an explicit SHARED/COORDINATES/DURABILITY need, not assumed).
7. **Cache scoping:** the lazy-fetch cache is keyed **per authenticated principal** (not
   per ephemeral session), with a TTL, so a thin client reconnecting doesn't have to
   re-fetch already-authorized content — but it remains capped and evictable regardless,
   never growing into a de facto full replica.

## Consequences

**Positive.** Resolves a real, explicitly-flagged product blocker (users can't reasonably
be expected to download every KB they might want to search) without weakening the
E2E-encryption threat model for KBs that use it. Reuses proven query-layer code rather than
reimplementing search logic a second time.

**Costs (honest).** This is new, security-relevant network surface with a real bug class
(accidentally allowing the "capped candidate metadata" leak to become a content-search
oracle for encrypted KBs must be actively guarded against, not just assumed safe by
construction). The lazy-fetch cache is new client-side state that must be correctly bounded
(see ADR-055/P5's memory-leak concerns) or it becomes exactly the storage-cost problem this
ADR exists to avoid, just deferred and unbounded instead of upfront and capped.

## Alternatives rejected

- **Require full local replication for all remote KB access, encrypted or not.**
  Rejected — this is the status quo the project owner explicitly identified as
  unreasonable for storage and security reasons.
- **Server-side plaintext search for all KBs, encrypted or not, trusting the hub
  operator.** Rejected — contradicts ADR-037's stated threat model (a key-blind relay/host)
  outright; not a legitimate design option for KBs that opted into E2E encryption.
- **A single unified "fetch everything matching, cache forever" primitive for both
  encrypted and unencrypted KBs.** Rejected — conflates two structurally different trust
  situations into one API, making it easy to accidentally apply the wrong guarantee to the
  wrong KB type; keeping them explicitly distinct (with capability negotiation) is safer.

## Verification

- A thin client with no local replica can search/read an unencrypted hub KB it has
  Viewer+ access to, correctly gated by role.
- A thin client can incrementally read an E2E-encrypted KB via the lazy-fetch primitive,
  with bounded, evictable local cache growth verified never to exceed its configured cap
  under sustained use.
- **Adversarial test (required, none exists today):** a "hostile/curious hub operator"
  test proving server-side plaintext search of an E2E-encrypted KB is structurally
  impossible through this surface — same test class as ADR-037's existing key-blind-relay
  tests.
- The surface is confirmed unreachable when `daemon_mode` is not `shared` or the network
  listener is disabled.
