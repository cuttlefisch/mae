# ADR-076: The system of bundled knowledge bases (Manual, MaePractices, DevPractices, ADR)

**Status:** Accepted, implementation in progress.
**Depends on:** ADR-063 (guidance-delivery uniformity — `read_guidance_kb_context`'s
consumption mechanism, unchanged by this ADR). Builds on the precedent set by issue
#370 (MaePractices).
**Relates to:** ADR-059 (ADR KB — deliberately opt-in, not auto-registered; this ADR
keeps that decision and generalizes around it rather than revisiting it), ADR-062
(`KbRegistry`'s noted-but-unfixed O(n) scan — this ADR grows the registry by one
typical entry, doesn't change its shape).
**Tracking:** issue #514.

## Context

MAE ships a built-in help/manual KB (code-generated + `assets/manual/*.org`) and, since
issue #370, a second bundled KB — **MaePractices** (`assets/practices/*.org`) — curated
guidance for people developing *MAE itself*, auto-registered at every startup so
`ai_guidance_kb` has something to point at by default. A third bundled KB — the **ADR
KB** (`docs/adr/*.md`, ADR-059) — exists too, deliberately opt-in rather than
auto-registered (injecting dozens of ADR summaries into every AI session by default
would be noisy).

Issue #514 asks for the natural next step: a **generic, vendor-neutral** DevPractices
KB — the same value MaePractices demonstrated, but for the much larger audience of
people using MAE as their editor to build *other* software, not MAE itself. That
content already existed, built and validated independently at
`~/Projects/dev-practices-kb` (89 hub/atom/molecule-structured `.org` notes, measured
retrieval-efficiency evidence, covers GitHub and GitLab workflows) — never previously
connected to MAE's release pipeline.

Researching how to bundle it surfaced a second, more consequential gap: **neither
MaePractices nor the ADR KB was ever wired into `release.yml` or `install.sh`** — both
existed only via `make install` (source builds). Anyone using the official installer or
a downloaded release tarball — the overwhelming majority of real users — got neither.
Issue #514's actual goal (a usable, reproducible out-of-the-box default) was
unreachable for that audience without closing this gap too. This ADR closes it for all
three non-manual KBs together, not just the new one, via one shared mechanism rather
than a fourth divergent one-off pipeline.

## Decision

### D1 — The taxonomy

| KB | Source | Auto-registered? | Bundled in release? | Consumption | Purpose |
|---|---|---|---|---|---|
| **Manual** (`mae-manual.cozo`) | code-gen + `assets/manual/*.org` | N/A — loaded read-only in-memory, not a federation instance | Yes | `:help` system, SHA-validated | MAE's own built-in help |
| **MaePractices** (`mae-practices.cozo`) | `assets/practices/*.org`, MAE-specific | Yes, additive/idempotent | Yes (this work) | `ai_guidance_kb` → `read_guidance_kb_context` | Guidance for contributors working *on MAE itself* |
| **DevPractices** (`mae-devpractices.cozo`) | forked from `~/Projects/dev-practices-kb`, generic | Yes, same mechanism | Yes (this work) | `ai_guidance_kb` (new fresh-install default) → `read_guidance_kb_context` | Guidance for anyone using MAE to build *other* software |
| **ADR** (`mae-adr.cozo`) | `docs/adr/*.md`, generated | No — deliberately opt-in (ADR-059) | **No — its own release asset** (see below) | `make adr-kb` / `make fetch-adr-kb`, then manual `kb_register` | Queryable MAE decision history, not injected into every AI session |

**Amended 2026-08 — the ADR KB left the bundle.** It was tracked in git and copied
into every user package. Two things were wrong with that. It is ~57 MB of MAE's own
decision history, useful only to people working *on MAE*, so every end user
downloaded it to never register it. And because it is a build artifact regenerated
from `docs/adr/*.md`, each regeneration wrote a fresh ~57 MB object into history —
GitHub had begun warning on the push. Nothing read the committed copy: `make install`
depends on the `adr-kb` target and the release workflow runs `build-adr-kb` before
packaging, so both rebuilt it first, and `verify-adr-kb-sync` only diffs the checksum
sidecar.

It is now untracked (`.gitignore`), built with `make adr-kb`, or downloaded with
`make fetch-adr-kb` from a standalone `mae-adr.cozo.tar.gz` release asset covered by
the release's `SHA256SUMS`. `assets/mae-adr.cozo.sha256` stays tracked for ADR-059's
Phase E staleness gate — but note it is **not** a verification oracle: a sled store is
rewritten in place on first open and is not byte-reproducible, so a rebuilt or
once-opened store hashes differently from the committed value. The tarball's hash in
`SHA256SUMS` is what a download is checked against.

The three remaining bundled KBs are unaffected: unlike the ADR KB, `mae-manual.cozo`
and the two guidance KBs are read out of `assets/` at runtime by source builds, so
untracking them would need a fallback path first.

The axis that matters: **auto-registered guidance KBs** (MaePractices/DevPractices —
`ai_guidance_kb` needs *something* to point at automatically) vs. **opt-in reference**
(ADR KB, per ADR-059's existing decision). A fifth bundled KB in the future should
classify itself against this axis rather than re-deriving the question from scratch.

### D2 — Content fork, not a sync mechanism

The full ~89-note corpus is forked from `~/Projects/dev-practices-kb/kb/*.org` into
`assets/devpractices/*.org` as a one-time copy. The two repos diverge independently
afterward; MAE's copy becomes canonical for what ships with MAE. No submodule, no
sync mechanism — matches how `assets/practices/*.org` is itself hand-authored
in-tree, not generated from anywhere external.

The one required content adaptation: `dev-practices-kb`'s entry point is `hub:start-here`,
not the literal `"index"` node ID `read_guidance_kb_context` requires. A new, small
wrapper node with literal `:ID: index` was added (pointer/summary body linking to
`hub:start-here`), leaving all other note IDs untouched — mirrors
`assets/practices/index.org`'s own shape.

### D3 — Shared build-pipeline library (`shared/kb/src/kb_build.rs`)

The three existing build binaries (`build_manual_kb.rs`, `build_practices_kb.rs`,
`build_adr_kb.rs`) duplicated their SHA-256 checksum/sidecar logic verbatim, and
`build_practices_kb.rs`'s org-ingestion loop (read_dir → sort → parse → insert) was
structurally identical to half of `build_manual_kb.rs`. `mae_kb::kb_build` extracts
the shared pieces — `open_fresh_store`, `ingest_org_dir`, `compute_db_checksum` +
`write_checksum_sidecar`, `require_index_node` — used by all four build binaries
(the three existing ones, refactored, plus the new `build_devpractices_kb.rs`).

Deliberately **not** one parameterized `build-kb-asset` binary: the manual KB
(code-gen, no index-node requirement) and the ADR KB (cross-reference/cycle
validation, no raw org-dir ingestion) have genuinely different input shapes.
Forcing them into one binary means a runtime `match` on invocation mode inside
`main()` — the "one binary, per-invocation special-casing" shape CLAUDE.md's
principle #8 warns against. Four thin, individually `cargo run --bin`-able binaries
sharing one library gets the DRY win without an artificial common shape.

### D4 — Shared guidance-KB auto-registration engine

`practices_kb.rs`'s location/copy-then-register logic is generalized into a
descriptor-driven engine (`BundledGuidanceKb { instance_name, asset_filename,
env_override }`), with `practices_kb::{INSTANCE_NAME, ensure_registered}` kept as
thin wrappers so `bootstrap.rs` and the existing test suite are untouched. A new
sibling `devpractices_kb.rs` provides the DevPractices descriptor. Both are wired
into `bootstrap.rs::init_kb_federation()`, before `KbRegistry::load` — same ordering
guarantee `practices_kb` already relied on.

The existing "never overwrite an existing same-named entry" behavior is retained
unchanged and is an **intentional property**, not just a safety net: anyone who wants
to override a shipped default KB with their own live copy can do so simply by
registering something under the same name first — no override flag or special config
needed. (Confirmed live during this ADR's own development: a contributor's personal
`DevPractices` registration pointing at a live source directory silently took
precedence over the bundled one until explicitly unregistered.)

### D5 — Release/install bundling, generalized

`release.yml`'s existing manual-KB bundling pattern (build the binary, run it, strip
sled lock files, `cp` the `.cozo`+`.sha256` pair into every package destination a job
produces, export `MAE_MANUAL_PATH` for self-contained AppImage/macOS bundles) is
replicated for practices/ADR/devpractices across `build-linux`, `build-linux-gui`,
`build-macos-arm`. `build-windows` remains KB-less (TUI-only, per ADR-066 Phase B) —
not a new gap, a pre-existing and explicitly out-of-scope exclusion until Windows
becomes a full GUI release target.

`install.sh` gains matching install/uninstall/verify blocks for the three
newly-bundled KBs, using `warn` rather than `fail` on a missing file — consistent
with these KBs' already-documented "silent no-op if not found" runtime behavior,
unlike the manual KB's hard `fail` (appropriate there since `:help` is core
functionality).

