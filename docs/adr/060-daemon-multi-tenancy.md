# ADR-060: Daemon multi-tenancy

**Status:** In progress (Phases A/B/C/D landed — see "Implementation note" sections below;
Phase C's collab/OAuth-side principal-keyed wiring is explicitly deferred, tracked as issue
#456, not silently gapped. Phases E–G remain, tracked as real follow-on work.)
**Extends:** ADR-035, ADR-054, ADR-057.
**Relates to:** ADR-017, ADR-018, ADR-025.
**Tracking:** issue #375 (epic tracker).

## Context

### Scoping call — state this up front, it bounds every phase below

MAE is GPL-3.0-or-later and local-first (CLAUDE.md principle #12). `mae-daemon` is
described in this project's own vision as running on "a dedicated server or the client
machine" — never as a hosted, multi-region SaaS control plane serving mutually
adversarial paying customers. This ADR's bar is therefore **trusted-org-scale
multi-tenancy**: one operator (a team, a lab, a household, a single company's internal
tooling) running one `mae-daemon` process on behalf of several independent users or teams
who trust the *operator* but not necessarily each other's KB contents. It is explicitly
**not** adversarial-tenant cloud isolation. That means: no container- or cgroup-per-tenant
sandboxing, no billing/usage metering, no multi-region control plane, no tenant
self-service provisioning API. Every decision below is sized to that bar. Where a design
choice would only make sense at cloud-hosting scale (Phase E's process-isolation escape
hatch is the closest this ADR comes, and even that is a systemd unit, not a container
runtime), it is called out explicitly as staying inside the trusted-org bound rather than
reaching past it. Readers evaluating this ADR against a cloud-multi-tenancy threat model
are evaluating it against the wrong bar — see "Alternatives rejected" for why that bar was
deliberately not chosen.

### Today's reality: one global lock hiding behind already-partitioned storage

The daemon's KB Unix-socket path already stores data per-instance —
`DaemonState.instance_stores: HashMap<String, Arc<CozoKbStore>>`
(`daemon/src/handler.rs:22-23`) keys each registered KB's `CozoKbStore` handle separately.
On paper this looks like the storage layer is already tenant-partitioned. It is not,
because the *lock* around that map doesn't respect the partition it guards: the entire
`DaemonState` — the map, the query layer, everything — is wrapped exactly once in
`Arc<tokio::sync::Mutex<DaemonState>>` (`daemon/src/main.rs:155`), and essentially every
`handler::dispatch` arm takes `state.lock().await` and runs inside that single global
critical section. ADR-054 already diagnosed and partially closed this for the
*single-tenant, single-operator* case — via the snapshot-then-drop +
`tokio::task::spawn_blocking` pattern documented in that ADR's "Implementation note" — but
that fix operates entirely inside the existing single global `DaemonState`. It makes
concurrent reads against different KBs *within one tenant's session* not serialize behind
each other at the query-execution level; it does nothing about the fact that there is no
tenant concept in the daemon's addressing, authorization, or resource-accounting model at
all. Two independent teams pointing their respective MCP clients at the same daemon today
get exactly the same shared, unscoped resource pool, the same shared connection cap, and
the same shared failure domain as two sessions belonging to the same person. There is no
notion of "tenant" anywhere in `DaemonState`, `handler.rs`, or `daemon.toml` — only
"instance" (a KB) and "principal" (an authenticated Ed25519 key), and nothing today groups
principals or instances into an operator-meaningful tenant boundary with its own quota,
addressing, or blast-radius semantics.

Deployment tooling reflects the same single-tenant assumption. `assets/mae-daemon.service`
is a fixed, non-templated systemd unit — one process, one `%h/.local/share/mae` data
directory, one `~/.config/mae/daemon.toml`, started with `systemctl --user enable --now
mae-daemon`. There is no way to run two independently-configured daemon instances for two
tenants on the same host without hand-editing unit files or duplicating the whole
`assets/` directory per tenant. Contrast this with `assets/mae-headless@.service`, which
*is* already a proven systemd **template unit** in this same codebase: "This is a systemd
TEMPLATE unit: one instance per project, instantiated with the project's absolute path as
the instance name, systemd-escaped" (`assets/mae-headless@.service:5-6`), giving each
`mae --headless` instance its own `WorkingDirectory=%i`, its own process, its own failure
domain, activated per-project via `systemctl --user enable --now
'mae-headless@'"$(systemd-escape /path/to/project)"'.service'`. That pattern already
exists, is already shipped, and already solves "one systemd unit definition, many
independently-addressable running instances" for the headless engine. `mae-daemon` has no
equivalent.

Finally, ADR-054's own benchmark — the daemon's only existing published capacity number —
never asked the multi-tenant question at all. Its criterion bench
(`daemon/benches/kb_dispatch_concurrency.rs`) measured "~8 concurrent MCP sessions before
p99 latency exceeds 2x the single-client baseline" against **one** 20,000-node store with
**one** implicit tenant. It is a real, honestly-measured number for the question it asked
("how many concurrent sessions can hammer one KB before it degrades"), but it says nothing
about what happens when those sessions belong to N independent tenants each with their own
KBs, quotas, and expectations of isolation from each other's load. Treating that number as
"the daemon's multi-tenant capacity" would be a misapplication of a single-tenant
benchmark to a question it was never designed to answer — exactly the kind of unverified
capacity claim CLAUDE.md principle #15 says should be closed with evidence, not assumed
by extrapolation.

### Grounded in real-world evidence — why the adversarial-testing bar is raised, not lowered, here

No tool in MAE's own Emacs lineage has ever attempted multi-tenancy. `org-roam-server` and
`org-roam-ui` are explicitly single-user local tools; no hosted, multi-tenant Emacs service
of any kind was found during research for this ADR. That absence matters: this is a
genuinely novel direction for MAE, with no directly-inherited MAE-lineage precedent to lean
on for "here's how we already know this class of bug shows up in our own codebase." Per
CLAUDE.md principle #14, novelty is a reason to raise the adversarial-testing bar, not an
excuse to lower it because "nothing like this has broken before" — nothing like this has
been *tried* before, which is different.

What MAE does inherit, directly, is Emacs's own daemon architecture — `mae-daemon` and
`mae --headless` are this project's answer to the same "one long-running process, many
connecting clients" shape Emacs's `emacs --daemon` pioneered. Emacs's daemon has two real,
sourced bug histories directly on point:

1. **A single misbehaving client can hang the entire daemon for every other client.**
   GNU bug#11639 (lists.gnu.org/archive/html/bug-gnu-emacs/2012-06/msg00161.html) and
   bug#23499 (lists.gnu.org/archive/html/bug-gnu-emacs/2016-05/msg00554.html) both document
   this failure shape recurring years apart in the same codebase. This is direct, concrete
   precedent — not speculation — for why Phase B's lock-split below is closing a
   documented, *recurring* bug class in this project's own architectural lineage, per
   CLAUDE.md principle #15 ("bugs are drift signals... fix the drift for that whole feature
   area"). A daemon that serializes unrelated clients behind one lock will eventually
   reproduce this bug shape; it is not a hypothetical risk being hardened against out of
   excessive caution.
2. **Unbounded memory growth over long-running daemon sessions**, severe enough that real
   users report needing weekly forced restarts to keep a daemon usable (GNU bug#38345,
   lists.gnu.org/archive/html/bug-gnu-emacs/2019-12/msg00181.html). This is direct
   precedent for why Phase C below treats per-tenant restart/eviction as a *distinct*
   mechanism from resource quotas, not an optional refinement of them.

The closest real production analog to this ADR's overall shape is **gopls's `-remote`
daemon mode** (go.dev/gopls/daemon) — a shared Go-tooling daemon serving multiple editor
sessions from a shared cache with a per-session view/state split. Its existence and
continued use validates that this ADR's general shape (one shared long-running process,
many logically-separated sessions, shared cache where safe) is a proven pattern in
production tooling, not a speculative architecture. But gopls's own documentation is
candid about two constraints this ADR should adopt rather than assume away:

- Memory/resource savings from a shared daemon are **limited to cache overlap between
  sessions** — it is not a blanket claim that multi-tenancy saves "most" resources across
  unrelated tenants with no shared data. Phase F's published capacity numbers must be
  worded to match this reality, not overstate savings a genuinely independent-KB tenant
  workload would never realize.
- **A live gopls daemon cannot be reconfigured — a config change requires a restart.**
  Phase G below adopts this same discipline explicitly for `mae-daemon`: state plainly
  whether a given class of config change (new tenant registered, quota adjusted) applies
  live or requires a restart, rather than leaving that contract implicit or assumed.

Finally, **rust-analyzer's own still-open, multi-year issue trail of unbounded memory
growth ending in OOM** — github.com/rust-lang/rust-analyzer/issues/20949, /18127, and
/13673 — with "restart the server" as the only documented user-facing workaround, is a
direct, current-day warning of exactly the failure mode this ADR must not let a
multi-tenant `mae-daemon` inherit *multiplied*. In a single-tenant LSP server, one leaking
process affects one user. In a multi-tenant daemon serving many tenants from one process,
the same unbounded-growth bug affects every co-resident tenant at once — and worse, a
daemon operator who restarts the whole process to reclaim memory (rust-analyzer's only
workaround) evicts every *other* tenant's live session along with the one that actually
leaked. This is precisely why Phase C's per-tenant restart/eviction mechanism is not
optional polish layered on top of resource caps — it is the difference between "a daemon
that degrades gracefully per-tenant when one tenant misbehaves" and "a daemon that
inherits rust-analyzer's known failure mode, but now with a wider blast radius."

**Real-world precedent for the quota mechanism itself** (Phase C, distinct from the
eviction precedent above): **Kubernetes `ResourceQuota` + `LimitRange`** is the standard
trusted-multi-tenant resource-governance pattern — a namespace-level aggregate hard cap
(`ResourceQuota`) paired with per-object defaults/maximums within that namespace
(`LimitRange`), and the field's own stated best practice is to start with generous quotas
and tighten them from observed real usage rather than invent numbers up front. This is
directly adopted below: Phase C's `daemon.toml` quota defaults are deliberately generous,
sized against ADR-054's own measured ~8-concurrent-session ceiling
(`docs/adr/004-kb-scaling.md`'s Tier 1 table), not picked arbitrarily. Separately,
**GitHub's own production REST API rate limiting** (docs.github.com/en/rest/using-the-rest-
api/rate-limits-for-the-rest-api) is real, current precedent for *how* to shape a quota
budget for a mixed read/write API: alongside a simple per-hour primary bucket, GitHub
layers a **secondary limit** combining a concurrent-request cap with a cost-weighted
points-per-minute budget where a write (POST/PATCH/PUT/DELETE) costs 5 points against a
read's 1 — not four independent flat counters per resource type. Phase C adopts this
cost-weighted-single-budget shape directly below, rather than the ADR's own earlier,
vaguer "connection count, query rate, result size, background-job priority" framing
implying four separate mechanisms.

## Decision

This ADR proceeds in seven phases (A–G). Phases A–D are the core multi-tenancy mechanism
inside a single daemon process; Phase E is the process-isolation escape hatch for tenants
that must not share a process at all; Phases F–G are measurement and documentation.

### Phase A — per-tenant RPC addressing

Every daemon RPC gains an explicit tenant/instance address, reusing the
already-partitioned `instance_stores: HashMap<String, Arc<CozoKbStore>>` keys
(`daemon/src/handler.rs:22-23`) as the address space rather than inventing a new
identifier scheme (principle #8 — no ad-hoc solutions, no duplicated addressing concept
next to one that already exists). This is deliberately the smallest possible first step:
it adds a field to the RPC envelope, it does not yet change locking or resource
accounting. **Backward compatibility is load-bearing, not incidental**: an RPC that omits
the address resolves to today's single primary instance exactly as it does now, so every
existing single-tenant deployment — which is the overwhelming majority of deployments
today and will remain common — sees zero behavior change from this phase alone. Phase A is
purely additive plumbing that the later phases build on; it introduces no new
authorization surface and no new failure mode by itself.

### Phase B — split the global lock into a directory plus per-instance state

This is the actual fix, and the one ADR-054 did not attempt because ADR-054 was scoped to
single-tenant concurrency inside the existing single `DaemonState`. Replace the single
`Arc<Mutex<DaemonState>>` with two things: a small, always-cheap-to-lock top-level
"directory" structure mapping tenant → instance-state handle, and a genuinely separate
`Arc<Mutex<InstanceState>>` (or an equivalent finer-grained construct — see ADR-054's own
"Implementation note" for the precedent of resolving a decision's literal mechanism
against what Cozo's own concurrency control already provides, which applies here too and
should be re-checked during implementation rather than assumed) per tenant. The directory
lock is held only long enough to look up or register a tenant's handle — never held across
an actual query or mutation. Two tenants' concurrent operations must stop serializing on
each other's lock entirely; this is the specific, measurable property that distinguishes a
real fix from a superficial per-tenant API sitting on top of the same shared critical
section (see Verification below for the test that falsifies exactly this shortcut).

### Phase C — per-tenant quotas and independent restart/eviction

**Corrected during design (principle #15 — re-checked before implementing, the same
discipline Phase B and Phase D applied to their own originally-proposed mechanisms).** This
phase's text originally said quotas would "extend ADR-054's already-existing per-principal/
per-IP soft throttle mechanism." That mechanism does not exist: `daemon/src/conn_limit.rs`'s
`ConnLimiter` is the only admission-control primitive in this daemon today, and it is a
global, identity-blind, per-*listener* connection-**count** cap — not per-principal, not
per-IP, not a time-windowed rate limiter of any kind. `daemon/src/config.rs:65-70` and
`:435-442`, and `daemon/src/main.rs:1259-1264`, independently confirm this in their own
words: "there is no principal or IP on a Unix domain socket... filesystem-permissions-only
trust." No rate-limiting crate (token bucket, sliding window, leaky bucket) is a dependency
anywhere in this workspace. Phase C is therefore "design and build the first identity/
tenant-scoped accounting this daemon has ever had," not an extension.

**A second correction, found in the same pass: quotas cannot be keyed on principal identity
uniformly across all three listeners, because only two of the three have one.** The KB Unix
socket — `daemon/src/handler.rs::dispatch`, where Phase A's `instance_addr()` addressing
lives, and which carries every locally-connected frontend's routine `kb_search`/`kb_get`
traffic — has zero principal concept, by the same deliberate, documented design cited above.
The collab (mTLS) listener and the OAuth listener *do* carry real identity
(`Session::authenticated_principal()`, `shared/mcp/src/session.rs:140-146`; `OAuthConfig.
principal_claim`-mapped JWT `sub`, `daemon/src/oauth.rs:113-183`). Resolution: **two quota
keys, not one** — the KB socket keys on Phase A's own instance address (already resolved on
every dispatch arm); collab/OAuth key on the real authenticated principal. This is not a
compromise forced by a gap; the KB socket's local-trust model is a permanent design choice
(three independent code comments say so), not something a quota mechanism should route
around by inventing a new local-auth handshake it was never meant to need.

