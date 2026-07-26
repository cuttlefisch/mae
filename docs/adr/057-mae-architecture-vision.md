# ADR-057: MAE architecture vision — the 5-layer model and its confirmed gaps

**Status:** Accepted.
**Relates to:** ADR-014 (binary architecture — editor/daemon/shared workspace split), ADR-029
(KB source of truth = CRDT, CozoDB = derived projection), ADR-030 (in-text typed-link grammar +
parser-as-projector), ADR-035 (editor↔daemon boundary + `daemon_mode`), ADR-050 (external-editor
MCP pairing — VS Code/Copilot & cross-editor compatibility), ADR-053 (live scoped read-through KB
query surface). This ADR is the root of a new ADR set (ADR-058 through ADR-066); every child ADR
in that set Extends this one.
**Tracking:** epic tracker to be filed.

## Context

The project owner articulated MAE's emerging long-term architecture as a 5-layer model —
core engine, native frontends, non-native MCP-speaking editors, `mae-daemon`, and the KB
substrate connecting them all — and asked for a critical, no-shortcuts architectural review
of that vision against what actually exists in the codebase today. The deliverable is this
ADR plus its nine children (ADR-058 through ADR-066) and their corresponding GitHub issues.

**The vision, verbatim.** A core engine drives AI-peer decision-making and record-keeping
through molecularly structured, ADR-style documents, building per-project knowledge bases for
code and supporting "any number" of local/federated instances for non-coding second-brain use.
Connected to that core: the native MAE GUI (today's code/text-focused frontend), plus other
native MAE frontends sharing the same KB/CRDT core for different workflow shapes — visual
design is named explicitly as one such shape. Non-native, MCP-speaking editors and agent
harnesses (VS Code + Copilot, any other MCP client) get KB and guidance parity with native
frontends, not a second-class subset. `mae-daemon` is a genuine server — either dedicated or
client-local — handling KB maintenance, enrichment, and optimization, reachable smoothly from
any chosen editor across hosting servers, federated instances, and per-project local KBs. This
is a materially more ambitious claim than what MAE's docs currently assert: it names
"any number" of KB instances, a second native frontend for a different workflow shape, and a
daemon that does real background work rather than just persisting state, as load-bearing parts
of the architecture rather than aspirational mentions.

**Why this needs a gated review, not a mission-statement update.** CLAUDE.md principle #7
requires no hardcoding and no ad-hoc solutions; principle #9 requires every change to state bug
risk, performance impact, and downstream impact before it lands; principle #15 requires bugs
and gaps to be traced to root cause, not patched at the symptom. Writing the vision into
README.md or CLAUDE.md as prose would satisfy none of those disciplines — it would commit the
project to a direction without first establishing which parts are already true, which parts are
false today for identifiable and fixable reasons, and which parts are large enough that they
need their own phased, adversarially-tested rollout. This ADR is that establishment step. It
does not implement anything; it inventories the gap between vision and reality with file:line
evidence for every claim, and hands each gap to a dedicated child ADR sized to the actual work
required to close it.

**Method note.** This ADR's evidence table was produced via six independent parallel research
passes against MAE's own codebase — three initial passes covering the core engine, the daemon,
and the frontend/MCP surface respectively, followed by three targeted follow-up passes to close
gaps and contradictions the first round surfaced — with every claim in the table cited to a
specific file and line range, not summarized from memory. A dedicated synthesis pass then
merged the six passes' findings into the single table below, resolving overlaps (for example,
gaps #6 and #7 both trace to `shared/kb/src/federation.rs` and `query.rs` and are closed by the
same child ADR). A fourth pass, run after the internal research was complete, deliberately went
external: four parallel research threads examined real, comparable open-source and production
systems — org-roam and Logseq for per-project/federated personal-KB precedent, the Emacs daemon's
multi-decade bug history for what a single-process shared-server architecture actually breaks
under concurrent multi-client load, gopls and rust-analyzer for how language servers handle
per-workspace state in a shared-daemon model, two real CVEs illustrating the failure modes of
under-scoped daemon trust boundaries, AWS Amazon Q for enterprise-scale AI-agent KB integration
patterns, AFFiNE/BlockSuite for a CRDT-backed multi-frontend architecture with a genuinely
separate visual-canvas surface sharing a text core, and tldraw/Excalidraw/Figma plus Martin
Kleppmann's tree-move CRDT algorithm for the specific hazards of a second, structurally
different (spatial/graph rather than linear-text) frontend built on the same CRDT substrate.
Every load-bearing claim in the table below was independently spot-checked against either live
MAE code or one of these external sources before being finalized here — nothing in this ADR
rests on a single research pass's unverified word.

