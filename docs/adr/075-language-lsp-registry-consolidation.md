# ADR-075: Language/LSP registry consolidation, IaC/DevOps language support (Terraform, Dockerfile, Ansible, Kubernetes, Helm)

**Status:** Accepted, implemented.
**Depends on:** none of the existing numbered ADRs directly — this is foundational
registry work, not built on a prior architectural decision. Loosely relates to ADR-057
(MAE architecture vision) for the general layered-architecture framing, but is not a hard
dependency.
**Relates to:** none — no prior ADR governs "how to add a language" to MAE (confirmed by
grep across `docs/adr/`; the absence itself is part of why this ADR exists rather than
amending an existing one).
**Tracking:** implemented directly (five phases, no separate tracker issue needed — this
is bounded, single-session work, not a multi-week epic).

## Context

The user asked for syntax highlighting and LSP support for IaC/DevOps tooling files
(Terraform, Dockerfile, Ansible playbooks, Kubernetes manifests, Helm charts), and
explicitly asked that this be an opportunity to review and improve the underlying
language/LSP registry code, not just bolt four more languages onto it.

Research before implementation found the registry was already showing real drift:

- **Three hand-duplicated LSP tables**, not generated from one source:
  `crates/mae/src/bootstrap.rs::setup_lsp()`'s `defaults` table (the actual source of
  truth for what MAE launches), `crates/mae/src/doctor.rs`'s health-check table (a
  hand-copied duplicate), and — confirmed via `git blame` — **already silently wrong**:
  doctor checked for binary `"pyright"` while bootstrap actually launches
  `"pyright-langserver"`, and a prior commit that claimed to "reconcile" the two only
  aligned which *package* they both referenced, never touched doctor's binary string.
- **Two copies of `language_id_from_path`** (`crates/core/src/lsp_intent.rs`, the one
  actually used by ~18 call sites, and a dead copy in `crates/lsp/src/client.rs` with zero
  external callers) with inconsistent values for the same input (`.tsx` → `"typescript"`
  in one, `"typescriptreact"` in the other) — confirmed harmless in practice (the dead copy
  was never reached) but a real landmine for anyone editing without knowing that.
- **No `initializationOptions` support anywhere** in MAE's LSP client
  (`LspClient::initialize()` hand-builds the `initialize` request with no
  `initializationOptions` key, ever) — meaning `yaml-language-server`'s real, standard
  mechanism for "this file is a Kubernetes manifest" (`yaml.schemas`, a
  glob-pattern-to-JSON-schema-URL map delivered via `initializationOptions`) could not be
  wired through MAE at all.

Separately, three of the four target languages/dialects turned out to need genuinely
different design treatment, not uniform "add a `Language` variant":

- **Terraform**: a clean, mechanical addition — except `tree-sitter-hcl` 1.1.0 ships no
  highlights query at all, in the published crate or upstream, despite its own
  `Cargo.toml` listing `queries/*` in `include`.
- **Dockerfile**: the canonical `Dockerfile` filename carries no extension at all
  (extension-only detection can never see it), and the natural crate name
  (`tree-sitter-dockerfile`) turned out to be stuck on an incompatible old `tree-sitter`
  0.20 API with no bundled queries either — a maintained fork, `tree-sitter-containerfile`,
  was the real answer.
- **Kubernetes/Ansible/Helm** are all YAML — indistinguishable by file extension. Ansible
  and Kubernetes tooling communities have already solved this two different ways
  (Ansible: pure client-side path heuristics, confirmed via the still-open
  `ansible/vscode-ansible#582` — there is no server-side content-sniffing to defer to;
  Kubernetes: `yaml-language-server`'s own server-side glob-schema matching, which only
  needed the `initializationOptions` plumbing above to become reachable). Helm needed its
  own judgment call — see D5.

## Decision

Five phases, sequenced so the registry consolidation (Phase 0) lands before — and is
validated by — four new language/dialect additions.

### D1 — Phase 0: registry consolidation

- Fixed the live `doctor.rs`/`bootstrap.rs` pyright drift (one-line fix, `"pyright"` →
  `"pyright-langserver"`).
