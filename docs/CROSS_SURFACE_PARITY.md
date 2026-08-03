# Cross-Surface Parity (WS6)

CLAUDE.md principle #3 ("The AI is a peer, not a plugin... same API surface for
human and AI") is enforced across **four parallel surfaces**:

| Surface | What it is | Where it lives |
|---|---|---|
| **Commands** | Buffer/keybinding-driven actions (`:kb-find`, `SPC n f`, ...) | `crates/core/src/commands.rs` (`CommandRegistry`) |
| **Options** | Runtime-configurable behavior (`:set`, `:set-save`) | `crates/core/src/options.rs` (`OptionRegistry`) |
| **Scheme** | Primitives callable from `init.scm`/`config.scm`/the REPL | `crates/scheme/src/runtime/*.rs`, `crates/scheme/src/introspect.rs` (`Vm::register_fn`) |
| **MCP** | Tools callable by an AI agent (embedded or external, e.g. Claude Code via `mae-mcp-shim`) | `crates/ai/src/tools/*.rs` (schema) + `crates/ai/src/tool_impls/*.rs`/`crates/ai/src/executor/*.rs` (impl) |

A capability that exists on the Command and MCP surfaces but not Scheme means
a human clicking a keybinding and an external AI agent calling an MCP tool
can both do the thing — but a Scheme script, and the *embedded* AI agent
insofar as it drives behavior through `(run-command ...)`/Scheme rather than
MCP, cannot get the same structured result back. That asymmetry is exactly
what principle #3 rules out.

This document is the visibility mechanism principle #3 needs: an explicit
table of what's on which surface, refreshed by grep against the real
registries (see "Methodology" below) rather than maintained by hand from
memory. `crates/core/src/kb_seed/scheme_api.rs`'s
`every_registered_scheme_fn_has_a_scheme_api_doc` test is the automated,
CI-enforced half of this — it guarantees the Scheme *doc* column can't drift
from Scheme *registration*. This document covers the wider four-surface
question that guard doesn't: whether a capability is on Scheme **at all**.

## Headline gaps

1. **LSP and DAP have zero Scheme primitives.** CLAUDE.md's own principle #3
   illustrates the parity claim with `(lsp-references ...)` and
   `(dap-inspect-variable ...)` — neither exists. `crates/scheme/src/runtime/`
   has no `lsp-*` or `dap-*` prefixed `register_fn` call at all. Commands use
   `lsp-*` (9 registered: `lsp-goto-definition`, `lsp-find-references`,
   `lsp-hover`, `lsp-rename`, `lsp-format`, `lsp-code-action`, ... — UI
   side-effecting) and `debug-*` (DAP, ~15 registered: `debug-start`,
   `debug-continue`, `debug-step-over/into/out`, `debug-toggle-breakpoint`,
   `debug-inspect`, `debug-attach`, `debug-eval`, ... — also UI
   side-effecting, plus one stray `dap-refresh`). MCP has full structured
   read/write coverage on both (`lsp_definition`/`lsp_references`/`lsp_hover`/
   `lsp_diagnostics`/`lsp_workspace_symbol`/`lsp_document_symbols`/
   `lsp_rename`/`lsp_format`/`lsp_code_action` — 9 tools; `dap_start`/
   `dap_continue`/`dap_step`/`dap_set_breakpoint`/`dap_remove_breakpoint`/
   `dap_evaluate`/`dap_inspect_variable`/`dap_list_variables`/
   `dap_expand_variable`/`dap_select_frame`/`dap_select_thread`/`dap_output`/
   `dap_disconnect` — 13 tools, plus `debug_state` for the stack-frame/
   variable snapshot). So today: a Scheme script (or the embedded agent
   acting through Scheme rather than MCP) can *trigger* an LSP/DAP UI action
   via `(run-command "lsp-hover")`, but cannot get a `hover` string, a
   `references` list, or a `debug_state` snapshot back as a **Scheme value**
   it can branch on. See "Proposed primitives" below for concrete signatures
   — not implemented on this branch (see "Why not implemented" below).

