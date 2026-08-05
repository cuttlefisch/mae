# ADR-096: Scheme is the only editor config surface

**Status:** Proposed. Phased (0–4), each phase independently shippable behind a gate that
fails before the change. **Phase 1 is blocked on ADR-084 D7** and says so.
**Extends:** ADR-084 D7 (making the `ai_tier` option reach the enforced policy — this ADR
cannot own the permission tier from Scheme until that lands).
**Relates to:** ADR-090 (the tier this governs; its D5 default is what the current generated
template contradicts), ADR-089 (workspace trust for project-local init — which this ADR
promotes from defence-in-depth to load-bearing).
**Evidence:** `docs/AI_AGENT_FRICTION_AUDIT.md` §A.

Note the title: the **editor** config surface. The scoping is load-bearing — see
*Explicitly out of scope*.

## Context

MAE has four surfaces that set editor configuration, applied in this order
(`crates/mae/src/bootstrap.rs:1013-1015`, `crates/mae/src/main.rs:596`):

```
config.toml  →  init.scm  →  module autoloads.scm  →  config.scm     (+ env vars)
```

The intended relationship is already documented. `assets/manual/concept-options.org:41-42`:

> `init.scm` is the primary config surface; `config.toml` is legacy bootstrap (AI provider +
> theme for pre-Scheme loading). `:set-save` persists to `init.scm`.

The problem is that "legacy" has never been made true. config.toml still holds
security-relevant settings, still ships a generator, and still accumulates keys that lose
silently. The audit records what that costs an agent: two wrong answers about where config
lives, a `Unknown option` error that denies the existence of a setting that exists, and an
option (`ai_tier`) that is named, mapped, and guarded like the permission control while being
inert.

**The migration is mostly already built.**

- `resolve_ai_config_with_scheme` (`crates/mae/src/config.rs:568-579`) already implements
  `env > scheme > file > default` for AI config. The pattern exists and is in production.
- `set-option-save!` / `save_option_to_init`
  (`crates/scheme/src/runtime/editor_ops.rs:150-153`) persist to **init.scm**, and the MCP
  `set_option` `persist` flag uses the same path (`crates/ai/src/tool_impls/editor_tools.rs:170`).
  The write path already points at Scheme; only the read path still favours TOML.
- `OptionDef.config_key` (`crates/core/src/options.rs:56-57`) already annotates which options
  correspond to which TOML paths — 147 of 233 registered options — so the migration inventory
  is machine-derivable rather than a manual audit.
- **There is no circular dependency**, which is the objection this proposal would otherwise
  founder on. In `main.rs`: `load_config()` `:596` → `apply_app_config` `:601` →
  `SchemeRuntime::new()` `:605` → `load_init_file` `:630` → `setup_ai` **`:717`**. Since
  `resolve_permission_policy` runs inside `setup_ai` (`crates/mae/src/bootstrap.rs:590`),
  init.scm has already executed by the time the policy is resolved — and
  `SchemeAiOverrides::from_editor(editor)` at `bootstrap.rs:642` proves Scheme state is
  readable at that point. **The permission tier's absence from the Scheme layer is
  incidental, not architectural.**
- **The house has done this before.** `daemon/src/config.rs:1-4,648-650` already deprecated
  `state-server.toml` in favour of `daemon.toml` by prefer-new / fall-back-to-legacy /
  auto-migrate. This ADR reuses that shape, adding the explicit deprecation warning the
  original omitted.

## Decision

**Make Scheme the single editor config surface. Reduce `config.toml` to a deprecated
read-only shim, then remove it.**

Phased, because two of the five phases have real prerequisites and one is a bug fix that
should not wait for the others.

### Phase 0 — Correct the generated template

`default_config_template()` (`crates/mae/src/config.rs:1276`), written to disk by
`--init-config` (`:356`) and printed by `--print-config-template`
(`crates/mae/src/cli.rs:145`), currently emits:

```
# Tiers: "readonly", "write", "shell" (default), "privileged"
# auto_approve_tier = "shell"
```

The shipped default is `ReadOnly` (`crates/ai/src/tools/categories.rs:405`, ADR-090 D5),
under a comment reading *"Raising it back to `Shell` re-creates the fail-open default ADR-084
D4 identified; don't."* MAE therefore ships a generator producing precisely the outcome
ADR-090's *Alternatives considered* rejected. Independently correct; do it first.