**Tenant representation.** A new `[[tenant]]` array-of-tables in `daemon.toml`, sibling of
`[collab]`/`[oauth]`/`[kb_socket]` on `DaemonConfig` (`daemon/src/config.rs:38-71`):

```toml
[[tenant]]
name = "team-a"
instances = ["team-a-kb", "shared-ref"]   # Phase A addresses this tenant owns
principals = ["ed25519:AbCd...", "psk:teamA-key1"]  # collab/OAuth identities it owns

[tenant.quota]
max_connections = 32
budget_per_minute = 1000
max_result_bytes = 4194304
idle_evict_secs = 1800
```

Zero `[[tenant]]` tables means zero behavior change, matching Phase A's own backward-
compatibility contract. Runtime state lives in a **new sibling structure**, `TenantRegistry`
— not inside `DaemonState` — passed into `dispatch` alongside `Arc<Mutex<DaemonState>>`.
This follows directly from Phase B's own finding: `DaemonState`'s lock is safe today
precisely because nothing per-request-contended lives inside it; adding live quota
counters there would either reintroduce the contention risk Phase B just proved doesn't
exist, or bury a second lock inside the first for no benefit over a sibling structure.
`TenantRegistry` is backed by `dashmap` (v6.2.1, already fully resolved in both this
workspace's and the daemon's own `Cargo.lock` — transitively via `yrs` — so promoting it to
a direct `daemon/Cargo.toml` dependency adds zero new crates to the build graph), reusing a
proven sharded-concurrent-map primitive rather than hand-rolling a new locking scheme.

