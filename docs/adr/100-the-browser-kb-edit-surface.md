# ADR-100: The browser KB edit surface — structured chrome plus source-backed live preview

**Status:** Proposed. D4 was decided by spike (`docs/research/100-org-parser-in-the-browser-spike.md`);
its bundle-size condition remains open.
**Depends on:** ADR-097 (Browser MAE is a KB surface), ADR-099 (the transport this edits over),
ADR-093 (the node CRDT carries the whole node — this ADR's structured chrome is bound directly to
its schema v2 types).
**Relates to:** ADR-092 (one write path for a KB node — this ADR contradicts its D3 premise on
evidence, see Context), ADR-030 (in-text typed-link grammar), ADR-072 (KB read mode — the
native-editor sibling of this surface), ADR-064 (a second native frontend — a different question,
see ADR-097 D4).
**Evidence:** `docs/research/097-browser-crdt-interop-spike.md`,
`docs/research/100-org-parser-in-the-browser-spike.md`.
**Blocked on (partially):** issue #655 (properties stored twice).
**Tracking:** issue TBD.

## Context

The requirement is a WYSIWYG editor for KB nodes in the browser. Two facts about MAE's data model
determine what that can mean, and one of them contradicts an existing ADR.

**Fact 1 — the schema already splits the node into typed fields.** ADR-093's schema v2 gives a
`KbNodeDoc` a root `Y.Map` in which `title` and `body` are `Y.Text`, `tags`/`links`/`aliases` are
`Y.Array`, `meta`/`props` are `Y.Map`, and `kind`/`todo`/`prio`/`src_v` are scalars. The Phase 0
spike confirmed a browser sees all of these as live shared types, not a flattened decode. So most of
a node needs **no parsing at all** to edit — the structure is already in the CRDT.