2. **KB has graph-navigation primitives but no basic CRUD/search.**
   `kb-links-from`/`kb-links-to`/`kb-links-typed`/`kb-graph`/`kb-neighborhood`/
   `kb-related`/`kb-shortest-path`/`kb-get-block`/`kb-block-count` all exist
   in Scheme (`crates/scheme/src/runtime/kb_queries.rs`) — but there is no
   `kb-search`, `kb-get`, `kb-create`, `kb-update`, or `kb-delete` primitive.
   Commands have `kb-find` (search, SPC n f), `kb-create`, `kb-update`,
   `kb-delete` (all buffer-UI-driven). MCP has `kb_search`, `kb_get`,
   `kb_create`, `kb_update`, `kb_delete` (`crates/ai/src/tool_impls/kb.rs`),
   backed directly by `Editor::kb_create_node`/`kb_update_node`/
   `kb_delete_node` (`crates/core/src/editor/kb_ops/nodes.rs`) and
   `Editor::kb_federated_search_scoped` (`crates/core/src/editor/kb_ops/
   search.rs`) — all **already in `mae-core`**, so wiring the missing five
   Scheme primitives needs no new cross-crate dependency, only new
   `register_fn` call sites (see below).

   One correctness note found in passing: `kb-related`'s own `register_fn`
   doc string says "distinct from lexical search (kb-search)" — referencing
   a Scheme primitive that doesn't exist. Confirms the gap is real (even the
   existing primitive's own doc assumed `kb-search` existed) rather than
   correcting the finding.

3. **`:set-save` was a 1-of-4-surface feature; now 2-of-4.** Persisting an
   option to `init.scm` (vs. a runtime-only change) previously only existed
   as the `:set-save` colon-command
   (`crates/core/src/editor/command.rs:761`), backed by
   `Editor::save_option_to_init` (`crates/core/src/editor/option_ops.rs:1718`).
   **Fixed on this branch**: the `set_option` MCP tool now accepts an
   optional `persist: bool` parameter that calls the same
   `save_option_to_init` after the value is applied (see
   `crates/ai/src/tool_impls/editor_tools.rs::execute_set_option`,
   `crates/ai/src/tools/core_tools.rs`). Scheme still has no
   `set-option-save!`/persisting equivalent of `(set-option! ...)` — needs a
   new `register_fn` call site (see below).

4. **~257-primitive doc gap was real but smaller: 35, now closed to 0.**
   `crates/core/src/kb_seed/scheme_api.rs`'s `SCHEME_API_FUNCTIONS` table
   (the `scheme:*` KB doc nodes both the human `:help` surface and the AI's
   `kb_search`/`kb_get` read) had drifted behind the actual `register_fn`/
   `register_collab_command_prim!` call sites in
   `crates/scheme/src/runtime/*.rs` + `crates/scheme/src/introspect.rs`. The
   new `every_registered_scheme_fn_has_a_scheme_api_doc` test (added this
   branch) makes this gap impossible to reopen silently. It found **35**
   undocumented primitives (not ~257 — the larger figure likely conflated
   this editor-API surface with R7RS-small stdlib builtins in
   `crates/scheme/src/stdlib/*.rs`/`vm.rs`, which are the host language, not
   MAE's editor API, and are out of `SCHEME_API_FUNCTIONS`' scope by design).
   All 35 were closed directly (real doc entries added, not allowlisted) —
   see the "WS6 cross-surface-parity gap-closing pass" block in
   `SCHEME_API_FUNCTIONS`. The complementary
   `every_scheme_api_doc_corresponds_to_a_real_registration_or_library_fn`
   test guards the reverse direction (a stale doc for a renamed/removed
   primitive), with an explicit, narrow allowlist for the 11
   `scheme/lib/mae-test.scm` pure-Scheme BDD functions (`should`, `it-test`,
   ...) that are correctly documented but never `register_fn`-registered.

## Why the LSP/DAP/KB-CRUD/`set-option-save!` primitives aren't implemented here