**Quota mechanism: one cost-weighted points budget per fixed 60-second window, not four
independent flat counters.** Direct precedent from GitHub's own production secondary rate
limit (see Context): reads cost 1 point (`kb/get`, `kb/links_from`, `kb/links_to`,
`kb/list_ids`), broader scans cost 3 (`kb/search`, `kb/related`, `kb/neighborhood`), any
mutating hygiene arm costs 5, and a result-size overage (over `max_result_bytes`) adds +2 on
top of the base cost. Connection count stays its own separate mechanism — a second,
tenant-scoped `ConnLimiter` instance (the same struct Phase A's listeners already use,
instantiated once per tenant instead of once per listener) — a genuinely different resource
dimension, mirroring GitHub's own two-tier split rather than folding everything into one
number. Enforcement plugs into `handler::dispatch` immediately after `instance_addr(&params)`
resolves and *before* `snapshot_query_layer`/`snapshot_store` ever touch `DaemonState`'s
lock or spawn blocking work — a rejected request costs only a `dashmap` lookup and an atomic
compare, never contending with any other tenant's in-flight work.

**Independent restart/eviction**, a genuinely separate mechanism from quotas because quotas
alone do not solve the problem the Emacs bug#38345 / rust-analyzer precedent describes.
Resource caps prevent a tenant from *starting* to consume unboundedly, but do nothing once a
tenant's in-process state has already grown pathologically inside its cap-respecting
steady-state footprint. Reuses existing precedent exactly, inventing no new idiom:
`DaemonScheduler::run_maintenance_tick` (`daemon/src/scheduler.rs:237`, already the periodic
per-instance hygiene home) gains an idle-tenant sweep evicting any tenant past its
`idle_evict_secs`, plus a manual `daemon/evict_tenant` RPC for an operator who notices
pathological growth without waiting for an idle window a still-connected tenant may never
hit (the direct, named rust-analyzer-precedent lesson: an operator's only tool must not be
"restart the whole process"). Concretely, what gets evicted and what happens on the next
access:
- The `TenantQuotaState` itself — dropped; the next request re-inserts a zeroed one via
  `dashmap`'s `entry().or_insert_with()`, mirroring
  `crates/core/src/editor/window_ops.rs:634-644`'s `mcp_session_windows` coarse-evict-then-
  self-heal idiom (`crates/core/src/editor/ai_state.rs:48,111`).
- The `Arc<CozoKbStore>` handle(s) in `DaemonState.instance_stores` — removed under Phase
  B's own brief snapshot-then-drop lock discipline, forcing a lazy reopen on next access —
  the direct precedent for this is `daemon/src/doc_store.rs:827`'s `evict_idle` (paired with
  `pick_lru_evictable`, `daemon/src/doc_store.rs:171`), which already "lazy-reloads from
  SQLite on next access."
- The federated `query_layer`'s per-name entry — rebuilt via the existing
  `DaemonState::rebuild_query_layer()`, reused rather than reinvented.
- If org-dir-backed, `DaemonScheduler.watchers[uuid]` — removed the same way
  `run_watcher_tick`'s existing `watchers.retain(...)` (`daemon/src/scheduler.rs:162`)
  already prunes unregistered instances; its existing "recreate if missing" check
  (`daemon/src/scheduler.rs:165`) already self-heals this one for free, with zero new code.

