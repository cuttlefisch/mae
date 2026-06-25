# ADR-027: Collaboration & P2P observability

**Status:** Accepted (design); **partially implemented.** Foundational tracing exists (the `kb_sync`
edit-flow target + symmetric daemon collab `info!`/`warn!`, and the dialer's `mesh peer …` lifecycle
logs from PR #102). The **mesh-specific surfaces are NOT yet built**: the `*Mesh*` introspection buffer,
`collab-doctor` mesh mode, and exported metrics (#93, Phase 6). #79 (migrate set_status emitters to the
attention bus) is a predecessor for the notification surfaces.
**Extends:** ADR-024 (notification attention bus), ADR-001 (`$/debug`), ADR-006 (collab engine).
**Feeds:** ADR-025 (mesh transport), ADR-026 (peer-verifiable integrity).

## Context

A daemon mesh with peer-verified integrity has far more failure surface than the v0.14 hub: hole-punch
vs relay paths, partitions, gossip loops, convergence lag, signature/chain verification, and stale-epoch
fences — much of it on remote, untrusted peers. v0.14's visibility (`collab_status`, `collab_doctor`,
`kb_sharing_status`, status-line glyphs) is hub-shaped and insufficient to operate or debug a mesh. A
distributed protocol that cannot be *observed* cannot be trusted or maintained. The user's requirement is
explicit: **rigorous logging + observation/visibility implemented alongside the functional code**, not
bolted on after.

## Decision

Treat observability as a **first-class pillar with a fixed contract**: every P2P phase ships its
instrumentation in the **same change** as its functional code, and the verification gate (ADR-026 etc.)
asserts the instrumentation exists so visibility cannot silently regress.

**1. Structured tracing (`tracing`).** Spans cover the full path — `connect → handshake → path-select
(direct|relay) → subscribe → sync → verify(chain|sig|epoch) → apply|fence → gossip` — each tagged with a
consistent field set: peer fingerprint (short), KB id, doc, epoch, op count, decision. Levels: `info`
for lifecycle, `warn` for fences/verification failures/relay-fallback, `debug` for per-op. No secrets,
keys, or plaintext content in spans.

**2. Metrics (counters / gauges / histograms).** A `CollabMetrics` registry (cheap atomic path, sampling
on hot spans) covering at minimum: peers known/connected, **direct-vs-relay ratio**, hole-punch
success/failure, sync round-trips + bytes, ops applied/**fenced**, **signature verify pass/fail**,
membership-chain verify pass/fail, **convergence lag**, partition/heal events, gossip fan-out, eviction
decisions. Exposed via the daemon JSON-RPC (`$/debug` extension) and an **optional** metrics endpoint
(off by default; localhost-bound; documented).

**3. Operator + AI-peer visibility (parity, principle #3).** This is the **read** half of parity; the
**drive** half — every lifecycle *action* (enable/share/join/approve/leave) exposed across CLI + editor
command + Scheme + MCP over one backend — is ADR-025 §"Driving surfaces". Together they guarantee a human
(editor or shell) and an AI peer can both *see* and *do* the entire mesh workflow. All three actors see the
same mesh state from **one builder** (the `kb_sharing_snapshot` pattern extended):
- A **`*Mesh*` introspection buffer** (magit-style, via the shared sectioned-buffer infra): peer table —
  fingerprint, label, direct/relay, RTT, shared KBs, last-sync, verification status; per-KB convergence.
- **`collab-doctor` gains a mesh mode** (reachability, relay health, partition detection, clock/epoch
  consistency across peers).
- **`kb_sharing_status` gains mesh + verification fields** (peers, sync state, chain-verified, fenced
  counts) so the AI peer introspects the mesh exactly as the human does.

**4. Negative-event surfacing through the ADR-024 bus.** Security/availability-relevant events —
**fenced ops, signature/chain verification failures, contact from a revoked peer, partition/relay
fallover, rotation** — raise notifications (severity-routed), never buried on the clobberable status
line. This is precisely the B-19 lesson that motivated ADR-024, applied to the mesh.

**Cross-OS (#13):** all instrumentation is portable Rust (`tracing`/atomics); no platform-gated logging.

## Consequences

- Every P2P PR carries spans + metrics + (where user-facing) a visibility surface; review rejects
  functional mesh code that lands without its instrumentation.
- One snapshot builder feeds buffer + MCP tool + Scheme + doctor (no human/AI divergence; DRY per #8).
- Small runtime cost (sampled spans, atomic counters) — and that cost is itself measured, so the
  signature/hash overhead from ADR-026 is observable rather than assumed.
- Reviewer guardrail: a new mesh code path with no span/metric/decision-log is an incomplete change.

## Verification

Unit: metric counters increment on simulated connect/fence/verify-fail; the snapshot builder is the sole
source for buffer + tool + doctor. Integration: a two-daemon mesh test asserts the **presence** of key
spans/metrics (peer table populated, `fenced_ops` increments on a stale-epoch op, `verify_fail`
increments on a forged mutation) — visibility is part of the test gate. Manual: `*Mesh*` buffer +
`collab-doctor` mesh mode render a 3-peer mesh with one peer on relay and one partitioned.