### Phase 1 — Scheme participates in tier resolution *(blocked on ADR-084 D7)*

Extend `resolve_permission_policy` to consult Scheme state, exactly as
`resolve_ai_config_with_scheme` already does for provider and model. This requires `ai_tier`
to reach the enforced policy, which ADR-090:174 defers to ADR-084 D7 as needing "a
live-mutable policy shared between the main thread and the spawned `AgentSession` task."

**This ADR does not attempt that change and must not be read as approving it.**

### Phase 2 — Scheme primitives for structured config

Options are flat typed scalars (`OptionKind`). Some config is not:

- `LspSection` (`crates/mae/src/config.rs:80-84`) uses `#[serde(flatten)]` over arbitrary
  language keys, each with nested `init_options` maps — the k8s YAML-schema case at
  `:73-77` is the worked example. No `register-lsp-server!` primitive exists; the Scheme
  surface today is `define-option!`, `set-option!`, `set-theme`, `register-ai-tool!` and
  friends, none of which expresses this.
- `[org] agenda_files` is a list.

New primitives are required before these sections can move.

### Phase 3 — Deprecate loudly

config.toml becomes read-only with warnings: one when a key is read that Scheme could own
(naming the Scheme form), and one when a key is shadowed by a later surface — the audit's §A5
case, where a dotfiles-committed `model = "qwen3.5:9b"` loses silently to `init.scm:74`.

### Phase 4 — Migrate and remove

Ship `mae --migrate-config`, emitting equivalent `init.scm` forms; then stop reading
config.toml. `--init-config` and `--print-config-template` are redefined to emit Scheme, or
retired.

## Verification

Gates follow ADR-093's convention: each is a named test, stated as an observation rather than
a review, **failing before the change**, with an explicit oracle separating a real pass from a
trivially-passing one. A phase does not begin until the prior gate is green.

**Gate 0** — the template agrees with the shipped default.
1. `generated_template_states_the_shipped_default` — the template's stated default is
   *derived from* `PermissionPolicy::default().auto_approve_up_to.config_name()`, not a
   literal. *Oracle:* the test fails if someone changes the default without touching the
   template. It asserts agreement between two sources; asserting the string `"readonly"`
   would pass forever and prevent nothing.
2. `template_offers_no_tier_above_the_default` — the commented example does not suggest
   `shell`. *Oracle:* fails on the current tree.

**Gate 1** — Scheme reaches enforcement, and only the auto-approval line.
1. `ai_tier_set_from_scheme_changes_the_enforced_policy` — *Oracle:* asserts on a **dispatch
   outcome** (`decide` returns `Allow` for a tool that was `Ask`), never on the option's own
   value. Reading back what you just wrote is what makes today's decoy look like it works.
2. `tier_precedence_is_env_then_scheme_then_toml_then_default` — mirrors the existing
   `resolve_ai_config_with_scheme` tests. *Oracle:* each layer is tested with the layers below
   it set to a **different** value, so a pass proves precedence rather than coincidence.
3. `scheme_set_tier_cannot_exceed_a_session_hard_ceiling`, alongside the existing
   `approval_can_never_promote_a_deny`. *Oracle:* ADR-051's ceiling and ADR-056's category
   allowlist survive a Scheme-set tier. This is the regression that would matter most, and the
   only one that turns a config change into a security change.

**Gate 2** — the Scheme surface is not lossy.
1. `lsp_server_configured_from_scheme_equals_the_toml_equivalent` — round-trips the nested
   `init_options` fixture. *Oracle:* compares the resolved `LspLanguageConfig` field-wise
   against the TOML-loaded one. Asserting "it parsed" would pass on a surface that silently
   drops `init_options`.
2. `every_config_key_has_a_scheme_equivalent` — enumerates `OptionDef.config_key` against the
   `Config` schema. *Oracle:* the failure message **lists the unmigrated keys**, so the gate
   doubles as the live migration inventory rather than a boolean.

   This gate also catches an existing defect: eleven options declare `config_key`s in
   `[babel]`, `[mcp]`, `[spell]`, and `[format]` — four sections `Config`
   (`crates/mae/src/config.rs:27-50`) does not parse. Those keys are documented to users by
   `crates/core/src/kb_seed/mod.rs:514` and silently ignored if written.