**Safety, stated explicitly:** an in-flight `spawn_blocking` query already holds its own
`Arc<CozoKbStore>` clone (Phase A's own snapshot-then-drop design), so removing the map
entry during eviction never disturbs it — a stronger guarantee than the `try_lock`-skip
`daemon/src/doc_store.rs`'s own `pick_lru_evictable` needs for its busier, more contended
case, meaning tenant eviction here can run unconditionally without checking for in-flight
work first. This is the direct, named lesson from both precedents cited in Context: Emacs's
daemon has no per-client reset short of a full restart, and rust-analyzer's only workaround
is the same blunt instrument. A multi-tenant `mae-daemon` that also has no better answer
than "restart the whole process" inherits that exact failure mode, but multiplies its cost
by every co-resident tenant forced to restart along with the one that actually leaked.

**Explicitly out of scope for this phase, stated plainly rather than left implicit:** a full
token-bucket/leaky-bucket algorithm (a fixed window's known boundary-burst weakness is an
accepted, disclosed trade-off — the same tier as `ConnLimiter`'s own existing `Relaxed`-
ordering approximation — and no such crate exists in this dependency graph to build on);
adaptive/dynamic quota tuning (the Kubernetes `ResourceQuota` precedent's own "start
generous, tighten from real usage" guidance is adopted directly — static `daemon.toml`
values only, auto-tuning is Phase F-contingent future work once real multi-tenant capacity
numbers exist); unifying the KB-socket and collab/OAuth accounting into one identity model
(the two-key split above is final for this phase, not an interim step — the KB socket's
trust model is a permanent design choice, not a gap to close later); and per-tenant
CPU-seconds budgeting (GitHub's third rate-limit dimension — MAE's existing scan/fanout
caps, `DEFAULT_MAX_FANOUT_INSTANCES` and `OAuthConfig`'s `kb_query_max_scan_nodes`/
`kb_query_max_search_results`, already bound this cost dimension; principle #8, no
duplicate mechanism).

### Phase D — tenant-boundary role composition and the IDOR-shaped adversarial case

Per-KB roles (ADR-017/ADR-018's Owner/Editor/Viewer model) continue to compose normally
across tenants a given principal happens to be a member of — a principal that is Owner on
one tenant's KB and Viewer on another's is still exactly that, unchanged by this ADR.
Tenant-level quotas apply regardless of role; quota headroom is never a substitute for
authorization. **Explicit non-goal, stated plainly**: this ADR creates no new cross-tenant
trust relationship. Federation and collaboration membership (ADR-018's join
policy/roles, the existing sharing mechanism described in `docs/KB_SHARING.md`) remain the
*only* path by which a principal gains visibility into another tenant's KB content;
multi-tenancy on the daemon is purely an operational/resource-isolation boundary layered
underneath the existing authorization model, not a new authorization primitive that
bypasses it.

The single most important adversarial case this phase must close was found via real,
sourced precedent from outside this project — two independent, recent CVEs both
root-caused to the same shape of bug: authorization checked correctly at the *outer*
request-routing layer, but not re-checked at the point an inner, request-supplied
identifier is actually resolved against data. Gitea's container-registry authorization
bypass (CVE-2026-27771, corgea.com/research/gitea-forgejo-private-container-registry-bypass)
and the related CVE-2026-58444 (advisories.gitlab.com/golang/code.gitea.io/gitea/CVE-2026-58444/),
alongside Vaultwarden's CVE-2026-27898 (sentinelone.com/vulnerability-database/cve-2026-27898/),
each involve a request that is *correctly addressed* at the resource the requester is
entitled to, but whose payload references a raw internal identifier that actually belongs
to someone else's data — and that inner identifier gets resolved and served without a
second, independent authorization check at resolution time.

Applied directly to this ADR's Phase A addressing scheme: a request correctly addressed
at tenant A's own instance (Phase A's outer address is valid, the principal is genuinely
authorized for tenant A) whose payload separately references a raw KB node/resource ID
that actually belongs to a *different* tenant's data must be **rejected at the point that
inner ID is resolved against tenant A's own scope** — not silently served just because the
outer RPC address checked out. This is a distinct, IDOR-shaped (Insecure Direct Object
Reference) failure mode from "wrong instance addressed" (which Phase A's addressing
already prevents structurally by construction). It is a failure mode only Phase D's
resolution-time check closes, and the two cited CVEs demonstrate concretely that this
exact bug shape survives in real, security-conscious codebases when only the outer
addressing/routing layer is checked and every downstream identifier resolution is assumed
safe by association. This is named as the primary adversarial test for this entire ADR in
Verification below, not a secondary item in a longer list.

### Phase E — a `mae-daemon@.service` systemd template unit for process-level isolation

For tenants that must not share a process or failure domain at all — the case where
Phases A–D's in-process isolation, however correct, is still one crash or one resource
exhaustion event away from affecting a co-resident tenant — this phase adds a
`mae-daemon@.service` systemd **template** unit, directly mirroring the already-shipped,
already-proven `assets/mae-headless@.service` pattern cited in Context. Each template
instantiation gets its own process, its own PID, its own `daemon.toml`, its own data
directory (parameterized the same way `mae-headless@.service` parameterizes
`WorkingDirectory=%i` off the systemd-escaped instance name), and its own systemd
lifecycle (`systemctl --user enable --now 'mae-daemon@'"$(systemd-escape
tenant-name)"'.service'`). Phases A–D remain the recommended default — they let multiple
*related* tenants (e.g. several teams inside the same trusted organization) share one
process efficiently, with real isolation guarantees enforced in software. Phase E is the
escape hatch for when process-level separation is the actual requirement: a tenant whose
operator wants "if this tenant's daemon crashes or gets OOM-killed, it must not take any
other tenant down with it" gets that guarantee for free from the OS process boundary,
exactly as `mae-headless@.service` already gives per-project process isolation for the
headless engine today.

State explicitly, because it bounds this phase and closes off a question that might
otherwise be read as an open gap: **this phase is Linux-only, per the project's Gate W
cross-platform scoping.** `mae-daemon` is confirmed never expected to run on macOS or
Windows as a deployed service — CLAUDE.md principle #13's cross-platform-parity
requirement governs the *editor* (`mae`, run on both macOS and Linux by the same
developers on the same day) and explicitly does not extend to a systemd-templated
background service with no macOS (`launchd`) or Windows (Service Control Manager)
equivalent shipped or planned. systemd is therefore the complete, sufficient design for
this phase, not a partial solution awaiting a cross-platform follow-up.

### Phase F — re-benchmark with an explicit N-tenant dimension

Re-run ADR-054's own benchmark methodology (`daemon/benches/kb_dispatch_concurrency.rs`'s
criterion harness against the real `mae-daemon` binary) with an explicit tenant-count
dimension added — not just "N concurrent sessions against one store," but "N tenants, each
with M concurrent sessions against their own store(s), running simultaneously" — and
publish the resulting capacity ceiling as the daemon's documented multi-tenant claim,
distinct from and cross-referenced against ADR-054's existing single-tenant number rather
than silently replacing it (a single-tenant number and a multi-tenant number answer
different questions and both remain useful). Per gopls's own documented caveat cited in
Context, the published claim must not overstate resource *savings* attributable to
multi-tenancy — real, independent-KB tenants with no data overlap should not be promised
savings the architecture cannot actually deliver for that workload shape; any savings
claim must be scoped to the cache-overlap case where it is actually true.

### Phase G — document `daemon_mode` and multi-tenant deployment in the pairing doc

`docs/EXTERNAL_EDITOR_MCP_PAIRING.md` currently contains zero mentions of `daemon_mode`
(verified directly against the file) despite `daemon_mode` (`off`/`on-demand`/`shared`,
ADR-035) being exactly the option an operator setting up a shared multi-tenant daemon for
several paired external editors needs to understand first. This phase adds that
documentation, including how multi-tenant deployment (Phases A–E) interacts with
`daemon_mode`'s existing three-way behavior set. This addition must be cross-linked with
ADR-057 item 3 so the two documentation efforts — this ADR's deployment-facing
documentation and ADR-057's own scope — do not diverge or silently duplicate coverage of
the same option over time.