- Deleted the dead `crates/lsp/src/client.rs::language_id_from_path` and its tests;
  `crates/core/src/lsp_intent.rs::language_id_from_path` is now the SOLE authority for LSP
  `language_id` routing, documented as such directly on the function: intentionally
  decoupled from `crate::syntax::Language`/tree-sitter grammar selection (verified
  `Language::id()` is purely a tree-sitter/display identifier, never an LSP wire value —
  no consumer relies on the two matching), and a single choke point for any future
  dialect override — added logic lives inside this one function, never duplicated at a
  call site, so all ~18 existing consumers inherit it for free.
- Added `LspServerConfig.init_options: Option<serde_json::Value>`
  (`crates/lsp/src/client.rs`), threaded into `initialize()`'s request params as
  `initializationOptions` when set (omitted entirely when `None`, not sent as `null` —
  some servers distinguish the two). Sourced from a new, additive
  `LspLanguageConfig.init_options` field in `crates/mae/src/config.rs`'s `[lsp.<lang>]`
  schema. This is real, reusable infrastructure independent of this epic — any future LSP
  server needing settings-object configuration benefits, not just `yaml-language-server`.

**Hot-path constraint** (a real downstream-impact finding, not previously flagged):
`language_id_from_path` is called from `Editor::should_auto_complete`, which fires on
*every keystroke* in insert mode. Anything added to this function — now and in the
future — must be pure lexical path-string inspection (`Path::file_name()`/`extension()`/
`components()`), never filesystem I/O (`.exists()`, directory walks).

### D2 — Phase 1: Terraform

`Language::Terraform` (`crates/core/src/syntax/languages.rs`), `tree-sitter-hcl`
dependency, `.tf`/`.tfvars`/`.hcl` detection, `terraform-ls` as the LSP default
(`terraform-ls serve`, stdio).

Since `tree-sitter-hcl` ships no highlights query, one was hand-authored against the
grammar's `node-types.json` (`crates/core/src/syntax/queries/hcl_highlights.scm`) — the
**one deliberate exception** to "every language's highlights query comes bundled from its
own grammar crate, no local `.scm` files," documented as such with an `@ai-caution`
marker and instructions to switch to a real upstream query if one ever ships. Writing this
query surfaced a real, non-obvious bug: `tree-sitter-highlight` resolves overlapping
captures at the same node by **last-pattern-wins**, so the general `(identifier)
@variable` catch-all had to be moved to the TOP of the query file, not the bottom, for the
more specific `(block (identifier) @keyword)` pattern below it to actually win. Caught by
a real highlight-span test (`terraform_highlights_keyword_string_number_and_comment`), not
by the query merely failing to compile — a hand-authored query can be syntactically valid
against a grammar and still silently mis-highlight everything.

### D3 — Phase 2: Dockerfile