### D6 — Fresh-install default flips to DevPractices

The shipped `init.scm` template (`crates/mae/src/config.rs::write_init_template`,
written only when no `init.scm` exists yet) now sets `ai_guidance_kb` to
`"DevPractices"` instead of `"MaePractices"` — the right default for the ~100% of
fresh installs who are not MAE contributors. A contributor switches back with one
`:set-save ai-guidance-kb MaePractices`. MaePractices stays auto-registered and fully
available either way.

## Alternatives considered

- **Single monolithic parameterized build binary** — rejected; manual/ADR KBs have
  genuinely different input shapes than a flag-driven common path would allow
  cleanly (D3).
- **Renaming `practices_kb.rs`** as part of generalizing its logic — rejected; the
  logic needed sharing, not the file identity. Renaming an established file with its
  own test suite and doc comments for no additional DRY benefit fails CLAUDE.md
  principle #9's regression-risk bar.
- **A new `KbInstanceKind::Bundled`/`System` variant** — rejected; `Guidance` (the
  existing variant MaePractices already uses) already covers the descriptive role.
  Nothing in the query/storage layer branches on `kind` beyond this descriptive tag.
- **Auto-registering the ADR KB too** — rejected; contradicts ADR-059's explicit,
  still-valid decision that ADR content is reference material, not default AI-session
  context.