**Central finding.** The vision is largely architecturally compatible with what MAE already is.
Three foundational decisions already accepted and shipped do almost all of the heavy lifting the
vision assumes: CRDT/text is already the portable, source-of-truth authoring substrate
(ADR-029), CozoDB is explicitly a rebuildable projection of that CRDT truth rather than a hard
dependency any new frontend or KB shape must accommodate directly (ADR-029, ADR-030), and MCP
tool parity for external, non-native editors is already uniform across ~700+ tools rather than a
curated subset (ADR-050). None of the nine gaps below require re-litigating those decisions.
What is actually missing is nine concrete, independently real gaps — not a single monolithic
"the vision isn't built yet" finding, but nine separately scoped, separately verifiable pieces
of work, each closed by its own child ADR (ADR-058 through ADR-066). They are sized honestly:
most are contained, but three — a genuine multi-tenant daemon server (ADR-060), a second native
frontend for visual-design workflows (ADR-064), and native Windows client support (ADR-066) — are
large, multi-phase efforts in their own right, comparable in scope to the external-editor MCP
pairing initiative (ADR-050 through ADR-055) that preceded this one.

**A requirement clarified mid-review.** The owner's original framing of "reachable smoothly from
any chosen editor" left the platform matrix ambiguous. Clarified explicitly during this review:
end-users must be able to run their chosen frontend — the native MAE GUI/TUI/headless editor,
`mae-mcp-shim`, VS Code plus the `mae-vscode` extension, or any other MCP-speaking editor — on
Linux, macOS, or Windows. `mae-daemon` and any hosted-KB server, by contrast, remain explicitly
Linux-only by design; no Windows or macOS work is needed or in scope for the daemon binary
itself. This asymmetry — client-side platform parity is a hard requirement, server-side platform
parity is explicitly not — is significant enough to its own set of child ADRs that it is called
out as a standalone cross-cutting decision (Gate W, below) rather than left implicit in each
child ADR's scope.

## Decision

Ratify the 5-layer vision as MAE's architectural direction, with the following two decisions:

1. **Adopt the evidence table below as the authoritative statement of what must change**, and
   commit each row to its named child ADR. No row is deferred without a named reason; no row is
   bundled into another without a named reason. This is the concrete mechanism by which
   principle #9's "every change must consider downstream impact" is honored at the initiative
   level, not just per-PR: before any of the nine child ADRs' implementation phases begin, the
   full blast radius of the vision — what's already true, what's false and why, what's large —
   is on record in one place.

2. **Adopt Gate W (below) as a cross-cutting requirement binding on every child ADR** that
   touches a client-facing binary, so that "any chosen editor, any OS" is enforced as an explicit
   verification phase in each relevant child ADR rather than left as an unstated assumption that
   silently erodes as each child ADR ships independently.

### The evidence table

Each row states the vision requirement, what the codebase actually does today, the file:line
citation backing that claim, and the child ADR that closes the gap.

