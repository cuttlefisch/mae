# AI-agent friction audit: config surfaces and the Ask state

Findings from using MAE's MCP server as a working tool — an external AI agent (Claude Code)
driving a real org-roam KB through `mcp__mae-editor__*` for several hours. Every failure
below was hit in the course of doing unrelated work, not by probing for defects.

Verified against `main` at **`a85886f9`** in a clean shallow clone. Every claim was
re-derived from source in that clone; the reconnaissance pass that motivated this audit ran
against a feature branch and its findings were **not** carried over — see
[Reconciliation](#reconciliation), which records three hypotheses that did not survive.

Nothing here proposes code changes. Two decisions follow from it:
[ADR-095](adr/095-mcp-elicitation-carries-the-ask-state.md) (§C) and
[ADR-096](adr/096-scheme-is-the-only-editor-config-surface.md) (§A).

## Headline

Three failures, in descending order of consequence:

1. **`mae --init-config` writes a config file that contradicts the shipped security
   default** and instructs the user toward the exact posture ADR-090 rejected. (§B1)
2. **External MCP is the only surface with a human available that cannot ask.** The
   `initialize` handshake already carries the client's declared ability to be asked, and
   MAE never reads it. (§C)
3. **Config provenance is computed and then discarded**, so no tool can tell an agent — or
   a human — which of four surfaces set a value. (§A2)

---

## A. The config surface

### A0. What the model actually is

Four surfaces set configuration, in this order:

```
config.toml  →  init.scm  →  module autoloads.scm  →  config.scm     (+ env vars)
```

- `config.toml` — `config::load_config()`, `crates/mae/src/config.rs:301`; called from
  `crates/mae/src/bootstrap.rs:534`, `:641` and `crates/mae/src/main.rs:596`
- `init.scm` → modules → `config.scm` — `crates/mae/src/bootstrap.rs:1013-1015`

**This is documented**, and the first version of this audit was wrong to say otherwise.
`assets/manual/concept-options.org:41-42` states it plainly:

> `init.scm` is the primary config surface; `config.toml` is legacy bootstrap (AI provider
> + theme for pre-Scheme loading). `:set-save` persists to `init.scm`.

That single sentence answers the question that cost this session two wrong answers to the
user. The findings below are therefore **not** "MAE is underdocumented." They are about the
gap between that sentence and everything an agent actually touches.

### A1. `config.toml` routes to a help topic that does not exist

`config.rs:1268` emits, into every generated config file:

```
# Full docs: :help config   (inside the editor)
```

There is no `config` topic. `assets/manual/` has 237 topics and none is named `config`; the
topic that actually answers the question is `:help options` (`concept-options.org`), which
`assets/manual/index.org:19` lists as *"configuring MAE from Scheme"* — phrasing that reads
as Scheme-specific and not as the authoritative account of the whole model.

So the file whose status is in question points at documentation that doesn't exist, while
the documentation that would resolve it is indexed under a name that sounds like it covers
only the other surface.

### A2. Provenance is computed, then thrown away

`config::resolve_permission_policy` (`config.rs:728-741`) determines exactly which surface
supplied the permission tier:

```rust
let (tier_str, source) = match std::env::var("MAE_AI_PERMISSIONS").ok() {
    Some(v) => (v, "MAE_AI_PERMISSIONS"),
    None => match config.ai.auto_approve_tier.clone() {
        Some(v) => (v, "[ai] auto_approve_tier in config.toml"),
        None => (/* … */, "built-in default"),
    },
};
```

`source` is used **only** in the error message for an unparseable tier (`config.rs:744`).
On the success path it is dropped, and `bootstrap.rs:590` keeps only the policy.

The consequence is felt through the tools. `get_option` returns `name`/`value`/`default`/
`doc` and no source — so an agent reading `ai_model = "mistral:7b"` has no way to learn that
`config.toml` says `qwen3.5:9b` and lost. `ai_permissions` reports the active tier and the
tier ladder, but not which surface set it or that three others were consulted.

This is a small gap, not a missing subsystem: the value already exists at the point where
the answer is decided.

### A3. `ai_tier` is a guarded decoy

`get_option "auto_approve_tier"` answers `Unknown option`. That is misleading twice over.

First, the setting does exist under a different name. `crates/core/src/options.rs:180-182`
registers:

```rust
opt!("ai_tier", &["ai-tier"],
    "Current AI permission tier (ReadOnly, Write, Shell, Privileged)",
    OptionKind::String, "ReadOnly", Some("ai.auto_approve_tier"),
    &["ReadOnly", "Write", "Shell", "Privileged"]),
```

Right `config_key`, right valid values, and MAE guards it like the real thing:
`PERMISSION_TIER_OPTION = "ai_tier"` (`crates/ai/src/tools/authorization.rs:109`) drives an
argument-sensitive escalation so that `set_option ai_tier` demands Privileged, closing the
self-escalation path (`:271-278`).

Second — and this is the actual finding — **setting it changes nothing.**
`crates/core/src/kb_seed/terminology.rs:166` states it "currently updates only the status-bar";
ADR-090:174 confirms, deferring the fix to ADR-084 D7 because making it live "means a
live-mutable policy shared between the main thread and the spawned `AgentSession` task."

So MAE ships an option that is named like the permission control, mapped to the permission
control's config key, protected as if it were the permission control, and inert. An agent
that recovered from the `Unknown option` error by finding the right name would be walked into
a stronger false belief than the one it started with. That is worse than an absent setting.

### A4. `config_key` looks like a mapping and is documentation

`OptionDef` carries `config_key: Option<Cow<'static, str>>` — *"TOML path in config.toml, if
persistable"* (`crates/core/src/options.rs:56-57`). 147 of 233 registered options declare one,
which reads as a complete registry↔TOML bridge.

Nothing reads config.toml through it. Its only non-test consumer is
`crates/core/src/kb_seed/mod.rs:514`, which renders a `**Config key:** \`…\`` line into seeded
help documentation. The actual TOML→option wiring is hand-written elsewhere
(`apply_app_config`, `resolve_ai_config_with_scheme`, …).

Because it is an unenforced annotation, it has drifted. Options declare `config_key`s in
**four TOML sections that `Config` does not parse** — `[babel]` ×7, `[mcp]` ×2, `[spell]` ×1,
`[format]` ×1. `Config` (`crates/mae/src/config.rs:27-50`) has `ai`, `editor`, `agents`,
`lsp`, `performance`, `org`, `collaboration`, `kb`, `daemon` and nothing else. So MAE's own
generated help documents eleven config.toml keys that are silently ignored if a user writes
them.

(Separator style is *not* a finding: `kb-graph-view-mode` and `kb_graph_view_mode` both
resolve to the same canonical option. Tolerant, and working as intended.)

### A5. Shadowing is silent

On the audited machine, `config.toml:10` sets `model = "qwen3.5:9b"`; `init.scm:74` sets
`ai-model` to `mistral:7b`; the running value is `mistral:7b`. Nothing warns. The dead line
is committed in a dotfiles repo, where it reads as a statement of fact.

Given A0 — config.toml is *legacy bootstrap* — this is expected behaviour. It is still a
trap, because the losing file gives no sign it lost.

### A6. `audit_configuration` reports two of the four surfaces

`crates/ai/src/tool_impls/editor_tools.rs:953-969` populates `init_files` with exactly two
entries: `<config-dir>/mae/init.scm` and `<cwd>/.mae/init.scm`. Neither `config.toml` nor
`config.scm` is ever included.

The tool's own description tells agents to *"Call FIRST when diagnosing config problems or
when you need absolute paths to config files."* An agent that follows that instruction
receives a config-file list that omits the file holding `auto_approve_tier`.

This is the single highest-leverage item in section A: it is the designated entry point,
and it under-reports the surface by half.

---

## B. The generated config contradicts the shipped default

### B1. `--init-config` writes a template that is wrong and harmful

`default_config_template()` (`config.rs:1276`), written to disk by `--init-config`
(`config.rs:356`) and printed by `--print-config-template` (`crates/mae/src/cli.rs:145`),
contains:

```
# Permission tier for AI/MCP tool execution.
# Tiers: "readonly", "write", "shell" (default), "privileged"
# Env override: MAE_AI_PERMISSIONS=full
# auto_approve_tier = "shell"
```

The shipped default is **not** `shell`. `crates/ai/src/tools/categories.rs:405` sets
`auto_approve_up_to: PermissionTier::ReadOnly`, per ADR-090 D5 — and the comment
immediately above it (`:402-403`) reads:

> Raising it back to `Shell` re-creates the fail-open default ADR-084 D4 identified; don't,
> without reading ADR-090's "Alternatives considered".

Meanwhile the template states Shell *is* the default and shows the user the line to set it.

ADR-090's *Alternatives considered* anticipated this outcome precisely, as the reason for
rejecting a tier drop without an Ask state:

> the predictable result is that users set `auto_approve_tier = "shell"` in config —
> restoring the same posture while adding the false comfort of a configured value.

MAE ships a config generator that produces that outcome by default. This is the one finding
in this audit that is a plain bug rather than a friction point, and it is worth fixing
independently of everything else.

`--print-config-template` is also the natural thing for an agent to consult when asked
"how do I configure this," which is how it was found.

---

## C. The Ask state does not reach external MCP

### C1. The surface map

ADR-090 established `Decision { Allow, Ask, Deny }`
(`crates/ai/src/tools/decision.rs:62-71`). Dispatch returns `NeedsApproval` for `Ask`
rather than deciding (`crates/ai/src/executor/tool_dispatch.rs:121-129`), leaving the choice
to each surface. Every non-test consumer:

| Surface | Site | Behaviour | Correct? |
|---|---|---|---|
| `mae-agent` TUI | `crates/agent-cli/src/main.rs:508` | prompts `y`/`a`/`n` | ✅ |
| Embedded session | `crates/ai/src/session/handle_prompt.rs:45` | prompts via `ConfirmToolCall` | ✅ |
| Embedded race guard | `crates/mae/src/ai_event_handler.rs:264` | denies | ✅ — a mid-turn policy race, nothing ran |
| `--self-test` | `crates/mae/src/terminal_loop.rs:822` | denies | ✅ — headless by definition |
| **External MCP** | `crates/mae/src/ai_event_handler.rs:1223` | **denies** | ❌ — a human *is* present |

Four of five are right. External MCP is the only surface where a human is sitting in front
of a client that could ask them, and the answer is a hard denial.

This is not undiscovered. ADR-090's *Still open* names it — *"An interactive `Ask` for
external MCP"* — and `SECURITY.md:50` and `docs/EXTERNAL_EDITOR_MCP_PAIRING.md:324` both
document it as deliberate, on the grounds that *"MAE implements no MCP elicitation, and the
requesting client is not the local human."*

### C2. The client already says it can be asked

The first half of that justification is now falsifiable.

- MAE speaks MCP **`2025-11-25`** (`shared/mcp/src/protocol.rs:11`) — the revision that
  specifies elicitation — and accepts four versions back (`:15`).
- Claude Code's real handshake, captured as a regression fixture at
  `shared/mcp/src/lib.rs:3116`, declares:
  ```json
  "capabilities": { "roots": {}, "elicitation": { "form": {}, "url": {} } }
  ```
- The server's `initialize` handler (`shared/mcp/src/lib.rs:767-832`) reads
  `protocolVersion`, `clientInfo`, `declaredProvider`, `permissionCeiling`, and
  `toolCategoryAllowlist` — **and never reads `capabilities`.** The only `capabilities` in
  that range is MAE's own `ServerCapabilities` in the response.
- `ClientSession` (`shared/mcp/src/session.rs:30-52`) has no field to hold it.

So the client states it can carry a prompt to its human, in the same object MAE is already
parsing, and MAE drops it on the floor.

### C3. The asymmetry

MAE already trusts three self-declared client parameters on that exact code path:
`declaredProvider` (gated on PSK auth, `lib.rs:790-800`), `permissionCeiling` (ADR-051,
`:811`), and `toolCategoryAllowlist` (ADR-056, `:823`). The latter two are trusted from any
client precisely because they can only *narrow* a session's authority.

The result is one-directional: a client can restrict itself, and can never be asked to
approve anything. There is no agent-facing counterpart either — `request_tools` grants tool
*schemas*, not permission.

### C4. What it costs

With no Ask on this path, the only way to make a paired editor work is to pre-authorize
statically. `docs/EXTERNAL_EDITOR_MCP_PAIRING.md:326` says exactly that: a paired-editor
deployment *"needs an explicit `auto_approve_tier`."* Combined with §B1's template, the path
of least resistance for a new user is a permanently elevated MCP session.

In this audit's own session, `kb_reimport` (Shell) and `kb_raw_query` (Privileged) both
dead-ended. The agent could not ask, could not request elevation, and could not tell the
user what to change without first getting the config surface wrong twice. The work stopped.

**Caveat, carried forward from ADR-090:** Ask is a usability mechanism that makes a
restrictive default affordable — not a security control. Users approve ~93% of permission
prompts. Adding elicitation must not be described as hardening.

---

## Recommendations

Ordered by value per unit of risk. None is implemented here.

| # | Change | Rationale |
|---|---|---|
| R1 | Correct `default_config_template()` — `"readonly" (default)`, and drop the `auto_approve_tier = "shell"` example | §B1. A shipped generator contradicting the shipped default, in the direction ADR-090 rejected |
| R2 | Add `config.toml` and `config.scm` to `audit_configuration`, with an `exists` flag and load order | §A6. It is the documented first stop and reports half the surface |
| R3 | Return `source` from `resolve_permission_policy`; surface it in `ai_permissions` and add a `source` field to `get_option` | §A2. The value is already computed and discarded |
| R4 | Make `Unknown option` namespace-aware: if the name is a known config.toml key, say so and name the section | §A3. Turns a dead end into a signpost |
| R5 | Add a `config` help topic (or alias `config` → `options`), and restate `concept-options.org:41-42` in `docs/` | §A1. The routing target is missing, not the content |
| R6 | Warn when a `config.toml` key is shadowed by a later surface | §A5. Prevents dead config accreting in dotfiles repos |
| R7 | Either make `ai_tier` reach the enforced policy (ADR-084 D7) or mark it read-only and say so in its `doc` | §A3. Today it is a decoy that MAE guards as if it were real |
| R8 | Assert `OptionDef.config_key` against the `Config` schema in a test | §A4. Eleven declared keys point at sections `Config` cannot parse; nothing catches that |
| R9 | Negotiate and honour `capabilities.elicitation` | §C. See ADR-095 |

R1 is a bug fix and should not wait for the rest. R7 and R8 are the two places where MAE
currently tells a confident untruth, which is the failure mode most costly to an agent —
a missing answer prompts a search, a wrong one does not.

---

## Reconciliation

The reconnaissance pass produced three claims that **did not survive** re-derivation. They are
recorded here rather than quietly dropped, because `AUDIT_TAIL_TRIAGE.md` documents prior
audit citations that "no longer match current line numbers or, in two cases, were never
true," and an audit that hides its own misses has no standing to report that.

- **"No documentation describes the config model."** False. `concept-options.org:41-42`
  describes it correctly and concisely. Replaced by §A0 and §A1, which are about routing and
  discoverability instead.
- **"MAE's own doc comments call this a 'three-file model', omitting config.toml."**
  Misleading. The phrase at `bootstrap.rs:1761` describes the Scheme chain specifically, and
  "three-file model" is an established term for the *module* layout
  (`crates/core/src/kb_seed/concepts.rs:1088`, after Doom Emacs). Dropped.
- **"The option registry and config.toml keys are two disjoint namespaces with nothing
  mapping between them."** False, and it was hiding something worse. `OptionDef.config_key`
  maps them (§A4), and `auto_approve_tier` *is* registered — as `ai_tier` (§A3). The real
  findings are that the mapping is unenforced documentation and the option is inert.

The first two errors shared a cause: the first pass inferred absence from a failed `grep`
over `docs/`, without checking `assets/manual/`. Absence of evidence was read as evidence of
absence, in a repo that keeps its user-facing documentation somewhere the search did not
reach. The third had the same shape at the level of code — a tool error (`Unknown option`)
was taken as ground truth about the system rather than as one tool's answer.

Findings that *were* confirmed and sharpened: A2 (provenance discarded, not merely absent),
A5, A6, C1–C3. Findings new in this pass and absent from reconnaissance: **B1** — the
strongest item in the audit — the `ai_tier` decoy (A3), the `config_key` drift (A4), the
complete surface map in C1, and the trust asymmetry in C3.

## Coverage and limits

Deliberately not examined: the `write` tier in practice, the Scheme ambient-tier path
(ADR-084 D3), whether these ambiguities affect non-AI consumers, and the daemon's config
handling. The permission review followed `Decision::Ask` and `ExecuteResult::NeedsApproval`
to every non-test consumer, so §C1's surface map is complete for that path; it does not
cover surfaces that bypass `tool_dispatch` entirely, if any exist.

Section A findings A2, A3 and A5 were additionally observed live against a running instance, so they
hold independently of any single source citation.
