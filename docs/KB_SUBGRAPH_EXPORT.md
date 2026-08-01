# KB Subgraph HTML Export — Real Invocation Recipes

> How to actually use `kb-export-subgraph-html`/`kb_export_subgraph_html` — the tool that exports
> a curated KB neighborhood to one self-contained, bilingual, interactive HTML page (chord-diagram
> nav, EN/ES toggle, browser-history navigation). Full parameter docs live as doc comments on
> `crates/ai/src/tool_impls/kb_export_html.rs::execute_kb_export_subgraph_html` and
> `crates/scheme/src/runtime/kb_export.rs`'s primitive registration — this doc is the "how do I
> actually call this" companion, not a duplicate of that reference.

## Why this doc exists

Earlier real exports of this session (the gitlab-migration project, mae's own help manual) were
each produced by a small, throwaway `cargo run --example ...` Rust binary under
`crates/ai/examples/` that hardcoded a real KB path, a real anchor id, and a real output path
directly in source. Per CLAUDE.md principle #3 (AI/human parity), the Scheme primitive and the
MCP tool are provably the same code path (`kb_export.rs` queues the exact JSON
`execute_kb_export_subgraph_html` consumes) — so a Scheme call *is* the real production interface,
not a third, parallel thing. The recipes below replace those throwaway binaries; see
`crates/ai/examples/export_demo.rs` for a synthetic (no real paths) smoke test kept for
contributors who want to `cargo run --example export_demo` against fixture data.

## The real exports, as Scheme calls

**mae's own help manual** (a scaling/portability stress test — 237 org files, developed/tested
against a 6-7 node fixture and a 21-node real guide, neither near this KB's actual density):

```scheme
(kb-export-subgraph-html "index" "/tmp/mae_manual_export.html" 1)
```

**The gitlab-migration project**, rooted at its real top-of-cluster hub (`GitLab CE Self-Hosted
Migration`, tagged `:hub:`, the node that says outright "This is the entry point for this
project"). Needs a larger `node_cap` than the factory default (60) to cover the whole project —
set that via the persistent option rather than reaching for the primitive's 5th positional arg,
since that's the more idiomatic path now that this is configurable at all (see below):

```scheme
(set-option! "kb-export-default-node-cap" "200")
(kb-export-subgraph-html "d4e7af90-1c7a-4d60-8c5f-04c51eb626c8"
                          "~/Projects/gitlab-migration/export.html"
                          4)
```

**An onboarding guide with a translation overlay** (the optional 4th/5th args — a translations
JSON file and a display title override):

```scheme
(kb-export-subgraph-html "<seed-node-id>"
                          "~/notes/media/onboarding-guide.html"
                          1
                          "~/notes/media/onboarding-guide.translations.es.json"
                          "Onboarding Guide")
```

## Configuring the chord diagram itself

Every layout/timing constant behind the exported page's chord diagram (hover-growth amount,
wedge rounding, animation speed, the reachable-set safety net, etc.) is a real `kb-export-*`
option — persistently `set-option!`-able (e.g. in `init.scm`, so it applies to every future
export without repeating it per call), each defaulting to exactly the value this tool always
used before these options existed:

```scheme
(set-option! "kb-export-hover-growth-factor" "2.2")
(set-option! "kb-export-wedge-gap-radians" "0.03")
```

See ADR-081 for the full list and rationale, or `(get-option "kb-export-hover-growth-factor")` /
`command_list`/`M-x set` in a running editor to discover the rest by their `kb-export-*` prefix.

## Reaching NODE-CAP / GUIDANCE-IDS / CHORD-CONFIG directly (positions 5-7)

The primitive takes its arguments **positionally** (`ID PATH [DEPTH] [TRANSLATIONS] [TITLE]
[NODE-CAP] [GUIDANCE-IDS] [CHORD-CONFIG]`) — reaching a later optional argument means supplying a
real value for every earlier one too (`#f`/`'()` are **not** accepted as "skip this slot"
placeholders; `TRANSLATIONS` in particular must be a path to real, readable JSON, even an empty
`{}` overlay). For most calls, prefer the `set-option!` route above instead of fighting
positional order. When you genuinely need a **per-call** override (not a persistent default) for
`GUIDANCE-IDS` or `CHORD-CONFIG`, keep one reusable empty overlay file around:

```scheme
;; once: echo '{}' > ~/.config/mae/empty-translations.json
(kb-export-subgraph-html "index" "/tmp/export.html" 2
  "~/.config/mae/empty-translations.json" "My Title" 60
  '("style-guide" provenance-note)                    ; GUIDANCE-IDS: strings or symbols
  '((hover-growth-factor . 2.75) ("history-depth-cap" . 15)))  ; CHORD-CONFIG: alist, string or symbol keys
```

## The synthetic smoke-test example

`crates/ai/examples/export_demo.rs` builds a tiny in-memory KB (no real personal paths, no
dependency on any specific machine's KB content) and runs it through the exact same
`execute_kb_export_subgraph_html` call path as everything above — a `cargo run --example
export_demo` sanity check for contributors who don't have real KB data on disk.