| # | Vision requirement | Today's reality | Citation | Closed by |
|---|---|---|---|---|
| 1 | Per-project KB provisioning | One machine-global primary KB plus one flat global registry; `KbInstance` has no project-path field, so there is no structural way to say "this KB belongs to this project" | `crates/mae/src/bootstrap.rs:2117`, `shared/kb/src/federation.rs:34-59`, `crates/core/src/project.rs:90` | ADR-058 |
| 2 | Molecularly structured, ADR-style decision records living in the KB | Only 4 of 57 ADRs exist as KB nodes at all, and those 4 are hand-authored one at a time with no importer that keeps the KB in sync with `docs/adr/*.md` as new ADRs land | `crates/core/src/kb_seed/concepts.rs:1477,1496,1517,1539` | ADR-059 |
| 3 | `mae-daemon` as a genuine dedicated or shared server | One global `Arc<Mutex<DaemonState>>` serializes every RPC through a single lock; `mae-daemon.service` is a non-templated, one-per-OS-user systemd unit with no multi-tenant instantiation story; ADR-054's own concurrency benchmark has no tenant dimension, so "shared server" capacity is unmeasured, not just unimplemented | `daemon/src/main.rs:155`, `assets/mae-daemon.service`, `docs/adr/054-daemon-concurrency-hardening.md:163-172` | ADR-060 |
| 4 | KB enrichment (AI-driven derivation of new KB content, not just storage) | Zero AI-driven enrichment exists anywhere in the codebase; `store_embedding` is fully implemented but has no non-test callers — the write path exists, nothing calls it in production | `shared/kb/src/cozo_store/vector.rs:10`, `docs/adr/031-derived-intelligence-projection.md:54-56` | ADR-061 |
| 5 | Daemon background maintenance | Two of the scheduler's three ticks are literal `// TODO` stubs that increment a counter and do nothing else; only the third (health/hygiene) tick is actually wired to real work | `daemon/src/scheduler.rs:60-70` (stubs) vs `daemon/src/scheduler.rs:72-108` (wired) | ADR-065 item 2 |
| 6 | "Any number" of local/federated KB instances | The federation registry is an unindexed `Vec`; every read does an O(N) unconditional fan-out across all registered instances; `KbScope::All` (fan-out-everything) is the default scope, not an opt-in — the mechanism degrades linearly as instance count grows and defaults to the most expensive behavior | `shared/kb/src/federation.rs:114-115,210,287-311`, `shared/kb/src/query.rs:209-393`, `crates/core/src/options.rs:429-431` | ADR-062 |
| 7 | One search experience spanning local and remote KBs | Local federation is already unified and blended by default — that part of the vision is already true. A remote hub KB (ADR-053's live scoped read-through query surface) is a structurally separate data model with its own request/response shape; zero MCP tools wrap `kb/query.*`, so a remote hub is reachable by the daemon protocol but invisible to the same `kb_search`/`kb_search_context` surface a local federated instance uses | `crates/core/src/editor/kb_ops/search.rs:268,278-372`, `docs/adr/053-live-scoped-kb-query-surface.md:106-116` | ADR-062 |
| 8 | KB + guidance parity for external MCP clients | An external MCP client receives only a short pointer string where `mae-agent-cli` (the native harness) receives the full guidance-KB body inlined directly into context; the live-sync fallback that would let an external client pull the full body on demand defaults off | `crates/mae/src/main.rs:793-836` vs `crates/agent-cli/src/main.rs:202-218`, `option_tests.rs:1206` | ADR-063 |
| 9 | A second native frontend for visual-design workflows, sharing the same KB/CRDT core | ADR-016 is the only design document for this and is still `Status: Proposed`, with its Phases 2-3 unshipped; no ADR or ROADMAP entry names an actual separate frontend *application* (as opposed to an in-editor buffer kind) for visual design. The real foundation for one already exists, though — `mae-canvas` and `VisualBuffer` are shipped, general-purpose primitives, not a dead end | `docs/adr/016-artifact-interaction-model.md:3`; foundation at `crates/canvas/src/*.rs`, `crates/core/src/visual_buffer.rs:1-46` | ADR-064 |
| 10 | Correct federated health reporting | `FederatedQuery::health_report` silently returns data from only the primary instance, ignoring every federated instance — while its sibling function in the same file, `id_title_body_triples`, correctly aggregates across all instances just a few lines above it. This is not a missing feature; it is a diverging implementation of the same "aggregate across instances" contract that its neighbor gets right | `shared/kb/src/query.rs:359-361` vs `shared/kb/src/query.rs:341-357` | ADR-065 item 1 |
| 11 | Consistent org-directive semantics regardless of which path wrote the content | `#+TRANSCLUDE:` is parsed only at file-import time. A node created or updated directly via MCP `kb_create`/`kb_update` never re-derives it — unlike the sibling typed-link directive path in the same subsystem, which correctly re-derives on every write regardless of path. Two directives in the same grammar, one respects write-path independence and one doesn't | `crates/core/src/editor/kb_ops/nodes.rs:198-216,449-462` vs `shared/kb/src/cozo_store/links.rs:16-24` | ADR-065 item 4 |
| 12 | End-users can run their chosen frontend — native MAE GUI, VS Code + extension, or any MCP-speaking editor — on Linux, macOS, or Windows, while `mae-daemon`/hosted-KB servers remain explicitly Linux-only | Zero Windows CI legs and zero Windows release targets exist for any client binary; the editor's own local MCP socket hard-depends on Unix domain sockets with no Windows-native path; a real instance of exactly this failure class is already confirmed live in `cuttlefisch/mae-vscode#1` | `crates/mae/src/main.rs` (primary/agent MCP socket construction), `crates/mae/src/headless_loop.rs` (ADR-055 stable-socket claim/discovery), `shared/mcp/src/shim.rs` (`mae-mcp-shim` client-side connection logic), `.github/workflows/release.yml` (zero `windows-latest` jobs), `.github/workflows/ci.yml` (zero Windows legs), `mae-vscode` issue #1 | ADR-066 |

Rows 6 and 7 are listed separately because they are separately real claims in the vision — "any
number of instances" is a scaling/architecture question, "one search experience" is a data-model
unification question — but they are closed by the same child ADR because their root cause is the
same unindexed, unconditionally-fan-out federation registry; splitting them into two ADRs would
violate principle #8 (shared computation, no duplicated fixes for one underlying cause). Rows 10
and 11 are grouped as items within ADR-065 alongside row 5 rather than given their own ADR numbers
because each is a small, independently-verifiable correction (a wrong aggregation call, a missing
re-derivation call, an unwired scheduler tick) rather than a design decision requiring its own
Context/Decision/Alternatives structure — bundling them respects principle #9's "every change"
discipline without inflating the ADR count with what are, in substance, three linked bugfixes.

### Gate W — client cross-platform compatibility (cross-cutting requirement)

This gate binds every child ADR that touches a client-facing binary. It is scoped precisely, per
the owner's explicit clarification during this review, to avoid two symmetric mistakes: treating
the daemon as needing cross-platform work it does not need, and treating "the vision requires
cross-platform reach" as satisfied by daemon-side work alone when it is not.

**In scope — must work on Linux, macOS, and Windows:** the native MAE GUI, TUI, and headless
editor binaries; `mae-mcp-shim`; the `mae-vscode` extension; any other MCP-speaking editor
integration; any future native MAE frontend built under ADR-064. This must hold in both the
fully-local case — a per-project KB living only on the user's own machine, served by their own
local `mae --headless` or GUI instance with `daemon_mode=off`, with no `mae-daemon` involved at
all — and the remote case, where a client on any of the three OSes reaches a Linux-hosted
`mae-daemon` over the network. The fully-local case matters independently: it is the only
topology where "any OS" is a claim about the editor binary alone, with no daemon in the loop to
paper over a client-side platform gap.

**Explicitly out of scope — Linux-only by design, not by oversight:** `mae-daemon` itself, and
any hosted-KB server built under ADR-060's multi-tenancy work. ADR-060's daemon-process and
service-management phases (systemd unit templating, process supervision, per-tenant socket
lifecycle) are explicitly exempt from Gate W and must not be redesigned for cross-platform
service management — that would be scope creep with no corresponding entry in the vision, which
names hosting servers as infrastructure the end-user *reaches*, not infrastructure the end-user
*runs on their own laptop regardless of OS*.

**Enforcement mechanism.** Every child ADR whose Decision section touches a client-facing binary
must include an explicit Windows verification phase in its own Verification section — not a
blanket "and it should also work on Windows" aside, but a named phase with its own success
criteria, mirroring how ADR-013's cross-platform corollary (CLAUDE.md principle #13) already
requires CI to exercise both macOS and Linux for anything touching paths, sockets, or scripts.
Gate W extends that existing discipline to a third OS specifically for the client surface this
initiative adds, rather than inventing a new discipline from scratch.

## Consequences

**Positive.** The vision, once this ADR's table is accepted, stops being an unverified assertion
and becomes nine tracked, independently gradable pieces of work with a shared evidence base. Any
future contributor asking "does MAE already do X from the vision" has one table to check against
live citations, rather than needing to re-derive the answer from scratch or trust prose in
README.md that could silently drift out of date. The central finding — that CRDT/text portability,
Cozo's projection-not-dependency status, and MCP tool parity are already load-bearing and correct
— means none of the nine child ADRs need to re-open those three foundational decisions; they can
build directly on ADR-029, ADR-030, and ADR-050 as given.

**Costs, stated honestly.** This is a large, multi-year program of work — ten ADRs including this
one, an estimated ~46 phase issues across the nine children once each is broken down the way
ADR-050 was (its own lettered-phase precedent, Phases A–J, is the direct model for how ADR-058
through ADR-066 should each be phased). Ratifying this ADR commits the project to treating it as
the roadmap's next major initiative immediately after the currently-shipping external-editor MCP
pairing work (ADR-050 through ADR-055) finishes landing — not a parallel track competing for the
same review and implementation bandwidth, a successor track. Three of the nine children — ADR-060
(genuine daemon multi-tenancy), ADR-064 (a second native frontend), and ADR-066 (native Windows
client support) — are large enough on their own that each is comparable in scope to the entire
ADR-050-055 initiative that preceded this one; none of the three should be treated as a quick
follow-on to a smaller ADR, and their own child issues must be scoped accordingly rather than
compressed into a single phase each.

**Downstream/bug-risk framing (principle #9), applied at the initiative level.** Every row in the
evidence table is, in effect, a drift finding under principle #15: rows 5, 10, and 11 in
particular are not missing features but existing code whose neighbor in the same file or module
already does the correct thing (`health_report` vs. `id_title_body_triples`; the typed-link
directive vs. `#+TRANSCLUDE:`; two of three scheduler ticks wired, one not) — meaning the fix in
each case is convergence toward an already-proven-correct pattern in the same codebase, not a
novel design. That lowers the bug risk of ADR-065's three items specifically relative to the
larger, more architecturally novel children (ADR-060, ADR-062, ADR-064, ADR-066), and each child
ADR's own Consequences section should say so explicitly rather than presenting all nine gaps as
equally risky.

## Alternatives rejected

- **Leave the vision as aspirational prose in README.md or CLAUDE.md.** Rejected — the owner
  explicitly asked for a gated architectural review, not a mission-statement update. Prose without
  a verified gap analysis would let the vision drift further from reality with each release,
  exactly the failure mode principle #15 exists to catch: a stated intention with no mechanism
  tying it back to what the code actually does becomes indistinguishable from marketing.
- **One mega-ADR covering all nine gaps.** Rejected — bundling all nine into a single ADR would
  make ratification and implementation all-or-nothing, which is unworkable at this scope and
  unusable for issue tracking: a reviewer could not accept "per-project KBs" without simultaneously
  accepting "native Windows client support," even though the two have no dependency on each other
  and radically different sizes. ADR-050's own lettered-phase precedent — ten phases (A-J) tracked
  as separate issues under one epic, not one PR — is direct evidence that granularity matters at
  this scope; this ADR set follows the same shape one level up, as separate ADRs rather than
  separate phases of one ADR, because unlike ADR-050's phases these nine items are independent
  design decisions in their own right, several large enough to need their own Alternatives-rejected
  and Verification sections.
- **Treat `mae-daemon` as needing Windows/macOS parity to satisfy "reachable from any editor."**
  Rejected during the review, not before it — the owner's original framing was ambiguous enough
  that this was a live candidate reading. Rejected because the vision's own language distinguishes
  the client the end-user runs from the server infrastructure the client reaches; conflating the
  two would mean redesigning `mae-daemon`'s process/service-management model (systemd-specific
  today) for no corresponding requirement in the vision, and would roughly double ADR-060's scope
  for a platform-parity claim nobody actually needs the daemon itself to satisfy.

## Verification

This ADR makes no code change, so its own verification is evidentiary rather than executable:
every citation in the evidence table above was checked against live code as part of writing this
ADR, not carried forward unverified from an earlier draft. That said, principle #14 (adversarial,
not confirmation testing) still applies to how this ADR's claims get used downstream, and each
child ADR inherits specific, falsifiable obligations from it:

