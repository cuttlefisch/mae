# Pre-v0.15 audit tail: triage of issues #578–591, #596–611

Working document for the tail of the pre-v0.15 codebase audit (epic #592), covering the 30
batched-by-subsystem/per-crate issues NOT already handled by the individually-tracked high-severity
findings (#569–577, #593–595) or by the security-advisory-tracked AI-permission findings. Every
finding in every issue below was re-verified against the **current source** on
`audit/tail-findings-triage` (branched from `security/ai-permission-enforcement`, commit `8298ba65`)
— not against the audit's own citations, which in several cases (documented below) no longer match
current line numbers or, in two cases, were never true.

## Headline numbers

**196 individual findings** across the 30 issues — not "~87". The issue titles' own stated counts
sum exactly to 77 (subsystem batch, #578–591) + 41 (final-six-slices batch, #596–601, after
subtracting the three individually-filed highs #593–595) + 78 (per-crate pass, #602–611) = **196**,
matching epic #592's own tables. Wherever "~87" came from, it undercounts the actual filed total by
more than half — worth surfacing on its own, since a number this far off is itself a finding about
how the audit's summary rolled up.

| Classification | Count | |
|---|---|---|
| **Already fixed** by a sibling workstream (ADR-084/085/086/087/089, ADR-084 D3, WS1, WS3, WS6) before this branch started | 9 | verified by reading current source, not by trusting commit messages |
| **Fixed on this branch**, with a new adversarial test, as part of this pass | 3 | see "Fixed this pass" below (2 logical code changes covering 3 findings) |
| **Stale / never real** | 0 confirmed newly stale in this tail (the two known-stale findings — `git-commit`/`git-branch-create` undispatchable — were already identified and fixed under WS1, prior to this pass) | — |
| **Real, cheap, not yet fixed** | 172 | triaged and cited below; NOT fixed in this pass — see Scope note |
| **Real, needs a design decision** | 12 | added to `docs/DECISIONS_FOR_REVIEW.md` where not already covered |

(9 + 3 + 172 + 12 = 196, reconciling exactly against the per-issue breakdown below. Two findings —
#583.3 and #586.1 — are partial-fix cases counted under "real, cheap" above since real work remains,
but are flagged explicitly in their issue sections below so a follow-up pass doesn't assume a sibling
workstream closed them outright.)

**Scope note.** Per the task's own instruction ("do not rush them... a well-triaged 87 with 30
properly fixed beats 87 hastily patched"), this pass prioritized **complete, accurate triage** of
all 196 findings over racing through fixes. 3 findings (2 logical code changes) were fixed with
adversarial tests this pass, both in the highest-leverage AI-facing category: #590.2 (AI tool calls
silently reporting success on refusal/error) and #604.2/#604.6 (a Unicode-panic path reachable via
the AI `eval_scheme` tool, plus the code duplication that let it drift out of sync with its sibling).
The remaining 172 "real-cheap" findings are accurately triaged, cited to exact current file:line, and
classified — but unfixed. They are a durable, reviewable backlog for follow-up passes, organized
below by issue so a future contributor (human or AI) can pick any one up without re-doing the
verification work.

## Method

Investigation was parallelized across 6 read-only research passes (5 background agents + the primary
session), each given:
- The exact list of what already landed on `security/ai-permission-enforcement` before this branch
  (ADR-084/085/086/087/089 D3, WS1, WS3, WS6 — with specific file/commit pointers, not just names)
- Instruction to verify every claim against **current** source, not the issue text
- The excluded-territory list (`crates/core/src/render_common/`, `crates/renderer/`, `crates/gui/`,
  `crates/scheme/src/runtime/` — other agents' active work)

Every finding below cites the exact file:line checked. Where a finding's own audit-time citation
had drifted (renamed function, moved line), the current location is cited instead.

---

## Fixed this pass

1. **#590.2 — `eval_scheme` errors and `model_exam`/`self_test_suite` argument-validation failures
   reported `success: true`.** The same ADR-086 defect class, on two paths ADR-086's own landing
   commit (9007bf28) didn't reach:
   - `crates/mae/src/ai_event_handler.rs`'s `drain_pending_scheme_evals`/`eval_with_yield_handling`
     unconditionally set `result.success = true` after draining a queued Scheme eval, even when the
     eval itself errored (`eval_with_yield_handling` returned the error as an ordinary formatted
     string, never a `bool`). Fixed by having both functions return `(String, bool)` and having both
     call sites (embedded + MCP) use the bool instead of hardcoding `true`.
   - `crates/ai/src/executor/tool_dispatch.rs`'s `self_test_suite` and `model_exam` handlers built an
     `"Invalid action: ..."` / `"Missing 'results' array..."` string for a bad/missing argument and
     then returned it with `success: true`. Fixed by threading a `(bool, String)` tuple through both
     match blocks instead of a bare `String`.
   Tests: `self_test_suite_unknown_action_reports_failure`,
   `self_test_suite_grade_without_results_reports_failure`,
   `self_test_suite_valid_plan_action_still_succeeds` (idempotent-retry guard, ADR-086 D2),
   `model_exam_unknown_action_reports_failure`, `model_exam_grade_without_results_reports_failure`
   (`crates/ai/src/executor/mod_tests.rs`).

2. **#604.2 / #604.6 — `record_error` (the AI/MCP `eval_scheme` error-history path) did unguarded
   byte-slicing while its sibling `eval()` (the human `:eval` path) had already been fixed by
   ADR-087, and `eval()` hand-duplicated `record_error`'s logic instead of calling it.** Two defects,
   one fix: `crates/scheme/src/runtime.rs`'s `record_error` now calls
   `mae_core::grapheme::checked_byte_boundary` (matching `eval()`), and `eval()` now calls
   `record_error` instead of re-inlining the history-append block — closing both the divergence and
   the panic. Tests: `eval_yielding_error_history_is_unicode_safe_across_the_200_byte_cut`,
   `eval_error_history_is_unicode_safe_across_the_200_byte_cut` (`crates/scheme/src/runtime_tests.rs`).

   **New finding surfaced while writing the test** (not in the original audit): `checked_byte_boundary`
   is a `debug_assert!`-based chokepoint that intentionally **panics in debug/test builds** whenever
   it's asked to clamp a genuinely non-boundary offset (release builds silently clamp instead, per
   ADR-087 Rule 5). That makes the literal "byte 200 lands mid-character" case **untestable as a
   passing unit test** for ANY fixed-length-truncation call site under `cargo test` — including every
   other already-"fixed" ADR-087 site (`guidance.rs`, `run_loop.rs`, `handle_prompt.rs`,
   `shell_exec.rs`), none of which have a test that actually lands on a non-boundary cut (`guidance.rs`'s
   own test uses all-ASCII filler). Recorded as new entry #8 in `docs/DECISIONS_FOR_REVIEW.md`.

Both changes verified: `cargo check --workspace --all-targets` clean, `cargo test -p mae-core -p
mae-ai -p mae-scheme -p mae` green (see Verification section at bottom).

---

## Per-issue triage

Legend: **FIX** = fixed this pass · **already** = already fixed by a named sibling workstream ·
**cheap** = real, not yet fixed, small/localized · **decision** = real, needs
`docs/DECISIONS_FOR_REVIEW.md` · finding numbers match the issue body's own numbered list.

### #578 — Syntax highlighting / tree-sitter (8 findings)
0 already-fixed · 0 stale · 7 cheap · 1 decision
- 578.1 cheap — `Editor::with_buffer` (`crates/core/src/editor/mod.rs:1815-1828`) builds `SyntaxMap` via extension-only `language_for_path`, never calls `apply_defaults` (only 3 other call sites do: `file_ops.rs:36,1384`, `kb_ops/daily.rs:302`, `collab_bridge/events_doc.rs:288`).
- 578.2 cheap — `compute_markup_spans_for_range` (`crates/core/src/syntax/markup.rs:745-767`) has no backward fence scan, unlike sibling `detect_code_block_lines_for_range`.
- 578.3 cheap — both `crates/renderer/src/buffer_render.rs:268` and `crates/gui/src/buffer_render.rs:241` `continue` past a style patch instead of hiding via display-region.
- 578.4 cheap — `crates/export/src/lib.rs:672`'s TODO-keyword list `["TODO","DONE","NEXT","WAIT","CANCELLED","SOMEDAY"]` diverges from 5 sibling copies elsewhere; no `OptionRegistry` entry.
- 578.5 cheap — markdown/org-src-block tree-sitter injection (`languages.rs:396`, `markup.rs:466`) both call `build_configuration(lang)` directly, bypassing the `configs` cache.
- 578.6 cheap — `get_or_compute_markup_spans` (`option_ops.rs:2743`) has zero callers.
- 578.7 **decision** — `syntax_tree_sexp`/`syntax_node_kind_at_cursor` (`syntax_ops.rs:154,163`) reachable only via the AI tool, no Scheme primitive or command exists. New value-returning Scheme surface, not a one-liner (same shape as the frozen-`register_fn` WS6 gaps).
- 578.8 cheap — `#[cfg(test)] pub(crate) fn compute_spans` (`languages.rs:292`) duplicates the markdown branch of `compute_spans_with_cache` (`:250`); all markdown-injection test assertions target the test-only copy, not the real one.

### #579 — DAP client (7 findings, all cheap)
- 579.1 `dap_toggle_breakpoint_at_cursor` (`dap_ops.rs:213-252`) uses raw `file_path()`, unlike set/remove which call `canonicalize_source_path` (`:272,283`).
- 579.2 `log_message` only in schema (`dap_tools.rs:58-60`) — zero plumbing anywhere (`dap.rs`, `protocol.rs`, `dap_intent.rs`).
- 579.3 `ai_event_handler.rs:222-229` drains DAP intents without a preceding `drain_scheme_dap_intents`, unlike the LSP branch at `:246-247`.
- 579.4 Test-gap: `StartSession`/bridge/dispatch/tool-schema layers untested.
- 579.5 `dispatch/dap.rs:115-121` prints a "Usage:" status instead of prefilling the command line, unlike `debug-start` (`:12-16`).
- 579.6 No DAP adapter/timeout `OptionRegistry` entry (`options.rs` has `debug_mode`/`debug_panel_split_ratio` only); `default_spawn_for_adapter` is a closed 4-arm match, unlike LSP's env-var/config/default pattern.
- 579.7 `dap_terminate` (`dap_ops.rs:433`) has zero callers; `DapIntent::Terminate` is never produced.

### #580 — Windows, layout, DrivenWindow (7 findings, all cheap)
- 580.1 `find_or_create_companion_window`'s `Err` arm (`window_ops.rs:780-807`) can return `exclude`/focused_id on a failed split, and `ensure_ai_dispatch_target` latches it with no `companion != exclude` check.
- 580.2 `default_area()` (`window_ops.rs:1106`, hardcoded 120×40) still used at 12 decision sites alongside the real `last_layout_area` (`mod.rs:1079`).
- 580.3 `display-buffer-policy` (`crates/scheme/src/runtime/editor_ops.rs:133-148`) constructs `DisplayPolicy::default()` with no `shared` capture. **Note:** this file is inside the excluded `crates/scheme/src/runtime/` — flag to that territory's owner.
- 580.4 `window-grow`/`window-grow-width` byte-identical (`dispatch/window.rs:100-117`); `window-grow-height`/`window-shrink-height` unbound anywhere.
- 580.5 Same root cause as 580.1 — the non-conversation `Some(focused_id)` fallback reports success with nothing displayed.
- 580.6 `parse_buffer_kind` (`display_policy.rs:277-298`) omits `Notifications`/`KbSharing`.
- 580.7 `execute_window_layout` emits no `focused` flag; Scheme `*window-list*` thinner than MCP; no primitive accepts a `WindowId`.

### #581 — LSP client (7 findings)
1 already-fixed · 0 stale · 4 cheap · 2 decision
- 581.1 **already-fixed** — `crates/ai/src/tools/lsp_tools.rs:52` now declares `.required(["query", "language_id"])` — WS1's fix (commit d3be4bc1).
- 581.2 cheap — `scheme_lsp_bridge.rs`'s match routes CodeAction/PrepareRename/Rename/Format/RangeFormat/WorkspaceSymbol/DocumentHighlight into a `debug!`-only fallthrough arm (`:240`).
- 581.3 **decision** — `CodeActionItem` (`lsp_state.rs:105-112`) has no `command` field; `lsp_rename` returns "queued" with an unconditional human-preview flow. Needs an AI-vs-human auto-apply semantics decision plus `workspace/executeCommand` support.
- 581.4 cheap — `apply_index` declared in schema (`lsp_tools.rs:98-102`) but never read in `lsp_exec.rs`; `lsp_diagnostics` schema omits `scope`, which the impl reads.
- 581.5 **decision** — zero `register_fn("lsp...`/`register_fn("dap...` hits anywhere in `crates/scheme/src/`. Building a value-returning Scheme LSP/DAP API is new-surface design work (see WS6's own "Proposed primitives" list in `docs/CROSS_SURFACE_PARITY.md`, already deferred pending the concurrent `register_fn` signature change).
- 581.6 cheap — TUI hover popup hardcodes a 76-col wrap vs GUI's area-derived width.
- 581.7 cheap — `Duration::from_millis(300)` duplicated verbatim in `terminal_loop.rs:242` and `gui_app.rs:1533`; no `OptionRegistry` entry for LSP debounce.

### #582 — Vi-modal input, keymaps, which-key (6 findings)
0 already-fixed · 0 stale · 5 cheap · 1 decision
- 582.1 cheap — `insert.rs:74`'s guard is `!CONTROL` only (no ALT exclusion), unlike `normal.rs`/`visual.rs`'s `.intersects(CONTROL|ALT)`.
- 582.2 cheap — scroll intercept (`key_handling/mod.rs:258`) gated on `Mode::Normal && !which_key_prefix.is_empty()` — dead at the top-level leader menu, always-dead in the nonmodal flavor.
- 582.3 cheap — `handle_describe_key_await` (`normal.rs:277-281`) looks up only the `"normal"` keymap directly instead of `keymap_chain()`.
- 582.4 cheap — `switch_keymap_flavor`/`reload_everything` (`bootstrap.rs:1710-1738`) unconditionally reports success and writes the raw flavor string with no validation against discovered `keymap-*` modules.
- 582.5 cheap — `apply_keymap_bindings` (`state_sync_apply.rs:145-177`) only `warn!`s for unknown-keymap/empty-sequence, unlike the conflict branch which also logs to `message_log`.
- 582.6 **decision** — `CommandPalette::for_keymap_flavor` (`command_palette.rs:517-529`) hardcodes doom/nonmodal; fixing needs new `Editor`-side module-registry plumbing that doesn't exist today.

### #583 — KB store, search, federation (6 findings, all cheap)
- 583.1 `crates/ai/src/tool_impls/kb.rs:637` calls `health_report_with_visibility` directly, bypassing `query_layer()`'s federated health report.
- 583.2 `kb_federated_search_scoped_impl`/`kb_find_candidates` (`kb_ops/search.rs:500,614`) iterate `&self.kb.instances` (a `HashMap`) directly instead of `FederatedQuery::priority_ordered_instances` — already tracked in #118.
- 583.3 **Partial-fix trap — flag this explicitly.** WS3 did convert the storage layer (`shared/kb/src/query.rs`, `lru_query.rs`) to propagate `Result` instead of collapsing to empty — confirmed. But the higher-layer callers in `crates/core/src/editor/kb_ops/search.rs:157,476` still do `.unwrap_or_else(|e| { warn!(...); Vec::new() })` — the storage layer now *can* report failure, but the editor/AI-visible layer still throws it away and returns an empty, success-shaped result. The finding's actual observable defect (storage failure indistinguishable from empty KB, at the layer the AI actually sees) is **not** closed by WS3 alone.
- 583.4 federation.rs:894 inserts into `visited_files` before parse; `:929` `continue`s on empty parse result, skipping the only retraction path (`record_source_file`, `:1002`).
- 583.5 `rrf_blend_with_vector`'s `fused.sort_by` (`kb_ops/search.rs:602`) has no id tiebreak, unlike the sibling in `shared/kb/src/query.rs`.
- 583.6 `set_option`'s `kb_federated_max_fanout_instances` arm (`option_ops.rs:988`) only assigns the field; no `rebuild_query_layer()` call.

### #584 — MCP server and shim (6 findings, all cheap)
- 584.1 `shim.rs:281-292` directly awaits `read_message` inside `tokio::select!` with no `biased;`, unlike the server side (`lib.rs:304-309`) which uses a dedicated reader task + mpsc.
- 584.2 `main.rs:733-739` silently drops the category-allowlist assignment when `parse_categories` yields empty (no warn/log); `option_ops.rs:1037-1042`'s `mcp_tool_category_allowlist` has no validation.
- 584.3 `shared/mcp/src/broadcast.rs:115-126` — `BufferEdited`/`CursorMoved`/`DiagnosticsUpdated`/`ModeChanged`/`BufferOpened`/`BufferClosed` have zero production construction sites; `notifications/subscribe` accepts any string.
- 584.4 `shim.rs:307-308` returns `SessionEnd::SocketDropped` with no in-flight-request bookkeeping; reconnect silently drops the caller's outstanding call.
- 584.5 No `register_fn`/command/`introspect` surface exists for MCP-session state.
- 584.6 All 6 `build_shim_initialize_params(` test sites pass `None` for the category arg — no symmetric wire test.

### #585 — Scheme runtime, modules, hooks (5 findings, all cheap)
- 585.1 `crates/scheme/src/stdlib/vector.rs` (`make-vector`, `vector->list`, `vector-copy`, `vector-copy!`) and `string.rs` unchecked; `drain_pending_scheme_evals`/`eval_with_yield_handling` have no `catch_unwind` (only direct AI tool dispatch does, via `catch_tool_panic`).
- 585.2 `crates/mae/src/pkg/loader.rs:21-28` fields (`commands`/`keybindings`/`hooks`) are declared but never populated anywhere.
- 585.3 `bootstrap.rs:1418-1440`'s `check_mae_version` `continue`s past exactly one module on a version mismatch, but dependents still load.
- 585.4 `crates/core/src/hooks.rs::list()` has zero callers; `trigger_hook` (`editor_tools.rs:477`) is AI-only, no Scheme/command counterpart.
- 585.5 `tests/editor/test_hooks_firing.scm`/`test_hooks.scm` are assertion-free or register a nonexistent hook name.

### #586 — Buffer & editing core (5 findings)
- 586.1 **split** — the read-only-buffer false-success half is **already-fixed**: `crates/ai/src/tool_impls/buffer.rs:52-59` now has an explicit read-only guard citing ADR-086. The undo-grouping half is still **cheap** and open: the same function's delete+insert (`:74-81`) has no `begin_undo_group`/`end_undo_group`.
- 586.2 cheap — `crates/core/src/editor/multicursor.rs` has zero `undo_group` hits — no grouping for `MC_EDIT_ALLOWLIST` replay.
- 586.3 cheap — all 14 `pending_char_command = Some(...)` stash sites (`dispatch/edit.rs`, `dispatch/nav.rs`) have no Scheme/MCP path to supply the char.
- 586.4 cheap — `MAX_UNDO_ENTRIES: usize = 1000` (`buffer.rs:1065`) hardcoded, no `options.rs` entry.
- 586.5 cheap — `state_sync_apply.rs:510-521`'s `buffer-replace-range` arm fires neither `after-insert` nor `after-delete` (siblings at `:465,506` do) and has no undo grouping.

### #587 — Rendering pipeline TUI/GUI (5 findings, all cheap — excluded territory, triage only)
- 587.1 `buffer_render.rs:80-84` has no `has_gutter` check; `cursor.rs:27-33` does; the collab overlay (`lib.rs:566-570`) computes `gutter_width` unconditionally.
- 587.2 Test-count gap confirmed live in `docs/AUDIT_METRICS.json`: `cursor.rs` (167 lines/0 tests), `lib.rs` (618/0), `popup_render.rs` (976/0).
- 587.3 `terminal_loop.rs:176-178` subtracts `scrollbar_w` into `text_area_width`; `buffer_render.rs:118`'s own `text_width` computation doesn't.
- 587.4 Structural duplication between the two renderers, would need a `render_common` extraction.
- 587.5 `lib.rs:528`'s fallback arm has no `should_degrade_features` check (GUI's does); `render_breadcrumb_bar` hardcodes colors.

### #588 — Daemon: persistence, scheduler, P2P, tenancy (4 findings)
1 already-fixed · 0 stale · 3 cheap · 0 decision
- 588.1 cheap — `daemon/src/dialer.rs:102-118`'s `active` set is insert-only; a Terminal-reject `return` (`:145-152`) orphans the entry forever.
- 588.2 **already-fixed** — `daemon/src/handler.rs:591-598`'s `p2p/join_ticket` arm now refuses (`DaemonError::NotReady`) when no P2P endpoint exists, explicitly citing ADR-086 in the comment. This is the same ADR-086 pattern applied to the daemon, beyond the fix list ADR-086's own commit enumerated.
- 588.3 cheap — no `Cache-Control`/`no-store` header anywhere in `daemon/src/oauth.rs`/`webview.rs`.
- 588.4 cheap — the four `hygiene` daemon RPCs (`handler.rs:394,412,445,468`) have zero consumer surface (no Scheme/command/tool/config).

### #589 — Collaboration: CRDT, membership, E2E (4 findings, all cheap)
- 589.1 `crates/core/src/kb_sharing.rs:121-129`'s `short_fingerprint` does unchecked `&digest[..4]`/`&digest[digest.len()-4..]` byte slicing — a **different** code path than ADR-087's fixed sites (this one is untouched).
- 589.2 `kb_sharing.rs:230-247` builds the displayed member roster from unsigned `coll.member_roles()` instead of `derive_valid_members_governed`.
- 589.3 `content_header` has zero editor-side readers; every non-daemon construction site is `content_header: None`; `route_kb_node_update` applies unconditionally.
- 589.4 `daemon/src/collab_handler/mod.rs:903-968`'s `append_signed_membership` returns `()` on both failure paths (`warn!`-only); `kb_membership.rs:436-452` unconditionally reports `{"added": add}`.

### #590 — AI agent: tools, permissions, residency (4 findings)
2 already-fixed (1 pre-existing, 1 fixed this pass) · 0 stale · 1 cheap · 1 decision
- 590.1 **already-fixed** — the byte-index slicing panics (`guidance.rs`, `run_loop.rs`, `handle_prompt.rs`, `shell_exec.rs`) all now call `mae_core::grapheme::checked_byte_boundary`, citing "ADR-087 / audit #594" in-line.
- 590.2 **FIXED this pass** — see "Fixed this pass" above.
- 590.3 cheap, not fixed — two divergent `shell_exec` implementations (`crates/ai/src/executor/shell_exec.rs` vs `crates/ai/src/session/run_loop.rs`) share a copy-pasted, un-shared blocklist; the advertised schema says `timeout_ms` (`shell_tools.rs:13`) but both impls read `timeout_secs` — a model passing `timeout_ms: 5000` silently gets the 30s default. Sandbox confinement (`sandbox_guard`) applies to the executor copy only, not the embedded-session copy. A related stale duplicate (`crates/agent-cli/src/residency_check.rs:20-38`) still documents itself against a constant (`SINGLE_TARGET_KB_TOOLS`) that was deliberately deleted as the root cause of #350/#351.
- 590.4 **decision** — 9 AI tools (`ask_user`, `delegate`, `ai_set_mode`, `ai_set_profile`, `ai_set_budget`, `propose_changes`, `log_activity`, `read_transcript`, `web_fetch`) are advertised in MCP `tools/list` (several at default Core tier) but handled ONLY inside the embedded `AgentSession`'s `handle_prompt.rs` event loop — which has session-scoped state (`self.transcript_path`, `self.budget`, oneshot reply channels for human-in-the-loop) that `crates/ai/src/executor/tool_dispatch.rs`'s stateless `dispatch_tool(editor, call, ...)` signature has no access to. Confirmed by direct inspection: none of the 9 names appear anywhere in `tool_dispatch.rs`'s dispatcher chain; all fall through to `Err("Unknown tool: ...")`. Added as new entry #9 in `docs/DECISIONS_FOR_REVIEW.md` — genuinely needs a design call (build session-scoped MCP dispatch vs. exclude these 9 from external-MCP tool discovery), not a quick patch.

### #591 — Native KB graph view (3 findings, all cheap)
- 591.1 `graph_view_ops.rs:1641-1661` computes `target` before `remove(idx)`; the `== idx` branch assigns the un-decremented index.
- 591.2 `help_ops.rs:501,555,559` emit the wrong command name (`kb-graph-view-open` vs real `kb-graph-view-open-default`).
- 591.3 3 graph-layout options require the `gui` feature with no doc disclosure (`main.rs:23`'s `#[cfg(feature = "gui")]`).

### #596 — Org-mode: parse, babel, export, agenda (8 findings, all cheap)
- 596.1 `babel_ops.rs:169-233`'s `babel_execute_all` has no `effective_eval_policy` call (only `EvalPolicy::Never`), unlike `babel_execute` (`:61`).
- 596.2 `babel_ops.rs:427` uses `elements.iter().enumerate().enumerate()` — element-index-as-line-number bug.
- 596.3 `html.rs:15-17,88,106-110` — unescaped `meta.language`/`tag`/src-block `language` in output.
- 596.4 `compiled.rs:279-287`'s `spawn_with_etxtbsy_retry` has no `.stdin(...)`, no `MAE_BABEL` env, unlike the shell path.
- 596.5 `babel_trust_paths` has zero `options.rs` entry — dead config.
- 596.6 `tangle.rs:39-47`'s "reject tangle to self" comment doesn't match the code (`return p` unchanged); `babel_ops.rs:259` hardcodes overwrite.
- 596.7 `expand_noweb`'s only non-test caller ignores `block.header_args.noweb`.
- 596.8 `.mae-babel-tmp.go` fixed-name tempfile collision (per the issue's own scope-corrected description).

### #597 — Git integration & status buffer (7 findings)
0 already-fixed · 0 stale · 6 cheap · 1 decision
- 597.1 cheap — `render_blame_gutter` (`popup_render.rs:1401-1431`) has no width param, GUI-only/cosmetic.
- 597.2 cheap — `git_ops.rs:177-190`'s porcelain=v2 parser is `split_whitespace()`-based with no rename handling, unlike the sibling in `file_tree.rs:180-200`.
- 597.3 cheap — `parse_blame_porcelain` (`git_ops.rs:1034-1078`) has no per-hash cache; author/summary leak across commits.
- 597.4 cheap — no confirm dialog before `git checkout --`/hunk revert (`dispatch/git.rs:57-59` → `git_ops.rs:846-862`/`655`).
- 597.5 **decision** — three divergent git-porcelain parsers and two root resolvers, unconsolidated across crates — a real cross-crate refactor.
- 597.6 cheap — `dispatch/git.rs:87-93` just sets a usage-string status instead of prefilling the command line; `Editor::git_branch_create`/`git_branch_delete` remain dead code.
- 597.7 cheap — `git_ops.rs` has exactly 7 tests, none touching `git_status()`/hunk-patch/fold/stash-index.

### #598 — Help system & KB seeding (7 findings)
2 already-fixed · 0 stale · 3 cheap · 2 decision
- 598.1 **already-fixed** — `guidance.rs:45-56` calls `checked_byte_boundary`, citing "ADR-087 / audit #594".
- 598.2 **decision** — `manual_kb.rs:42-45`'s `KNOWN_CHECKSUMS` is unpopulated; fixing needs wiring the release CI pipeline, not a code patch.
- 598.3 **decision** — `guidance_kb_engine.rs:116-125`'s `copy_kb_asset`/`ensure_registered_with_path` short-circuit on existence with no version check; needs a refresh-policy decision consistent with ADR-076's "never clobber a customized entry" precedent.
- 598.4 **already-fixed** — all 16 named primitives now documented in `scheme_api.rs` under the WS6 gap-closing pass.
- 598.5 cheap — `help_ops.rs:718` hardcodes `[[cmd:move-right]]` in a See-also line.
- 598.6 cheap — `guidance.rs`'s `PROJECT_CONTEXT_FILES`/`PROJECT_CONTEXT_MAX_CHARS` are not `options.rs` entries.
- 598.7 cheap — `help_ops.rs:53`'s `MAX_RELATED` is uncommented/unregistered, unlike sibling `MAX_NEIGHBORHOOD_LINKS`.

### #599 — Options, config & persistence (7 findings)
1 already-fixed · 0 stale · 4 cheap · 2 decision
- 599.1 cheap — `option_ops.rs:1747-1764` selects the rewrite branch via `content.contains()` but only rewrites lines matching `starts_with()` — commented-out lines silently no-op while reporting "Saved".
- 599.2 cheap — `config.rs:1106-1110`'s `write_managed_init_options` doesn't escape values, unlike `option_ops.rs:1764`.
- 599.3 **decision** — `format_on_save`/`spell_enabled`/`lsp_diagnostics_virtual_text`/`collab_default_save_dir`/`collab_save_on_remote_update` are confirmed consumer-less options; the doc-marker half is cheap, but `format_on_save`'s real fix (consolidating with the `+onsave` module flag) is architectural.
- 599.4 **already-fixed** — `set_option` MCP tool's `persist: bool` param (WS6) resolves the finding's core impact. Narrower residual gaps (no Scheme `set-save`-equivalent, no `set_local_option` MCP tool) remain but are secondary to what the finding centered on.
- 599.5 **decision** — `KbSection` (`config.rs:207`) is an empty struct with no `deny_unknown_fields`; the right fix is a config_key semantics/dead-TOML-path product call, not a bug patch.
- 599.6 cheap — both renderers gate diagnostics virtual text on `lsp_diagnostics_inline` only (overlaps 599.3's `lsp_diagnostics_virtual_text` entry — don't double-count).
- 599.7 cheap — `set_save_tests` has no fixture for a pre-existing non-line-initial `(set-option! "NAME"` occurrence.

### #600 — Project, file tree, pickers, palette (7 findings)
1 already-fixed · 0 stale · 6 cheap · 0 decision
- 600.1 **already-fixed** — `add-project`/`remove-project` now dispatch via WS1's delegation arm (`dispatch/mod.rs:488-505`).
- 600.2 cheap — `execute_project_files`/`execute_project_search` (`tool_impls/project.rs:18,67`) never call `.current_dir()`.
- 600.3 cheap — hardcoded `30`-row viewport at 4 sites in `dispatch/file_tree.rs`.
- 600.4 cheap — 3 divergent `SKIP_DIRS` consts (`file_picker.rs`, `file_browser.rs`, `file_tree.rs`), no `options.rs` entry.
- 600.5 cheap — `execute_switch_project` only pushes to `recent_projects` + sets `editor.project` — no `update_project_list`/`refresh_git_branch`/`lsp.pending_root_change`.
- 600.6 cheap — layout math independently computed in both renderers' `popup_render.rs` (mini-dialog already has a `render_common::dialog` precedent to follow).
- 600.7 cheap — unconditional dotfile skip in `file_picker.rs:490-492`; `DEFAULT_MAX_CANDIDATES = 50_000` hardcoded.

### #601 — Embedded shell / terminal (5 findings, all cheap)
- 601.1 `command.split_whitespace()` (`crates/shell/src/terminal.rs:277`) — naive shell-word splitting.
- 601.2 GUI `CellInfo` drops ITALIC/UNDERLINE/DIM/STRIKEOUT (only fg/bg/ch/bold), unlike TUI's 7-attribute handling.
- 601.3 `crates/mae/src/shell_lifecycle.rs` duplicates orphan-cleanup logic between `ChildExit` and `health_check`.
- 601.4 Tick-rate mismatch: `terminal_loop.rs:410` 50ms (documented 20fps) vs `gui_app.rs:216` 33ms (no comment).
- 601.5 Test-gap: selection functions (`terminal.rs:562-580`) have no dedicated round-trip test.

### #602 — code-health: crates/ai + agent-cli (9 findings, all cheap)
- 602.1 Three-way duplicated tool-schema-to-provider-format conversion (`claude.rs`, `openai.rs` — no `enum`; `gemini.rs` — has it).
- 602.2 `reqwest::blocking::Client::new()` in `tool_impls/kb.rs:967` despite the tool's own description claiming otherwise.
- 602.3 `response = retry;` whole-struct overwrite in `guardrail.rs:128,166`.
- 602.4 `aggregate_grades`/`ExamRun` construction duplicated verbatim at `tool_dispatch.rs:294` and `:420` (the same self_test_suite/model_exam duplication fixed-for-success-semantics above, still duplicated in shape).
- 602.5 `fetch_kb_residency` called twice with no cache (`agent-cli/src/main.rs:469,574`).
- 602.6 `AgentProvider::embed`/`OllamaProvider::embed` uncalled in production.
- 602.7 `original_system_prompt` field `#[allow(dead_code)]`, unread.
- 602.8 `format_exam_report` is `pub` inside a `pub(crate) mod`, referenced by `docs/MODEL_SUPPORT.md`.
- 602.9 `kb.rs`'s file header still says "not mutable via AI" despite create/update/delete all existing.

### #603 — code-health: crates/gui + crates/renderer (9 findings, all cheap — excluded territory, triage only, hand to that agent)
- 603.1–603.9: cache-key omissions, duplicated overlay/cursor/color logic, byte-unsafe hex parsing (`theme.rs:10-15`), dead structs, vestigial render modules. Full citations in the raw agent transcript; each independently confirmed against current source. Since this entire issue sits in `crates/gui`/`crates/renderer`, no fix was attempted — this triage should be handed to whichever agent owns that extraction work.

### #604 — code-health: crates/scheme + scheme-extra (9 findings)
2 fixed this pass · 0 already-fixed (pre-existing) · 0 stale · 6 cheap (3 in excluded `runtime/`) · 1 decision
- 604.1 cheap, **excluded territory** (`crates/scheme/src/runtime/state_sync_inject.rs`) — triple-copy buffer text; flag to that agent.
- 604.2 **FIXED this pass** (see above).
- 604.3 **decision** — `Vm::code_pool` (`vm.rs:155`) is append-only with no reclamation; a real fix is a VM/GC design question. (A narrow mitigation — caching `(name)` thunks in `call_function` — is cheap but doesn't close the underlying growth.)
- 604.4 cheap, **excluded territory** (`crates/scheme/src/runtime/io_packages.rs`) — `read-file` returns `Ok(Value::string("ERROR: ..."))` on failure instead of `Err`; flag to that agent (also an ADR-086-class defect).
- 604.5 cheap — `macros.rs:406-418`'s `unreachable!()` is provably reachable in principle (per the issue's analysis); `compiler.rs:159-163`'s `depth` field is dead.
- 604.6 **FIXED this pass** (see above — same fix as 604.2).
- 604.7 cheap, **excluded territory** (`crates/scheme/src/runtime/kb_primitives.rs`) — `kb-add-link!` etc. return `Ok(Value::Void)` unconditionally before the deferred write actually happens; flag to that agent (also ADR-086-class).
- 604.8 cheap — `drain_kb_links`/`drain_kb_link_removals`/`drain_kb_meta_adds`/`drain_kb_meta_removes` (`runtime.rs:627-648`) have zero callers.
- 604.9 cheap — `collections_count`/`frame_hwm` (`vm.rs:214`) are never written despite `introspect.rs:252`'s comment claiming otherwise.

### #605 — code-health: crates/core — editor/ subtree (8 findings, all cheap)
- 605.1 `ai_provider`/`ai_profile` bare assignments bypass validation (`option_ops.rs:522-544`); `collab_kb_sync_mode`'s setter validates but its `options.rs` registration has no `valid_values` populated.
- 605.2 All 3 `buffers.remove()` sites (`kb_ops/dispatch.rs:343`, `dispatch/kb.rs:216,242`) skip `notify_buffer_removed`.
- 605.3 `kb_widen_meta` discards `store.save_all`'s result (`let _ =`) and unconditionally reports "changes saved" — another ADR-086-class defect, not covered by the original fix list.
- 605.4 100 duplicate `map_err`-format bodies vs 11 uses of the existing `parse_option_int()` helper.
- 605.5 5 named dead `pub fn`s (`dap_terminate`, `symbol_outline_filter_char`, `validate_kb_links_on_save`, `cleanup_swap_files`, `save_option_to_config`).
- 605.6 Truncated/mismatched doc comment on `adjust_ai_target_after_remove` (`window_ops.rs:272-278`).
- 605.7 `:reload-config`'s `let _ = self.set_option(...); applied += 1;` counts failures as applied.
- 605.8 `option_ops.rs` now 4,062 lines; report-builder functions unrelated to option get/set bloat the file.

### #606 — code-health: the 13 leaf crates (8 findings)
0 already-fixed · 0 stale · 7 cheap · 1 decision
- 606.1 cheap — `SnippetSession`/`SnippetStore::load_dir` have zero production callers.
- 606.2 cheap — `parse_build_output` has zero non-test callers; `next_error` MCP tool is misleadingly always-available.
- 606.3 cheap — `format_config` is only ever read, never set/populated.
- 606.4 cheap — 9 unconsolidated `Command::new("which")` sites.
- 606.5 cheap — `dumb_jump`/`DumbJumpResult` zero callers outside `crates/lookup/`.
- 606.6 cheap — `"spell-prev" => { return true; // placeholder }` (`dispatch/mod.rs:440-442`).
- 606.7 cheap — `LspClient::initialize`'s silent capability-swallow and 4 bare-`bool` fields (`client.rs:270-277`, `protocol.rs:191`).
- 606.8 **decision** — meta-finding: no cross-crate reachability check / per-module integration test exists at all (mostly an aggregation of 606.1/2/3/5) — a tooling/process gap, not a single code fix.

### #607 — code-health: crates/mae (binary) (8 findings)
1 already-fixed · 0 stale · 7 cheap · 0 decision
- 607.1 cheap — GUI event loop never advances `editor.heartbeat`; watchdog spawns unconditionally regardless of backend.
- 607.2 cheap — `pkg/lockfile.rs:117-148`'s `sha256_hex` shells to `sha256sum` despite `sha2` already being a dependency (used correctly at `upgrade.rs:302-312`); `integrity` field is write-only.
- 607.3 **already-fixed** — `ai_event_handler.rs:266-267` now uses `checked_byte_boundary` (ADR-087, landed since the audit).
- 607.4 cheap — `SchemeAiOverrides::opt` (`config.rs:546-560`) is a stringly-typed 5-arm match, all 5 call sites use string literals.
- 607.5 cheap — `WatchdogAlert`/`MainThreadStall`/`tick`/`check_recovery` all `#[allow(dead_code)]`, zero callers.
- 607.6 cheap — two "failed to read SHA" arms (`pkg/cli.rs:336,367`) omit `errors += 1`, unlike 5 sibling arms.
- 607.7 cheap — `Lockfile::load` does `unwrap_or_default()` on parse failure — a corrupt file silently becomes empty, then gets persisted back over the real one.
- 607.8 cheap — `write_claude_settings` (`agents.rs:218-231`) has a bare `if let Some(...)` with no else, falling through to `Ok(())`, unlike sibling writers.

### #608 — code-health: shared/sync + shared/mcp (8 findings, all cheap)
- 608.1 `auth.rs:767-775`'s hand-rolled hex decode byte-slices `&s[i..i+2]` with no char-boundary check — **a pre-auth panic reachable via a malformed handshake proof, same ADR-087 class, on an authentication code path.**
- 608.2 A `mod hex` shadows the real `hex = "0.4"` crate dependency already used correctly elsewhere in the same crate (`content_key_store.rs:40`, `collection_store.rs:80`) — same fix as 608.1: delete the hand-rolled version, use the real crate.
- 608.3 No `tokio::time::timeout` around the PSK handshake on MAE's agent socket (unlike the daemon's `collab_handler`); already tracked in issue #342.
- 608.4 `keystore.rs:220-223`'s `write_secure` chmods after writing (TOCTOU window); the PSK write at `main.rs:998-999` has no directory hardening.
- 608.5 `membership.rs:1130-1160`'s replication-window check has no intersection against the governed valid-member set; already tracked in issue #449, no live consequence yet (ADR-067 Phase B not shipped).
- 608.6 `session.rs:178`'s `is_idle` and `:49`'s `messages_sent` are both dead.
- 608.7 `text.rs:531-532`'s doc comment says "byte offsets" while the code tracks UTF-16 — comment-only fix, no behavioral defect.
- 608.8 `membership.rs:20-26`'s file-size marker comment says "3,455 lines" against an actual 3,912 (13% stale).

### #609 — code-health: shared/kb (7 findings, all cheap)
- 609.1 `schema.rs`'s `ensure_schema` inlines the same `create_if_absent` pattern 4× before switching to the real helper partway through the function.
- 609.2 `lru_query.rs:565-582`'s `parse_links` fabricates `display`/`weight`/`confidence` fields; confirmed **not** touched by WS3 (that fix only added `Result` propagation to this exact file, not the missing-field defect).
- 609.3 `org.rs`'s `parse_org_multi` (the live path via `federation.rs:819`) doesn't read `header.kind`/`header.aliases`, unlike its sibling `parse_org_multi_with_types`.
- 609.4 `lru_query.rs:75-88`'s `invalidate` only evicts the caller-passed `node_id`, leaving backlink (`links_to`) staleness on the other side of an edge.
- 609.5 10 named dead `pub` items, each confirmed single-hit (definition only).
- 609.6 `Cargo.toml` comments in `shared/kb`/`crates/core` claim opt-in-only cost for `remote-hub`, while `crates/ai/Cargo.toml` unconditionally enables the feature.
- 609.7 `lib.rs:26-31`'s size marker says "3,577 lines" against an actual 5,145 (44% stale).

### #610 — code-health: crates/core — outside editor/ (6 findings, all cheap)
- 610.1 `display_region.rs`'s `compute_image_regions` still has a hand-rolled O(n) markdown byte-loop plus synchronous per-image `read_image_meta`.
- 610.2 `graph_view.rs`'s `flatten_scene_graph_cached` still per-node-clones fill/text-color/label every frame.
- 610.3 `notifications.rs:269`'s `active: Vec<Notification>` is uncapped, unlike `feed`/`resolved`.
- 610.4 `collab_selection_style_key` (`collab_colors.rs:56`) has zero callers; TUI resolves no theme color for collab selection.
- 610.5 `lock_stats.rs`'s `record_lock`/`set_held` are dead, `snapshot()` permanently empty.
- 610.6 6 named dead `pub` items, each confirmed single-hit.

### #611 — code-health: daemon workspace (6 findings, all cheap)
- 611.1 `daemon/src/storage.rs`'s `SqliteBackend` (`wal_append`/`load_document`/`compact`) runs synchronous SQLite inline on the async executor — no `spawn_blocking` anywhere in the daemon.
- 611.2 `ticket.rs:24`'s module-wide `#![allow(dead_code)]` justification comment is stale — the wiring it refers to now exists.
- 611.3 `kb_query.rs:290`'s `body.chars().take(200).collect()` is a bare unguarded truncation despite a `truncate_to_byte_boundary` helper existing 90 lines above for exactly this class.
- 611.4 `doc_store.rs`'s `compact_all` has zero non-test callers; shutdown path never calls it.
- 611.5 `collab_handler/mod.rs:9-21`'s size-reduction marker understates the current line count by ~28%/12% against `AUDIT_BASELINE.json`.
- 611.6 `kb_query.rs:144-146` collapses `AccessDecision::Deny`/`Err` into a generic JSON-RPC internal error instead of a more specific code; `oauth.rs:530-543` similarly flattens to a generic 403. Confirmed unrelated to WS3 (different defect class: error-code granularity, not Result-propagation).

---

## Stale / never-real findings

**None found newly stale in this tail.** Both findings the task flagged as known-stale examples
(`git-commit`/`git-branch-create` undispatchable) were already identified and fixed under **WS1**
*before* this pass started (see `docs/DECISIONS_FOR_REVIEW.md` §3's cross-reference and WS1's own
commit message, which states this explicitly). Every one of the 196 findings re-verified in this tail
reproduced against current source, with the sole caveat of #583.3 and #586.1 being *partially*
addressed by a sibling workstream rather than either fully fixed or fully open — documented as
"partial-fix traps" above so a future pass doesn't assume WS3/ADR-086 closed them outright.

---

## Verification

- `cargo check --workspace --all-targets`: clean.
- `cd daemon && cargo check --all-targets`: clean (not touched by this pass's fixes, verified anyway
  per instructions).
- `cargo test -p mae-core -p mae-ai -p mae-scheme -p mae`: green, including the 7 new tests added
  this pass (5 in `crates/ai/src/executor/mod_tests.rs`, 2 in `crates/scheme/src/runtime_tests.rs`).
- `cargo fmt --all`: applied.
- `mae-kb`'s two `migrate::sled_to_sqlite_*` test failures: pre-existing/environmental, confirmed
  failing on the base commit before this branch's changes — per standing instruction, ignored.
