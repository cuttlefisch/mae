# ADR-054: Daemon concurrency hardening & benchmarked capacity figure

**Status:** Accepted (implemented — see "Verification" for status per bullet).
**Extends:** ADR-035 (editor↔daemon boundary), ADR-019/ADR-020/ADR-022 (durable/replicated/
crash-safe sync — this ADR generalizes the per-document locking pattern already proven
there onto a second code path that lacks it).
**Supersedes-with-evidence:** `docs/adr/004-kb-scaling.md`'s "Tier 1: Single-Machine...
< 20K nodes, 5-10 concurrent editors — IMPLEMENTED" claim, for the specific path shown
below to be unbenchmarked and architecturally bottlenecked. Per CLAUDE.md principle #15
(bugs are drift signals), this ADR closes that drift with measured evidence rather than
inventing new scope.
**Tracking:** issue #375 (epic tracker); phase issue #379.

## Context

Many concurrent users across a mix of editor frontends (MAE, VS Code+Copilot, other
MCP-capable editors) hitting the same daemon is the near-term expected case this whole
initiative exists to support — so the daemon's actual concurrency behavior under real
parallel load needed to be verified, not assumed. Direct code research found the daemon is
architecturally **two very different concurrency models bolted together**:

**The collab/CRDT TCP path is mature:** `DocStore` (`daemon/src/doc_store.rs`) uses
`RwLock<HashMap<String, Arc<Mutex<DocEntry>>>>` — per-document locking, WAL-first, a
4-shard `SqlitePool`. Two clients editing *different* documents never contend on each
other's lock. This is proven correct by a real ≥3-principal convergence test
(`collab_handler_n_way_convergence_tests.rs`). The TCP listener also has a hardened
connection cap (`collab.max_connections`, default 256, RAII-counted) and a 10s handshake
timeout (issue #342's fix).

**The local KB Unix-socket path — the one every routine `kb_search`/`kb_get`/etc. call
from every locally-connected frontend actually uses — is dramatically less mature:**
- `DaemonState` is wrapped once in `Arc<Mutex<DaemonState>>` (`daemon/src/main.rs:33,148`),
  and essentially every `handler::dispatch` arm (`daemon/src/handler.rs:88-346`) takes
  `state.lock().await` and runs its query **while holding that single global lock** — with
  **no** per-KB, per-node, or per-store granularity anywhere on this path.
- The accept loop (`daemon/src/main.rs:235-240`) has **no connection cap, no per-client
  limit, and no handshake/idle timeout** — unlike the TCP collab listener.
- **No backpressure/push mechanism** exists on this path at all — `EventBroadcaster`'s
  bounded-queue/drop/write-timeout model (`shared/mcp/src/broadcast.rs`) is used only by
  the TCP collab and P2P mesh paths.
- The **P2P mesh accept loop** (`daemon/src/p2p.rs`) also has no connection-count cap,
  unlike the hardened TCP listener.
- The `SqlitePool`'s shard count (4) is hardcoded, not exposed via `daemon.toml`.
- `docs/save_intent`/`docs/save_committed` is an advisory two-RPC protocol with a real
  TOCTOU window between two frontends' concurrent save attempts — doesn't corrupt CRDT
  content, but "who actually gets to write the file to disk" isn't daemon-enforced across
  that gap.

**No real load/concurrency test exists anywhere in the repo today.** The only ≥3-writer
convergence test dispatches sequentially in-process (not real racing parallel
connections). The closest real-network stress test (`daemon/tests/collab_stress.rs`) tops
out around 6 distinct client identities, is mostly sequential even then, and is opt-in
only (`MAE_STRESS_TEST=1`, `#[ignore]`, not run in default CI). ADR-004's only concrete
capacity figure for the current architecture — "5-10 concurrent editors" — is not backed
by any benchmark against today's implementation, and is directly in tension with this
initiative's "many concurrent users very shortly" requirement.

## Decision

1. **Replace the single global `Arc<Mutex<DaemonState>>`** serializing every `kb/*` RPC
   with **per-KB-instance** locking. Recommended granularity is a KB, not an individual
   node — this matches the existing federation model (`shared/kb/src/federation.rs:34-49`,
   `KbInstance`/`KbRegistry`) more directly than per-document CRDT granularity, which is a
   separate, already-solved problem inside `DocStore`. Concurrent reads against *different*
   KBs must not serialize behind each other; concurrent reads must not serialize behind
   writes to unrelated stores.
2. **Add connection hardening to the KB Unix-socket accept loop**: a config-driven
   `max_connections` cap (RAII-counted, mirroring the collab listener's `#342` fix), a
   handshake/idle timeout, and per-principal/per-IP soft sub-limits.
3. **Add the same connection-cap treatment to the P2P mesh accept loop** (currently
   unbounded).
4. **Make the SQLite shard count configurable** via `daemon.toml` rather than hardcoded.
5. **Build genuinely parallel multi-connection load tests** — N≥3 real, simultaneously
   racing socket connections issuing overlapping reads/writes, run in **default CI**, not
   opt-in — producing an actual measured concurrent-session ceiling.
6. **Publish a benchmarked capacity number** replacing ADR-004's unverified "5-10
   concurrent editors," restated in "concurrent MCP sessions" terms (a superset of "editor
   frontends" once VS Code/other-editor clients are counted, matching this initiative's
   actual unit of scale).

## Consequences

**Positive.** Removes the single biggest identified scaling bottleneck before it's exposed
to real multi-frontend load, rather than discovering it in production. Gives the project a
real, defensible capacity claim instead of an unverified 2-year-old estimate. Closes an
asymmetry where the network-facing path (TCP collab) is well-hardened but the
locally-facing path (KB socket) — soon to be hit by every paired external editor — is not.

**Costs (honest).** Per-KB-instance locking is a non-trivial refactor of `handler.rs`'s
dispatch structure and every call site that currently assumes a single coarse lock;
regressions here would affect every existing MAE user, not just this initiative's new
clients (gate G2 applies with extra weight here). A multi-hour or genuinely
high-concurrency load test may not fit comfortably in a fast per-PR CI job — the default-CI
requirement may need a scheduled/nightly variant for the heaviest cases while a lighter
smoke version runs per-PR; this trade-off is left to implementation, not resolved by this
ADR.

## Alternatives rejected

- **Per-document (not per-KB) locking on the query path, mirroring `DocStore` exactly.**
  Rejected as the default — the query-layer path's natural unit is a KB/store, not an
  individual CRDT document; forcing per-document granularity here would be a larger,
  less-motivated refactor for the same practical benefit.
- **Leave ADR-004's capacity figure as-is and just fix the lock.** Rejected — principle #15
  applies: an unverified capacity claim sitting next to a known architectural bottleneck is
  exactly the kind of drift this project's own principles say must be closed with evidence,
  not left standing.
- **Add backpressure/bounded-queue semantics to the KB socket to match the collab path
  exactly.** Deferred, not rejected outright — the KB socket is a synchronous
  request/response protocol today (no push notifications), so the collab path's "drop an
  event, keep the connection" semantics don't directly translate; if/when push
  notifications are added to this path, revisit.

## Implementation note (added during Phase D implementation planning, principle #15)

Decision point 1 above, as originally written, described the mechanism as **"per-KB-instance
locking."** During implementation planning that literal mechanism was verified against Cozo's own
source and found to be the wrong shape — not because the goal was wrong, but because a new
application-level lock would be redundant with concurrency control Cozo already provides:

- Cozo's `Db<S>` (`cozo-0.7.6/src/runtime/db.rs:97-109`) already carries its own fine-grained
  internal concurrency control: `relation_locks: Arc<ShardedLock<BTreeMap<..., Arc<ShardedLock<()>>>>>`
  (per-*relation*, finer than per-KB) plus `running_queries: Arc<Mutex<...>>`.
- `CozoKbStore`'s own write path, `run_mut_params` (`shared/kb/src/cozo_store/db.rs:184-193`), takes
  `&self`, not `&mut self` — the type is already `Send + Sync` with interior mutability, relying
  entirely on Cozo's own locking, never on external synchronization MAE provides.
- None of the 12 fully-locked read arms (`kb/get` through `kb/hygiene_dismiss`) take an
  instance-scoping parameter on the RPC surface at all — `state.query_layer` is a single
  `FederatedQuery` that fans out across every registered instance inside one synchronous call
  (`shared/kb/src/query.rs`), so "per-KB-instance" doesn't correspond to an addressable unit here.

**Resolved mechanism:** generalize the **snapshot-then-drop** idiom already used by 3 of the 19
`handler.rs` arms (`kb/node_crdt`, `daemon/status`, `p2p/share_kb`) to the other 12 — clone the
needed `Arc` under `state.lock().await` in a tight scoped block, drop the lock, then run the actual
synchronous CozoDB call inside `tokio::task::spawn_blocking` (currently absent from this file
entirely). This adds **zero new locks**: the single `Arc<tokio::sync::Mutex<DaemonState>>` remains
the only lock in the picture, so a lock-ordering deadlock is structurally impossible (there's
nothing to order against). It fully satisfies this ADR's Verification bullets below via a smaller,
more precisely justified change than the original literal phrasing implied.

Cross-checked against existing precedent: MAE already has an adversarially-tested concurrent-write
path for Cozo's SQLite backend (`sqlite_multi_instance_concurrent_writes_converge`,
`crates/core/src/editor/kb_ops/tests/kb_ops_concurrency_tests.rs:76+`, backed by
`run_with_busy_retry`'s exponential-backoff-with-jitter, `shared/kb/src/cozo_store/db.rs:194-238` —
`MAX_ATTEMPTS` deliberately raised 100→400 after a real observed CI flake under contention). That
test covers a different-but-related axis (two separate `CozoKbStore` handles on the same file,
modeling cross-*process* contention); this ADR's own new adversarial test
(`kb_write_concurrency_tests.rs`) covers the complementary axis actually needed here: one shared
`Arc<CozoKbStore>` accessed by many `spawn_blocking` tasks within the *same* daemon process,
exercising Cozo's in-process `relation_locks` directly rather than the cross-connection retry path.