- **Each child ADR's Context section must re-cite its own table row's evidence independently**
  at the time that child ADR is written, not merely reference this ADR's table by number — code
  moves between when this ADR is ratified and when a given child ADR's implementation phase
  starts, and a stale citation silently reintroduces the exact "aspirational prose" failure mode
  this ADR exists to prevent. A child ADR whose Context section cannot re-derive its own cited
  file:line from current `main` is invalid until it does.
- **Gate W's Windows verification phase, where required, must include at least one test that is
  expected to fail on the current codebase before the child ADR's implementation lands** — for
  example, ADR-066's own verification should include a CI leg that attempts to build/run
  `mae-mcp-shim` on `windows-latest` and confirms it currently fails for the cited reason
  (`shared/mcp/src/shim.rs`'s Unix-socket dependency, and the editor-side sockets it connects to in
  `crates/mae/src/main.rs`/`headless_loop.rs` — never `mae-daemon`, which is out of scope per Gate
  W above), not merely a leg added after the fix that trivially passes. A cross-platform test that
  only ever runs after the platform gap is closed proves nothing about whether the gap was real.
- **Row 5's, row 10's, and row 11's "convergence to an existing correct sibling" framing must be
  falsified, not assumed, before each is closed** — the child ADR (ADR-065) must show the fixed
  `health_report` genuinely returns federated-instance data when a federated instance exists and a
  wrong/absent instance is excluded (not a single-instance happy path that can't distinguish
  "aggregates correctly" from "still only returns the primary"), the fixed `#+TRANSCLUDE:` path
  must show a node written via direct MCP `kb_update` re-derives the directive identically to one
  written via file import (a round-trip/equivalence check across both write paths, not a check of
  only the previously-broken path), and the two newly-wired scheduler ticks must show a genuine
  side effect occurring on tick (an incremental reimport actually enqueued, an integrity check
  actually run) rather than only that the counter increments — a passing counter increment is
  exactly the kind of confirmation-only assertion principle #14 warns against, since the stub code
  already made the counter pass before this ADR.
