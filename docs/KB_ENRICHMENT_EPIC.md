# Epic: Enterprise-grade autonomous KB link enrichment

Execution tracker tying ADR-101 (structured CRDT edges), ADR-102 (KB engine), and ADR-103
(autonomous local-LLM link enrichment) into one dependency-ordered plan. Each milestone ships behind
an **adversarial test that fails first** (principle #14); a milestone is *not* done because a PR
merged, but because the failure mode it guards is provably closed.

## Why (one paragraph)
MAE's daemon enrichment sweep is built but computes **embeddings only** (ADR-061 §575: "no 'derive
relationships' AI logic exists anywhere yet"). The capability users actually need — a KB that stays
connected and healthy as humans and agents add material — requires a local model to *propose typed
links with calibrated confidence* and apply them safely. Two prerequisites block it: machine links
have **no durable home** in the CRDT (they're text substrings the projector wipes), and the hosted DB
engine is **bottlenecked** at ~8 concurrent sessions (ADR-054) against a target of ~50.

## Dependency order (do not reorder)
```
ADR-102 Track-1 (engine) ─┐   (can proceed in parallel; gates hosted deploy, not the feature logic)
ADR-101 (structured edges)├─▶ ADR-103 (enrichment) ─▶ hosted enterprise deploy
        └── prerequisite of ADR-103 (a machine edge has nowhere to live without it)
```
ADR-101 is a **hard** prerequisite of ADR-103. ADR-102 is a **deployment** prerequisite (the feature
can be built and tested on sqlite; it cannot be *hosted at scale* until the engine work lands).

## Milestones

### M0 — Phase-0 grounding (DONE this pass)
- Prior-art review → `docs/research/103-kb-link-enrichment-prior-art.md`. **Done.**
- DB benchmark protocol specified (ADR-102 Phase 0). **Benchmark run is M4.**

### M1 — ADR-101: structured CRDT edges (prerequisite)
Sub-issues = ADR-101 phases 0–5. Gate tests: a machine `accepted` edge survives a body edit +
reproject; N-way convergent edge upsert; no upcast-on-read; pending/rejected never projected live;
human-vs-machine collision → human wins. **Blocks M3.**

### M2 — PREREQ-C enabling fixes (ADR-103 Phase A)
Independent, small, parallelizable: federated embedding access (unblocks cross-KB discovery);
`min_set_tier` on `OptionDef` (agent-unsettable threshold); wire `with_format_constrained`
(two-phase decoding); surface confidence on `kb_links_from` read. Gate tests per fix.

### M3 — ADR-103 core loop (Phases B–E) — depends on M1, M2
Candidate selection (federation-aware) → embedding discovery → two-phase judgment + multi-signal
calibration → three-band gating → structured-edge write → review buffer (tri-surface, agent-safe
display). Gate tests: residency blocks a hosted provider before any read; agent cannot lower the
threshold; below-threshold edge is pending not live; false-positive candidate rejected; malformed
output quarantined; density cap holds; cross-KB routed stricter.

### M4 — ADR-102 engine (Track 1) + benchmark gate — parallelizable, gates hosted deploy
Backend-abstraction seam; cozo upgrade + RocksDB behind daemon flag; concurrency/read-path work; run
the M0 benchmark at 50 clients / 100K nodes. **Decision point:** pass → done; miss → open ADR-102
Track 2 (replace-Cozo evaluation). Gate tests: 50-way writers converge zero-loss; concurrent
first-create no panic; kill-9 recovery; cozo-upgrade regression guard; directory-store backup
round-trip; daemon-less sqlite no regression.

### M5 — ADR-103 autonomy + safety (Phase F) — depends on M3
24/7 daemon sweep + in-process twin; staleness re-validation; review-heavy initial mode; observability
+ acceptance-rate feedback (informs, never auto-tunes). Gate tests: N-way lease race converges;
staleness flips a stale edge off-graph; initial-mode keeps a 0.99 edge pending; feedback never moves
the agent-unsettable threshold.

### M6 — index hygiene + KB regeneration
CLAUDE.md ADR index (DONE for 101–103); regenerate the ADR-KB (`make adr-kb` / `build_adr_kb`) so the
new ADRs are queryable as `concept:adr-*` nodes; update `docs/adr/` cross-references.

## Cross-cutting adversarial slate (principle #14)
The union of each ADR's test section — the primary tests encode the attacker/failure model (wrong
provider blocked, agent-can't-loosen-controls, false-positive rejected, no-upcast, lease race, stale
edge flipped, hairball-capped), not the happy path.