This phase also closes an ambiguity this ADR would otherwise leave implicit: **document
the config-change contract explicitly**, per gopls's own "cannot be reconfigured live,
requires a restart" constraint cited in Context. State plainly, for each class of
multi-tenant-relevant config change (a new tenant registered, a quota adjusted, a
per-tenant limit changed), whether `mae-daemon` applies it live or requires a restart to
take effect. Leaving this ambiguous is itself a failure mode — an operator who believes a
quota change took effect live when it actually didn't (or vice versa) is exactly the
untested middle ground Verification's Phase G test below is designed to falsify.

## Implementation note (added during Phase B implementation, principle #15)

Phase B's Decision text above, as originally written, described the mechanism as
**"replace the single `Arc<Mutex<DaemonState>>`... with a directory plus a genuinely
separate `Arc<Mutex<InstanceState>>` per tenant."** During implementation, that literal
mechanism was re-checked against the current codebase first — the same discipline
ADR-054's own Implementation Note applied when its originally-proposed "per-KB-instance
locking" turned out to be redundant with concurrency control Cozo already provides — and
found to already be substantially delivered by ADR-054's prior work, not something this
phase needed to build from scratch:

- ADR-054 generalized "snapshot-then-drop" (clone the needed `Arc` under
  `state.lock().await` in a tight scoped block, drop the lock, then run the actual
  synchronous CozoDB call inside `tokio::task::spawn_blocking`) to **every** read/hygiene
  arm in `daemon/src/handler.rs` — confirmed by direct reading of all 15 arms that touch
  `state.query_layer`/`state.store`, not assumed from the ADR's own prose. The daemon
  `Arc<Mutex<DaemonState>>` is therefore already held only for the O(microseconds) it
  takes to clone an `Arc`, never across an actual query, mutation, or blocking I/O call.
- `daemon/src/scheduler.rs`'s background `watcher_tick`/`maintenance_tick`/`health_tick`
  arms independently apply the identical snapshot-then-drop pattern (confirmed by direct
  reading) — background maintenance work was already not a lock-hold-duration risk either.
- `daemon/src/main.rs`'s `handle_client` already spawns one independent `tokio::task` per
  connection (`accept_loop`), and the blocking `read_message().await` that waits for a
  client's next message is never performed while holding `DaemonState`'s lock. A stalled,
  malformed, or disconnected client can only ever block or end *its own* connection task —
  structurally, not merely empirically — which is exactly the Emacs bug#11639/bug#23499
  shape this ADR's Context cites as the risk to close.