- **This ADR itself must be revisited if any child ADR's implementation discovers its cited gap
  was already partially closed by unrelated work landing between this ADR's ratification and that
  child's start** — per principle #15, that is itself worth recording as a correction in this
  ADR (mirroring how ADR-056 recorded its own mid-write correction) rather than silently absorbed
  into the child ADR with no trace back to the root table.

### Ratification note (issue #395)

Before flipping status, the evidence table's 12 citations were spot-checked against current
`main` rather than trusted from the original authoring pass (per the requirement above: code
moves between ratification and use). Six of the nine child ADRs this table justifies (058, 059,
062, 063, 065, and 066 Phases A-C) have since shipped real code — checked directly:

- Row 3 (`daemon/src/main.rs`) — `CozoKbStore::open_with_engine(&db_path, "sqlite")` still present
  at the cited call site; the ADR-054 doc citation's capacity/connection-cap claims still match.
- Row 5 (`daemon/src/scheduler.rs`) — the maintenance/health tick split ADR-065 item 2 closed is
  present and now carries an explicit in-code comment naming both ADR-065 item 2 and the ADR-061
  Phase C enrichment work it deliberately leaves for later — exact line numbers shifted (expected,
  since this is the code ADR-065 modified) but the claim holds.
- Row 6 (`shared/kb/src/federation.rs`) — `Vec<KbInstance>`, `register_remote_hub`, and
  `FederatedKb`'s `HashMap<String, KnowledgeBase>` all still present, confirming unbounded
  federated-instance registration remains the shape ADR-062 built on.
