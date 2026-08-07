# ADR-097: Browser MAE is a KB surface, not a browser editor

**Status:** Proposed.
**Depends on:** ADR-014 (editor/daemon/shared workspace split), ADR-035 (editor↔daemon
boundary + `daemon_mode`), ADR-053 (live scoped read-through KB query surface), ADR-052 (OAuth
2.1 resource server).
**Relates to:** ADR-050/051 (external-editor MCP pairing — the *other* non-native surface, and
the one this is most often confused with), ADR-064 (a second native frontend — explicitly a
different thing, see D4), ADR-073 (the live network-shareable HTML KB view this supersedes as
the browser's read surface), ADR-074 (SSE push — narrowed by this ADR's D3).
**Amends:** CLAUDE.md principle #12 (`CLAUDE.md:134`), narrowly and in the open, per principle
#17. See D2.
**Tracking:** issue #650, which recorded this scope descriptively and asked whether it warranted
an ADR to become normative. This is that ADR.

## Context

Three surfaces reach MAE's knowledge base, and the third keeps being reasoned about as though
it were the second or the first:

1. **Native MAE** (TUI + GUI) — the full editor: buffers and windows, vi-modal editing, LSP,
   DAP, shell, babel, the KB.
2. **External editors over MCP** (ADR-050/051) — VS Code + Copilot first. The paired editor is
   the human's GUI; MAE runs underneath as the KB and guidance backend.
3. **Browser MAE** — a web frontend served by `mae-daemon`, for KB work.

Issue #650 recorded the scope decision for (3) — knowledge-base work only — because it was
being re-derived from scratch in each planning pass, each time at some cost. This ADR makes it
normative so the boundary stops moving.

**The substrate for (3) largely exists, and that is exactly why the boundary needs writing
down.** A planning pass can look at what is built and reasonably conclude that a general editor
in the browser is a short step away. It is not.

What is built and browser-reachable today:

- The OAuth 2.1 resource server (`daemon/src/oauth.rs`, ADR-052): rustls TLS, JWKS validation,
  a configurable `principal_claim`, RFC 9728 protected-resource metadata at
  `/.well-known/oauth-protected-resource` (`oauth.rs:381`), connection caps, `no-store` hygiene.
- The read-through KB query surface (ADR-053), dispatched at `daemon/src/kb_query.rs:83-126`.
  It is exactly five methods — `kb/query.capabilities`, `.get`, `.search`, `.graph`,
  `.my_wrapped_key` — and every one is read-only. A sixth method is not refused at runtime; the
  `match` has no write arm at all, and `other =>` falls through to `method_not_found`
  (`kb_query.rs:125`).
- A live HTML KB view, already served (ADR-073): `daemon/src/webview.rs` (308 lines), routed at
  `oauth.rs:393` via `parse_view_path`, rendered at `oauth.rs:456`, config-gated by
  `webview_enabled` and default `false` (`daemon/src/config.rs:553,590`).

What is not built, and would each be a substantial initiative in its own right: any write path
on a browser-reachable surface, any bidirectional transport (there is no WebSocket or SSE
anywhere in the tree), and any of the editor machinery — buffers, windows, modes, LSP, DAP,
shell — none of which has a network-reachable representation at all.

So the honest position is that the browser is *close* to a good KB surface and *nowhere near* an
editor. Leaving that implicit invites the scope to drift one plausible increment at a time.

## Decision

### D1 — Browser MAE is scoped to knowledge-base work

**In scope:** search, read, navigate, visualize, and edit KB nodes; see which KBs the caller can
reach given their role; node history and rollback; outline and graph navigation.

**Out of scope, normatively:** the buffers-and-windows model, vi-modal editing, LSP, DAP, the
embedded shell, org-babel execution, project/file browsing, and the Scheme runtime as a
user-facing surface. These are not "later phases." A future ADR may revisit any of them, but it
must argue the case explicitly rather than inheriting it.

The test for whether a proposed browser feature is in scope: **does it operate on KB nodes and
their relationships?** Not "could it be useful in a browser."

### D2 — Amend principle #12 narrowly, in the open

CLAUDE.md principle #12 states that *"the daemon is an optimization for persistence and
discovery, not a requirement for collaboration,"* with the in-process embedded KB as the floor
(`CLAUDE.md:134`).

A browser client cannot satisfy that. It has no in-process MAE core to fall back to; it reaches
MAE only over the network, and `mae-daemon` is the only thing on the other end. **For the
browser surface specifically, the daemon is a hard requirement, not an optimization.**

Per principle #17, this is recorded as an amendment with its evidence rather than violated
silently. The amendment is deliberately narrow:

- It binds **only** to the browser surface. Native MAE keeps `daemon_mode = off` as its default
  and the embedded KB as its floor, unchanged.
- It does **not** make the daemon a required component of MAE. It makes the daemon a required
  component *of choosing to use a browser*.
- It does **not** concede online-only operation. A browser client holding its CRDT state in
  IndexedDB retains genuine offline editing and later convergence — several of the local-first
  ideals principle #12 tracks survive intact. What is lost is only the "no server needed at all"
  property, and only for this surface.

CLAUDE.md principle #12 must be updated in the same PR as this ADR, naming this surface as the
carve-out. An amendment recorded only in an ADR is the drift this project has already been
bitten by.

### D3 — The browser is a read *and* write surface, which narrows ADR-074

ADR-073's view and ADR-074's proposed SSE push are correct for a read-only view, and ADR-074's
D1 says so explicitly: the feed is *"strictly one-directional"* and *"the browser never needs to
push anything back."* That premise holds for a viewer and fails for an editor.

