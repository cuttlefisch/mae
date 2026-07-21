---
description: Source-level audit of the MAE codebase — structural health, AI-slop, Rust anti-patterns, cross-workspace consistency, DRY/design-principle adherence, docs/onboarding, plus metadata & coverage checks. Reports a structured markdown report.
argument-hint: "[crate | file | full]   (default: files changed since last commit)"
---

# /mae-audit — MAE Code Audit & Structural Health Check

A source-level audit encoding MAE's architecture constraints (CLAUDE.md), design principles, file-size
ceilings, AI-slop detection, DRY/reuse + design-principle analysis, documentation/onboarding review, and a
metadata/coverage pass. Two workspaces (editor + daemon), 20 crates (17 editor + 3 shared), ~95k lines.

## Scope Control

Interpret `$ARGUMENTS`:

- **(empty):** Audit files changed since last commit (`git diff --name-only HEAD~1`)
- **`<crate>`:** Audit a single crate (e.g. `mae-ai`, `daemon`)
- **`full`:** Full codebase structural audit (expensive — use sparingly)
- **`<file>`:** Single-file audit

## Repository Layout

Two workspaces + shared crates (ADR-014):

```
mae/                              (repo root)
├── Cargo.toml                    (editor workspace — cozo+sled, rusqlite OK)
├── Cargo.lock                    (editor lock)
├── crates/                       (editor-only crates — 17 crates)
│   ├── core/  scheme/  ai/  mae/  renderer/  gui/  lsp/  dap/
│   ├── shell/  babel/  export/  canvas/  snippets/  format/
│   ├── make/  lookup/  spell/    (state-server was merged into daemon, v0.13.2)
├── daemon/                       (daemon workspace — cozo+sqlite, separate Cargo.lock)
│   ├── Cargo.toml + Cargo.lock
│   └── src/ (main.rs, collab_handler.rs, scheduler.rs, hygiene.rs, config.rs, storage.rs, doc_store.rs)
└── shared/                       (shared crates — editor workspace members, also used by daemon)
    ├── kb/    (mae-kb: CozoDB store, org parser, federation, query layer, LRU cache)
    ├── sync/  (mae-sync: yrs CRDT, ropey bridge)
    └── mcp/   (mae-mcp: JSON-RPC protocol, shim, daemon client, auth, tls, keystore)
```

Build commands:
- `make ci` — editor workspace (fmt + clippy + build + test, excludes GUI)
- `make ci-all` — both workspaces
- `cd daemon && cargo test` — daemon tests
- `cd daemon && cargo clippy --all-targets -- -D warnings` — daemon clippy

## Hard Ceilings

| Metric | Ceiling | Action |
|--------|---------|--------|
| Source file | 800 lines | Split into module directory |
| Test file | 500 lines | Split into focused modules |
| Function | 80 lines | Extract helpers or restructure |
| Match arms in one block | 30 arms | Use dispatch table or trait dispatch |
| Struct fields | 15 fields | Extract sub-structs or builder pattern |
| Nesting depth | 4 levels | Early return, extract function |

**Known exceptions** (tracked as architectural debt, not audit failures — re-measure each run with `wc -l`).
Each of these also carries an in-code `@ai-caution: [architecture-debt]` marker (see CLAUDE.md's
"Debt/Invariant Tagging" section) cross-linking back here and to `ROADMAP.md`'s "Architecture Debt"
section — when you add a new exception, add the marker + both cross-references, not just this list.
**2026-07 update**: a dedicated splitting pass closed out most of the list below — `shared/kb/src/cozo_store.rs`
and `shared/sync/src/kb.rs` are now fully resolved (every resulting file under ceiling) and dropped
from this list entirely; the rest were substantially reduced but their residual/dispatcher file is
still over the source ceiling, so they stay listed with updated numbers:
- `crates/mae/src/main.rs` — 951 lines (was 3,329). CLI dispatch moved to `cli.rs`, `GuiApp` +
  its `ApplicationHandler` impl moved to `gui_app.rs`, config-application/KB-federation-init/
  daemon-connect moved to `bootstrap.rs`. Residual is genuinely sequential entry-point glue
  (panic/logging setup, editor construction, Scheme init, final channel-wiring handoff) with no
  further obvious seam — still ~18% over the 800-line ceiling, accepted as-is rather than forced.
