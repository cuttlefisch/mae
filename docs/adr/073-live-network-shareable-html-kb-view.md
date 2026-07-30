# ADR-073: Live, network-shareable HTML KB view on mae-daemon (OAuth-only v1)

**Status:** Proposed.
**Depends on:** ADR-052 (OAuth 2.1 resource server — the listener this view's route is
added to), ADR-053 (live scoped read-through KB query surface — the pull-based data path
this view reuses unchanged for v1).
**Relates to:** ADR-067 (admin-enforced live-query-only KB access — a per-subject
`ReplicationPolicy::{Full, QueryOnly}` designed for exactly this population, "structurally
the same population a live-query browser view would serve"; this ADR explicitly scopes
ADR-067's Phase D, self-pointing RemoteHub + OAuth self-scoped token issuance + mTLS→
`kb_query` wiring, OUT of v1 — see Decision D1 below — rather than blocking on or
duplicating that separately-tracked, unshipped work), ADR-071 (the wedge/petal visual
design this view's HTML/CSS/JS draws design cues from, with no code dependency — this
view is dependency-free of `mae-canvas`/`mae-core`, matching the visual-reference sister
project's own zero-dependency posture).
**Tracking:** tracker issue TBD (see ADR-070's header); ADR-067's own unshipped Phase D
remains tracked under issue #448, not duplicated here.

## Context

The user wants a live, auto-updating HTML view of a KB, served by `mae-daemon` and
shareable across the network — reusing MAE's existing KB-sharing trust model rather than
inventing new auth. Prior research into `mae-daemon`'s current HTTP surface found:

- The ADR-052 OAuth listener (`daemon/src/oauth.rs`) is a single monolithic `handle_request`
  function (line 359), not a router — it matches only
  `/.well-known/oauth-protected-resource`, and every other path goes through bearer-token
  validation then unconditionally JSON-RPC-dispatches to `kb_query::dispatch`, with every
  response hardcoding `Content-Type: application/json`. Nothing today serves HTML.
- `kb/query.*` (ADR-053, `daemon/src/kb_query.rs`) is confirmed **pull-only** — "live"
  means "queries real current state on each request," never push. A browser view built on
  it alone must poll to appear live.
- The role model (`Owner ⊇ Editor ⊇ Viewer`) already treats a bearer JWT (OAuth-mapped
  claim) as a first-class principal identity feeding the same `kb_access` gate an mTLS
  fingerprint does — a JWT is already "a credential a human can paste into a browser,"
  designed for exactly this by ADR-052.
- ADR-067's Phase D (the piece that would let an **mTLS-only** deployment reach
  `kb/query.*` at all, via a self-minted OAuth token bound to a member's own Ed25519
  identity) is Proposed but not implemented — confirmed via
  `kb/query` absent from `daemon/src/collab_handler/`.

## Decision

### D1 — Scope: OAuth-only v1, ADR-067 Phase D explicitly deferred

This ADR builds the live view on the OAuth listener alone. An mTLS-only deployment (OAuth
listener disabled) gets no access to this view until ADR-067 Phase D separately lands —
this is a deliberate, reversible scoping decision, stated here explicitly (not silently
assumed), and tracked under ADR-067's own existing issue (#448), not duplicated as scope
under this initiative. This mirrors how MAE's own issue #375 tracker separates
"in a milestone now" from "later" without treating the deferred item as cut.

### D2 — Route addition, not a framework migration

A new path match inside `handle_request` (`daemon/src/oauth.rs:359`), e.g.
`GET /kb/{kb_id}/view`, returning a single self-contained HTML/CSS/JS page — no separate
static-asset route, no new static-file-serving surface to harden, matching the visual
reference project's own dependency-free single-file posture. This requires
`handle_request`'s response-building path to stop universally hardcoding
`Content-Type: application/json` for this one route specifically (a real code change, not
just a new branch). New generation code lives in a new `daemon/src/webview.rs` module,
with **zero dependency on `mae-canvas`/`mae-core`/`mae-gui`** — this view's visual design
takes inspiration from ADR-071's wedge/petal geometry and state-layering concepts (both
are portable browser-side JS regardless of what emits the HTML), but the code itself is
independent, mirroring the sister project's own confirmed zero-dependency relationship to
the rest of MAE.

### D3 — Data path: reuse `kb/query.*` unchanged, poll-based for v1

The served page's client-side JS polls `kb/query.get`/`.graph` on an interval (using the
SAME bearer token the page itself was fetched with — the browser tab is the "client" in
`kb/query`'s existing pull model) to refresh content and diagram state. No changes to
`kb_query.rs`, `kb_access`, or the encryption/redaction logic — this view is a new
*consumer* of the existing gated surface, not a new access path. Push-based updates (a
genuinely live feed instead of polling) are explicitly out of scope for this ADR — see
ADR-074.

### D4 — Scope of visual polish: lighter subset than the native editor

Per explicit user decision, this view ships a simpler read-content-plus-diagram page —
no fuzzy search, no in-page history panel, no hover popovers in the first pass (all of
which the visual-reference sister project has, and all of which ADR-071/072 bring to the
*native* editor's chord diagram in full). This view can grow toward that level of polish
in a later phase once the core live-view mechanism (this ADR + ADR-074) is proven.

### Config surface

New fields nested inside the existing `OAuthConfig` struct (`daemon/src/config.rs:471`),
following the established pattern `kb_query_enabled` already set (a sibling capability
toggle on the SAME listener, not a new bind address/section) — e.g. `webview_enabled: bool`
(default `false`, per principle #12), sharing the struct's existing
`max_connections`/request-body-size caps rather than inventing parallel ones.

## Consequences

- The OAuth listener's `handle_request` gains its first non-JSON response path — a real,
  scoped change to a previously JSON-only assumption, not a cosmetic addition.
- mTLS-only deployments have no path to this view until ADR-067 Phase D lands separately
  — documented here explicitly so an mTLS-only operator discovers this from the ADR, not
  by surprise.
- "Live" in v1 means "the page polls," not "the server pushes" — this must be stated
  plainly in the shipped page/docs, never implied to be push-based before ADR-074 lands
  (tracker gate G1: no silent capability degradation/overstatement).

## Verification

Three-tier daemon convention (matching ADR-052/053's own existing test suites): (1) unit
tests on the new route's auth-gating — wrong/expired/forged/missing-claim tokens get
IDENTICAL rejection behavior to every other route on this listener, confirmed by direct
comparison, not a separate assumption; (2) a dispatch-level test with a real `DocStore`
asserting a Viewer-role principal's rendered view is scoped to exactly their accessible
KB — scanning the RAW response bytes for any other KB's content (the same
plaintext-leak-scan discipline `kb_query_tests.rs` already uses for E2E-adjacent surfaces);
(3) a real-binary e2e test (mirroring `daemon/tests/oauth_e2e.rs`) spawning the actual
compiled daemon, fetching the route over real TLS with a real JWT, asserting the response
renders as genuinely non-JSON `Content-Type` and the page loads without a browser-side
script error in an automated headless-browser check.