ADR-074 names its own escape hatch — WebSocket *"remains available to reconsider if a future,
genuinely bidirectional need arises."* D1 above establishes that need. The transport decision
itself is **not made here**; it belongs to a dedicated ADR gated on the proving spike (below).
What this ADR settles is only that a one-directional transport cannot serve the scope in D1, so
ADR-074's reasoning does not extend to it. ADR-074 remains valid, unchanged, for read-only
views.

### D4 — The browser is a client, not a second native frontend

ADR-064 rejected *"a web/Electron app instead of a native Rust binary linked against
`mae-core`"* — on the grounds that it *"would not share the core in-process at all"* and would
need *"its own separate sync/IPC layer to reach the real engine."*

That rejection stands and does not apply here, because it answers a different question. ADR-064
asks how to build a **second native frontend** that proves MAE's core supports more than one
in-process consumer; a web app cannot prove that claim, so it was correctly rejected *for that
purpose*. Browser MAE makes no such claim. It is a network client of the daemon, in the same
family as VS Code over MCP (ADR-050) — and the separate sync layer ADR-064 counts as a cost is,
for a browser, simply the medium.

Stating this explicitly matters because the two ADRs would otherwise read as contradicting each
other, and a future reader could cite ADR-064 to block this work.

## Consequences

**Positive.** The boundary stops being re-derived. A contributor asking "should the browser get
X" has one test (D1) instead of an argument. ADR-064 and this ADR stop appearing to conflict.
And principle #12's carve-out is on the record with its reasoning, so the next person to notice
the tension finds an answer rather than a contradiction.

**Costs, stated honestly.** This ADR forecloses a genuinely attractive option — a full MAE in
the browser — without proving it impossible, only out of scope. If the hosted deployment later
finds that users want to edit code in the browser, this decision has to be reopened rather than
extended, and that will feel like friction precisely because the substrate will by then look
even closer to sufficient than it does today. That is the intended cost: the boundary is worth
more than the flexibility.

**Downstream/bug-risk framing (principle #9).** This ADR changes no code, so it carries no
direct bug risk. Its downstream risk is the opposite failure — that a narrow scope written down
becomes a reason not to fix real gaps that sit inside it. Three such gaps were confirmed while
writing this ADR and are in scope by D1, not excluded by it:

- `kb/list` (`daemon/src/collab_handler/kb_membership.rs:53`, dispatched at
  `collab_handler/mod.rs:1893`) takes no principal and performs no `kb_access` check — it
  returns `doc_store.list_kb_metas()` in full. D1 puts role-filtered KB enumeration in scope;
  this is the gap that must be closed to deliver it, and it is security-relevant independently
  of the browser.
- `kb/query.search` (`daemon/src/kb_query.rs:100`) does not use the Cozo FTS projection running
  on the same daemon.
- `kb/query.graph` (`kb_query.rs:123`) is bounded by `max_scan_nodes` (default 500) and reports
  truncation in the payload — a caller that ignores that field silently renders an incomplete
  graph.

Each gets its own issue; none is resolved by this ADR.

## Alternatives rejected

- **Leave the scope descriptive, in issue #650 only.** Rejected — that is the state that
  produced the problem. An issue is not consulted by someone planning a feature; ADRs are, and
  this project's ADR index is the documented entry point for "has this been decided."
- **Scope the browser as a full editor and phase toward it.** Rejected. Every editor subsystem
  named in D1's exclusion list would need a network representation that does not exist today,
  and the KB surface — which does have a substrate — would be delayed behind that work. The
  ADR-050 initiative already answers "I want my familiar editor plus MAE's KB" for the case
  where a user genuinely wants an editor; a second, weaker answer in the browser competes with
  it for no gain.
- **Amend principle #12 broadly, to "the daemon is required whenever a network client is
  involved."** Rejected as overreach. It would sweep in the P2P mesh (ADR-025), where peers
  reach each other without a daemon in the middle, and would contradict ADR-035's `daemon_mode`
  design for no benefit. The narrow carve-out in D2 is the smallest amendment that is true.
- **Say nothing about principle #12 and just build the browser surface.** Rejected explicitly.
  Principle #16 exists precisely because three security fixes each quietly contradicted
  principles #3 and #7, and *"the contradictions were right and unwritten, which is the worst of
  both."* This is the same shape, and the same remedy applies.

## Verification

This ADR makes no code change, so its verification is evidentiary and procedural. Every citation
above was checked against `main` at the time of writing (`33282fb9`), not carried forward from
an earlier pass — per ADR-057's standing requirement that a child ADR re-derive its own
citations rather than trusting a parent's table.

Falsifiable obligations this ADR takes on:

- **The principle #12 amendment must land in the same PR as this ADR**, editing `CLAUDE.md:134`
  to name the browser carve-out. A reviewer can check this trivially, and its absence means D2
  has not actually been adopted — only described. This is the specific failure mode principle
  #17's "amend in the open" rule exists to prevent.
- **D1's scope test must be applied to the first browser feature that lands**, and the ADR
  updated if the test proves unusable in practice — a scope rule that cannot classify a real
  feature is not a rule. Recording that as a correction here is preferred to quietly widening
  the boundary.
- **D3 must not be read as deciding the transport.** Any ADR that cites this one as
  justification for a specific transport is misusing it; the transport decision requires the
  proving spike's evidence and its own Alternatives-rejected section.
- **The three gaps named in Consequences must be filed as issues before this ADR is marked
  Accepted.** An ADR that names gaps and leaves them untracked converts a finding into folklore,
  which is how ADR-073's own status came to claim "Proposed" for shipped code (issue #633).
