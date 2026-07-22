# ADR-048: AI residency policy for sensitive KBs

**Status:** Proposed.
**Extends:** ADR-045 (provider parity — the `editor.ai.provider` value this gate keys on for
MAE's own in-process sessions), ADR-046 (the CLI harness this gate's PSK handshake exists to
authenticate).
**Relates to:** `SECURITY.md` (the existing, already-documented same-user trust boundary this
ADR reuses rather than trying to exceed).

## Context

A user may keep a knowledge base whose contents must never be sent to a hosted/cloud AI provider
(Claude, OpenAI, Gemini, DeepSeek APIs) — only a local, self-hosted model (Ollama) should be
allowed to read or write it. The concrete example motivating this: an external MCP client driven
by Claude (Claude Code CLI, connected over MAE's tool socket exactly the way this repository's own
development sessions connect) reading a sensitive KB's contents into a request that leaves the
machine.

`execute_tool()` (`crates/ai/src/executor/tool_dispatch.rs`) is reached from exactly three sites:
MAE's own embedded/`delegate()` session path, the external-MCP-client path
(`ai_event_handler.rs::handle_mcp_request`), and the self-test path. For the **embedded** path,
`editor.ai.provider: String` is authoritative — MAE constructed that provider itself, it cannot be
lying about which model is driving it. For the **external MCP** path, there is, as of today, **no
requester identity at all** by the time a `tools/call` reaches dispatch —
`McpToolRequest{tool_name, arguments, reply}` carries nothing about who's asking. A self-reported
"I am Ollama" field at `initialize` time would be spoofable by any client and is not a real
boundary on its own — precisely the gap that matters for the motivating example, since an external
MCP client is, structurally, unverifiable without some additional mechanism.

MAE already has two mechanisms that look superficially applicable and are both the wrong fit:

- `kb_set_encryption`/`kb_set_policy` — a signed, anti-downgrade op-log living inside a *shared*
  KB's collaborative CRDT doc (`shared/sync/src/membership.rs`), built for a different threat
  (an untrusted relay/daemon among collaborating peers) and structurally absent for any KB that
  isn't shared. A private, local-only sensitive KB has no such doc to hang a flag on.
- `shared/mcp/src/auth.rs`'s `PskAuth`/`KeyAuth` — already implemented, tested, but currently wired
  only into the daemon's TCP collab listener. The local Unix tool socket
  (`shared/mcp/src/lib.rs::McpServer::run()`) has **no handshake at all** today — exactly matching
  `SECURITY.md`'s already-documented posture ("protected by filesystem permissions only... any
  process running as the same user can connect... no per-client authentication").

## Decision

1. **A new, deliberately non-colliding policy enum.** `shared/kb/src/federation.rs` already has
   `KbScope::LocalOnly` meaning "only the primary/local KB instance participates in a federated
   query" (a network-locality axis). A provider-trust axis needs its own name:
   ```rust
   pub enum AiResidency { Open, LocalModelsOnly } // #[default] Open
   ```
   Stored as `KbInstance.ai_residency` and `KbRegistry.primary_ai_residency` (mirroring the
   existing `primary_shared`/`primary_collab_id` no-row-for-primary pattern), persisted via the
   existing `KbRegistry::update()` — not the heavier signed op-log, which doesn't apply to
   non-shared KBs and solves an unrelated threat. Freely toggleable: this is one local user's own
   KB, not a multi-peer trust problem, so no anti-downgrade protection is needed.

2. **Embedded/delegate path: gate on the authoritative value directly.** No new plumbing —
   `editor.ai.provider` is compared against the target KB instance's `ai_residency` at the same
   call sites that already run `execute_tool()`.

3. **External MCP path: reuse `PskAuth`, don't invent a new mechanism.** MAE writes a random PSK
   to `/tmp/mae-{pid}.psk` (mode 0600) at MCP-server startup, alongside the existing
   `/tmp/mae-{pid}.sock` (same lifecycle, same discovery pattern). The Unix listener performs
   `PskAuth::server_handshake` before entering the per-connection request loop, if a PSK exists for
   this run — absent, it's today's unauthenticated behavior, unchanged (backward compatible; an
   unmodified Claude Code CLI connects exactly as it does today, just without residency-gated KB
   access). A client that completes the handshake may declare `declaredProvider` at `initialize`;
   the server trusts that declaration **only if the handshake succeeded** — an unauthenticated
   client's self-report is logged but never trusted for gating.

