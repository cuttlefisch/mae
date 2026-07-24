# Final Adversarial Review — External-Editor MCP Pairing Epic (Phases A-I)

> Closes #385's "final per-phase adversarial review" DoD item. Per CLAUDE.md principle
> #14: "what did these tests *not* try to break?" — this is that review, done once across
> the whole epic rather than only per-phase, since a cross-cutting gap (e.g. no macOS CI
> anywhere) doesn't show up if each phase is only ever reviewed in isolation.

## What was tried (a real, evidence-based tally, not a generic claim)

**Original QA pass** (before this document existed, recorded in PR #387's own body) found
and fixed 5 real bugs via an independent adversarial code-review + CI/e2e coverage audit:
OAuth request-body DoS (no size cap), a VS Code extension async-spawn-error crash (no
`'error'` listener), `kb/query.get`'s byte-truncation overshoot (`.chars()` instead of
byte-boundary-aware), `kb/query.graph`'s unbounded edge list, and an OAuth dispatch
ambiguity conflating two distinct failure cases.

**K1-K5 pass** (issue #388, closed) — found via *live* interactive VS Code + Copilot
Agent-mode testing, the one verification step no automated suite could substitute for:
a real AI-residency data-integrity bug (org-parsed KB content never stamped
`NodeSource::Seed`), a tool-selection-at-scale bug (unfiltered ~758-tool `tools/list`
degrading external agent accuracy), a missing deterministic guidance-setup path, an
undocumented required VS Code settings step, and no Makefile integration for the
extension.

**This pass** (#376-385 DoD verification, L1-L7) — re-verified every phase issue's
*literal* Definition-of-Done against the actual codebase (not the PR's own summary claims)
via 3 parallel Explore-agent audits, then closed the real gaps found:
- **L1 (#376):** a genuinely live, non-hypothetical gap — 5 existing tools
  (`propose_changes`, `debug_start`, `git_stage`/`git_unstage`, `kb_update`) declared
  `"array"`-typed MCP params with zero `items` schema, giving an external client nothing to
  construct a valid call from. Fixed: `ToolProperty` now supports nested `items`/
  `properties`, applied to `propose_changes`'s `changes` param (the genuinely
  array-of-objects case).
- **L2 (#378):** the adversarial test itself (N≥3 real MCP sessions, ≥2 with differing
  permission ceilings, asserting BOTH window-isolation and ceiling-enforcement hold
  simultaneously) found a **real, previously-undiscovered cross-session window-stealing
  bug**: `find_or_create_companion_window`'s "already visible" fast path (branch 1) never
  checked `is_dedicated_window` — a protection its own doc comment already promised, but
  which was only actually wired into branches 2/2.5. Two sessions sharing one agent-shell
  buffer, where the second session's dispatched command didn't redisplay a different
  buffer (e.g. a plain movement/read command), could silently hand session B session A's
  already-established companion window. This is exactly the confused-deputy class ADR-051
  exists to close, reproducible only when a permission *denial* interleaved with
  window-resolution across 3+ sessions — neither the original 2-session ceiling test nor
  the original 3-session (single-tier) window test alone could have found it. Fixed by
  adding the missing `!self.is_dedicated_window(w.id)` check to branch 1.
- **L3 (#380):** added the previously-missing idle-CPU test, a soak-shaped churn test
  (honestly scoped down from "multi-hour," see below), a two-headless KB-convergence test
  (honestly reframed from literal "GUI+headless," see below), and closed the
  `release.yml` headless-service-packaging gap (the systemd/launchd unit files existed on
  disk but were never bundled into a release artifact).
- **L4 (#381):** no code changes — the "real external IdP" DoD wording was reframed
  against what the existing adversarial OAuth test suite actually and defensibly proves
  (see below).
- **L6 (#377/#385):** a real host-compatibility matrix (naming what's verified vs.
  not-yet-verified per host, not generic "any client" language) and this document.

## What these tests do NOT try to break — stated plainly, not silently implied covered

- **No macOS CI anywhere in the standard PR/push workflow.** Confirmed via direct
  `.github/workflows/*.yml` audit: every job in `ci.yml` runs `ubuntu-latest`; the only
  `macos-latest` runner in the entire `.github/workflows/` tree is `release.yml`'s
  Apple-Silicon release-build job, which doesn't run the test suite and doesn't run on
  every PR. This affects every phase that claims "gate G3" (macOS+Linux parity) —
  the underlying code is written XDG-first/cross-platform-aware (principle #13), but
  nothing has *run* it on macOS in CI. Filed as its own separate tracked issue (not
  folded into any single phase, since it's a repo-wide gap) — see the issue linked from
  #384/#385's closing comments.
- **L3c's soak test is NOT the literal multi-hour DoD requirement.** It's a real,
  CI-feasible (~70s) connect/disconnect churn test that catches the *kind* of bug a soak
  test looks for (unbounded per-connection growth), scaled down from "multi-hour" because
  a true multi-hour run doesn't belong gating every PR. A real multi-hour soak, run on a
  schedule (`workflow_dispatch`/cron) rather than per-PR, remains a legitimate fast-follow
  — not attempted here.
- **L3d's convergence test is headless-vs-headless, not literally GUI-vs-headless.**
  ADR-055's own design makes a literal same-project GUI-vs-headless test impossible to
  build (headless mode structurally refuses a second same-project instance — the
  collision-safe-claim mechanism, itself already adversarially tested). Two different
  projects, both headless, both joined to the same daemon-hosted KB, is what actually
  exercises the property the DoD cares about (renderer-independent CRDT convergence) — a
  real interactive GUI/TUI process can't be scripted non-interactively in CI regardless.
- **OAuth's adversarial suite validates against a real self-signed TLS listener and a
  real local mock JWKS server standing in for an external IdP — never a genuinely
  external, real-world IdP with an actual interactive user-consent flow** (confirmed via
  direct read of `daemon/tests/oauth_e2e.rs`: `spawn_mock_jwks_server`'s own doc comment
  says exactly this). A real external-IdP flow would need a registered OAuth app and a
  human clicking through consent — impractical for CI or an agent to drive, and
  ADR-052 already tracks self-hosted minimal-AS mode as its own explicit fast-follow.
  This is the same class of "real components, mocked network boundary" testing already
  used throughout this epic, not a shortcut unique to OAuth.
- **`initialize.instructions` forwarding into VS Code Copilot's actual model context is
  still an open empirical question** as of this document. Live VS Code+Copilot testing
  happened this session (that's how K1-K5 were found) but never specifically probed this
  question. A targeted live check is the one item in this whole review that depends on a
  human, not automatable — see the tracking issue for the result once reported.
- **Zed, Cursor, and JetBrains have zero host-specific testing.** `mae-mcp-shim`'s stdio
  surface has nothing host-specific to block them (same protocol every Path 2 host uses),
  but "should work because the protocol is generic" was exactly the kind of unverified
  assumption principle #14 exists to catch — the host compatibility matrix
  (`docs/EXTERNAL_EDITOR_MCP_PAIRING.md`) marks these explicitly "not yet verified,"
  not silently assumed working.
- **P2P mesh sharing and self-hosted minimal-AS OAuth mode** were never in scope for this
  epic (both are pre-existing, explicitly-tracked fast-follows from their respective
  ADRs) — mentioned here only so their absence isn't mistaken for an oversight.

## Net effect

Two real, previously-undiscovered bugs were found and fixed by this review pass alone
(L1's nested-schema gap, L2's cross-session window-stealing bug) — both found specifically
*because* the review insisted on re-deriving evidence from the codebase rather than
trusting the epic's own prior "done" claims. Every remaining gap above is recorded with
enough specificity (what's untested, why, and what it would take to close) that it can't
silently regress into an implied-covered claim later.