- **Windows KB bundling** — deferred, not rejected, pending Windows becoming a full
  GUI release target (task #225).

## Consequences

- Fresh installs get real, generic developer-guidance content out of the box, closing
  issue #514's actual goal — not just the narrower "ship a `.cozo` file" reading of it.
- `KbRegistry`'s already-tracked O(n) linear scan (ADR-062) grows by one typical
  entry — acceptable at this scale, not a new architectural concern.
- MAE's `assets/devpractices/` becomes a second, independently-evolving source of
  truth relative to `~/Projects/dev-practices-kb` — an explicitly accepted trade-off
  (D2), not an oversight.
- The "your own registration under the same name always wins" precedent (D4) is now
  documented project-wide behavior, not an incidental implementation detail a future
  contributor might "fix" into requiring an explicit override flag.

## Verification

Per CLAUDE.md principle #14: `kb_build` unit tests cover checksum determinism and
`ingest_org_dir` counts against fixture directories; a real test asserts
`build_devpractices_kb` panics (not silently ships an empty/unindexed KB) against a
fixture with no `index` node. `devpractices_kb.rs`'s additive/idempotent tests are
copy-adapted from `practices_kb.rs`'s existing three (env-override locate,
add-when-absent, never-overwrite-existing) — the same adversarial shape, applied to
the new module. A real end-to-end test confirms a fresh `mae --init-config`'s shipped
template resolves `ai_guidance_kb` to actual DevPractices *content* via
`read_guidance_kb_context`, asserting a known substring from the index-wrapper node's
body — not merely `is_some()`, which would pass even if the wrong node's content
leaked through. Release/install bundling is verified via local dry runs (release jobs
only fully execute on a tag push): running each build binary locally and confirming
output matches what the workflow's `cp` lines expect, and a scripted dry run of
`install.sh`'s new blocks against a fake package directory confirming verify passes
for all four KBs and `--uninstall` removes all four cleanly.