## Verification

- The new parallel-connection load test runs in default CI (not opt-in) and passes.
  Status: **done** — `daemon/src/tests/kb_socket_concurrency_tests.rs`'s
  `concurrent_reads_across_different_kbs_do_not_serialize` (N=8, a `SleepyQueryLayer`
  decorator makes contention measurable), runs in default `cargo test`.
- Concurrent reads against *different* KBs are empirically shown not to serialize behind
  each other (measured latency under concurrent load, not just asserted from the lock
  design). Status: **done**, same test — asserts total wall-clock stays under 3x a fixed
  per-call delay for 8 concurrent calls spanning two distinct stores (would show ~8x if
  still serialized behind `DaemonState`'s lock).
- A published, CI-verified capacity number replaces ADR-004's "5-10 concurrent editors"
  claim in that document. Status: **done** —
  `daemon/benches/kb_dispatch_concurrency.rs` (criterion, `cargo bench`, spawns the real
  `mae-daemon` binary against a 20,000-node store) measured **~8 concurrent MCP sessions**
  before p99 latency exceeds 2x the single-client baseline, with smooth (not cliff)
  degradation beyond that point — recorded in `docs/adr/004-kb-scaling.md`'s Tier 1
  section with the full per-level p50/p99 table. ("CI-verified" here means the
  concurrency *test* above (default CI) proves the mechanism is sound; the *bench* number
  itself is a manually-run, hardware-dependent snapshot, not re-verified per CI run —
  consistent with criterion benches generally not being CI-gated.)
- Connection-cap rejection and handshake-timeout tests exist for both the KB socket and
  the P2P listener, mirroring the existing `collab_handler_connection_limits_tests.rs`
  pattern already proven for the TCP listener. Status: **done** —
  `kb_socket_connection_limit_tests.rs` (cap rejection, idle-timeout self-heal, and the
  `idle_timeout=0`-disables-the-timeout case) and two new tests in `p2p.rs`'s existing
  `#[cfg(test)] mod tests` (`mesh_connection_cap_rejects_the_nplus1th_client`,
  `mesh_peer_that_never_opens_a_stream_is_dropped_within_the_handshake_timeout`).
