# ADR-112: One org parser produces a lossless document tree; every consumer derives from it

**Status:** Accepted (design); phased (O0–O5, OS). Nothing below is built unless "What exists" says so.
**Amends:** ADR-100 D4 (widens it from range scanners to a document tree, and replaces its
"identical output" gate with D5's divergence register).
**Depends on:** ADR-030 (in-text typed-link grammar), ADR-093 (schema v2 node fields).
**Relates to:** ADR-092 D3 (normalised org source and its round-trip corpus), ADR-100 (browser edit
surface), ADR-101 (structured edges with provenance), ADR-105 (node ids contain colons), ADR-110 D8
(store bodies keep the drawer and `#+` header), ADR-065 item 4 (`#+TRANSCLUDE` re-derived on every
write).
**Evidence:** `docs/research/112-org-parser-prior-art.md`; `docs/research/100-org-parser-in-the-browser-spike.md`.

## Context

**Fact 1 — MAE has four org parsers, and they disagree.** `shared/kb/src/org.rs` (KB semantics: ids,
properties, typed links, transclusions), `crates/export/src/lib.rs` (document structure for export,
inline markup as string rewriting), `crates/core/src/syntax/markup.rs` (the highlighter's regexes) and
`crates/core/src/editor/help_ops.rs` (read mode, with its own drawer stripping and link splitting).
They split links in different orders, hard-code different TODO keyword sets in about five places,
read heading tags differently and classify external links differently. Editor commands add a fifth
layer of ad-hoc text munging: `org_todo_cycle` rewrites the first `TODO `/`DONE ` anywhere on the line.

**Fact 2 — both structure parsers accept indented stars as headings.** Org requires the stars at
column 0. Moving a node boundary moves KB content between nodes.

**Fact 3 — the HTML export publishes content Org marks as not-for-export.** `:noexport:` removes the
heading but not its section; verse, center, comment, dynamic and special blocks have no handler, so
their delimiters and content become paragraphs.

**Fact 4 — two parsers in one renderer leak through their disagreement.** An external prototype
rendered org KBs to a static site with MAE's `org.rs` (compiled to WASM) for semantics and a
third-party parser for structure. Three adversarial reviews each found new leak classes, all
structural: the semantic parser registered ids and titles from regions the structural parser never
draws; the structural parser passed through constructs nobody modelled (`#+ATTR_HTML`, raw HTML,
export blocks); link targets reached output through paths the link policy did not cover. A typed tree
produced by one parser is an allowlist by construction: a renderer can emit only what the types model.

**Fact 5 — no existing Rust org parser passes a maintenance gate.** The research
(`docs/research/112-…`) evaluated nine candidates hands-on against MAE's corpus, Org's own repository
and an adversarial set, with Emacs `org-element` as the oracle. The two technically closest — a
mature lossless design whose upstream has had no commit in 27 months, and a well-tested lossless
design one week old with one author — both fail the gate, and both also failed checks MAE needs (a
stack-overflow abort and super-cubic time on a 24 KB line in one; a column-0 heading swallowed by an
open block in the other). **No candidate was close to linear on hostile inline input.** MAE re-ingests
a file on every change, in the editor and the daemon, so that is a denial-of-service property.

**Fact 6 — Emacs's own split, checked in its source (Org 9.7, org-roam).** Org's *extension* surface
is link types (`org-link-set-parameters`), derived export backends, dynamic blocks
(`org-dblock-write:NAME`), generic special blocks, and `org-element-map`/`org-element-interpret-data`
over the parse tree; Org offers no way to add syntax (`org-element-affiliated-keywords` is a constant).
But Org's **parse depends on user configuration** — about a dozen options (TODO keywords, inline-task
level, odd levels, footnote section, list syntax, priorities), and registering a link type rebuilds
the link regexps and decides whether `[[foo:bar]]` is typed `foo` or fuzzy. org-roam's derivation is
user code (`org-roam-db-node-include-function`, extra link elements). That is safe there because
org-roam's database is a per-user disposable cache. MAE's graph is computed by every peer and the
daemon and read by agents: user-dependent derivation would make peers disagree on the same text —
the class of bug v0.14.130 fixed between the editor's and the daemon's link filters.

## Decision

### D1 — One crate, one parser, one lossless tree

`mae-org` (`shared/org`, GPL-3.0-or-later like the rest of MAE) parses org text into a lossless
concrete syntax tree: every input byte belongs to exactly one token, so printing an unedited tree
reproduces the input byte for byte (CRLF, BOM and missing final newline included). Typed views sit on
top. It depends on `rowan` only, does no I/O, and builds for `wasm32-unknown-unknown`. Every consumer
— KB ingest, export, read mode, editor org commands, and later the highlighter — moves onto it.
`parse()` and `parse_fragment()` (a body at a given heading level) are total: malformed input becomes
text plus diagnostics, never a panic or an error.

### D2 — MAE's semantics are a projection of the tree

KB nodes (file, heading, list item), titles, aliases, kinds, tags, properties, typed links and
transclusions are computed from the tree by one function, never parsed separately. A region the tree
does not render (comment and export blocks, non-property drawers, `COMMENT` and excluded-tag subtrees,
unreferenced footnote definitions) contributes no node, title, tag or link; an id found only there is
reported as a diagnostic so consumers can assert on it. There is exactly one link grammar (ADR-030,
including the `[[T|D]]` store form, `?rel=&w=&c=` and `#fragment`) and one external-link classifier.

### D3 — Renderers are functions of the tree; link resolution is data

Org text (round trip; the normalised form of ADR-092 D3), a host-neutral element tree (JSON, for
embedding renderers such as a static-site generator over WASM), HTML, and `text()` for search. Link
targets are resolved from caller-supplied data, not a callback, so a WASM build has no re-entrancy; an
unresolved target renders as its description or a fixed placeholder, never as its raw target. Only
modelled constructs are emitted: no raw HTML passthrough, `#+ATTR_*` not applied, and an explicit
protocol allowlist (`http`, `https`, `mailto`) for external links.

### D4 — The parse is a function of the text and DECLARED configuration, never of code

TODO sequences (in-file `#+TODO`/`SEQ_TODO`/`TYP_TODO` and the KB's declared defaults), select and
exclude tags, the inline-task level, recognised link types, and the KB's node rules (which headings
become nodes, by tag or property; which properties become edges) are data — in the file or in the
KB's own settings — so every peer and the daemon evaluate them identically. This is deliberately
stricter than Emacs (Fact 6), and it is the whole of the strictness: everything users customise in
Emacs remains customisable, through D6.

### D5 — The conformance gate is "identical to Emacs except for listed, pinned divergences"

- **Oracle:** Emacs `org-element`, driven in batch, emitting element types and byte spans, run over
  the bundled corpus, Org's own repository, any private corpus an owner runs locally, and a synthetic
  adversarial set. Goldens are checked in so CI needs no Emacs.
- **Divergence register:** every difference is fixed or listed as intended with a test pinning MAE's
  behaviour (MAE's extensions, or the representation of a construct).
- **Round trip:** byte-identical print of every unedited corpus file and its CRLF variant.
- **Scaling:** hostile inputs that double in size (`*a `×N, `[[`×N, deep nesting, one long line) must
  grow roughly linearly; a nesting cap degrades to a diagnostic, never to recursion.
- **Totality:** fuzz targets for parse, round trip and edit; no panic in debug or release.
- **WASM:** builds, loads synchronously, and stays under a size budget measured in O3.

The body-boundary changes the column-0 fix makes ship with a re-ingest and repair path: per-node body
diffs measured on real KBs before and after, CRDT body diffs, a repair command.

### D6 — Scheme gets Emacs's flexibility in three layers; the shared graph stays deterministic

1. **Canonical projection** (D2 + D4): shared, computed by every peer and the daemon. Not
   redefinable at runtime — like the rest of MAE's Rust core, it was never Scheme code.
2. **User layer, Scheme (with O2–O4):** the read API (`parse` → data, with `org-element-map`-style
   helpers written in Scheme), exporters with an element-tree fallback, link-type and special-block
   presentation, span-based editing commands through the KB write contract, dynamic blocks, views,
   and **org-roam-equivalent derivation hooks** (which headings become nodes, extra link elements,
   custom link type → edge, personal parse options). Hooks build a per-user layer, like org-roam's
   database, rebuilt when they change; every query and tool states which layer it read. A Scheme
   workflow may promote an edge into the canonical graph only as an ADR-101 structured edge with
   provenance, through the write path.
3. **KB-carried derivation (later, tracked):** derivation functions stored in a KB, owner-signed, run
   by every peer and the daemon in a deterministic sandbox (no I/O, clock or randomness; step-limited).
   This goes beyond Emacs — customisation of the *shared* graph — and needs that sandbox, Scheme in
   the daemon, re-projection on rule change, and an owner-only, never agent-reachable control
   (principle #16), because KB content becomes code run on other machines.

Principle #6 is unchanged: every Scheme function stays redefinable, and layer 2 is where org
behaviour users write lives. One surface for human, Scheme and agent (principle #3): MCP `org_parse`
returns the same JSON the Scheme API reads.

### D7 — Build, borrowing designs rather than code

Every candidate failed the gate (Fact 5). `mae-org` borrows designs: a lossless green/red tree with
typed views; parse-time configuration only for settings that change the tree's shape; an event
grammar feeding a builder; a nesting cap that degrades to a diagnostic; fuzz targets; an Org parity
harness with an exceptions file; element extents in `org-element`'s representation (affiliated
keywords inside the element) so the oracle diff needs no mapping layer; and an edit model that keeps
unmodified subtrees byte for byte and rejects a splice that would change structure unintentionally.

**Re-evaluation trigger:** before `mae-org-v0.1.0` is frozen and again around 2027-Q2, revisit the
one-week-old lossless candidate if it then has two or more active committers, six months of releases,
demonstrated issue response, its column-0 bug fixed and an MSRV within MAE's. If so, building MAE's
layer on it becomes the cheaper path.

## Phases

- **O0** prior art and this ADR. *Done.*
- **O1** block structure: lossless tree, headings by the spec, sections, property and generic drawers,
  generic `#+begin_X` blocks (unknown kinds kept opaque), keywords and affiliated keywords, lists as
  trees with list-item drawers, tables, comments, fixed width, planning, footnote definitions;
  declared configuration; the differential harness, divergence register, scaling and fuzz gates.
- **O2** inline tree, the one link grammar, and `semantics()`.
- **O3** renderers and the WASM facade. Tag `mae-org-v0.1.0`: the first version an external consumer
  may pin.
- **O4** MAE switches ingest, export and read mode to it, with the repair path; the span-based write
  path lands on the same spans.
- **O5** the remaining constructs by measured use, privacy-relevant regions first.
- **OS** the Scheme surface (D6 layers 1–2) and its adversarial tests, alongside O2–O4.

## Consequences

- One place to change link grammar, TODO keywords, tags and external classification.
- Export stops publishing `:noexport:` sections and comment, verse and special-block content.
- Node bodies change where indented stars or list-item drawers moved boundaries; this ships with the
  repair path, not before it.
- Inline-as-tree restructures export rather than extracting it.
- An external renderer can consume MAE's semantics without reimplementing them.
- A user's derivation hooks shape their own layer, not what peers see; that difference is stated in
  every result rather than hidden.

## Alternatives rejected

- **Adopt or fork an existing Rust parser** (Fact 5): every candidate fails the maintenance gate, and
  a fork carrying MAE's extensions would be permanent from day one.
- **A third-party JS parser for structure:** rejected for semantics by the ADR-100 spike; Fact 4 shows
  it also fails for rendering beside a separate semantic parser.
- **Scanners only (ADR-100 D4 as written):** enough for decoration, not for rendering or for one
  source of node semantics.
- **tree-sitter-org:** no grammar compatible with MAE's tree-sitter, none for `wasm32-unknown-unknown`,
  and no way to model MAE's extensions.
- **User code in the canonical projection, as in Emacs:** peers would derive different graphs from
  the same text (Fact 6). D6's user layer gives the same flexibility without that.

## What exists

The Emacs oracle (byte spans for 23 element types plus links) runs over a 339-file public corpus with no
failures; the evaluation harness, adversarial set and scaling probes from the research exist outside
the repo and are the seed for O1's harness. No `mae-org` code exists yet.
