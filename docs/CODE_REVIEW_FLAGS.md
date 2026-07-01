# Code-review flags — collab / E2E / identity / mesh

Surfaced findings from the pre-dogfood deep dive (issue #246), tagged in-code with a greppable
convention so nothing is buried. **These are surfaced for review, not necessarily release-blocking.**

## Convention

`// BUG:` wrong behavior · `// KLUDGE:` works but ad-hoc · `// PERF:` scale hot-spot ·
`// DOGFOOD:` verify/measure under real data · `// FIXME:` known-incomplete — each with a one-line
why + an issue ref. Grep them: `rg '// (BUG|KLUDGE|PERF|DOGFOOD|FIXME)\(' shared crates daemon`.

## Index (as of Workstream A)

| Tag | Location | Summary | Disposition |
|---|---|---|---|
| PERF | `daemon/src/collab_handler.rs` (`kb_access`) | Full op-log decode + governance + membership derive on **every** anchored/E2E access | Workstream B / ADR-042 (#247) — derive cache |
| PERF | `shared/sync/src/membership.rs` (`causal_order`) | O(n²) — one pass per generation × full scan; near-linear op-chain ⇒ depth≈n | Workstream B (#247) — Kahn's O(n log n), same emit order |
| PERF | `shared/sync/src/membership.rs` (`owner_principal_chain`) | O(passes × n) fixpoint; now bounded + termination-tested | Flagged; cost folded into the derive cache (#247) |
| PERF/DOGFOOD | `shared/sync/src/kb.rs` (`append_signed_op`) | Op-log is append-only, never pruned — grows for the KB's lifetime | v0.16 compaction; dogfood measures the wall (ADR-042) |
| KLUDGE | `shared/sync/src/membership.rs` (`fingerprint_of`) | Fingerprint format is **unversioned** — an encoding change silently invalidates all ops | Note; a version tag needs a coordinated migration (#246) |
| KLUDGE | `daemon/src/collab_handler.rs` (`persist_and_broadcast_collection`) | Persist-then-broadcast not atomic; membership propagation eventually consistent | Inherent to CRDT; security rests on convergence + local blocklist (ADR-039) |
| FIXME | `shared/sync/src/membership.rs` (`find_wrapped_content_key`) | Join-after-removal can't open pre-rotation ops (no key history) | v0.16 key-history/rewrap (ADR-037 §D4, #237); documented in E2E_USER_GUIDE §7 |

## Fixed in Workstream A (not just flagged)

- **Stale docstring reconciled** — `plan_reactive_member_rewraps` (`crates/mae/src/collab_bridge.rs`)
  claimed "genesis owner only / rotated owner is a deferred edge," but #239 fixed exactly that via
  `is_owner_principal`. Docstring now matches shipped behavior.
- **Bounded fixpoint** — `owner_principal_chain` gained an explicit `max_passes` ceiling (it already
  terminated by set-growth; the bound is defensive) + `is_owner_principal_terminates_on_a_cyclic_rebind_set`.

## Deferred → v0.16 (tracked issues, not silent)

Op-log pruning/compaction; `owner_principal_chain` predecessor-retirement (governance tightening);
join-after-removal key history (#237); at-rest key encryption (I3). See the umbrella #251.