`Language::Dockerfile` via `tree-sitter-containerfile` (a maintained fork of the original,
now-stuck-on-tree-sitter-0.20 `tree-sitter-dockerfile`; ships real `HIGHLIGHTS_QUERY`/
`INJECTIONS_QUERY` constants, API-compatible with core's `tree-sitter = "0.26"`).

Filename-based detection (`Dockerfile`, `Dockerfile.<stage>`, `*.dockerfile`) factored
into a single shared `is_dockerfile_filename` helper in `crates/core/src/syntax/
detection.rs`, called from both `detection.rs` (tree-sitter grammar selection) and
`lsp_intent.rs` (LSP routing) — so the two intentionally-decoupled registries still can't
independently drift on what counts as a Dockerfile. `docker-language-server` (Docker's
new official Go-based server — the first Go-toolchain-installed dev dependency in
CLAUDE.md's Development Dependencies table, a deliberate choice over the older,
npm-installed `docker-langserver` for long-term maintenance quality) as the LSP default.

### D4 — Phase 3: Kubernetes

No new `Language` variant, no new detection logic — reuses `Language::Yaml` and Phase 0's
`init_options` plumbing entirely. A user opts a project into Kubernetes-schema-aware YAML
completions by setting `[lsp.yaml] init_options.yaml.schemas.kubernetes = "<glob>"` in
`config.toml`; `yaml-language-server` does its own glob matching server-side. This is the
cheapest of the three YAML dialects by a wide margin — no client-side detection needed at
all.

### D5 — Phase 4: Ansible, Phase 5: Helm — YAML dialect routing

Two new pure, filesystem-I/O-free functions in `lsp_intent.rs`,
`ansible_lsp_dialect`/`helm_lsp_dialect`, both gated behind `id == "yaml"` inside
`language_id_from_path` (Ansible checked first) and both returning a different
`language_id` string only — tree-sitter highlighting for both stays plain `Language::Yaml`
unchanged.

`ansible_lsp_dialect` replicates `ansible-language-server`'s own upstream convention
(confirmed no server-side alternative exists): `site.yml`/`site.yaml` filename, a filename
containing `"playbook"`, an ancestor path COMPONENT exactly `playbooks` (not a substring —
`playbooks-archive/` must not match), or a `.ansible.yml`/`.ansible.yaml` double
extension.

`helm_lsp_dialect` is a deliberately narrower, explicitly imperfect heuristic: an ancestor
path component exactly `templates`. Real Helm chart detection needs a `Chart.yaml`
sibling check, which is filesystem I/O this hot-path function cannot do, and a chart's own
directory is named after the app (not literally "chart"/"charts" — those names only
appear for Helm's dependency-subchart convention). This heuristic **will false-positive**
on non-Helm `templates/*.yaml` layouts (CI templates, email templates, etc.) — documented
plainly on the function itself and in CLAUDE.md, not silently overclaimed. A project where
this is disruptive can override `[lsp.yaml]`/`[lsp.helm]` explicitly.

`helm-ls` resolves Go-template-in-YAML entirely server-side, so LSP completions/
diagnostics work today. Real Go-template-aware **syntax highlighting** is explicitly
**out of scope** for this ADR: MAE's tree-sitter highlighter has never resolved a real
language injection (`languages.rs`'s `highlighter.highlight(..., None, |_| None)` — the
injection-resolution callback is a permanent no-op; Markdown's code-fence highlighting
works only via a separate hand-rolled regex-extraction pass, not tree-sitter's native
mechanism). Building MAE's first real injection-callback resolver is a bigger, separate
architectural project with regression surface across every language already carrying an
unused `INJECTIONS_QUERY` (Rust, JavaScript, Markdown) — it deserves its own future ADR if
pursued, not a sub-bullet here. Helm chart templates render with best-effort plain-YAML
highlighting in the meantime.

## Consequences

- LSP `language_id` routing and tree-sitter grammar selection are now an explicitly
  documented, intentionally decoupled dual-registry design, not an accidentally-diverged
  pair of tables — a YAML dialect can gain LSP-server routing without ever touching
  tree-sitter, and the reverse holds too.
- `initializationOptions` passthrough is new, real, reusable LSP-client infrastructure,
  not scoped narrowly to `yaml.schemas`.
- The Ansible and Helm dialect heuristics are honest best-effort client-side path matching
  with documented false-positive risk, not verified detection — a deliberate, stated
  trade-off given the hot-path I/O constraint, not an oversight.
- Helm chart templates get LSP support today; real hybrid syntax highlighting is deferred,
  stated plainly rather than implied — no silent capability overstatement.
- `docker-language-server` introduces MAE's first Go-toolchain-installed optional dev
  dependency; `CLAUDE.md`'s Development Dependencies table and env-var list were updated
  to reflect this honestly rather than silently reusing the npm-only framing.

## Verification

Per CLAUDE.md principle #14 (adversarial, not confirmation): comparative/adversarial test
pairs throughout — `docker_compose_yaml_is_not_dockerfile` (a filename merely containing
"docker" must not false-positive), `playbooks-archive/old.yaml` must resolve to plain
YAML (proving the ancestor-COMPONENT check, not a substring match), Ansible's heuristic
explicitly wins over Helm's when both could apply
(`language_id_ansible_wins_over_helm_when_both_heuristics_could_match`), and real
highlight-span tests for both Terraform and Dockerfile (not just "the query compiles" —
`terraform_highlights_keyword_string_number_and_comment`/
`dockerfile_highlights_keyword_and_string` assert actual `theme_key` values from real
source, which is what caught the last-pattern-wins capture-precedence bug in Terraform's
hand-authored query). `init_options` passthrough has both a positive test (the key is
present with the right value when set) and a negative comparative test (the key is absent
entirely, not `null`, when unset). `cargo test -p mae-core -p mae-lsp -p mae`, `cargo
clippy -D warnings`, `cargo fmt --check` all clean across every touched crate.
