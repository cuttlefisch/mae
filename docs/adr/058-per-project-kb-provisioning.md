# ADR-058: Per-project KB provisioning

**Status:** Accepted.
**Extends:** ADR-057.
**Relates to:** ADR-062, ADR-004, ADR-011.
**Closes:** ROADMAP #82.

## Context

MAE's knowledge base is, today, a single machine-global primary plus one flat
global registry. `init_kb_federation` (`crates/mae/src/bootstrap.rs:2117`)
loads one registry file and imports every enabled instance into the same
in-process `KnowledgeBase`, and the registry entry type itself,
`KbInstance` (`shared/kb/src/federation.rs:34-59`), has no notion of a
project at all — `uuid`, `name`, `org_dir`, `db_path`, `primary`, `enabled`,
`last_import`, `collab_id`, `shared`, `remote_peers`, `last_sync`, and
`ai_residency` (added by ADR-048), but no `project_root` field and no
`kind` discriminant beyond the boolean `primary` flag. Every KB a user
registers — their personal notes, a client's proprietary docs, a completely
unrelated side project's design notes — lands in the same flat bucket and is
visible to every query by default, because `KbScope` (the query-time
selector immediately below `KbInstance` in the same file) offers only `All`,
`LocalOnly`, `RemoteOnly`, and `Named(String)`. There is no `Project`
variant, and there is no path by which a registry entry could even carry
enough information for one to be resolved correctly if it existed.

This is not for lack of the underlying capability elsewhere in the
codebase. `crates/core/src/project.rs:90`'s `detect_project_root` already
implements exactly the anchor-then-build-marker walk this ADR needs — VCS
roots and `.project` files win immediately, `Cargo.toml`/`package.json`/
`go.mod`/`pyproject.toml`/`Makefile` are tracked as a fallback so a subcrate
manifest doesn't outrank a workspace root, and it already excludes `$HOME`
as a degenerate "project." Project-root detection is a solved, correct,
independently-tested primitive. It is simply never consulted anywhere in
the KB registration or query path. The gap this ADR closes is entirely
about wiring: connecting an already-correct project-root detector to an
already-correct multi-instance registry and scope-selection mechanism,
neither of which currently know the other exists.