**Fact 2 — the `body` is not the org file, and ADR-092 D3's premise is already false in code.**
ADR-092 D3 states the human edit surface is "the node's normalized org source text". It is not.
`shared/kb/src/org.rs:75` stores `body = rewrite_links(content)` — a *transformed* copy, where the
canonical `[[TARGET][DISPLAY]]` grammar is converted to a pipe form that issue #627 confirms is
"the intended ingest form". Separately, `title` is lifted out into its own CRDT field, while
`:PROPERTIES:` drawers remain *inside* the body text — and ADR-093 added `props` holding the same
properties again (issue #655).

So "edit the org source in the browser" was never an accurate description of what would be edited.
An ADR that assumed it would have produced an editor whose save path silently disagreed with the
file it claimed to round-trip.

**Prior art splits cleanly into two families, and only one is compatible with MAE's substrate.**

- **Block-CRDT WYSIWYG** — Notion, AFFiNE/BlockSuite, TipTap over `y-prosemirror`/`Y.XmlFragment`. The CRDT *is* the document structure. Rich editing is native and merges are structure-aware.
- **Source-text CRDT with decoration-based live preview** — Obsidian Live Preview, HedgeDoc, CodeMirror 6. Decorations are view-only, so what is stored is exactly what was typed.

MAE's truth is a `Y.Text`. Adopting a block CRDT would give the browser a *different source of
truth* from the TUI, the GUI, MCP and the AI peer, all of which read and write text — the
file-versus-database split ADR-064's own Alternatives-rejected section cites Logseq as having
"doubled" their engineering cost over. That is a decisive argument, and it is the project's own
recorded evidence rather than an outside opinion.

**A gap with no off-the-shelf answer.** There is no CodeMirror 6 / Lezer org-mode language mode.
Every live-preview implementation with prior art is markdown. MAE parses org with two hand-written
Rust parsers (`shared/kb/src/org.rs`, `crates/export/src/lib.rs`) and — contrary to what CLAUDE.md's
crate table claimed until issue #657 — has **no** tree-sitter org grammar. So how the browser
obtains org structure is a real open question, not a library choice.

**And a limit worth stating.** No collaborative org-mode browser editor appears to exist as prior
art at all. This part is first-of-kind, and the design should be framed as carrying that risk rather
than as assembling known parts.

## Decision

### D1 — Name what is actually being edited, and stop calling it the org source

The browser edits a **KB node**: a set of typed CRDT fields plus a prose body. The body is a
`Y.Text` whose content is MAE's stored, link-rewritten representation — **not** the bytes of any
`.org` file on disk, and not guaranteed to round-trip to one.

This is a documentation and naming decision as much as a technical one, and it is the most
important thing in this ADR: it prevents an editor being built whose implied contract ("this is your
org file") is one the storage layer never offered. `ADR-092` D3 should be amended to match what
`org.rs` actually does; this ADR does not silently work around it.

### D2 — Structured chrome bound directly to schema v2; the body is the only prose surface

`title`, `tags`, `aliases`, `links`, `properties`, `todo` and `priority` are edited as real form
controls bound to **their own CRDT types**. No parsing, no serialization, no round-trip risk — the
Phase 0 spike showed a browser already sees each as the right shared type.

Only `body` is a text-editing problem. This is what shrinks the hard part of the work to its
smallest possible surface, and it falls out of ADR-093's schema rather than being invented here.

**The chrome is the sole writer of typed fields.** Until issue #655 resolves which store is
canonical, the `:PROPERTIES:` drawer inside the body renders **read-only**, and the chrome's
property table is the only way to change a property. Two writers for one fact is the defect; one
writer plus a visible-but-inert rendering is the containment.

### D3 — Live preview ships an honest subset, and says which parts are source

The body editor decorates **headings, emphasis, inline code, and links** — markup hidden until the
cursor enters, the Obsidian Live Preview behaviour. Decorations are **view-only**, so what is stored
is byte-identical to what was typed.

**Tables, source blocks, and drawers render as monospace source, not WYSIWYG.** These are precisely
the constructs every live-preview implementation reports breaking on ("abstraction leaks in tables",
fence deletion corrupting formatting), and org tables are common in real notes. Concurrent remote
edits make it worse, because remote cursors and decorations both move.

Shipping the honest subset beats shipping a richer editor that corrupts a table under a concurrent
edit. The UI must make the distinction visible rather than leaving a user to discover that some
constructs behave differently.

### D4 — The browser gets org structure from an extracted, WASM-compiled scanner crate

*Decided by spike (`docs/research/100-org-parser-in-the-browser-spike.md`), which changed the
question this decision was originally framed around.*

The original three options were JS `uniorg` (a), daemon-served ranges (b), and compiling
`shared/kb/src/org.rs` to WASM (c). Measurement disposed of two and corrected the third:

- **(b) is rejected on latency.** Native parse of the real corpus is **microseconds** — full
  structure p50 8.6 µs, max 39.3 µs over 98 bundled org files. A network round-trip is three orders
  of magnitude worse than the parse it would replace. (Latency is therefore *not* a constraint on
  any local option, which is worth stating because it was (b)'s only rationale.)
- **(c) as written was based on a half-wrong premise.** `org.rs` has **no inline-emphasis scanner
  at all** — its only contact with `*`/`=`/`~` is heading stars and skipping links inside verbatim
  spans. It cannot produce most of D3's decoration set.
- **But the missing half already exists as offsets.** `mae-export`'s `find_markup_end_str` returns
  `Option<(usize, &str)>`; the inline logic is already range-based and only its public wrapper
  formats to HTML.

**The decision: extract the pure scanners from both parsers into a leaf crate (provisionally
`shared/org-scan`) and compile that to WASM.** The browser consumes it for block structure, links,
drawers, properties *and* inline emphasis.

This is chosen over (a) because drift in the *semantic* layer — typed links, drawers, headings —
produces a wrong graph rather than wrong-looking text, and a third independent implementation is
most dangerous exactly there. It is also a principle #8 win rather than a cost: MAE currently has
two hand-written org parsers with overlapping responsibilities, and this consolidates their
scanning core into one place both native code and the browser consume.

**Two conditions bind this decision:**

1. **A conformance gate before any call site switches.** The extracted crate and the current implementations must produce identical output over the full bundled corpus. Extracting from two shipped, heavily-tested parsers is the real risk here, not the WASM build.
2. **Bundle size is still unmeasured**, and is a prerequisite rather than a formality — `wasm32-unknown-unknown` std is not installed on the development machine and CI would need the target added to `.github/actions/setup-rust`. If the compiled crate turns out large enough to hurt initial page load, the trade reopens **for the cosmetic inline layer only**; the semantic layer's argument does not depend on size.

Recording the correction rather than quietly rewriting the option list is deliberate. The earlier
planning pass proposed reusing a `tree-sitter-org` grammar that does not exist (#657), and this
ADR's own first draft proposed reusing an emphasis scanner that does not exist either. Both were
plausible; both were wrong; both were caught by checking rather than by reasoning.

### D5 — Block WYSIWYG is permitted only as a projection, never as a source of truth

If a Notion-style block editor is later wanted, it must be a **view over the body text**, with the
`Y.Text` remaining canonical. A design in which the browser's block model is authoritative — with
text derived from it — is rejected, because it gives the browser a different source of truth from
every other MAE surface.

This is stated now, while it is cheap, because it is the decision that would be hardest to reverse
after a block editor shipped.

## Consequences

**Positive.** Round-trip risk for the structured fields is not small, it is **zero** — nothing
serializes. The body's storage is byte-identical to what the user typed, so an edit made in the
browser and an edit made in the TUI are the same kind of change to the same `Y.Text`, and converge
by the mechanism the Phase 0 spike already proved. The design also surfaces two real defects
(#655, ADR-092 D3's stale premise) rather than building over them.

**Costs, stated honestly.**

- **This is not full WYSIWYG, and should not be described as such.** Tables and source blocks are source. A user expecting Notion will notice.
- **First-of-kind risk.** No collaborative org-mode browser editor exists to learn from. Live preview is reported as "deceptively hard" even single-user; MAE adds concurrent remote cursors to that.
- **D4 is unresolved**, and if the WASM spike fails, the fallback introduces a third org parser and a permanent conformance-test obligation.
- **The properties containment is temporary.** A read-only drawer is honest but odd-looking, and it stays until #655 decides which store is canonical.

**Downstream/bug-risk framing (principle #9).** D2 is low-risk: binding form controls to existing
CRDT types adds no new write path and no parsing. D3 is where the bugs will be — decoration
positions and remote cursors both index into the same `Y.Text`, and every reported failure in this
class is a position bug. D4 is the largest unknown. None of this touches the membership, signing, or
content-key paths, so the blast radius is confined to one surface.

## Alternatives rejected

- **Block-CRDT WYSIWYG (TipTap/`y-prosemirror` over `Y.XmlFragment`) as the source of truth.** Rejected: it gives the browser a different source of truth from the TUI, GUI, MCP and AI peer, which is the file-versus-database split ADR-064 already cites Logseq as having "doubled" their engineering cost over. The editing experience would be better; the cost is a second representation of every node, permanently.
- **Editing the raw `.org` file rather than the CRDT body.** Rejected: the file is not what syncs, and `body` is a transformed copy of it (Context, Fact 2). An editor writing files would bypass the CRDT entirely and reintroduce exactly the parallel write path ADR-092 exists to eliminate.
- **Plain source editing with a separate rendered preview pane.** Rejected as the *default* — it ships fastest and carries no position-bug risk, but it is not the requirement, and a side-by-side preview wastes half the viewport on a KB node that is usually short. Worth keeping as a per-user toggle, which costs almost nothing once the body is a CodeMirror surface.
- **Deciding D4 now in favour of `uniorg` because it is available today.** Rejected: it is the option with a permanent structural cost (a third parser), and choosing it before measuring whether (c) works would be choosing the worse long-term answer for a short-term reason. The spike is cheap relative to that.

## Verification

Per principle #14.

- **D2 — no round-trip, by construction.** Set every typed field from the browser and assert the values the daemon materializes are byte-identical, including a non-BMP emoji, CJK text, and a property value containing org markup that would break a naive serializer. Adversarially: setting a property via the chrome must not alter the body text, and editing the body must not alter `props` — the #655 divergence must be *observable* as a test, not merely avoided by convention.
- **D3 — decorations are view-only.** Type a document containing every decorated construct, round-trip it through the editor with the cursor visiting each construct, and assert the stored `Y.Text` is **byte-identical** to the input. A decoration that accidentally rewrites source would pass a visual check and fail this.
- **D3 — position correctness under concurrency.** A remote edit landing inside a hidden markup region while the local cursor sits after it must not move the local cursor or corrupt the decoration. This is the failure class every prior implementation reports, so it is a named test rather than a general convergence assertion.
- **D3 — the excluded constructs stay excluded.** A table edited concurrently by two clients must converge to valid org. If it cannot, the exclusion in D3 is load-bearing and must not be quietly relaxed later.
- **D4 — the spike's own gate.** `org.rs` compiled to WASM must produce **identical** parse output to the native build over a real corpus (the bundled KBs), not a hand-picked sample. Bundle size and parse latency on the largest real node body must be measured against a stated budget before (c) is adopted. If (a) is taken instead, the conformance test comparing `uniorg` to `org.rs` over the same corpus becomes a permanent CI obligation.
- **D5 — no second source of truth.** Any future block-editor work must be able to demonstrate that disabling it leaves every node's `Y.Text` unchanged and fully editable by every other surface.