- `crates/mae/src/bootstrap.rs` (NEW, surfaced by the `main.rs` split above) — 3,068 lines (2026-07:
  3,062, growing further; was already 2,397 before the original split, pre-existing untracked debt).
  Holds app bootstrapping: config application, KB federation init, daemon connect, collab user-name
  resolution. Not split further this pass — needs its own dedicated look.
- `crates/mae/src/gui_app.rs` (NEW, surfaced by the `main.rs` split above) — 1,662 lines (2026-07:
  1,270 lines, +392/+31% since — drifting fast). `GuiApp` struct + `ApplicationHandler` impl
  (`window_event`'s 3 largest arms were already extracted to private methods during the move). All
  state lives in `self` fields — a real candidate for a future per-arm/per-phase split, not
  attempted this pass.
- `crates/mae/src/collab_bridge/mod.rs` (was flat `collab_bridge.rs`) — 5,302 lines (was 6,546).
  `handle_collab_event` (28-arm match) and `drain_collab_intents` (~10 drain sections) split into
  sibling `events_kb.rs`/`events_connection.rs`/`events_doc.rs` (all under 800). The remaining debt
  is narrower and more precise now: `run_collab_task` (1,695 lines) is a `tokio::select!` loop with
  ~19 raw locals threaded across 29 `CollabCommand` match arms and no state struct to group them —
  see its own in-code `@ai-caution` marker for why it's deliberately NOT split mechanically.
  `handle_response`/`handle_disconnected_cmd` also remain unmoved (reasonably sized on their own).
- `crates/core/src/editor/mod.rs` — 1,626 lines (2026-07: 1,382 lines, +244/+18% since — drifting
  fast). A dozen orphaned value-structs moved
  into the sibling files that already imported them (`lsp_state.rs`, `git_ops.rs`, `ai_state.rs`);
  ~90 `impl Editor` methods regrouped into `window_ops.rs`/`render_ops.rs`/`session_ops.rs`/
  `conversation_ops.rs` (new) plus extended `keymaps.rs`/`option_ops.rs`/`project_ops.rs`. Residual
  is the `Editor` struct definition itself (~410 lines, separately tracked as field-count debt, see
  its own `@ai-caution: [dispatch]` marker) plus constructors and small lifecycle methods.
- `crates/scheme/src/runtime.rs` — Scheme VM runtime; `SchemeRuntime::new()`'s ~186 `register_fn` calls and
  `inject_editor_state`/`apply_to_editor` were split into `crates/scheme/src/runtime/*.rs` submodules by
  category (keybindings/editor-ops/kb-primitives/kb-queries/io-packages/misc-primitives/test-primitives/
  state-sync). The residual `runtime.rs` (SharedState, SchemeRuntime's core methods) is 990 lines (2026-07:
  ~950); its `#[cfg(test)] mod tests` was extracted to a sibling `runtime_tests.rs` — now **2,059 lines**
  (2026-07: ~1,526, +533/+35% since — now ~4x the 500-line test-file ceiling, the worst-drifted file in
  this list, a strong candidate for its own splitting pass) — both still over ceiling but no longer
  sprawling
- `crates/scheme/tests/r7rs_compliance.rs` — R7RS spec compliance tests (large by nature)
- `daemon/src/collab_handler/mod.rs` (was flat `collab_handler.rs`) — 1,936 lines (was 3,821).
  `handle_doc_request_inner`'s 31-arm match split into sibling `sync_methods.rs`/`docs_methods.rs`/
  `kb_membership.rs`/`kb_content.rs`/`kb_governance.rs` (all under 800; `kb_content.rs` lands 2 lines
  over) — it's now a thin ~340-line dispatcher. The residual is ~30 individually-reasonable
  auth/session/access-control functions (`run_session`, `verify_content_op`, `kb_access`,
  `verify_member_self_service_update`, etc.) that collectively still exceed the file ceiling — a
  candidate for a further domain-grouping split, not attempted this pass.

Both `collab_bridge_tests.rs` and `collab_handler_tests.rs` (and `crates/core/src/editor/kb_ops/kb_ops_tests.rs`)
were split into `tests/` submodule directories in the same pass — all resulting test files are under
the 500-line ceiling; see git history for the exact per-feature breakdown.

**2026-07 (round 5) additions** — confirmed over ceiling, not previously tracked here (all post-date
the 2026-07 splitting pass above — the KB graph view feature and ongoing membership/window work):
- `crates/core/src/editor/graph_view_ops.rs` — 4,464 lines. KB graph view Scheme/MCP-facing ops
  (navigation, zoom/pin, click handling). Candidate split: layout-drive / navigation / physics /
  overlay concerns.
- `crates/core/src/graph_view.rs` — 2,848 lines. `GraphView` core (scene graph, viewport, flattening).
- `crates/core/src/buffer.rs` — 3,648 lines. Rounds 1-4 (branch `fix/backlog-review-root-cause-patterns`)
  fixed a CRDT-offset *drift* bug here but never addressed *size* — still ~4.6x the 800-line ceiling.
- `shared/sync/src/membership.rs` — 3,455 lines. Signed-membership derivation (ADR-026); growing with
  the P2P mesh initiative's ongoing work.
- `crates/core/src/window.rs` — 3,437 lines.
- `shared/kb/src/lib.rs` — 3,577 lines.

None split this pass — that's design work, appropriately deferred; this entry exists so the debt is
discoverable (per this file's own cross-reference discipline above) instead of silently untracked.

Flag these if they've grown since last audit, but don't remediate without explicit request.

**2026-07 full-codebase audit** found ~60 additional files over these ceilings beyond the list
above (not yet individually tracked here) — see `ROADMAP.md`'s "Architecture Debt" section for the
summary; that list still needs a full re-audit/refresh (it predates the splitting pass described
above, so it may now also be stale in the other direction — double-check before trusting it). That
same audit pass also resolved two Phase-5 DRY findings (remote-cursor render duplication, the
git_status/notifications_view/kb_sharing hand-mirrored view pattern) via `render_common::collab_cursor`
and `crates/core/src/foldable_view.rs`.

## Test Organization (Rust convention — do NOT "fix" co-located tests)

Co-located unit tests are **idiomatic Rust**, not a smell. Apply these rules:

- **Unit tests** belong in a `#[cfg(test)] mod tests { … }` in the SAME crate as the code they test, so
  they can exercise **private** items (`#[cfg(test)]` compiles them out of release builds). NEVER recommend
  moving unit tests into `tests/` (integration crates can only see the public API — that would force
  `pub(crate)` leaks just for tests).
- **The maintainability lever is the 500-line test-module ceiling, not the convention.** When a
  `#[cfg(test)] mod tests` block is large, extract it to a **sibling submodule file** — `mod tests;` →
  `foo/tests.rs`, or `#[path = "tests/foo_tests.rs"] mod tests;` — keeping `#[cfg(test)]` + private access.
  MAE already does this (e.g. `crates/core/src/editor/tests/mouse_tests.rs`). Flag source files whose
  inline test module dominates the file (e.g. tests are >50% of lines, or push the file over the source
  ceiling) and recommend this extraction — it improves source signal-to-noise WITHOUT breaking convention.
- **Integration / e2e tests** (public-API, real daemon, multi-crate flows) correctly live in `tests/`
  (each its own crate). Don't move those into `#[cfg(test)] mod`.
- **Doc tests** (`///` examples) stay in doc comments.

## Architecture Constraints (from CLAUDE.md — non-negotiable)

1. **Concurrency from day one** — no global mutable state, no GIL patterns
2. **Modular display layer** — renderer is a trait; no platform code in core
3. **AI is a peer** — same Scheme API for human and AI, no separate "AI mode"
4. **LSP/DAP first-class** — structured data, not string scraping
5. **Module boundaries** — each crate has clear responsibility, no 10k+ files
6. **Runtime redefinability** — Scheme layer must support live reload
7. **Shared computation, backend-specific drawing** — layout/formatting in core, drawing in backends
8. **No hardcoding** — user-visible behavior through OptionRegistry; init.scm is the primary config surface
9. **Multi-client safety** — MCP server supports N clients, broadcast channels, bounded queues
10. **CRDT-first sync** — yrs/YATA for all collaborative state
11. **Local-first** — no spinners, works offline, daemon is optional
12. **Two-workspace separation** — daemon uses CozoDB+SQLite (no rusqlite conflict), editor uses CozoDB+sled
13. **Cross-platform parity (macOS + Linux)** — XDG-first dirs; portable shell tooling; no silent one-OS no-ops

## Design Principles

- **No bandaid fixes.** Every pattern must be structurally sound. Prefer designs reusable by future buffer
  kinds over one-off hacks.
- **Build toward the mode/package inflection point.** Hardcoded `BufferKind` matches outside of trait impls
  are structural debt.
- **Daemon is optional.** Editor must work standalone with local sled-backed CozoDB.
  `KbContext::query_layer()` transparently returns daemon or local query layer.

## Phased Process

### Phase 1: Structural Scan

```
For each file in scope:
  - Line count vs ceiling
  - Function count and max function length
  - Match arm count in largest match block
  - Struct field count
  - Import complexity (>15 use statements = smell)
  - pub fn / pub struct / pub enum count (API surface check)
```

Report files exceeding any ceiling. Prioritize by severity. Also:
- Grep `crates/*/src/lib.rs` for `@stability:` markers — report crates missing one.
- Flag any files >1500 lines in `crates/core/src/editor/`.
- Cross-reference crate count against `Cargo.toml` workspace members.

**For daemon workspace:** also check handler dispatch method count vs test coverage; scheduler task intervals
match config defaults; hygiene check categories match `shared/kb/src/hygiene.rs` constants.

### Phase 2: AI Slop Detection

1. **Over-verbose doc comments** — restating the function signature in prose
2. **Defensive match arms** — unreachable branches with "should never happen"
3. **Redundant error wrapping** — `.map_err(|e| format!("failed to X: {e}"))` when caller has context
4. **Copy-paste tool definitions** — identical struct patterns that should use a macro
5. **Unnecessary intermediate variables** — `let result = foo(); result`
6. **Traits with single implementations** — premature abstraction
7. **String allocations in hot paths** — `format!()` or `.to_string()` in per-frame code
8. **Cargo cult patterns** — stale `#[allow(dead_code)]`, `#[allow(unused)]`
9. **Comment archaeology** — `// removed`, `// TODO: remove`, `// was: ...`
10. **Padded descriptions** — tool/prompt descriptions inflated beyond usefulness

### Phase 3: Rust-Specific Anti-Patterns

1. **Clone storms** — `.clone()` on types that could use references or `Cow<'_, str>`
2. **Arc<Mutex<>> overuse** — when channels or owned state would be simpler
3. **Stringly typed** — `String` where an enum enforces invariants
4. **Missing `#[must_use]`** — on functions returning `Result`/`Option` that callers ignore
5. **Panicking in library code** — `unwrap()`, `expect()` outside of tests (esp. `.lock().unwrap()` on
   long-lived mutexes — poison cascades; prefer `unwrap_or_else(|e| e.into_inner())` or `parking_lot`)
6. **Blocking in async / on the event loop** — `std::fs`, `std::thread::sleep` in async or main-thread paths
7. **Silent `.ok()`** — swallowing errors without logging (esp. decode/apply paths on external bytes)
8. **Stale negative caching** — caching "not found" results that become stale on creation
9. **Non-atomic multi-lock operations** — acquiring multiple locks with gaps between them
10. **Unbounded allocations from external input** — no size limits on Content-Length, query results, etc.

### Phase 4: Cross-Workspace Consistency

1. **JSON-RPC contract alignment** — daemon handler methods match LRU query layer / client calls
2. **Type serialization round-trip** — `NodeKind` (etc.) serialized the same way in both directions
3. **Feature flag consistency** — shared crates compile with both `storage-sled` and default features
4. **Config option wiring** — every `[daemon]` config key has a matching OptionRegistry entry
5. **Error handling symmetry** — daemon errors map to clean client error types
6. **CozoDB schema consistency** — hygiene suggestion schema in cozo_store.rs matches daemon/hygiene.rs usage

### Phase 5: DRY, Reuse & Design-Principle Adherence (the structural-soundness pass)

The highest-value phase: find one-off code that *should* route through a shared abstraction, and
design-principle violations. The canonical lesson — the TOFU host-key prompt was built as a one-off, then
generalized into the **MiniDialog / ADR-024 notification bus**, and wiring a new caller revealed the shared
component itself needed extending. Hunt for both directions: (a) duplication that should be extracted, and
(b) a shared component that a new caller needs but that wasn't updated.

1. **Duplicated logic across backends (#7 — shared computation, backend-specific drawing).** Any layout
   math, span/highlight computation, content formatting, or coordinate transform that appears in BOTH
   `mae-renderer` (TUI) and `mae-gui` (Skia) is a violation — it belongs in `crates/core/src/render_common/`
   or `text_utils`. Grep for parallel logic between the two backends; the click-coordinate `window_relative`
   helper and `active_overlay`/`mini_dialog_layout` are the model (one shared fn, two thin drawing sites).
2. **Near-duplicate "magit-style buffer" implementations.** `git_status` + `notifications_view` +
   `kb_sharing` view all share the SAME pattern (semantic LineKind + View + CollapseKey + build → rope/spans
   + at-point dispatch + buffer-local keymap). If they were hand-mirrored rather than sharing a generic
   `FoldableView`/`SectionedView` abstraction, flag it — a 4th copy is the inflection point to extract a
   shared trait/helper. Check whether fold, span rendering, and cursor→line dispatch are genuinely duplicated
   (and whether a new buffer kind was added to `render_common::spans` or silently left uncolored).
3. **Multiple consumers re-deriving the same data → one builder.** Introspection/status that feeds the
   buffer + an MCP tool + a Scheme primitive should come from ONE source-of-truth builder (the
   `kb_sharing_snapshot` model: `build_snapshot` → buffer + `kb_sharing_status` + `(kb-sharing-status)`).
   Flag any place where the human path and the AI/MCP path compute the same thing separately (also a #3
   "AI is a peer" violation — divergent human/AI capability or representation).
4. **One-off solution where a shared component exists.** Before any new buffer kind, dialog, notification,
   pick-list, or option-read: was an existing component reused? If a new caller needs a shared component to
   do slightly more, the fix is to EXTEND the component (and re-verify all existing callers), not fork a
   private copy. Flag forks of MiniDialog, NotificationCenter, CommandPalette, render_common helpers.
5. **Hardcoded user-visible behavior (#8 no-hardcoding).** Magic numbers/strings that differ between users
   but aren't in the OptionRegistry; values read inconsistently (cached snapshot vs live `get_option`) —
   connect-critical/feature-gating reads must be live from the single source. (Also: docs/help/strings that
   say config.toml when init.scm is the primary surface.)
6. **Stringly-typed dispatch that should be an enum/intent.** Lifecycle actions built as
   `(execute-ex "kb-share …")` strings instead of a typed intent (the `KbCollabAction` →
   `queue_kb_collab_action` → one `CollabIntent` model). Flag command/MCP/Scheme paths that don't lower
   through the SAME intent.
7. **Hardcoded `BufferKind`/mode matches outside trait impls** — the documented inflection toward a
   mode/package abstraction; growing match arms on `BufferKind` are structural debt.

For each finding, state: the duplication/violation, the principle (#n), and the concrete extraction (which
shared module/trait/helper it should live in). Prefer "extract + update the shared component" over "add
another copy." A 2nd duplicate is a note; a 3rd is an extraction mandate (rule of three).

### Phase 6: Documentation & Onboarding (user-facing readiness)

Review `docs/` (esp. `README.md`, `docs/COLLABORATION.md`, getting-started), the help-system KB nodes
(`crates/core/src/kb_seed/`), and `:help`/`:tutor` entry points. Weight **onboarding** for new users over
exhaustive reference.

1. **First-run path exists and is correct.** Can a new engineer go clone/install → first edit → first AI
   action → (if relevant) first collab share, following docs verbatim? Flag missing/outdated install, config
   (`init.scm` is primary; `config.toml` is legacy bootstrap), and "hello world" steps. Verify
   commands/flags/option names match the code (stale flags are the recurring bug — e.g. past
   `--unix-socket` / `MAE_COLLAB_ADDR` drift; daemon flags are `--bind`/`--data-dir`/`--config`).
2. **Onboarding > reference for the landing docs.** A wall of every option/tool is worse than a short guided
   path with a few copy-paste examples and links to deeper reference. Flag reference-dumps that bury the
   getting-started flow.
3. **Examples are runnable + current.** `init.scm` snippets, command sequences, and Scheme examples must
   actually work against the current API (no invented hooks/primitives). Cross-check against the code.
4. **AI-peer onboarding.** A short "drive MAE with your agent" path (MCP shim, `kb_sharing_status`-style
   introspection) so the engineer's agent is useful from day one.
5. **Discoverability.** New features reachable via the leader/which-key tree + documented (`SPC` menu), not
   only via `:command`. Flag capabilities with no keybinding/menu entry.
6. **Honest state.** Docs must not claim unimplemented behavior (e.g. a stubbed discovery). Flag over-claims;
   mark not-yet features clearly. Cross-reference deferred items to their tracking issues where applicable.

Report concrete doc edits (onboarding gaps, stale commands/flags, missing first-run steps), not vague
"improve docs."

### Phase 7: Metadata & Coverage (quick mechanical pass)

A fast, mostly-grep checklist. Report `[OK]` / `[WARN]` / `[ERROR]` per item.

1. **Module system maturity** — parse all `modules/*/module.toml`: validate required fields (`name`,
   `version`, `description`, `mae_version`, `category`), semver format, `mae_version` constraint. Check each
   `autoloads.scm` for a `(provide-feature …)` call and an `@module` header. Report the module dependency
   graph (modules that declare `[dependencies]`).
2. **Scheme API documentation coverage** — count `register_fn` registrations across
   `crates/scheme/src/runtime.rs` and `crates/scheme/src/runtime/*.rs` (the `register_fn` calls live in the
   `register_*_fns` submodules; `runtime.rs` itself only holds `SchemeRuntime::new()`'s dispatch calls plus a
   handful of registrations before the module split); count `scheme:*` nodes in `crates/core/src/kb_seed/`.
   Report undocumented primitives (registered but no KB node) and orphan KB nodes (node but no registration).
   Spot-check documented arities against the registered `Arity`.
3. **AI provider coverage** — check `crates/ai/src/context_limits.rs` TABLE for model coverage (list model
   prefixes); verify `ProviderHint::from_model()` covers every family in the TABLE; report verification-status
   distribution (Verified / Testing / Untested). Confirm prompt XML exists for all profiles × tiers
   (`pair-programmer`, `explorer`, `planner`, `reviewer`, `verifier` × `{,-compact}`). Cross-reference models
   in `context_limits.rs` vs `pricing.rs` — report models with limits but no pricing (intentional for OSS
   models) and vice versa.

## Output Format

```markdown
## MAE Audit Report: <scope>

### Ceiling Violations
| File | Lines | Ceiling | Status |
|------|-------|---------|--------|
(Known exceptions marked "tracked", new violations "NEW")

### AI Slop Found
- [ ] <file>:<line> — <smell type>: <description>

### Anti-Patterns
- [ ] <file>:<line> — <pattern>: <description>

### Cross-Workspace Issues
- [ ] <description>

### DRY / Reuse / Design-Principle Adherence
- [ ] <file:line> — <duplication or violation> (principle #n) → extract to/reuse <shared component>

### Documentation & Onboarding
- [ ] <doc:section> — <onboarding gap / stale command / over-claim> → <concrete edit>

### Metadata & Coverage
- [OK]/[WARN]/[ERROR] <check> — <result>

### Remediation Applied
- <description of each change>

### Verification
- Editor tests: <count> passed
- Daemon tests: <count> passed
- `make ci`: pass/fail
- Clippy warnings: <count>
```

## Execution Notes

- Use `cargo clippy --workspace -- -D warnings` (editor) and `cd daemon && cargo clippy --all-targets -- -D warnings` (daemon)
- Use `wc -l` for line counts, `grep -c "pub fn"` for API surface
- Use parallel agents for multi-crate audits (one agent per crate); Phases 5–6 (DRY/reuse + docs) benefit
  from a cross-cutting agent that looks ACROSS crates for duplication, not per-crate
- Don't create busywork: if the code is clean, say so and stop
- Run after major milestones, not after every commit
- Default scope is recent changes — full audit only when explicitly requested
- Test count must be preserved or increased after any remediation
- **Never recommend moving unit tests to `tests/`** — co-located `#[cfg(test)] mod tests` is idiomatic; the
  only test refactor to suggest is extracting a LARGE inline test module to a sibling submodule file
- If MAE is running with MCP, you may also call `audit_configuration` (runtime config health) and
  `self_test_suite` (structured test plan — check availability, don't execute)