- Row 11 (`shared/kb/src/cozo_store/links.rs` vs `crates/core/src/editor/kb_ops/nodes.rs`) — the
  cited inconsistency is gone, replaced by `update_links_for_node`'s `parse_typed_links` call with
  an in-code comment naming the exact conformance gap ADR-065 item 4 closed. This row's "today's
  reality" text is now stale in the sense that the gap it describes no longer exists — expected
  once a closing ADR ships, not a defect in the table (see the "revisited if discovers... already
  partially closed" bullet above, which anticipates exactly this).
- Row 12 (CI/release workflows) — `windows-latest` legs present in both `ci.yml` and
  `release.yml`, matching ADR-066 Phases A-C's shipped state.

No fabricated or dangling citation was found. Line-number drift on rows tied to since-shipped
child ADRs is expected and doesn't invalidate the table — it recorded a real point-in-time gap
that has since closed, which is precisely what "Closed by" attributes it to. Gate W (Windows
verification requiring a pre-fix failing test, defined earlier in this section) is confirmed
agreed and binding on ADR-060, ADR-064, and ADR-066 as cross-cutting scope — ADR-066 Phases A-C
already followed it (a real `windows-latest` CI leg exists and was iterated on against actual
failures, not added post-hoc); it remains binding on ADR-066's unstarted Phases D-E and on
ADR-060/ADR-064 once their own Windows-relevant surfaces are implemented.

Status flipped `Proposed` → `Accepted` on this basis.
