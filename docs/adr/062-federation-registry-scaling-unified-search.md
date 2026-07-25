# ADR-062: Federation registry scaling + unified local/remote-hub search

**Status:** Accepted (implemented — see "Status note" at the end of this document).
**Extends:** ADR-029, ADR-057.
**Relates to:** ADR-053, ADR-058.

## Context

**Today's reality.** The KB federation registry (`shared/kb/src/federation.rs`) is, at its
core, an unindexed `Vec<KbInstance>` (`KbRegistry::instances`, `federation.rs:114-115`).
Every lookup by name or UUID — `find`, `find_mut`, `find_by_uuid`, `find_by_collab_id`, and
the duplicate-registration check inside `register` — is a linear `.iter().find(...)` scan
(`federation.rs:210` is one such scan; the same pattern recurs at every other lookup site in
the file). Worse, the read path that actually matters most — search — does an unconditional
O(N) fan-out across every registered instance on every call: `FederatedKb::search` and the
editor-level `kb_federated_search_scoped` both loop `for (uuid, kb) in &self.instances`
(`federation.rs:287-311` and its sibling in `crates/core/src/editor/kb_ops/search.rs`) with
no cap, no early-exit, and no way to skip an instance the caller doesn't care about short of
naming it explicitly via `KbScope::Named`. And naming it explicitly is not what happens by
default: `KbScope::All` — fan-out-to-literally-everything — is the default value of the
`kb_search_scope` option (`crates/core/src/options.rs:429-431`, default `"all"`), so every
`kb_search`/`kb_search_context` call an AI peer or a human issues, unless it opts into a
narrower scope, pays the full linear cost of every registered instance.

This is a fine, invisible cost at the scale MAE has operated at so far — a handful of
registered instances (the primary KB plus `DevPractices`, maybe one or two client/project
KBs). It stops being invisible the moment the vision ADR-057 documents is taken seriously.
ADR-057 quotes the project owner's stated vision verbatim: a core engine "supporting 'any
number' of local/federated instances for non-coding second-brain use," reachable "across
hosting servers, federated instances, and per-project local KBs." ADR-058 — the sibling ADR
that gives `KbInstance` a `project_root` and a `kind` discriminant so **per-project** KBs
become a first-class, opt-in-provisioned thing rather than everything landing in one flat
bucket — is explicitly designed to grow the number of registered instances a single
installation carries: a contributor working across `mae` itself, several client codebases,
and a personal second-brain KB is no longer a hypothetical multi-tenant scenario, it is
ADR-058's own stated motivating case. An unindexed `Vec` with an unconditional linear fan-out
on every search is the wrong data structure for a registry that ADR-058 is deliberately
designed to make larger, and ADR-057 itself names this exact gap: its own gap table (item 7,
"One search experience spanning local and remote KBs") records that local federation is
already unified and blended by default, but a remote hub KB reachable through ADR-053's
`kb/query.*` surface is "a structurally separate data model with its own request/response
shape; zero MCP tools wrap `kb/query.*`" — so a remote hub is reachable by the daemon
protocol but invisible to the same `kb_search`/`kb_search_context` surface a local federated
instance uses, and ADR-057 names ADR-062 (this ADR) as the owner of closing that gap.

**The two problems are related, not coincidental.** Fixing the registry's scaling ceiling and
fixing the local/remote-hub search split are the same piece of architectural work because
both are symptoms of the same underlying fact: the registry and the search fan-out logic were
built for "a small, exclusively-local set of instances," and both the instance count (ADR-058)
and the instance *kind* (a live remote hub, reachable only through ADR-053's structurally
different query surface) are about to stop being small and exclusively local at the same
time. Solving them together — one indexed registry, one blended fan-out loop that treats a
`RemoteHub`-kind instance as just another source to merge results from — is cheaper and more
correct than solving them as two separate follow-on patches later, each of which would
otherwise have to re-discover the same registry-shape questions independently.