**Gate 3** — deprecation is audible.
1. `reading_a_migratable_toml_key_warns_once_naming_the_scheme_form` — *Oracle:* exactly once,
   and the message contains the replacement form. A warning that only complains teaches
   nothing.
2. `a_shadowed_toml_key_warns` — *Oracle:* fires on shadowing specifically, not on every read.

**Gate 4** — removal is a no-op.
1. `migrate_config_emits_scheme_that_resolves_identically` — *Oracle:* compares **resolved
   editor state**, never generated text. Text comparison would fail on formatting and pass on
   a semantically wrong emission.
2. `removing_config_toml_after_migration_changes_nothing` — *Oracle:* resolved state is
   identical with the file deleted. This is the definition of done.

## Consequences

**Positive.** One surface, so shadowing (§A5), split provenance (§A2), and namespace
confusion (§A3, §A4) stop being possible rather than being separately mitigated. Makes the
manual's existing claim true. Aligns the read path with the write path, which already targets
init.scm.

**Negative / Risks.**

*Config becomes executable code.* This is the real cost.
`crates/core/src/workspace_trust.rs:3-6` states it plainly: Scheme "can spawn processes, and
init files run during bootstrap — *before* any `PermissionPolicy` exists — so the AI
permission tier does not and cannot bound this path." For a user's own
`~/.config/mae/init.scm` this is no worse than any dotfile. For project-local
`$CWD/.mae/init.scm` it is only safe because ADR-089 gates evaluation on an explicit trust
list. **This ADR promotes ADR-089 from defence-in-depth to load-bearing**, and any future
weakening of it must be evaluated against that.

*Data-only reads are lost.* TOML can be read without executing anything; Scheme cannot. Any
future context wanting to know the configured tier without booting a VM loses that option.
`--check-config` already boots Scheme (`crates/mae/src/cli.rs:513`) so it is unaffected today,
but the capability is genuinely given up.

*Documentation debt.* Forty files under `docs/`, `assets/manual/`, and the repo root mention
config.toml, including `SECURITY.md`, `docs/EXTERNAL_EDITOR_MCP_PAIRING.md`, and fourteen
ADRs. This is a work item, not a footnote.

*A long deprecation window.* Phases 3 and 4 are separated by however long users need. The
in-between state — two surfaces, one warning — is strictly more confusing than today for
anyone who ignores warnings.

## Explicitly out of scope

Three neighbouring TOML files are **not** affected, and the ADR's title says "editor" because
of them:

- **`daemon.toml`** — the daemon has no Scheme runtime (no `mae_scheme` dependency) and loads
  `~/.config/mae/daemon.toml` in a separate process. It cannot migrate.
- **`kb-registry.toml`** — machine-written *state* in `~/.local/share/mae/`
  (`shared/kb/src/federation.rs:339-352`), not user config. Worth noting that the
  config/state boundary is not obvious in practice: a user hit exactly this, finding a
  registry-level `primary_ai_residency` flag had drifted to `local_models_only` on its own and
  reasserting it declaratively from init.scm. Making the boundary explicit is a reason to
  state it here, not a reason to move the file.
- **`module.toml`** — a module manifest, part of the module three-file model
  (`crates/core/src/kb_seed/concepts.rs:1088-1091`), not user config.

**`mae-agent` is unaffected.** It also has no Scheme runtime, but takes its tier from a CLI
argument (`policy_for_mode(&args.permission_mode)`, `crates/agent-cli/src/main.rs:288`) and
never reads config.toml. Recorded so that no one assumes this ADR breaks it.

## Alternatives considered

**Keep both surfaces; fix only the observability gaps** (audit R2–R6: report all surfaces in
`audit_configuration`, return provenance from `resolve_permission_policy`, make `Unknown
option` namespace-aware, warn on shadowing). Cheaper and lower-risk, and it leaves every
finding in audit §A permanently *possible* — better-signposted, but still there. Worth doing
regardless as the first increment; insufficient as the endpoint.

**Make config.toml authoritative and demote Scheme.** Rejected: it contradicts
`set-option-save!`'s existing target, the manual's stated intent, and `OptionKind`'s
inability to express what Scheme already can. It would also convert every existing init.scm
into dead configuration.

**Remove config.toml in one step.** Rejected: Phase 1 is blocked on ADR-084 D7 and Phase 2
requires primitives that do not exist. A single-step removal would either strand LSP
configuration or ship a permission tier that Scheme cannot set — the second being a security
regression wearing a refactor's clothes.