`Vm::register_fn`'s signature is being changed concurrently in another
worktree (the #521 extra-kernel-crates/JSON-primitives line of work). Adding
new call sites here would conflict with that change at merge, so this branch
does **not** add new `register_fn` calls. The five KB-CRUD primitives and the
one options primitive are low-risk (their `Editor` methods already exist in
`mae-core`, used today by the equivalent MCP tools) and the LSP/DAP set is
higher-effort (needs new `Vm`-callable read paths into the LSP/DAP client
state) but architecturally straightforward — `mae-scheme` already depends on
`mae-ai` for `kb-export-subgraph-html`, establishing the same non-circular
edge LSP/DAP primitives could reuse if the read path benefits from routing
through `mae-ai`'s existing `execute_lsp_*`/`execute_dap_*` implementations
instead of duplicating them. See "Proposed primitives" below for the
signatures to hand to whoever lands the `register_fn` change.

## Full capability table

Legend: ✅ present · ➖ present but different shape (UI action vs. structured
data) · ❌ absent.

| Capability | Commands | Options | Scheme | MCP | Notes |
|---|---|---|---|---|---|
| Buffer edit (insert/delete/replace/undo/redo) | ✅ | — | ✅ | ✅ | Full parity — the model surface for principle #3. |
| Cursor/navigation (goto, search) | ✅ | — | ✅ | ✅ | Full parity. |
| LSP: go to definition | ✅ `lsp-goto-definition` | — | ❌ | ✅ `lsp_definition` | Scheme can trigger the UI jump, not retrieve the location as data. |
| LSP: find references | ✅ `lsp-find-references` | — | ❌ | ✅ `lsp_references` | Same shape gap. |
| LSP: hover/type info | ✅ `lsp-hover` | — | ❌ | ✅ `lsp_hover` | Same shape gap. |
| LSP: diagnostics | ✅ `lsp-show-diagnostics`, `lsp-next/prev-diagnostic` | — | ❌ | ✅ `lsp_diagnostics` | Same shape gap. |
| LSP: workspace/document symbols | ✅ `lsp-symbol-outline` | — | ❌ | ✅ `lsp_workspace_symbol`, `lsp_document_symbols` | Same shape gap. |
| LSP: rename/format/code action | ✅ `lsp-rename`, `lsp-format`, `lsp-code-action` | — | ❌ | ✅ `lsp_rename`, `lsp_format`, `lsp_code_action` | Same shape gap. |
| DAP: session start/stop/step | ✅ `debug-start/stop/continue/step-*` | — | ❌ | ✅ `dap_start`, `dap_continue`, `dap_step` | Same shape gap; note the Command-surface naming is `debug-*`, not `dap-*` (a minor cross-surface naming inconsistency, not a functional gap). |
| DAP: breakpoints | ✅ `debug-toggle-breakpoint` | — | ❌ | ✅ `dap_set_breakpoint`, `dap_remove_breakpoint` | Same shape gap. |
| DAP: inspect variables/call stack | ✅ `debug-inspect`, `debug-panel` | — | ❌ | ✅ `debug_state`, `dap_inspect_variable`, `dap_list_variables`, `dap_expand_variable` | Same shape gap — this is CLAUDE.md's own `(dap-inspect-variable ...)` example. |
| KB: search | ✅ `kb-find` | — | ❌ | ✅ `kb_search` | **Gap.** No `kb-search` Scheme primitive. |
| KB: get node by ID | ➖ `kb-view`/`kb-preview` | — | ❌ | ✅ `kb_get` | **Gap.** No `kb-get`; `kb-get-block` only fetches a block within an already-known node. |
| KB: create/update/delete node | ✅ `kb-create`, `kb-update`, `kb-delete` | — | ❌ | ✅ `kb_create`, `kb_update`, `kb_delete` | **Gap.** Backing `Editor` methods already exist in `mae-core`; only the `register_fn` call sites are missing. |
| KB: graph/neighborhood/related/shortest-path | ✅ (via graph view) | — | ✅ `kb-graph`, `kb-neighborhood`, `kb-related`, `kb-shortest-path` | ✅ `kb_graph`, `kb_neighborhood`, `kb_related`, `kb_shortest_path` | Full parity (and now fully documented — see gap #4 above). |
| KB: links/authoring (`kb-add-link!` etc.) | ✅ | — | ✅ | ✅ | Full parity. |
| KB: sharing/collab lifecycle | ✅ | — | ✅ (guarded by `kb_sharing_actions_have_scheme_api_docs`) | ✅ | Full parity — the one area with an existing, working cross-surface test. |
| Options: read | ✅ `:describe-option` | ✅ | ✅ `get-option`/`set-option!` reads | ✅ `get_option` | Full parity. |
| Options: set (runtime only) | ✅ `:set` | ✅ | ✅ `(set-option! ...)` | ✅ `set_option` | Full parity. |
| Options: set + persist to `init.scm` | ✅ `:set-save` | ✅ (`config_key` opt-in) | ❌ | ✅ `set_option` with `persist: true` (**fixed this branch**) | **Gap** on Scheme only, now 2-of-4 (was 1-of-4). |
| Introspection: procedure arity/doc/name, GC | — | — | ✅ `procedure-arity`, `procedure-documentation`, `procedure-name`, `gc-collect!` | ➖ (`introspect` tool covers editor state, not Scheme procedure metadata) | Scheme-only capability; no command/MCP equivalent needed (REPL-oriented). |
| Scheme API discoverability (`scheme:*` KB docs) | — | — | 216 registered primitives; 192 documented pre-fix (35 gap), 227 documented post-fix (216 registered + 11 pure-Scheme-library, 0 gap) | ✅ `kb_search`/`kb_get` surface the same nodes | **Fixed this branch** — see gap #4. |

## Proposed Scheme primitives (not implemented — `register_fn` is frozen this branch)

For whoever lands the `register_fn` signature change next, in priority order
(cheapest/highest-value first):

### KB CRUD (no new crate dependency — `Editor` methods already exist in `mae-core`)

```scheme
(kb-search QUERY [SCOPE] [LIMIT])
  ;; → list of (id title kind instance excerpt), mirrors execute_kb_search
  ;; (crates/ai/src/tool_impls/kb.rs:113). Backed by
  ;; Editor::kb_federated_search_scoped.

(kb-get ID)
  ;; → alist/list of node fields (id title kind body tags ...), or #f if not
  ;; found. Mirrors execute_kb_get (kb.rs:84).

(kb-create ID TITLE BODY [KIND])
  ;; → #t on success, signals an error otherwise. KIND: "concept"|"command"|
  ;; "key"|"project"|"note" (default). Backed by Editor::kb_create_node
  ;; (crates/core/src/editor/kb_ops/nodes.rs:199).

(kb-update ID [TITLE] [BODY] [TAGS])
  ;; → #t on success. Backed by Editor::kb_update_node (nodes.rs:450).

(kb-delete ID)
  ;; → #t on success. Backed by Editor::kb_delete_node (nodes.rs:263).
```

### Options persistence

```scheme
(set-option-save! KEY VALUE)
  ;; → confirmation string, mirrors ":set-save KEY VALUE". Sets then calls
  ;; Editor::save_option_to_init (crates/core/src/editor/option_ops.rs:1718)
  ;; — the same method the :set-save colon-command and (as of this branch)
  ;; the set_option MCP tool's persist:true both already call.
```

### LSP (higher effort — needs a synchronous read path into LSP client state)

```scheme
(lsp-definition [FILE LINE COL])
  ;; → list of (file line col), or '() if none. Default FILE/LINE/COL =
  ;; cursor position in the active buffer.

(lsp-references [FILE LINE COL])
  ;; → list of (file line col preview-text).

(lsp-hover [FILE LINE COL])
  ;; → string, or #f.

(lsp-diagnostics [FILE])
  ;; → list of (line col severity message). Default FILE = active buffer.

(lsp-workspace-symbol QUERY LANGUAGE-ID)
  ;; → list of (name kind file line col). Mirrors lsp_workspace_symbol's
  ;; existing (query, language_id) shape.

(lsp-document-symbols [FILE])
  ;; → list of (name kind line col).
```

### DAP (higher effort — needs a synchronous read path into DAP session state)

```scheme
(dap-start CONFIG-ALIST)
  ;; → session status string.

(dap-set-breakpoint FILE LINE [CONDITION])
  ;; → breakpoint id.

(dap-continue) (dap-step-over) (dap-step-into) (dap-step-out)
  ;; → status string.

(dap-inspect-variable NAME)
  ;; → value string, or #f. CLAUDE.md's own principle #3 example.

(debug-state)
  ;; → structured snapshot: (threads frames breakpoints variables), mirrors
  ;; execute_debug_state (crates/ai/src/tool_impls/editor_tools.rs:162).
```

All six LSP + six DAP primitives above would want to route through
`mae-ai`'s existing `execute_lsp_*`/`execute_dap_*`/`execute_debug_state`
implementations (`crates/ai/src/tool_impls/lsp.rs`, `dap.rs`,
`editor_tools.rs`) rather than re-implementing the LSP/DAP client read paths
a second time in `mae-scheme` — the same "thin Scheme wrapper over an
existing mae-ai implementation" precedent `kb-export-subgraph-html` already
established for `mae-scheme` depending on `mae-ai`.

## Methodology

- **Commands**: `grep -n 'register_builtin' crates/core/src/commands.rs`.
- **Options**: `crates/core/src/options.rs`'s `OptionRegistry`; `:set-save`
  persistability requires a `config_key`.
- **Scheme**: `crates/core/src/kb_seed/scheme_api.rs`'s
  `every_registered_scheme_fn_has_a_scheme_api_doc` test extracts every
  `register_fn`/`register_collab_command_prim!` call site from
  `crates/scheme/src/runtime/*.rs` + `crates/scheme/src/introspect.rs` via
  source-text scanning (loose match — not pinned to `register_fn`'s exact
  argument list) and diffs it against `SCHEME_API_FUNCTIONS`. Run `cargo
  test -p mae-core every_registered_scheme_fn_has_a_scheme_api_doc --
  --nocapture` to regenerate the current gap.
- **MCP**: `crates/ai/src/tools/*.rs` (`ToolDefBuilder` schema definitions) +
  `crates/ai/src/tool_impls/*.rs`/`crates/ai/src/executor/*.rs`
  (implementations); `crates/ai/src/tools/dispatch_contract_tests.rs`
  guards schema/impl parameter agreement (a related but distinct concern
  from this document's cross-*surface* question).