**Grounded in real-world evidence.** org-roam is Emacs's own closest KB analog and the
closest real precedent for exactly this scaling problem: an org-mode-backed knowledge graph
with a derived, queryable database cache sitting in front of a directory of source text. Its
SQLite-cache-over-org-files design is direct, independent confirmation that MAE's
Cozo-as-rebuildable-projection model (ADR-029: "the yrs/CRDT layer is the canonical, durable
source of truth. CozoDB is a deterministic, durable, rebuildable PROJECTION") is
fundamentally the right shape — a real, shipped, multi-year project converged on the same
architectural answer independently. But org-roam's own real-world performance at scale is
extensively documented as a genuine pain point, not a hypothetical one: users report
multi-second `org-roam-node-find` latency and 14-15 second single-file DB updates at
thousands-of-nodes scale (github.com/org-roam/org-roam/issues/2474, /1752, /2241). This got
severe enough that a competing rewrite, **org-node**, was built specifically to fix it —
org-node's own README benchmarks 3,000-node indexing at **2 seconds versus org-roam's 2
minutes 48 seconds** (github.com/meedstrom/org-node, README.org). That is direct evidence
that "use a real indexed database" — which is essentially this ADR's Phase A — does not by
itself guarantee acceptable scaling. The *indexing/rebuild strategy* matters as much as the
storage engine choice: org-roam already used SQLite, a real indexed database, and still hit a
multi-minute wall at 3,000 nodes because its rebuild strategy was the bottleneck, not the
absence of an index. Phase A's verification therefore needs a concrete numeric target derived
from this real comparison, not a vague "sub-linear vs. today's linear baseline" claim that
would let a technically-sub-linear-but-still-slow implementation pass.

Second, the org-roam community's own hard-won best practice
(org-roam.discourse.group/t/org-roam-db-across-multiple-machines/332) is to **never sync the
derived SQLite DB itself between machines** — sync only the source org files, and let each
machine independently rebuild its own projection from them. This directly validates MAE's
existing architecture: ADR-029 already establishes that cozo is a rebuildable projection and
that CRDT truth, not the projection, is what crosses the wire (`doc_store.apply_update` is
the "one universal projection seam," per ADR-029 decision 3), and ADR-035 already treats the
daemon as optional infrastructure around that same CRDT-first design. But that rule has, so
far, only ever needed to be *implicit* — MAE has never before had a design that added new
cross-instance/cross-daemon federation surface at the exact layer where someone could
plausibly propose "just sync the Cozo file directly, it'd be faster" as a well-intentioned
optimization. This ADR is exactly that ADR, because Phase C/D add a `RemoteHub`-kind registry
entry and a live cross-network query path. The rule needs to be stated as an explicit, hard
constraint here, not left to be re-derived correctly (or incorrectly) later by whoever
eventually optimizes the remote-hub query path for latency.

Third, a real, sourced bug class: org-roam has hard SQL `UNIQUE constraint` crashes on
duplicate/colliding node IDs — e.g. arising from a sync-conflict copy of a file — that
**halt the entire database rebuild**, not just skip the offending node
(github.com/org-roam/org-roam/issues/1480, /1496). This is directly relevant once this ADR's
Phase C/D introduce federated instances from potentially independent, uncoordinated sources
(a locally-registered project KB and a separately-registered `RemoteHub` instance, or two
`RemoteHub` instances pointed at different hubs neither party controls) — ID collisions
become a realistic scenario an adversarial or simply careless multi-source registration can
produce, not a hypothetical edge case invented for the sake of having a test.

## Decision

**Phased A through E**, each independently shippable, ordered so that no phase depends on
functionality a later phase hasn't landed yet.

**Phase A — indexed registry, pure performance fix.** Replace the linear-scan `Vec` lookups
in `KbRegistry` (`find`, `find_mut`, `find_by_uuid`, `find_by_collab_id`, and the
duplicate-registration check in `register`) with indexed lookup by the two keys already used
throughout the codebase — UUID and name. The on-disk TOML registry format is unaffected: this
is a pure in-memory data-structure change, invisible to users and to every existing caller
beyond being faster. Consistent with the real-world evidence above, the concrete performance
target is not merely "sub-linear vs. today's baseline" but a target derived from the
org-node/org-roam comparison: rebuild/lookup cost at thousands-of-nodes, multi-hundred-
instance scale must scale with *changed* content, not total corpus size, and the regression
benchmark this phase ships must be measured against that comparison point (org-node's ~2s at
3,000 nodes vs. org-roam's ~2m48s), even though MAE's storage engine (Cozo) differs from
org-roam's SQLite — the point of the comparison is the rebuild/lookup *strategy*, not the
specific engine.

**Phase B — priority + capped, paginated fan-out.** Add a per-instance `priority: u32` field
to `KbInstance` (default equal-weight across all existing instances, so this is a zero-
behavior-change addition for every existing single/few-instance setup) consulted by the
existing dedup/merge logic. Today that logic's only tie-breaking rule is "local wins ties,"
with no further nuance once two or more federated (non-local) instances both match — `priority`
gives the merge step a deterministic second axis to break ties on beyond "local vs.
federated." Alongside `priority`, add a result-count cap and a pagination cursor to the
fan-out read path itself: today's unconditional full-fan-out search has no cap of any kind,
which is fine at three instances and becomes a real cost — and, combined with Phase D/E's
remote-hub fan-out, a real *latency* problem — as instance count grows toward the scale
ADR-058 is designed to produce.

**Phase C — `RemoteHub` as a registrable `KbInstance` kind.** Add a `RemoteHub` variant so a
hub reachable only via ADR-053's live-query surface (`kb/query.search` / `kb/query.get` /
`kb/query.graph`) can be *registered* in the same registry a local instance uses, the same
way, rather than existing in the completely separate, unregistered, tool-less limbo it
occupies today (per ADR-057's gap-table finding: reachable by the daemon protocol, invisible
to `kb_search`). ADR-058 is already proposing a `kind: KbInstanceKind` field on `KbInstance`
(`Primary`, `Project`, `Guidance`, `UserRegistered`) for an unrelated reason — per-project
provisioning — and it already lists this ADR under its own "Relates to," naming the exact
concern this Phase C exists to avoid: a second, parallel discriminant or a second lookup
surface would be "doubled indirection" that makes a scaling fix harder to reason about,
because it would have to account for two independently-evolving stores of "which KBs exist."
Phase C's `RemoteHub` variant therefore reuses ADR-058's `KbInstanceKind` enum unmodified if
ADR-058 merges first, adding `RemoteHub` as a fifth variant alongside `Primary`/`Project`/
`Guidance`/`UserRegistered` rather than introducing a competing enum. If Phase C ships before
ADR-058 does, it introduces the minimal two-variant enum (`Local`, `RemoteHub`) itself, sized
so ADR-058's later variants slot in without a breaking rename — coordination, not duplication,
either way.

**Phase D — the actual architectural fix this ADR exists to deliver: blended search.** A
bridging mechanism — a new async code path inside `kb_federated_search_scoped` and/or a
`KbScope` extension — so that when a `RemoteHub`-kind instance is registered and in scope,
the same fan-out call that already loops local/federated instances also calls out to that hub
via async HTTP against ADR-053's `kb/query.search` surface, with results merged by the
*same* Phase B dedup/priority logic used for purely local instances. This is deliberately
**one blended fan-out loop**, not two disconnected search experiences bolted together with an
if/else at the call site — the whole point is that a caller of `kb_search`/
`kb_federated_search_scoped` should not need to know or care whether a given matching node
came from the primary KB, a locally-registered federated instance, or a remote hub; the
scope, dedup, and priority machinery treats all three uniformly. This must correctly
translate across the "structurally different data model" boundary ADR-053 itself already
names explicitly in its own Implementation Note: hub-hosted content lives in `DocStore` as
`KbCollectionDoc`/`KbNodeDoc` yrs documents, a different shape from the local
`KbQueryLayer`/`CozoKbStore` node model `kb_federated_search_scoped` already operates over.
The translation at this boundary must not weaken either side's guarantees in either
direction — it must not accidentally expose hub-side access-control gaps (a locally-blended
result must still have passed ADR-053's own `kb_access(kb_id, principal, Read)` gate before
it ever reaches the merge step, never bypassed because it arrived through a "trusted" local
fan-out loop instead of a direct `kb/query.*` call), and it must not silently truncate the
richer local node structure (typed links, properties, activity metadata) down to whatever
simpler shape the hub's `DocStore` document happens to expose, in either direction of the
merge.

**Phase E — timeout-and-continue degradation contract.** A slow or unreachable `RemoteHub`
instance must not block or fail the *entire* blended query. Instead, a per-hub timeout
(budgeted against the same local-only latency window a purely-local fan-out already
delivers) causes that hub's contribution to be dropped from the current query, and the
overall result carries an explicit partial-result flag alongside whatever local and/or
reachable-remote results did come back in time. This is a genuinely new failure mode this
ADR introduces — today's all-local fan-out has no notion of "a source failed to respond" at
all, since every source is local and fast by construction — so it needs its own explicit
design and its own adversarial test (below), not an afterthought bolted onto Phase D's happy
path.

### Hard rule: the projection is never what crosses the wire between federated instances or peers

Stated explicitly here, not left implicit, because this is precisely the ADR that adds new
cross-instance/cross-daemon federation surface where an otherwise well-intentioned future
optimization could silently violate it: **the derived Cozo projection itself must never be
the thing synced or replicated between federated instances or daemon peers, at any phase of
this ADR.** Only source text/CRDT operations cross the wire (per ADR-029's already-decided
"CRDT is the canonical, durable source of truth; Cozo is a deterministic, rebuildable
projection" model); every instance and every peer always independently rebuilds its own
projection locally from that CRDT truth. This directly constrains the Phase C/D design: a
`RemoteHub`-kind instance is **always** queried live, on every call, per ADR-053's
already-decided live-scoped-query design — **never** mirrored as a local Cozo copy, even
though a local mirror would very plausibly be faster for repeated queries against the same
hub. Mirroring would violate this Hard Rule and reintroduce the exact staleness/corruption
risk the rule exists to prevent: a cached local projection of remote content is precisely the
kind of derived-artifact-as-truth mistake ADR-029's whole redesign exists to move MAE away
from, and org-roam's own community-established best practice (cited above: never sync the
derived DB, only the source, and always rebuild locally) is independent, real-world
confirmation that this constraint is not MAE-specific paranoia.

## Consequences

**Positive.** Removes a scaling ceiling before ADR-058's per-project provisioning work makes
it a live problem rather than a theoretical one — Phase A alone converts every registry
lookup from O(N) to effectively O(1)/O(log N) with zero user-visible behavior change. Phase D
closes the exact gap ADR-057's own gap analysis names against this ADR: local and remote-hub
search become one blended experience instead of two disconnected ones, which is the concrete,
buildable form of the vision's "one search experience spanning local and remote KBs" claim.
Reuses ADR-053's already-implemented, already-access-gated `kb/query.*` surface and ADR-058's
`kind` discriminant rather than inventing parallel mechanisms for either concern (principle
#8). The Hard Rule keeps this ADR's new federation surface from becoming a foot-gun for a
future contributor optimizing for latency at the cost of correctness.

**Costs (honest).** Phase D is genuinely new, security- and correctness-relevant network
surface layered on top of a fan-out loop that was previously pure in-process function calls —
a `RemoteHub` instance introduces network latency, network failure modes, and a foreign auth
boundary (ADR-053's `kb_access` gate) into a code path that every existing caller currently
assumes is fast and always-succeeds. Phase E's timeout-and-continue contract is new state
(a partial-result flag) every caller of `kb_federated_search_scoped`/`kb_search`/
`kb_search_context` must now be prepared to see and surface correctly, rather than silently
ignore — a caller that drops the flag on the floor will present a partial result as if it
were complete, which is a real, easy-to-introduce regression this ADR's own verification
must guard against. The translation across ADR-053's "structurally different data model"
boundary (Phase D) is genuinely fiddly correctness work, not a mechanical merge — get it
wrong in one direction and hub-side access control is silently weakened; get it wrong in the
other and local node structure is silently truncated for every blended query that happens to
include a remote hub in scope.

## Alternatives rejected

- **A separate, explicit "remote search" command/tool distinct from the regular search
  path**, instead of blending local and remote-hub results into one fan-out. Rejected — this
  would formalize the exact "two disconnected worlds" problem this ADR exists to close.
  Users and AI peers would have to know, out of band, which tool to invoke depending on
  where the content they're looking for happens to live, which defeats the entire point of
  federation (the caller shouldn't need to know or care which instance a result came from)
  and directly contradicts what ADR-057's gap analysis flags as missing.
- **Replicating remote-hub content locally as a cached mirror**, instead of live-querying it
  on every blended search. Rejected on multiple independent grounds: it defeats ADR-053's
  entire live/thin-client purpose (ADR-053 exists specifically so a thin client does not need
  to fully replicate a hub-hosted KB it only wants to search occasionally); it would silently
  turn what was designed as a lightweight, capped, evictable reference relationship into a
  full membership/replication relationship, reintroducing the storage-cost and
  encrypted-content-exposure problems ADR-053 was written to avoid; and it directly violates
  this ADR's own Hard Rule against syncing the derived projection between instances/peers —
  a local Cozo mirror of a remote hub's content is exactly the pattern the Hard Rule
  prohibits, however tempting it would be for query latency.

## Verification

Per CLAUDE.md principle #14, every phase below is verified adversarially — falsifying the
implementation, not confirming the happy path once in a fixed order.

- **Phase A.** Build a 500+-instance synthetic registry fixture and benchmark lookup/rebuild
  cost against the old linear-scan baseline, with the pass bar set against the org-node/
  org-roam comparison point cited in Context (org-node's ~2s vs. org-roam's ~2m48s at 3,000
  nodes is the target the new implementation must stay well clear of, adjusted for Cozo's
  different storage engine) — a technically-sub-linear-but-still-slow implementation must
  not pass this bar. Every existing dedup test and every existing `Named`-scope test must
  produce byte-identical results under the new indexed implementation, specifically
  falsifying "the index silently changed result ordering or silently dropped a result" as a
  regression class of its own.
- **Phase B.** Run 20 repeated identical queries against a fixed, seeded registry fixture;
  result ordering must be identical on every single run. This specifically falsifies
  nondeterministic tie-breaking — e.g. unstable hash-map iteration order leaking into
  user-visible result ordering — which a naive `priority`-tiebreak implementation could
  easily reintroduce if it iterates instances from an unordered collection.
- **Phase C/D.** An expired or revoked OAuth token mid-session (against ADR-053's auth layer)
  must produce a clean, explicit auth failure surfaced to the caller — never a silent empty
  result that is indistinguishable from "no matches." A malformed or oversized hub response
  must be rejected at the local translation boundary, never merged into the blended result
  set unvalidated — this specifically models a hostile or simply buggy hub server, and the
  test must confirm such a hub cannot inject content that violates local node-shape
  invariants (e.g. a `DocStore` document masquerading as a node with typed-link/property
  structure the hub was never authorized to assert). **The org-roam#1480/#1496 failure
  class, reproduced directly as a named test:** two independently-registered federated
  instances that happen to share a colliding node ID — simulating a real uncoordinated
  multi-source registration scenario (e.g. a local `Project`-kind instance and a
  `RemoteHub`-kind instance registered by different people pointed at content that
  coincidentally assigned the same ID), not a hand-picked convenient collision constructed
  to be easy to detect — must degrade gracefully on a blended query: skip the colliding
  node, surface a warning, and the query must **never** crash or halt the fan-out for every
  other, unrelated instance in scope. This is the specific "not just a hypothetical, but a
  documented real bug class in the closest comparable project" test this ADR's Context
  argues for by citing org-roam's own crash history.
- **Phase E.** An **N-way** blended query test — not a 2-source happy path — with a primary
  instance, 2 local federated instances, and 1 deliberately-hung `RemoteHub` instance that
  never responds. The other 3 sources must return results within the local-only latency
  budget, with the partial-result flag correctly set to `true` on the response. A subsequent
  query issued after the hung hub becomes reachable again must include its results normally
  with the partial-result flag correctly `false`, proving there is no stuck-in-degraded-mode
  state incorrectly persisting across queries once the hub recovers.
- **Cross-cutting test confirming the Hard Rule.** Force a `RemoteHub` instance's live data
  to differ from what a hypothetical local cached mirror of it would show — e.g. update
  content on the hub between two local blended queries in the same test — and assert the
  second query's blended result reflects the live, current hub data, never a stale local
  copy from the first query. This is the direct proof that nothing is being silently cached
  or mirrored in violation of the Hard Rule, exercised as an actual test rather than left as
  an assertion in prose.

---

## Status note (added on implementation)

All five phases are implemented, tested, and shipped.

**Phase A — evidence-based scope correction.** Before indexing anything, a real
benchmark (`registry_find_by_uuid_stays_well_under_budget_at_thousands_of_instances`,
`shared/kb/src/federation.rs`) measured the *actual* cost of `KbRegistry`'s linear scans
at 500/2,000/5,000 synthetic instances: worst case ~10.5μs even at 5,000 entries — five to
six orders of magnitude below any user-observable latency, and nowhere near org-roam's own
cited cliff (which is about *KB-node* count inside one store, not *registered-instance*
count — a population this project's own registry keeps several orders of magnitude
smaller than node counts by construction: one entry per project/second-brain KB a user
explicitly registers, not per note). A repo-wide grep of every real call site
(`shared/kb`, `crates/`, `daemon/`) also confirmed none does repeated lookups against one
loaded registry snapshot — every call site does exactly one `find()`/`find_by_uuid()` per
load. Given `KbRegistry.instances` is directly, publicly mutated at 15+ call sites across
8 files outside `federation.rs`, adding a persistent secondary index would have required
either a large encapsulation refactor (making the field private, migrating every call
site) or accepting real desync/correctness risk for a performance win that doesn't exist
at any realistic or even generously-projected scale. **Correction:** Phase A shipped as a
permanent regression benchmark (proving the linear scan stays fast, guarding against a
future accidental quadratic regression) rather than a new index — and redirected the real
indexing/scaling effort to Phase B, where the actual analog to org-roam's problem lives
(per-query fan-out cost, which DOES scale with registered-instance count on every single
search).

**Phase B — shipped as designed, plus a determinism fix.** `KbInstance.priority: u32`
added (`#[serde(default)]`, backward compatible); `FederatedQuery` now dedups colliding
node ids by priority (highest wins; primary is always implicitly highest) across every
merge-based method (`search`/`agenda`/`list_ids`/`id_title_pairs`/`id_title_body_triples`)
and every ownership-lookup method (`get`/`links_from`/`neighborhood`/`related`/`history`).
`search`'s final ordering is now fully deterministic (score descending, id ascending
tiebreak) — closing a real nondeterminism risk in the original `HashSet`-based merge (Rust
`HashMap`/`HashSet` iteration order is process-randomized; a tied score could silently
reorder across runs). A configurable fan-out cap
(`kb_federated_max_fanout_instances`, OptionRegistry-registered per principle #7, default
128) truncates to the *highest*-priority instances when a registry exceeds it, logging a
warning rather than silently dropping scope. Verified: `federated_query_priority_decides_colliding_instance_ids_regardless_of_registration_order`,
`federated_search_ordering_is_stable_across_twenty_repeated_identical_queries` (the ADR's
own named test), `federated_query_fanout_cap_excludes_only_the_lowest_priority_instances`.

**Phase C — shipped, with fields designed against the real ADR-053 implementation.**
`KbInstanceKind::RemoteHub` added; a `RemoteHubConfig{base_url, hub_kb_id, auth}` +
`RemoteHubAuth{Command(String), KeystoreKey(String)}` (a credential *reference*, never a
raw token — matching `collab_bridge::resolve_client_credential`'s existing precedent)
added to `KbInstance`. `KbRegistry::register_remote_hub` is idempotent on
`(base_url, hub_kb_id)`. Fields were designed by first investigating ADR-053's *actual*
shipped implementation (`daemon/src/kb_query.rs`, `daemon/src/oauth.rs`) rather than
assuming its prose — confirmed the real RPC shapes, the real 401/bearer-token contract,
and that no client-side Rust code calling `kb/query.*` existed yet anywhere in the repo
(Phase D had to write it from scratch). `org_dir`/`db_path` stay required, non-`Option`
`PathBuf`s (empty for a `RemoteHub` instance) rather than becoming `Option` everywhere —
avoids rippling a breaking change through 30+ existing call sites for two fields that are
already meaningless-but-harmless when empty.

**Phase D — shipped as a genuinely live-queried, blocking `KbQueryLayer`.** Initial
concern (mid-implementation): the ADR's own "via async HTTP" phrasing implied a large,
risky async refactor of the entire synchronous KB-search call chain
(`kb_exec::dispatch` → `Editor::kb_federated_search_scoped` → `FederatedQuery` are all
plain, non-async `&mut Editor`-taking functions). Investigation resolved this: every
`KbQueryLayer` implementor (including `CozoQueryLayer`) is *already* synchronous by
design — a blocking HTTP client with a strict timeout is not a workaround, it's the
architecturally consistent way to add a new backend to an already-synchronous trait.
Shipped `RemoteHubQueryLayer` (`shared/kb/src/remote_hub.rs`, feature-gated behind a new
optional `remote-hub` Cargo feature — off by default, forwarded through
`mae-core`/`mae` so the interactive binary doesn't pay for a TLS+HTTP stack unless a user
opts in) implementing `get`/`contains`/`search`/`list_ids` against real `kb/query.*`
shapes, with `links_from`/`links_to`/`neighborhood`/`id_title_pairs` correctly returning
empty (no such endpoints exist on ADR-053's surface; `id_title_pairs` deliberately does
NOT fall back to an N+1-per-node `get()` loop, which would be a hidden, unbounded
network-call scaling trap for a "list all titles" call on a large hub). E2E-encrypted hub
content is explicitly, observably unsupported (`last_outcome()` surfaces it distinctly,
never silently returned as plaintext) — decrypting it needs ADR-038/039's membership/key
machinery, which lives above this crate in the dependency graph; a real follow-up, not a
silent gap. Translation-boundary hardening: a response-size cap (8MB), malformed-JSON
rejection, schema-incomplete-result rejection, and JSON-RPC-error-surfacing are all real,
independently tested (`shared/kb/src/remote_hub.rs`'s own unit-test module, 8 tests
against a hand-rolled protocol-accurate mock server). Auth failures are recorded via a
`last_outcome()` diagnostic (not part of `KbQueryLayer` itself, which has no room for an
error value — matching `CozoQueryLayer::get`'s own existing "log + return empty"
precedent) so a caller/test can distinguish "the hub legitimately has nothing" from "the
call failed," proven end-to-end against a **real spawned `mae-daemon`** with real TLS +
real RS256-signed JWTs (`daemon/tests/remote_hub_query_layer_e2e.rs`, reusing
`oauth_e2e.rs`'s proven harness shape) — an expired token produces a real 401 over the
real wire and a clean, observable `AuthFailed` outcome, never a silent empty result.

**Phase E — shipped, plus a real bug found and fixed while wiring it in.** While writing
Phase E's own named "N-way blended query with a hung hub" test, discovered
`FederatedQuery::search`'s fan-out was **sequential**, not concurrent — a loop over
`priority_ordered_instances()` calling each instance's `.search()` one at a time. A slow
`RemoteHubQueryLayer` (bounded by its own ~1.5s default timeout) positioned anywhere in
that loop would have serialized its full timeout with every other instance's latency,
making "the other 3 sources return within the local-only latency budget" false by
construction — total latency would have been the *sum* of every source's latency, not the
*max*. Fixed: `search`'s fan-out now runs every source (primary + each instance) on its
own thread via `std::thread::scope`, joining all before merging — total latency is now
bounded by the single slowest source, which is itself timeout-bounded. Added
`KbQueryLayer::degraded()` (default `false`; `RemoteHubQueryLayer` overrides it from its
own `last_outcome()`) and `FederatedQuery::last_query_was_partial()` (set fresh on every
`search()` call, never sticky) as the partial-result flag the Decision text calls for,
without widening every `KbQueryLayer` method's return type to carry a `Result`. Verified
by `n_way_blended_query_with_a_hung_hub_bounds_latency_to_the_slowest_source_and_flags_partial`
(primary + 2 local `InMemoryQueryLayer` instances + 1 deliberately-hung `RemoteHubQueryLayer`
that accepts a connection and never responds): total latency stays near the hung hub's own
timeout rather than serializing with it, the 3 healthy sources' content is present, the
partial flag is set, and a subsequent query against an all-healthy federation correctly
clears it — no stuck-degraded state.

**Filed, not fixed this pass:** issue #448 — a real, adjacent, currently-uncovered gap
found while explaining this work: nothing today lets a KB admin force an *authorized
member* (not just a non-member thin client, which ADR-053 already covers) onto
live-query-only access instead of full `kb_join` replication. `RemoteHubQueryLayer` is
exactly the client-side vehicle a future ADR addressing that gap would rely on, but the
server-side policy + `kb_join`-dispatch enforcement is out of this ADR's scope and belongs
in its own design (tracked for a future ADR-067).
