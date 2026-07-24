# ADR-053: Live scoped read-through KB query surface (no full replication required)

**Status:** Accepted (implemented — see "Verification" and the Implementation note below).
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

## Implementation note (added during Phase G implementation planning, principle #15)

Two corrections to this ADR's original text, found during implementation planning and
verified directly against the code rather than assumed:

**Correction 1 — the reuse target is `DocStore`/`daemon/src/collab_handler/kb_content.rs`,
not `daemon/src/handler.rs`.** This ADR originally said the new RPCs reuse "`handler.rs`'s
existing query-layer execution logic." That's wrong: `handler.rs`'s `KbQueryLayer`/
`CozoKbStore` machinery (Phase D/ADR-054's own rewrite target) serves the daemon's
**locally-federated** KB instances (`kb-register`'d org directories) — a structurally
different data model from a **hub-hosted, collaboratively-shared** KB, which lives in
`DocStore` as `kbc:{kb_id}` (`KbCollectionDoc`, manifest) + `kb:{node_id}`
(`KbNodeDoc`/op-set) yrs docs. "A thin client searching a KB it doesn't locally replicate"
is, by definition, talking about the collaborative/hub model. The actual reusable
precedent is `kb_content.rs::handle_kb_node_fetch`, which already does exactly "load
`kbc:{kb_id}`, gate `kb_access(Read)`, load+decode `kb:{node_id}`."

**Correction 2 — `daemon_mode=shared` cannot be the daemon-side gate.** `daemon_mode`
(`crates/core/src/editor/kb_state.rs`) has zero presence in `daemon/src` — it is a pure
editor-side attach-policy concept (whether *this editor* spawns/owns a daemon), not
something the daemon binary can read or branch on. The real daemon-side gate is a TOML
boolean, matching the existing `config.oauth.enabled` pattern: `oauth.kb_query_enabled`
(new, default false, independently toggleable from `oauth.enabled` itself), plus
`collab.enabled` (a `DocStore` must exist to serve from at all — an implementation
consequence, not a design choice). `daemon_mode=shared` remains meaningful only as an
editor-side UX signal (whether the local editor's tooling should attempt to *use*
`kb/query.*`), never a security boundary.

**Scoping decision — `kb/query.graph` is a whole-KB snapshot, not a parameterized BFS.**
Implemented as `{kb_id}` only (no `node_id`/`depth` params): returns every node id in the
KB (+ real edges for unencrypted KBs), capped by the same `max_scan_nodes` cap `search`
uses. A depth-limited traversal from a specific node is the LOCAL `kb_neighborhood` tool's
job (a different, already-solved problem); this surface's job is "what does the whole
graph look like from a thin client with nothing cached yet," which a capped snapshot
answers directly and more simply.

## Verification

- A thin client with no local replica can search/read an unencrypted hub KB it has
  Viewer+ access to, correctly gated by role. **Done** —
  `daemon/src/tests/kb_query_tests.rs::thin_client_with_no_replica_reads_an_unencrypted_kb_it_has_viewer_access_to`.
- A thin client can incrementally read an E2E-encrypted KB via the lazy-fetch primitive,
  with bounded, evictable local cache growth verified never to exceed its configured cap
  under sustained use. **Done** — `daemon/src/lazy_fetch_client.rs` (reuses
  `mae_kb::cache::NodeCache` unmodified, principal-scoped keys), exercised end-to-end by
  `kb_query_tests.rs::a_member_can_decrypt_kb_query_get_and_cache_it_via_the_lazy_fetch_client`
  and bound/no-cross-contamination proven by its own 3 unit tests.
- **Adversarial test (required, none exists today):** a "hostile/curious hub operator"
  test proving server-side plaintext search of an E2E-encrypted KB is structurally
  impossible through this surface — same test class as ADR-037's existing key-blind-relay
  tests. **Done** —
  `kb_query_tests.rs::hostile_hub_operator_cannot_search_an_e2e_kb_for_plaintext`: real
  sealed op-set ciphertext, structural search refusal asserted, and a byte-level scan of
  the serialized wire response proving the plaintext secret never appears anywhere in it.
  Also new: `unencrypted_kb_search_is_capped_and_cannot_full_dump` (the scan cap, not just
  the result-count cap, is what's actually enforced) and
  `non_member_is_denied_regardless_of_encryption` (the access gate fires before the
  encryption branch on both KB types).
- The surface is confirmed unreachable when `collab.enabled` is false, `oauth.enabled` is
  false, or `oauth.kb_query_enabled` is false (see the corrected gating above —
  supersedes the original "`daemon_mode` is not `shared`" phrasing). **Done** —
  `kb_query_tests.rs::kb_query_unreachable_when_disabled` drives the real
  `oauth::route_authenticated_request` gate directly, asserting a distinct "disabled"
  error (not "method not found") even with an otherwise-valid principal and a real
  `DocStore` available.
