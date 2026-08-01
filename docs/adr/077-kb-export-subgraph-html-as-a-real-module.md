# ADR-077: `kb-export-subgraph-html` Ships as a Real mae Module, Not a Scheme Reimplementation

**Status:** Accepted, implemented.
**Depends on:** mae's own module system (`docs/EXTENSION_GUIDE.md`, `crates/mae/src/pkg/`) and
CLAUDE.md's architecture principle #5 (module boundaries) — both directly shaped the "thin
wrapper over a kernel primitive" decision below.
**Relates to:** ADR-050–056 (the extra-kernel-crates extension point, issue #521) — this
primitive deliberately does NOT use that mechanism; see Alternatives Considered.

*Ported from a downstream feature branch's own `kb/adrs/0002-mae-module-not-scheme-
reimplementation.org` during PR #567's integration, adapted for the actual shipped shape
(in-tree, not a standalone out-of-tree crate — see the note in Context).*

## Context

`kb-export-subgraph-html` (a whole-KB-subgraph export to one self-contained, interactive HTML
file — chord-diagram nav, bilingual EN/ES overlay) started as an MCP tool
(`kb_export_subgraph_html`, `crates/ai/src/tool_impls/kb_export_html.rs`) calling directly into
Rust. The natural next question: should it also become a genuine Scheme-callable primitive, so a
human using mae interactively (not only an AI agent) can trigger it natively — and if so, should
the Scheme side be a real reimplementation, or a thin wrapper over the same Rust function the MCP
tool already calls?

A pure-Scheme reimplementation was ruled out on concrete grounds, not difficulty alone: mae's
Scheme runtime had no JSON encoder anywhere (the export's entire architecture centers on
embedding a JSON payload for client-side JS to consume); there was no chord-ring layout
primitive; and across every shipped module's Scheme, there was zero precedent for generating a
large templated document at this scale. Forcing the real export logic into Scheme would mean
adding new Rust-side kernel primitives first — the same "patch mae's core" outcome this decision
was trying to avoid.

`modules/kb-graph-view/` was already the concrete counter-example showing the right shape: pure
Scheme, entirely command/keybinding wiring, calling straight into kernel-computed state. Its own
header comment states the pattern explicitly — "the view itself... lives in the kernel — this
module only wires human-facing keybindings onto the SAME primitives the AI's MCP tools call."

**Note on the standalone-project detour:** this feature was, for a time, developed in a separate
out-of-tree checkout with its own Cargo workspace, and this ADR's original form assumed that
project's own crate boundary (a real, versioned Cargo dependency from mae's Scheme runtime onto
the standalone project's compiled Rust). That path was tried once via mae's `extra-kernel-crates`
extension point (issue #521) and reverted the same day — the standalone project itself depended
on a further sibling checkout via a relative path, an unshippable, personal-machine-only
dependency chain that would never build for another contributor or CI (see PR #567's own
description for the full account). This integration vendors the export logic directly in-tree
instead (`crates/export/src/html_graph.rs`), which makes the "which crate is this a dependency
of" question in the original ADR moot — everything lives in the same workspace now.

## Decision

Keep the export logic — HTML/JS generation, translation handling, all of it — as
`mae-export`, a normal in-tree crate. Expose the same function as a Scheme-callable primitive
(`kb-export-subgraph-html`, `crates/scheme/src/runtime/kb_export.rs`) — the identical Rust
function (`mae_ai::execute_kb_export_subgraph_html`) the existing MCP tool already calls,
mirroring how `kb-graph-view-open` and its MCP counterpart already share one implementation — and
ship a genuine, `kb-graph-view`-sized module (`modules/kb-subgraph-export/`: a `module.toml` and
a short `autoloads.scm` defining a command and keybinding) so a human using mae interactively can
trigger an export natively, not only through an AI tool call.

## Consequences

**Positive**

- This is a real mae extension by the actual rules of mae's own module system — installable,
  `:module-reload`-able — not a workaround or a new mechanism bolted on.
- No new Rust kernel primitives needed beyond exposing what already existed
  (`execute_kb_export_subgraph_html`) to Scheme.
- The heavy logic stays in Rust, where it can be properly tested (`crates/export`'s own extensive
  `mod tests`, plus the Layer 2 real-browser suite added in `crates/export/tests/browser/`)
  rather than being split across a Rust/Scheme boundary mae's own testing conventions don't have
  a story for.

**Negative / Risks**

- The module depends on a compiled Rust primitive existing in the mae binary — it cannot be
  distributed as a pure out-of-tree Scheme package the way a keybinding-only module could.

## Alternatives Considered

**Pure Scheme reimplementation.** Rejected concretely: no JSON encoder, no layout primitive, and
zero precedent anywhere in shipped Scheme for generation at this scale.

**The `extra-kernel-crates` extension point (issue #521), as an out-of-tree dependency.** Tried
once, reverted the same day (see Context) — not because the extension point itself is wrong (it's
the right mechanism for a genuinely out-of-tree, downstream primitive), but because THIS
primitive is meant to ship upstream as a normal in-tree feature, and routing it through that
mechanism would have required an unshippable multi-repo dependency chain just to reach a Cargo
dependency that could instead simply be a normal crate in the same workspace.