**Why this matters now, not hypothetically.** As MAE moves from a
single-user personal-notes tool toward the external-editor MCP pairing
initiative (ADR-050–056) and the P2P daemon-mesh initiative (ADR-025–027,
issue #96), the number of KBs a single MAE installation touches grows past
"one global bucket" naturally: a contributor working across `mae` itself,
a client's proprietary codebase, and a personal dev-practices KB
(`DevPractices`, already a live registered federated instance in this very
session) all coexist in one registry today, and `kb_search`/`kb_search_context`
default to `KbScope::All` — searching across all of them at once unless the
caller explicitly opts into a narrower scope by name. ROADMAP issue #82
already names the intended shape precisely: a `kb_search_scope` option with
`"all"` (default, unchanged), `"user"` (exclude MAE-internal `scheme:*`/
`cmd:*`/`option:*` nodes), and `"project"` (only the active project's
registered instances) — plus per-workspace isolation so that opening a
second, unrelated project doesn't surface the first project's KB content by
default. That issue has stood unbuilt because the underlying `KbInstance`
schema had nowhere to record which project (if any) a KB belongs to, and
`KbScope` had no variant to resolve against it. This ADR is the schema and
lifecycle design that makes issue #82's `"project"` scope buildable.

**What "per-project" must NOT mean.** It must not mean silently multiplying
KB files on disk the first time a user opens a directory that happens to
look like a project — that would violate CLAUDE.md principle #10's
multi-client-safety spirit (state mutation should never surprise a
concurrent observer) by the softer but real analogue of surprising the
single local user with files they didn't ask for, and it would directly
contradict principle #7's "no ad-hoc solutions": a background auto-create
triggered off a read path is exactly the kind of implicit, undiscoverable
behavior the OptionRegistry-first design principle exists to prevent. Per-
project provisioning has to be opt-in-by-default, discoverable, and
explicit enough that a user (or an AI peer acting on the user's behalf)
always knows a new KB was created and why.

## Decision

Five phases, each independently shippable and independently testable,
building the `KbInstance` schema first and the query-time behavior last so
that no phase depends on functionality a later phase hasn't landed yet.

**Phase A — schema.** Add two fields to `KbInstance`
(`shared/kb/src/federation.rs`):

```rust
pub project_root: Option<PathBuf>,
pub kind: KbInstanceKind,
```

where `KbInstanceKind` is a new enum — `Primary`, `Project`, `Guidance`,
`UserRegistered` — mirroring the shape of the existing `AiResidency` enum
(`Copy`, `Serialize`/`Deserialize`, `#[serde(rename_all = "snake_case")]`,
`#[default]`). Both new fields carry `#[serde(default)]`, matching the
precedent `ai_residency` already set for exactly this situation: a registry
file written by pre-058 code deserializes cleanly with `project_root: None`
and `kind` defaulting to whichever variant a `From<&KbInstance>`-style
inference derives from the *existing* `primary` boolean and `collab_id`/
`shared` fields (`primary: true` → `Primary`; a `federation`-imported
non-primary, non-shared instance whose `name` matches the shipped
dev-practices KB name → `Guidance`; everything else → `UserRegistered`).
Zero migration step, zero registry-file rewrite required — Phase A is a
pure additive schema change, deployable and dormant on its own.

> **Drift recorded 2026-08-13 (ADR-104).** The `kind` field shipped, and the
> inference rule above did **not** hold in practice. On a real registry,
> `MaePractices` was recorded `UserRegistered` while `DevPractices` was
> `Guidance` — two different values for two rows MAE itself had written, because
> the "name matches the shipped dev-practices KB name" clause only ever matched
> one of the shipped corpora. The field intended to mark provenance had drifted
> into meaninglessness, which is why ADR-104's eviction migration keys on row
> *shape* rather than on `kind`.
>
> ADR-104 D2 removes the need for `Guidance` altogether: system KBs are no
> longer rows in `kb-registry.toml` at all, but a compile-time catalog
> (`mae_kb::system_kb`), and existing system rows are evicted at startup. The
> `Guidance` variant is therefore unreachable for the corpora it was invented
> for. `Primary` / `Project` / `UserRegistered` are unaffected and still
> describe exactly what the registry can describe correctly — user KBs.
>
> The general lesson, per principle #15: an inference rule that reconstructs a
> classification from *other* fields is only as good as its weakest clause, and
> it fails silently. The class distinction ADR-104 draws is structural — a
> different code path, not a different enum value on a shared one — precisely so
> there is nothing to infer.

**Phase B — opt-in-by-default provisioning trigger.** The first KB-touching
action performed inside a detected project root (per
`detect_project_root`) for which no registry entry has
`kind == KbInstanceKind::Project` and a matching `project_root` fires an
interactive prompt in the buffer/GUI surface ("No project KB found for
`<root>` — create one? y/n/never-ask-again"), or, in a non-interactive
context (headless/MCP session with no attached human), a once-per-session
notice surfaced through the existing notification/attention-bus mechanism
(ADR-024) rather than a silent skip — an AI peer connected headless still
needs to know provisioning didn't happen and why. Critically, this trigger
never fires on a pure read path (a `kb_search`/`kb_get` call must never
have the side effect of creating a new KB instance mid-query) — it fires
only at points that are already understood by the user as "starting to use
the KB here," e.g. the first `kb_ingest`/`kb-create`-adjacent action or an
explicit editor-open-in-new-project transition, never as a hook on a
read-only tool. Independent of the prompt, `:kb-init-project` (Scheme
`(kb-init-project)`) and the MCP tool `kb_init_project` are always
available as the explicit, no-prompt-needed path — a user or an AI agent
that already knows it wants a project KB should never have to wait for or
navigate an interactive prompt to get one.

**Phase C — `KbScope::Project` (closes ROADMAP #82).** A new variant on the
existing `KbScope` enum in `shared/kb/src/federation.rs`, resolved *at query
time*, not baked into a static token at registration time:

```rust
Project(PathBuf) // or resolved against the active editor's current project root
```

Resolution walks the caller's current buffer/working-directory context
through `detect_project_root` to get a concrete root, then filters the
registry to instances whose `kind == Project` and whose `project_root`
canonicalizes to the same path. Resolving at query time — rather than
caching a scope decision once per session — is what keeps this correct
under principle #13's cross-platform-parity and general robustness
expectations: if a project directory is moved, renamed, or accessed via a
different mount point/symlink between two queries in the same session, the
scope still resolves against where the project root actually is right now,
not a stale path captured whenever the session started. This is the same
`kb_search_scope` option surface issue #82 specified: `"all"` (existing
default, unchanged), `"user"`, and now `"project"` all map onto `KbScope`
variants, parsed by the same `KbScope::parse` entry point every other scope
token already goes through — no new parsing mechanism, no new AI-tool
argument shape beyond the `"project"` token value itself.

**Phase D — coexistence rules, made explicit so future phases don't
regress them.** `KbScope::All` — today's default for every existing caller
that doesn't pass an explicit scope — is completely unaffected by this
ADR; a `Project`-kind instance is just another instance to `All`, exactly
like a `UserRegistered` one. `KbScope::Project` *narrows* — it never
introduces results a broader scope wouldn't already contain. Guidance-KB
reachability is untouched: `ai_guidance_kb` (the option that names which
KB's standing-practices content gets surfaced into every AI session,
shipped as part of the dev-practices-KB dogfooding work) is a *named KB
selector*, not a scope filter, and stays that way — a session scoped to
`KbScope::Project` for its `kb_search` calls must still see guidance
content via `ai_guidance_kb`, because guidance delivery and search scoping
answer two different questions ("what should this AI always know" vs.
"what does this particular query search over") and conflating them would
silently break every existing guidance-KB deployment the moment a user
turns on project scoping.

**Phase E — graceful degrade + persistent decline.** Search and read
operations must keep working against whatever is currently registered at
every point during rollout of Phases B–D — a project with no `Project`-kind
instance yet simply behaves as it does today (falls through to whatever
broader scope is in effect), never errors, never blocks. A user's decline
of the Phase B prompt ("never ask again for this project") is persisted
per-project, not per-session-in-memory, reusing the exact `:set-save`/
config-persistence pattern already used everywhere else in the codebase for
this kind of durable per-scope preference (the same mechanism `keymap_flavor`
and other `OptionRegistry`-backed settings already use to survive process
restart) — the decline record is keyed by the canonicalized project root,
so it is found again correctly regardless of which buffer or session
re-enters that project later.

## Consequences

**Positive.** Closes ROADMAP #82 with the schema and lifecycle it was
actually blocked on — `KbInstance` finally has somewhere to record "this
KB belongs to this project," and `KbScope` finally has a variant that can
use it. Multi-project users (already the common case for anyone doing
client work, contracting, or maintaining more than one open-source project
alongside personal notes) get real isolation by default instead of every
KB search silently spanning every KB the machine has ever registered.
Because Phase A is purely additive and every later phase degrades
gracefully when nothing is registered yet, this ships incrementally without
a big-bang migration and without breaking any existing single-KB or
single-project workflow — a user who never encounters a second project
never sees any behavior change at all.

**Costs (honest).** A new interactive-prompt surface is another thing a
user has to learn to recognize and dismiss correctly, and getting the
"never silent, never annoying" balance right (once-per-project, not
once-per-session, not once-per-action) is genuinely fiddly product design,
not just an engineering task — the wrong tuning here either nags a user who
already declined, or silently provisions a KB a user didn't want. A fourth
`KbInstanceKind` variant and a new `KbScope` variant both add one more case
every future match arm over these enums must handle correctly (the same
maintenance-surface cost ADR-056 named honestly for `ToolCategory` — every
future addition to the registry-kind or scope-selector space is now one
more thing a contributor must reason through, not free). Canonicalized-path
comparison for project-root matching (needed for Phase D's collision test)
adds a real filesystem syscall to a path that was previously pure string
comparison, which — while cheap relative to any KB query itself — is a
small, deliberate performance/complexity tradeoff made in exchange for
correctness under `mv`/symlink scenarios.

## Alternatives rejected

- **Silent always-auto-create.** The first KB-touching action inside any
  detected project root just creates a `Project`-kind instance with no
  prompt, ever. Rejected — this is the softer analogue of the principle
  #10 concern named in Context: a user opening a scratch directory that
  happens to contain a `Cargo.toml` would silently get a new on-disk KB
  file they never asked for and may not notice for a long time, multiplying
  KB files across every directory anyone ever visits with MAE. Opt-in with
  an always-available explicit escape hatch (`:kb-init-project`) gives the
  same convenience to anyone who wants it immediately, without surprising
  anyone who doesn't.
- **A separate project-KB registry file, distinct from the existing
  `KbRegistry`/`KbInstance` federation registry.** Rejected on two grounds.
  First, it directly contradicts principle #8 ("shared computation,
  backend-specific drawing" generalizes here to "don't reimplement a
  solved lookup/persistence mechanism") — the federation registry already
  solves "durably record a set of KB instances with metadata and load them
  at boot," and a second, parallel registry file would mean every future
  reader (boot-time federation import, `kb_instances` introspection, the
  `*KB Sharing*` buffer, the MCP `kb_sharing_status` tool) needs to learn
  about and merge two sources of truth instead of one. Second, and more
  concretely, it directly undermines the scaling work ADR-062 is doing:
  a second lookup surface is exactly the kind of doubled indirection that
  makes a scaling fix (indexing, caching, connection-pooling — whatever
  form ADR-062 takes) harder to reason about, because it would now have to
  account for two independently-evolving stores of "which KBs exist"
  instead of one. Extending `KbInstance` with two new fields is strictly
  cheaper for every downstream consumer than adding a second registry
  next to it.

## Verification

Per CLAUDE.md principle #14, every phase below is verified adversarially —
falsifying the implementation, not confirming the happy path once in a
fixed order.

- **Phase A.** Round-trip a pre-058 registry fixture: take a TOML/JSON
  registry file exactly as written by today's (pre-`project_root`/`kind`)
  code, deserialize it under the new `KbInstance` schema, and assert (1) it
  deserializes without error, (2) `project_root` is `None` and `kind`
  infers to the expected variant for each fixture entry (a `primary: true`
  entry infers `Primary`; others infer per the rule in Phase A), and (3)
  re-serializing produces a stable, minimal diff — no field reordering, no
  spurious new keys written for entries that never had them, no data loss
  on the fields the old format did carry.
- **Phase B.** A TOCTOU (time-of-check-to-time-of-use) test: start
  provisioning against a project root, then delete the directory (or
  replace it with a symlink loop) between the trigger firing and the
  provisioning write actually happening — the operation must fail cleanly
  with a reported error, never panic, never leave a half-written registry
  entry. Separately, an N-way convergence test per principle #14's explicit
  "no 2-session happy path" requirement: spin up **three** concurrent
  sessions (not two) that all race to provision the *same* project root at
  the same time, and assert the registry converges to exactly one
  `Project`-kind entry for that root afterward — not three duplicate
  entries, not a corrupted registry file from an interleaved write.
- **Phase C.** A property test over a seeded-random mix of registered
  instances — multiple `Primary`/`Project`/`Guidance`/`UserRegistered`
  entries spanning multiple distinct project roots, generated with a fixed
  seed so failures reproduce deterministically, not hand-picked "unicorn"
  fixture data chosen because it happens to pass. The invariant asserted:
  for any query, `KbScope::Project` results are always a subset of
  `KbScope::All` results for the identical query (Phase D's narrowing
  guarantee, checked mechanically rather than just asserted in prose), and
  a `Project`-scoped query against root X never returns a node that lives
  only in a `Project`-kind instance registered under a different root Y —
  the negative half of the same property, checked in the same test.
- **Phase D.** A collision test constructed from a *real* filesystem
  operation, not a synthetic path string: register a `Project`-kind
  instance for directory `A`, then actually `mv`/rename `A` to `B` on disk
  (or the equivalent on the CI platform), then register a second, distinct
  `Project`-kind instance whose `project_root` is separately set to `B`.
  Assert the two entries remain two distinct registry entries — canonicalized-path
  comparison must not silently merge them into one just because a stale
  string-equal check would have matched, and equally must not silently
  merge them if a naive rename detection incorrectly treats "same inode,
  different current path" as "same entry" when the user's intent was two
  separate registrations.
- **Phase E.** A persistence test spanning **50** subsequent KB actions
  after a decline, including at least one process restart in the middle of
  that sequence (not just an in-memory session check) — asserting the
  decline is never re-prompted across any of the 50 actions or across the
  restart, proving the decline record lives in durable config storage via
  the same `:set-save` mechanism other durable per-scope settings already
  use, not merely a runtime flag that resets when the process exits.

## Status note (implementation, principle #15's "not just a symptom patch")

All five phases are implemented and tested, verified in both directions where the
adversarial test targets a specific fix (confirmed to genuinely fail against the pre-fix
code). `cargo fmt --check`/`cargo clippy --workspace --all-targets -- -D warnings`/`cargo
test --workspace` clean across both the editor and daemon workspaces.

**Design corrections made during implementation, on evidence, not assumption:**

- **`KbInstance::effective_kind()` originally special-cased `primary: bool` as an alias for
  `KbInstanceKind::Primary`.** A real 3-way-concurrent adversarial test
  (`kb_registry_register_converges_under_a_three_way_race`, mae-kb) caught this as wrong:
  `primary` (set by `register()` as `self.instances.is_empty()`) means "the first
  `KbInstance` row ever registered on this machine" — an artifact of registration order,
  not an alias for the machine-global primary KB, which structurally has no `KbInstance`
  row at all. The original logic silently reclassified the very first project a user ever
  provisions back to `Primary`, defeating `KbScope::Project` for exactly that instance.
  `effective_kind()` now simply returns the stored `kind` field.
- **Two genuine, previously-undiscovered concurrency bugs in the shared
  `mae_mcp::file_lock` primitive** (used by `KbRegistry::update` and by `projects.toml`/
  package-lockfile/config-known-set persistence elsewhere in the codebase — not
  ADR-058-specific), both found by the same adversarial test and both fixed at the
  primitive level rather than worked around locally:
  1. `acquire_lock` used a non-atomic read-then-write check — a TOCTOU window two
     contenders arriving within microseconds of each other could both slip through, each
     believing it acquired the lock. Fixed with `OpenOptions::create_new` (atomic
     `O_CREAT|O_EXCL`).
  2. The retry budget (`3 × 15ms`) was based on the pre-existing, over-optimistic
     assumption that a reload-before-mutate step "already closes most of the race even
     without the lock" — true only when one caller's `load()` happens after another's
     `save()` completes, not under genuine simultaneous starts. Widened to `30 × 20ms`.
  3. (Found alongside, same test): `acquire_lock` didn't ensure its lock file's parent
     directory existed first, so the very first write on a fresh data dir hit a generic
     I/O error that fell through to "proceed without a lock" — exactly when multiple
     simultaneously-starting processes most need it. Fixed by creating the parent
     directory before the atomic create.
- **A separate, deeper concurrency bug was found and deliberately NOT fixed here**: routing
  a genuine 3-way race through the *full* `Editor::kb_init_project` (which also opens/
  imports into a real CozoDB store per call via `kb_adopt_instance`) intermittently panics
  in `shared/kb/src/cozo_store/source_files.rs` — a pre-existing gap in concurrent-store-
  open safety unrelated to ADR-058's own registry-dedup contract. The adversarial test was
  scoped to exercise `KbRegistry::register`/`update` directly (what ADR-058 actually owns)
  rather than silently weakening coverage down to a non-concurrent call to route around it;
  the store-level bug is tracked as separate follow-up work, not hidden.