**Resolved mechanism:** rather than build a new directory + per-tenant lock structure that
would duplicate synchronization ADR-054 already put in place, Phase B is scoped to what was
actually missing: the **N-way concurrency-isolation test** this ADR's own Verification
section calls "the primary concurrency-isolation test... alongside Phase D's IDOR case" —
deliberately deferred out of Phase A (see that phase's own commit message) specifically
because it would have been meaningless to write before Phase B's mechanism existed. Written
and run against the current (post-Phase-A) code *before* any rewrite was attempted, per this
same principle-#15 discipline:

- `handler::tests::concurrent_slow_tenant_a_query_does_not_measurably_degrade_b_or_c_reads`
  (`daemon/src/handler.rs`) — a ≥3-tenant fixture (principle #14) where tenant A's real,
  measurably slow bulk query (5 sequential full-text scans over 500 real, varied-content
  nodes — empirically ~150-300ms in a debug build, no artificial sleep standing in for real
  work) runs concurrently with tenant B's and tenant C's single-node reads. Measured result:
  B's concurrent-with-A latency was statistically indistinguishable from its solo baseline
  (e.g. 3.4ms concurrent vs. 3.6ms solo in one representative run) — not the ~150-300ms a
  genuinely serialized implementation would show. The test asserts a generous-but-meaningful
  bound (10x the solo baseline, floored at 50ms) to avoid CI timing flakiness while still
  easily catching the ~40-90x gap a real regression would produce.
- `tests::kb_socket_malformed_and_disconnect_tests::malformed_json_on_one_connection_does_not_starve_or_hang_another`
  and `tests::kb_socket_malformed_and_disconnect_tests::client_disconnect_mid_request_does_not_hang_other_tenants_rpcs`
  (`daemon/src/tests/`, real `UnixListener`/`UnixStream` sockets via the existing
  `spawn_kb_socket` harness ADR-054 built) — the other two named adversarial cases from this
  ADR's Verification section, both passing against the current architecture: a connection
  sent deliberately malformed JSON, and a connection that sends a partial message then
  disconnects mid-request, each verified to have zero effect (neither elevated latency nor a
  hang) on a separate, concurrently-issued, unrelated tenant's request — including with a
  third, currently-stalled (connected but silent) peer also present.

**What this means for Phase B's status:** the concurrency-isolation *property* this ADR
exists to establish is verified, today, against the real architecture — not asserted from
the Decision text's own prose. No new lock-splitting code was written, because none was
load-bearing for the property under test; writing one anyway would have been exactly the
"ad-hoc solution... duplicated logic" principle #8 warns against; a directory structure and
per-tenant lock sitting *beside* concurrency control that already provides the same
guarantee. Phase B's issue is closed on this basis, cross-linked to this note. This does
**not** retroactively validate Phases C/D's own mechanisms (per-tenant quotas' actual
accounting structure, the IDOR resolution-time check) — each remains its own phase, to be
verified independently and on its own adversarial terms when implemented, exactly as
Phase A's own scope boundary was respected here.

## Implementation note (added during Phase D implementation, principle #15)

Phase D's own Decision text named three adversarial cases: the IDOR-shaped resolution-time
check, cross-KB role composition, and a forged/rotated-key signature regression check. Each
was written and run against the current architecture *before* assuming new enforcement code
was needed — the same discipline that resolved Phase B — and each resolved differently, for
a reason worth stating precisely rather than folding into one blanket "already fine":

- **The IDOR case is a genuine property of Phase A's own addressing mechanism, not a
  separate check layered on top.** `daemon/src/handler.rs::dispatch`'s
  `snapshot_query_layer`/`snapshot_store` (Phase A) resolve an `instance` address to a
  specific `Arc<CozoKbStore>` *before* any inner ID in the request is ever looked at — every
  subsequent `get`/`links_from`/`links_to`/etc. call is backed by exactly that one store's
  own SQLite-backed relations, with no shared ID space and no cross-instance join across
  stores anywhere in the call path. There is no separate "check if this ID belongs to me"
  step for an attacker to bypass, because the store handle itself *is* the scope boundary —
  structurally, not by a resolution-time check that could be forgotten on a new arm.
  `handler::tests::idor_a_valid_instance_address_never_resolves_a_different_tenants_id`
  proves this with a real cross-instance ID collision (a node ID inserted directly into
  tenant B's store, requested via a validly-addressed tenant A request): the result is
  `Null`, not tenant B's real content, across `kb/get`/`kb/links_from`/`kb/links_to`, with a
  third uninvolved tenant C included per principle #14's N-way requirement.
- **The role-composition case tests a code path Phase A/B never touched at all.** Per-KB
  Owner/Editor/Viewer roles (ADR-017/018) are derived per-`kbc:{kb_id}` collection in
  `daemon/src/collab_handler/mod.rs`'s `kb_access` — an entirely separate daemon listener
  (the mTLS collab/sync path) from `daemon/src/handler.rs`'s KB Unix-socket dispatch Phase
  A/B's addressing work modified. **Correction to this ADR's own framing**: the KB
  Unix-socket path Phase A/B built on has no principal/role concept on it at all by design
  (`daemon/src/config.rs`'s own comments: "there is no principal or IP on a Unix domain
  socket" — filesystem-permission trust only) — so the role-composition property Phase D
  names doesn't interact with ADR-060's own changes, it's a pre-existing guarantee of
  per-collection membership derivation that predates this ADR. No existing test explicitly
  proved a single principal holding *different* roles on *two different* KBs simultaneously
  doesn't leak the stronger role across the boundary, so
  `collab_handler::tests::collab_handler_cross_kb_role_isolation_tests::
  owner_of_one_kb_is_not_owner_of_another_kb_where_only_viewer` closes that specific gap —
  bob, genuinely Owner of his own KB, attempts an Owner-only `kb/add_member` on a second KB
  where he's only a Viewer; denied, and the reverse (alice, Owner of her own KB, has zero
  access on bob's) also verified.
- **The forged/rotated-key signature case was already covered, pre-existing, unrelated to
  this ADR.** `shared/sync/src/membership.rs`'s
  `tampering_any_field_breaks_the_signature` (plus `forged_rebind_wrong_signer_is_rejected`
  and several `daemon/src/collab_handler/tests/*.rs` forged/tampered-signature cases)
  already prove this — Phase A/B's addressing changes never touched signature verification
  or the signed op-log at all, so there was nothing to regression-test here beyond
  confirming (by direct reading, not by writing a duplicate test) that this coverage
  genuinely exists and genuinely still applies.

**Net effect:** Phase D closes with one genuinely new test (cross-KB role isolation), one
test proving a structural property of Phase A's own mechanism rather than a bolted-on check
(the IDOR case), and one citation of pre-existing, unrelated coverage (the signature case) —
not a uniform "nothing to do here," and not a rewrite either. Issue #412 is closed on this
basis, cross-linked to this note.

## Implementation note (added during Phase C implementation, principle #15)

The corrected design above (two-key `TenantRegistry`, cost-weighted points budget,
`dashmap`-backed state, eviction reusing `run_maintenance_tick`) shipped largely as designed,
with one deliberate, stated scoping decision made during implementation rather than design —
recorded here per the same "bounded down payment, not a silent gap" discipline that closed
Phase B/D.

**What shipped, KB-socket side (the full mechanism, wired end-to-end):** `daemon/src/tenant.rs`
(`RequestCost`, `TenantQuotaState`, `TenantRegistry`), `[[tenant]]`/`[tenant.quota]` in
`daemon/src/config.rs` (validated by `DaemonConfig::check_tenants` — duplicate names, an
instance/principal double-claimed by two tenants — called both from `--check-config` and
unconditionally at daemon startup), and enforcement plugged into
`daemon/src/handler.rs::snapshot_query_layer`/`snapshot_store` — the two functions every one of
`dispatch`'s 15 KB-query/hygiene arms already funnels through. **One simplification from the
original design, made with evidence, not assumption:** the Decision text above called for
enforcement to run *before* `DaemonState`'s lock is acquired at all. Implementation found this
unnecessary — Phase B already proved (real 3-tenant concurrent-load test) that this lock's
critical section is so brief it produces no measurable cross-tenant latency delta even under
genuine contention, so checking the tenant budget *inside* that same already-proven-cheap
critical section (immediately after acquiring the lock, before any store-resolution work) meets
every isolation/fairness property the original design wanted, without a second dispatch-signature
parameter threaded through all 15 call sites *and* all 44 existing test call sites that construct
raw `dispatch()` calls. `TenantRegistry` itself lives outside the lock (an `Arc<TenantRegistry>`
field on `DaemonState`, cloned out cheaply under the same brief lock — the identical pattern
`store`/`query_layer`/`doc_store` already use), so Phase B's own "no per-request-contended state
inside this lock" finding is still honored; only the *check* moved, not the *state*. The
concurrent-request cap (`ConnLimiter`, tenant-scoped) is bound to the lifetime of one
dispatch-arm's `(ql, _conn_guard)`/`(store, _conn_guard)` binding — a genuinely different
resource dimension from the points budget, verified independently
(`tenant::tests::connection_cap_and_points_budget_are_independently_enforced`). The manual
`daemon/evict_tenant` RPC and the idle-tenant sweep in `run_maintenance_tick` both shipped as
designed. Six adversarial tests in `tenant.rs` (3+-tenant isolation, instance- vs. principal-key
non-crossover, self-healing eviction that never disturbs a co-resident tenant, cost-weighting,
independent connection-cap enforcement, zero-config zero-behavior-change) plus three end-to-end
tests in `handler.rs` that exercise the real `dispatch()` path (not just `TenantRegistry` in
isolation) plus six `config.rs` tests (including a round-trip of the exact `[[tenant]]`/
`[tenant.quota]` TOML shape this ADR's Decision section documents) all pass; manually verified
end-to-end via `mae-daemon --check-config` against both a valid and a deliberately-conflicting
real `daemon.toml`.

**What did not ship this pass, stated explicitly rather than left as a silent gap:**
collab/OAuth-side principal-keyed enforcement. `TenantRegistry::check_and_charge_by_principal`
and the `principals` half of `[[tenant]]` config are fully implemented and tested
(`tenant::tests::principal_keyed_isolation_never_crosses_into_instance_keyed_tenants` proves the
two key spaces never cross-contaminate even given a colliding literal string) — but not yet
called from `daemon/src/collab_handler/mod.rs::handle_doc_request_inner`. Reason: that surface's
methods (`sync/*`, `kb/share`, `kb/node_update`, `kb/collection_op`, membership ops) don't map
onto the KB-socket's `kb/get`-shaped cost table the ADR names concretely — assigning them
plausible-looking costs without the same reasoned pass this note gives the KB-socket table would
be exactly the "unicorn"-shaped scope creep principle #14 warns against, not a faithful
implementation of this phase's own Decision section. Tracked as **issue #456** rather than left
implicit.

## Consequences

**Positive.** Closes a documented, recurring bug class inherited directly from Emacs's own
daemon lineage (bug#11639/#23499's single-client-hangs-everyone shape) before it has a
chance to reproduce in `mae-daemon`, rather than discovering it after a real multi-tenant
deployment hits it in production. Gives operators a real choice along a genuine spectrum —
shared-process efficiency (Phases A–D) versus process-level isolation (Phase E) — matched
to their actual trust and blast-radius requirements, instead of forcing every deployment
into one-process-per-tenant (wasteful for closely-related teams) or one-shared-process-with-
no-isolation (unsafe once tenants are genuinely independent). Reuses 100% of existing
identity (ADR-017's Ed25519 principals), authorization (ADR-018's roles/policy), and
concurrency-hardening (ADR-054's per-principal throttle, connection-cap pattern)
infrastructure — no parallel tenant-identity system, no parallel authorization model, no
parallel quota mechanism invented from scratch. Extends a systemd deployment pattern
(`mae-headless@.service`) already shipped and already proven in this exact codebase to a
second service, rather than inventing a new deployment shape.

**Costs (honest).** Phase B's lock split is a non-trivial refactor of `handler.rs`'s
dispatch structure and `DaemonState`'s ownership shape — larger in scope than ADR-054's
own snapshot-then-drop change, because it must introduce a genuinely new per-tenant
locking unit rather than restructure locking within a single existing state struct;
regressions here risk affecting every existing single-tenant deployment (the majority of
deployments today), not just new multi-tenant adopters, so this phase carries the same
elevated-scrutiny weight ADR-054 assigned its own lock work. Phase D's resolution-time
authorization check adds a mandatory check at every place a daemon RPC resolves a
request-supplied identifier against tenant-scoped data — every current and future RPC
handler that resolves such an identifier must remember to add this check, which is
friction future contributors must carry, the same way ADR-056's fail-closed
category-classification discipline is friction its own contributors must carry. Phase E's
process-per-tenant option, while real isolation, gives up Phase A–D's cache-and-resource
sharing entirely for any tenant deployed that way — an operator choosing Phase E for every
tenant gets none of the efficiency this ADR's shared-process phases were built to provide,
which is the expected, disclosed trade-off, not a hidden cost, but worth stating plainly
so it is chosen deliberately rather than defaulted into. Phase F's benchmark work is
genuinely harder to make reproducible than ADR-054's single-tenant version — an N-tenant,
M-session-per-tenant load test has a much larger parameter space than a flat N-session
test, and picking a small, defensible, representative slice of that space (rather than
attempting exhaustive coverage) is left to implementation.

## Alternatives rejected

- **Container- or cgroup-per-tenant sandboxing.** Rejected as foreign to MAE's deployment
  model (a plain binary plus systemd units, not a container-orchestrated service) and as
  solving a threat model — adversarial co-tenants who might attempt to break out of a
  shared kernel namespace — that this ADR's trusted-org-scale scoping call explicitly
  excludes. Phase E's systemd-template process isolation already gives real,
  OS-process-level blast-radius containment (a crash or OOM in one process cannot touch
  another process's memory) without the operational weight of a container runtime, which
  is the right level of isolation for the actual threat model this ADR targets: an
  operator's own misbehaving or resource-heavy tenant, not a hostile co-tenant actively
  trying to escape a sandbox.
- **In-process-only multi-tenancy with no process-level option at all (Phases A–D, no
  Phase E).** Rejected as insufficient on its own — even a perfectly correct in-process
  lock split and quota system still leaves every tenant sharing one crash domain and one
  address space. A daemon process that segfaults, panics, or gets OOM-killed takes every
  co-resident tenant down with it regardless of how well-isolated their locks and quotas
  were logically. "A dedicated server for many independent users," this ADR's own framing
  from Context, implies at least the *option* of real process separation for tenants that
  need it; shipping only the in-process mechanism would leave that option missing
  entirely rather than available and simply not the default.
- **A hosted, adversarial-tenant cloud multi-tenancy model** (per-tenant billing/metering,
  a provisioning control plane, container-per-tenant as the *default* rather than an
  opt-in escape hatch). Rejected per this ADR's own scoping call in Context — MAE's stated
  vision is a dedicated server or the client machine for a trusted operator, not a
  multi-region hosted SaaS product with mutually adversarial paying tenants. Building for
  that bar here would add substantial, unused complexity (billing hooks, a provisioning
  API, mandatory container isolation for every tenant regardless of trust level) in
  service of a threat model this project has not adopted and gives no present indication
  of adopting.

## Verification

Per CLAUDE.md principle #14, these tests are N-way (≥3 tenants, not 2) wherever tenant
isolation is the property under test — a 2-tenant test cannot distinguish "isolated" from
"coincidentally didn't collide this run," while a ≥3-tenant test with asymmetric load
profiles makes cross-tenant interference structurally visible. They use real, varied
tenant/principal identities freshly generated per test run, not fixed unicorn values
chosen to dodge an edge case.

**Phases A/B — the primary concurrency-isolation tests, and the highest-priority tests in
this ADR alongside Phase D's IDOR case:**

- A **≥3-tenant** stress test: tenant A runs a slow bulk query (deliberately long-running,
  e.g. a full-KB scan) concurrently with tenant B's and tenant C's latency-sensitive
  single-node reads. B's and C's measured p99 latency must **not** measurably degrade
  versus their own single-tenant baseline. This is the test that falsifies "a per-tenant
  API sitting on top of one shared lock underneath" — a superficial Phase A/B
  implementation that only changed the RPC envelope but left one directory-lock-held-too-
  long or one accidentally-shared `Mutex` in the critical path would show B/C's latency
  rising in lockstep with A's slow query, and this test is built specifically to catch
  that.
- A malformed or oversized RPC addressed at tenant A's instance must not starve or crash
  tenant B's connection or session state.
- **Named reproduction of the Emacs bug#11639/bug#23499 shape**, as a specific test, not a
  general property: a client that disconnects mid-request, or sends a malformed follow-up
  message after a valid handshake, must not hang the shared directory lock (Phase B) for
  other tenants' unrelated RPCs. This test exists because the precedent it reproduces is
  real and documented, not hypothetical.

**Phase C** (updated during design to match the corrected two-key mechanism above, then
updated again after implementation — see this section's status markers and the Phase C
Implementation Note above for what shipped and what's tracked as issue #456):

- **[shipped]** A quota-exceeding **tenant** (not "principal" — see the two-key correction
  above) must be rejected before consuming another tenant's capacity, tested against a
  **≥3-tenant** baseline specifically — a single-tenant quota test proves the mechanism
  exists but proves nothing about tenant isolation, which is the property actually at stake
  here. (`tenant::tests::quota_exceeding_tenant_rejected_without_touching_other_tenants_budget`;
  end-to-end through real `dispatch()` in
  `handler::tests::dispatch_rejects_a_quota_exceeding_tenant_with_a_real_daemon_error`.)
- **[shipped]** **KB-socket instance-keyed isolation**, its own distinct test from the
  principal-keyed case below: two connections both addressed at the same tenant's instance
  must share that tenant's budget; a third connection addressed at a different tenant's
  instance must be completely unaffected. This specifically falsifies "accidentally scoped
  the quota per-connection instead of per-instance-address" — a plausible implementation bug
  given `ConnLimiter`'s existing per-connection shape is the nearest copy-pasteable
  precedent, and the wrong one to copy here.
  (`tenant::tests::kb_socket_isolation_is_keyed_on_instance_not_per_call`.)
- **[implemented + tested in isolation; not yet wired to a listener — issue #456]** The
  mirror case for the collab/OAuth listeners' principal-keyed isolation: a quota-exceeding
  principal on one tenant's KB must not be rejected differently than intended when a
  *different* principal, member of a *different* tenant, is concurrently well within budget.
  `tenant::tests::principal_keyed_isolation_never_crosses_into_instance_keyed_tenants` proves
  the mechanism itself (including that the two key spaces never cross-contaminate on a
  colliding literal string); an end-to-end test through the real collab dispatch path is
  part of issue #456's scope, not written yet since there is no call site to exercise.
- **[shipped, simplified from the original design with evidence — see Implementation Note]**
  A synthetic unbounded-growth harness, modeling the rust-analyzer / Emacs-daemon
  memory-growth precedent cited in Context (a tenant whose in-process state grows
  pathologically over a long-running session while staying within its per-request quota at
  every individual step), must be individually resettable via Phase C's per-tenant
  restart/eviction mechanism, with **zero observable impact on co-resident tenants'** live
  sessions during and after the reset. Implemented as
  `tenant::tests::evicting_a_tenant_self_heals_and_never_disturbs_co_resident_tenants`: a
  direct state-level proof (evict tenant A, confirm tenant B's independent spend is
  untouched, confirm tenant A self-heals with a fresh budget on its next request) rather than
  Phase B's own latency-based statistical-bound technique — the two mechanisms differ in kind
  (Phase B: does contention leak across a lock; Phase C: does eviction leak across
  independent `dashmap` entries), so a direct state assertion is the more precise proof for
  this specific property, not a weaker substitute for the timing-based one. The evicted
  tenant's own next request succeeds with *correct* (fresh, zeroed) state after self-healing
  — not merely avoiding a crash. The whole daemon process never needs restarting just to
  reclaim one tenant's leaked state — that is the specific claim this test proves, not merely
  "restart works."
- **[shipped]** A cost-weighting negative case: a tenant issuing only cheap reads
  (`kb/get`-shaped, cost 1) must not exhaust its budget anywhere near the rate of a tenant
  issuing the same *count* of mutating hygiene-arm requests (cost 5) — proving the accounting
  genuinely applies the cost table rather than silently flattening to a per-request counter
  during implementation.
  (`tenant::tests::cost_weighting_makes_mutation_heavy_tenant_exhaust_far_faster_than_read_only`;
  end-to-end in `handler::tests::dispatch_cost_weights_scan_higher_than_read_through_the_real_path`.)
- **[shipped, not in the original bullet list — the connection-cap dimension needed its own
  negative case]** The concurrent-request cap and the points budget are independently
  enforced: a tenant with an exhausted connection cap but an untouched points budget must
  still be rejected, proving this isn't secretly the same mechanism as the points budget
  wearing a different name.
  (`tenant::tests::connection_cap_and_points_budget_are_independently_enforced`.)
- **[not done, genuinely deferred — tracked in Phase G, not fabricated as "out of scope"]**
  A config-reload/restart contract check: registering a new `[[tenant]]` after the daemon is
  already running either takes effect live (proven positively, not assumed) or the daemon
  surfaces an explicit, observable "restart required" signal. Confirmed during Phase C
  implementation: this daemon has **no live-reload mechanism for any `daemon.toml` section**,
  `[[tenant]]` included — `DaemonConfig::load()` runs once at startup
  (`main.rs`) and nothing watches the file afterward, so today a `[[tenant]]` edit is
  silently inert until a manual restart, with no observable signal either way. That silent
  gap is real, not merely untested — Phase C did not build a reload mechanism (out of scope
  for a quotas phase), but the test this bullet calls for still needs writing once Phase G
  documents the actual (currently silent) contract, so a reader doesn't have to discover it
  by editing the file and noticing nothing happened.

**Phase D — the named IDOR-shaped test, called out explicitly as the single highest-
priority adversarial test in this entire ADR, per the Gitea/Vaultwarden CVE precedent in
Context:**

- A principal correctly, validly addressed at tenant A's own instance (Phase A's outer
  address checks out; the principal is genuinely a member of tenant A) whose request
  payload separately references a raw node ID that actually belongs to tenant B's data
  must be **rejected at ID-resolution time** — not served just because the outer RPC
  address was correct. This must be tested as its own distinct case from "wrong instance
  addressed" (which Phase A already prevents structurally), because it is a different bug
  shape that survives exactly the kind of check Phase A alone provides.
- A principal that is Owner on tenant A's KB1 and merely a Guest/Viewer on tenant B's KB2
  must be rejected when attempting a mutating action on KB2, regardless of tenant B's
  quota headroom — proving quota availability is never mistaken for authorization.
- A forged or rotated-key signature must be rejected identically to this daemon's
  pre-multi-tenancy behavior — a direct regression check that Phase A's addressing
  plumbing did not accidentally create a bypass path around ADR-017's existing signature
  verification.

**Phase E:**

- `kill -9` tenant A's isolated process (started via the `mae-daemon@.service` template
  instantiation). Tenant B's separately-instantiated, separately-running process must show
  **zero observable impact** — no dropped connections, no elevated latency, no shared
  state corruption. This proves genuine OS-level process isolation, not merely "logically
  separated within one process," which Phases A–D alone cannot prove regardless of how
  correct their in-process locking is.

**Phase F:**

- ADR-054's original single-tenant benchmark (`daemon/benches/kb_dispatch_concurrency.rs`,
  the existing "~8 concurrent MCP sessions" measurement) must be re-run and reproduce as a
  **regression baseline before** the new N-tenant numbers are published alongside it — this
  proves the multi-tenancy work in Phases A–D did not regress the existing single-tenant
  case it extends.
- The published multi-tenant capacity claim must be checked against gopls's own documented
  caveat: it must not claim resource *savings* from multi-tenancy beyond what genuine
  cache overlap between tenants' workloads can actually deliver. A claim written for
  independent-KB tenants with no data overlap must not borrow numbers measured under a
  cache-overlap-favorable test setup.

**Phase G:**

- The daemon's config-change contract must be tested, not merely documented: for a
  representative config change relevant to multi-tenancy (e.g. registering a new tenant,
  adjusting an existing tenant's quota), either (a) the change is shown to apply live via a
  test that connects before the change, makes the change, and observes the new behavior
  take effect without a restart — a genuine positive proof, not an assumption — or (b) the
  test confirms the daemon surfaces an explicit signal (a log line, a rejected
  live-reconfiguration attempt with a clear error, documented behavior) that a restart is
  required, per gopls's own "cannot be reconfigured live" precedent. The specific failure
  mode this test is built to falsify is the untested middle ground: a config change that
  silently fails to take effect with no error surfaced anywhere, leaving an operator
  believing a quota or tenant registration is active when it is not.