4. **Thread requester identity through dispatch.** `McpToolRequest` gains a `requester:
   RequesterContext { psk_authenticated: bool, declared_provider: Option<String> }`, populated
   from the session at the `"tools/call"` handler. For a `kb_*` content-touching tool targeting an
   instance with `ai_residency = LocalModelsOnly`, `handle_mcp_request` requires
   `requester.psk_authenticated && requester.declared_provider` is in a small local-provider set
   (`["ollama"]` today) before forwarding to `execute_tool()` — otherwise it returns a denial
   result, same shape as the existing permission-denied branch.

5. **Every `kb_*`/`help_open` tool is explicitly classified into one of several residency shapes
   (`crates/mae/src/ai_residency.rs`) — not a hand-maintained allowlist.** An earlier
   implementation used two flat arrays (single-instance tools checked precisely; a fixed set of
   "federated-scan" tools denied outright); any tool not listed in either silently fell through
   to *allow*. This was found, during #350/#351's investigation, to have let nine tools
   (including `kb_raw_query` — arbitrary Datalog against the primary store — and `kb_graph` — an
   explicitly federated BFS walk) go completely ungated. The classification is now exhaustive and
   fails **closed**: a `kb_*`/`help_open` tool with no explicit classification is denied, not
   allowed, and a CI test (`every_kb_tool_and_help_open_is_explicitly_classified`) fails loudly if
   a new tool is ever added without one.
   - **`SingleTarget`** (`kb_get`/`kb_update`/`kb_add_link`/`kb_delete`/`kb_restore`/
     `kb_links_from`/`kb_related`/`kb_shortest_path`/`kb_neighborhood`/`kb_history`/
     `kb_preview_show`/`kb_create`/`kb_set_role`/`kb_reimport`/`help_open`) resolve one (or two,
     for `kb_add_link`'s `src`/`dst`) owning instance from their arguments and allow/deny
     precisely.
   - **`PrimaryOnly`** (`kb_raw_query`/`kb_view_query`) only ever read the primary store —
     checked against the primary's residency alone. Both run arbitrary Datalog with no
     schema-level per-row node-identity, so they're hard-denied outright rather than filtered
     (see Decision §7).
   - **`PrimaryOnlyFilterable`** (`kb_agenda`) also only ever reads the primary store — checked
     against the primary's residency alone — but its `store.agenda_query` results ARE real
     `Node`s, so the gate allows the call through and `execute_kb_agenda` post-filters its own
     materialized results instead of denying outright (#358, Decision §7). (`kb_agenda` was
     originally grouped with the federated-scan tools below; that was inaccurate — its
     implementation never reads `editor.kb.instances` at all, so grouping it there over-blocked
     it whenever an *unrelated* registered instance was restricted.)
   - **`ScopedFederatedScan`** (`kb_vector_search`, a permanent stub today with no real results
     to filter yet) scans across `editor.kb.instances` but accepts a `scope` argument (falling
     back to the `kb_search_scope` option) that names exactly which KB(s) participate — residency
     is checked only against KBs **within the resolved scope**, not every registered KB, and
     denied outright if that check fails. This is the `scope`-based escape hatch this ADR's error
     text and the Verification section below always intended (a call explicitly scoped away from
     a restricted KB must not be blocked by that KB's policy) — an earlier implementation checked
     *every* registered KB regardless of `scope`, which is the literal bug reported as #351.
   - **`ScopedFederatedScanFilterable`** (`kb_search`/`kb_search_context`) resolves `scope` the
     same way, but their results ARE real `(Option<String>, Node)` pairs — the gate allows the
     call through unconditionally and the tool impl post-filters its own materialized results
     instead of denying outright (#358, Decision §7).
   - **`UnscopedFederatedContent`** (`kb_graph`/`kb_graph_view_open`/`kb_graph_view_refresh`/
     `kb_graph_view_state`/`kb_list`/`kb_health`/`kb_id_audit`/`kb_links_to`) scan across multiple
     KB instances with **no** `scope` argument to narrow them, and their result shapes don't
     consistently carry per-result instance attribution — `kb_search` tags each hit
     `"instance": <name>`, but these tools mostly don't. Rather than ship a filter that silently
     no-ops on the tools where the shape doesn't match — false confidence in coverage — these are
     **denied outright** whenever any registered KB (or the primary) is `LocalModelsOnly` and the
     requester isn't local, naming the restricted KB and suggesting a `ScopedFederatedScan` tool
     (e.g. `kb_search` with an explicit `scope`) instead. Coarser than fine-grained per-result
     filtering but categorically safe (never leaks restricted content) and honest about the
     tradeoff (a denial, not a silently-incomplete success). Fine-grained per-tool filtering
     remains real, separate follow-up work once these tools carry consistent instance attribution.
   - **`NonContent`** (`kb_instances`/`kb_sync_status`/membership+policy+sharing-lifecycle tools/
     pure graph-view camera-state tools/etc.) never return node titles/bodies/links — never gated.

6. **New tool + human parity.** `kb_set_ai_residency` (Write tier), with a matching editor command
   and Scheme primitive, following the existing `kb_set_policy`/`command_kb_set_policy` precedent
   (CLAUDE.md principle #7 — every AI-facing capability gets a human-facing equivalent).

7. **Seeded/built-in content is exempt from `LocalModelsOnly` gating (#358).** MAE's own
   compiled-in manual content (help docs, commands, concepts) is identical on every install and
   never sensitive — a user restricting `primary` to protect their own notes must not also lose
   the built-in help system as an unintended side effect. Exemption keys on `Node::source ==
   Some(NodeSource::Seed)` (already stamped once at startup by `KnowledgeBase::stamp_source`, no
   new tagging infrastructure), checked by `mae_core::ai_residency::is_residency_exempt`/
   `filter_residency_exempt`/`filter_residency_exempt_primary` — living in `crates/core`, not
   `crates/mae`, purely because the `mae` package has no `[lib]` target and is therefore
   unreachable from `mae-ai`'s tool implementations (a Rust crate-graph constraint, not a
   conceptual split; `mae-core` is the closest crate both `mae` and `mae-ai` already depend on).
   Applied wherever a real `Node` is already in hand before a decision is made: `SingleTarget`'s
   `resolve_restricted_label` checks it directly on the node it already resolves; the new
   `PrimaryOnlyFilterable`/`ScopedFederatedScanFilterable` shapes (Decision §5) allow the call
   through unconditionally and the tool impl (`execute_kb_agenda`/`execute_kb_search`/
   `execute_kb_search_context`) post-filters its own materialized results instead of denying the
   whole call. Tools whose result shape has no per-row node-identity to filter (`kb_raw_query`/
   `kb_view_query` — arbitrary Datalog) or that are structurally incapable of ever surfacing seed
   content (`kb_id_audit` — only ever considers nodes with `source_file.is_some()`, which seed
   nodes never have) stay unchanged, documented in `ai_residency.rs`'s module doc rather than
   silently left as a gap. Remaining tools that are real, feasible candidates for the same
   exemption but need deeper plumbing (`kb_related`, `kb_graph`, `kb_graph_view_state`, `kb_list`,
   `kb_links_to`, `kb_shortest_path`, `kb_neighborhood`, `kb_links_from`, `kb_health`,
   `kb_history`/`kb_restore`) are a tracked follow-up, not silently deferred.

## Consequences

**Closes the motivating example concretely.** An unmodified Claude Code CLI session (or any other
external MCP client) has no PSK and is therefore denied `LocalModelsOnly` KB content by
construction — not by a fixable-later bug, by design. A first-party local harness (ADR-046) that
completes the handshake and honestly declares `ollama` is correctly granted access, which is the
entire point: the gate must not block the very tool meant to do local-model KB work.

**Honest scope limit, stated explicitly rather than implied.** This is a same-machine,
well-behaved-client guardrail. It prevents *accidental* cross-provider exposure from a client that
has no PSK and isn't trying to defeat the check. It is **not** a defense against a hostile process
already running as the same OS user — such a process could read the 0600 PSK file as trivially as
it could dial the socket today, which is not a new gap: `SECURITY.md` already documents this exact
boundary for the shell blocklist ("defense in depth, not a sandbox"). This ADR does not raise
MAE's security ceiling; it closes a specific, real, previously-ungated path within the ceiling
that already exists.

**A documented, accepted gap versus ADR-045's letter.** ADR-045 §3 describes the four-pillar
guardrail layer as applying "by reliability tier... not by provider name," which could be read as
universal. ADR-046 places that layer inside the new CLI harness, not MAE's core. The practical
result: after the `delegate()` provider-dispatch bug fix (a prerequisite of this same milestone),
an Ollama-primary session's **embedded** sub-agents are correctly provider-dispatched and correctly
subject to *this* ADR's residency gate, but do **not** get the harness's guardrail-layer
protection. Accepted because the embedded surface is frozen per ADR-046; `GuardrailProvider`
wraps the same `AgentProvider` trait every provider already implements, so extending it to the
embedded path later is additive, not a redesign, if this trade-off is ever revisited.

## Alternatives rejected

- **Reuse the `kb_set_encryption`/`kb_set_policy` signed op-log.** Rejected — that mechanism solves
  a different threat (an untrusted relay/daemon among *shared*-KB peers) and structurally doesn't
  attach to a non-shared, purely local KB, which is the common case this ADR needs to cover.
- **Trust self-reported `clientInfo`/`declaredProvider` with no cryptographic binding.** Rejected —
  the entire point is that an external client's self-report is unverifiable without it; trusting
  it unconditionally would not close the motivating example at all (a misbehaving or simply
  differently-configured client could claim anything).
- **A bespoke local-trust file scheme instead of `PskAuth`.** Rejected — `PskAuth` already exists,
  is already tested, and is the exact "mutual proof of a shared secret" primitive this needs;
  inventing a second, parallel local-auth mechanism duplicates existing infrastructure for no
  benefit (principle #8).
- **Silently drop restricted hits from a federated query's results, no marker.** Rejected outright
  regardless of the v1/fine-grained question below — the model must be able to tell its evidence
  was incomplete, never confidently under-perform on a partial result it can't see was partial.
- **Ship a best-effort per-tool result filter now, accepting it silently no-ops on tools whose
  shape doesn't match.** Rejected after implementation surfaced the shape inconsistency — a
  filter that appears to work but silently doesn't cover every tool is worse than an honest,
  coarser deny; `UnscopedFederatedContent` tools deny the whole call instead (see Decision §5)
  until the fine-grained version can be built correctly across all of them.
- **Keep the two-array (single-instance / federated-scan) classification instead of an
  exhaustive, fail-closed match.** Rejected after #350/#351's investigation found nine tools
  silently ungated because they were never added to either array — an allowlist that fails
  *open* for anything unlisted cannot be trusted to stay correct as tools are added over time.
  The exhaustive `classify_kb_tool` match plus a CI test enforcing every tool has an explicit
  arm (Decision §5) makes the same drift a loud build failure instead of a silent gap.
- **Deferring the PSK-handshake half to a later fast-follow, shipping only the embedded-path
  check now.** Considered and rejected after review — the deferred half is exactly the motivating
  example (an external Claude Code CLI session), the actual new plumbing is small (one field on
  `McpToolRequest`, one check in `handle_mcp_request`, reusing already-tested `PskAuth`), and
  shipping only the embedded half would let this be mistaken for "the residency gate" when it
  would not cover the case the user actually named.

## Verification

- Embedded session, `editor.ai.provider="claude"`, target KB `ai_residency=LocalModelsOnly` →
  denied. Same session, `editor.ai.provider="ollama"` → allowed.
- External MCP client with no PSK (today's Claude Code CLI, unmodified) → denied regardless of any
  self-declared provider field it might send; the declaration is logged, never trusted.
- External MCP client presenting a valid PSK and `declaredProvider: "ollama"` → allowed.
- A `ScopedFederatedScan` call (`kb_search`/`kb_search_context`/`kb_vector_search`) whose
  resolved `scope` excludes every `LocalModelsOnly` KB is **allowed**, even when some other,
  out-of-scope registered KB is restricted — the `scope`-based escape hatch this ADR's error
  text always promised (#351's fix).
- The same call with an unscoped (or `scope="all"`) request, when any registered KB (or the
  primary) is `LocalModelsOnly` and the requester isn't local, is denied outright with a message
  naming the restricted KB — never a silently-incomplete success.
- An `UnscopedFederatedContent` call (`kb_graph`/`kb_list`/`kb_health`/`kb_id_audit`/
  `kb_links_to`/the content-bearing graph-view tools), which has no `scope` to narrow it, is
  denied outright under the same condition (v1's conservative simplification — see Decision §5).
- A `PrimaryOnly` call (`kb_agenda`/`kb_raw_query`/`kb_view_query`) is denied only when the
  *primary* KB itself is restricted — an unrelated restricted federated instance never blocks it.
- Every real `kb_*`/`help_open` tool name resolves to an explicit classification
  (`every_kb_tool_and_help_open_is_explicitly_classified`); an unrecognized `kb_*`/`help_open`
  name is denied conservatively rather than silently allowed
  (`unclassified_kb_prefixed_tool_denied_conservatively`).
- `kb_set_ai_residency` is reachable identically via the AI tool, the editor command, and the
  Scheme primitive (human/AI parity check).
- A seeded node stays reachable via `kb_get`/`help_open`/`kb_search`/`kb_search_context`/
  `kb_agenda` even when `primary` is `LocalModelsOnly` and the requester isn't local (#358). A
  genuinely user-authored (non-seed) node in that same restricted `primary` is still denied for
  `kb_get`/`help_open`, and still filtered out of `kb_search`/`kb_search_context`/`kb_agenda`
  results — the exemption does not broaden past seed content.
- `kb_raw_query`/`kb_view_query` remain denied outright when `primary` is restricted, regardless
  of whether the queried content would otherwise be seed-only — confirms they did not silently
  get the filterable treatment (#358).
