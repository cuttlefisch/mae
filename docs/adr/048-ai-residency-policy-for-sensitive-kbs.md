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

5. **Single-instance tools deny outright; federated-scan tools deny the whole call (v1
   simplification — see below).** `kb_get`/`kb_update`/`kb_add_link`/`kb_delete`/`kb_restore`/
   `kb_links_from`/`kb_links_to`/`kb_related`/`kb_shortest_path`/`kb_neighborhood` resolve one
   (or two, for `kb_add_link`'s `src`/`dst`) owning instance from their arguments and allow/deny
   precisely. `kb_search`/`kb_agenda`/`kb_vector_search`/`kb_search_context` scan across
   `editor.kb.instances` — the original design here was to post-filter restricted hits out of
   the result (with a `residency_filtered` marker so the model knows evidence was incomplete,
   never silent truncation). **Implementation found this isn't safely generalizable in v1**:
   the four tools' result shapes don't consistently carry per-result instance attribution —
   `kb_search` tags each hit `"instance": <name>`, but `kb_agenda` doesn't tag results by
   instance at all today (a separate, pre-existing gap in how it's federated). Rather than ship
   a filter that silently no-ops on the tools where the shape doesn't match — false confidence
   in coverage — v1 conservatively **denies the entire federated-scan call outright** whenever
   any registered KB (or the primary) is `LocalModelsOnly` and the requester isn't local,
   naming the restricted KB and suggesting `kb_get`/an explicit `scope` argument instead. This
   is coarser than originally designed but categorically safe (never leaks restricted content)
   and honest about the tradeoff (a denial, not a silently-incomplete success). Fine-grained
   per-tool filtering is real, separate follow-up work once these tools carry consistent
   instance attribution.

6. **New tool + human parity.** `kb_set_ai_residency` (Write tier), with a matching editor command
   and Scheme primitive, following the existing `kb_set_policy`/`command_kb_set_policy` precedent
   (CLAUDE.md principle #7 — every AI-facing capability gets a human-facing equivalent).

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
  shape doesn't match (`kb_agenda`).** Rejected after implementation surfaced the shape
  inconsistency — a filter that appears to work but silently doesn't cover every tool is worse
  than an honest, coarser deny; v1 denies the whole federated-scan call instead (see Decision
  §5) until the fine-grained version can be built correctly across all four tools.
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
- A federated `kb_search`/`kb_agenda`/`kb_vector_search`/`kb_search_context` call, when any
  registered KB (or the primary) is `LocalModelsOnly` and the requester isn't local, is denied
  outright with a message naming the restricted KB (v1's conservative simplification — see
  Decision §5) — never a silently-incomplete success.
- `kb_set_ai_residency` is reachable identically via the AI tool, the editor command, and the
  Scheme primitive (human/AI parity check).
