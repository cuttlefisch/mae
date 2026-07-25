# ADR-059: ADR-as-KB-node generalization

**Status:** Proposed.
**Extends:** ADR-029, ADR-030, ADR-057.

## Context

MAE's founding vision names "molecularly structured ADR-style documents" as the substrate
for AI-peer decision-making and record-keeping — the same standing that org-roam-style
atomic notes hold for a human's own knowledge base, but applied to the project's own
architectural memory. In principle, every accepted design decision MAE makes about itself
should be as query-able, link-traversable, and RAG-retrievable by an AI peer as any other
KB node. In practice, that promise is almost entirely unmet: `docs/adr/` currently holds
57 real ADR files, and only 4 of them — ADR-002 (text sync model), ADR-005 (KB nodes as
CRDT documents), ADR-015 (keymap resolution chain), and ADR-016 (artifact interaction
model) — exist as KB nodes at all
(`crates/core/src/kb_seed/concepts.rs:1477,1496,1517,1539`, the `CONCEPT_ADR_TEXT_SYNC`,
`CONCEPT_ADR_KB_CRDT`, `CONCEPT_ADR_KEYMAP_RESOLUTION`, and `CONCEPT_ADR_ARTIFACT_INTERACTION`
constants). Every one of those four was written by a human, by hand, one at a time, as a
condensed Rust string constant embedded in the KB seed module. There is no generator, no
importer, and no scripted path from "a file lands in `docs/adr/`" to "a node lands in the
KB." The result is that the KB — the thing an AI peer actually queries via `kb_search`,
`kb_get`, and `kb_search_context` — carries almost none of the project's own real decision
history, and the 53 ADRs it is missing include foundational, frequently-cited ones: ADR-029
and ADR-030 themselves (the KB data-architecture redesign this ADR extends), the entire
external-editor MCP pairing series (ADR-050–056), and every P2P/E2E-encryption ADR in the
025–044 range. An AI peer asked "why does MAE use yrs instead of automerge-rs" can find the
answer today (ADR-002 is one of the lucky four); an AI peer asked "why does the daemon use
per-KB-instance locking instead of a single global mutex" (ADR-054) cannot — the KB has
nothing to return, even though the file sits right there in `docs/adr/`.

This is not merely an inconvenience; it is the exact drift problem CLAUDE.md principle #15
names. Every new ADR merged from this point forward, if left to the status quo mechanism,
requires a human to remember, separately from writing the ADR itself, to hand-transcribe a
second, KB-shaped copy of it into `concepts.rs`. Nothing enforces that this happens, nothing
flags it when it doesn't, and even when it does happen, nothing catches the hand-authored
copy silently going stale the next time the ADR's `Status:` line changes from `Proposed` to
`Accepted`, or a later ADR adds itself to the first one's `Extends:` list. The four existing
hand-authored nodes are themselves not immune: none of them reflects any ADR activity that
happened after they were written, and there is no mechanism that would notice if they did.
The KB's own principle — that it is the durable, queryable substrate for MAE's decisions —
is undermined by the exact mechanism (manual, ad-hoc, one-off transcription) that CLAUDE.md
principle #8 warns against for every other duplicated-logic problem in this codebase.

There is real-world precedent for a specific, dangerous shape this problem takes once a
project tries to automate its way out of hand-authoring: an automated generator racing a
live sync client over the same underlying store. Obsidian users have documented exactly
this failure mode across two related reports — "conflict file" storms where concurrent
writers to the same vault produce cascading `*.conflict.md` duplicates, and a narrower,
more dangerous variant where a startup-time content generator (a daily-notes template, the
Templater plugin) fires at the same moment a live sync client is reconciling remote state,
and the sync client resolves the race by "keeping the remote version without merging" —
silently discarding the generator's freshly-written content
(forum.obsidian.md/t/syncing-creates-endless-edit-conflict-files/104148). The lesson
generalizes directly to this ADR's Phase B/C generator: any mechanism that materializes ADR
KB nodes by writing straight to the underlying store — a raw file write, a raw CozoDB
`INSERT`/`UPDATE` — bypassing the KB's normal CRDT write path is exactly the shape of thing
that can race a live collaborative session over the same node and lose data silently, with
no error, no conflict marker, nothing for principle #15 to even notice went wrong. This ADR
treats that precedent as a concrete constraint on the design, not a hypothetical risk to
wave at.

## Decision

This ADR proposes a five-phase (A–E) build-out that turns "an ADR file exists" into "a
correctly-linked, non-stale KB node exists," end to end, with no phase treated as optional
scaffolding for a later phase to clean up.

**Phase A — parse.** Every ADR in `docs/adr/` already follows a machine-parseable header
convention (this ADR's own metadata block is an instance of it): `**Status:**`,
`**Extends:**`, `**Relates to:**`, `**Depends on:**`, `**Supersedes:**`, plus an issue
cross-reference where one exists (`**Tracking:**`/`**Tracker:**`). Phase A is a parser that
reads an ADR file and produces a structured `AdrMetadata` value: status, the ADR number and
slug parsed from the filename and the `# ADR-NNN: <title>` heading, and the four
relationship fields each resolved to a list of referenced ADR numbers, plus an optional
tracking-issue reference. The header convention already exists and is already followed by
all 57 files — Phase A does not invent a new convention, it formalizes parsing of the one
already in universal use. Per CLAUDE.md principle #14's requirement to test against real
inputs rather than synthetic fixtures, Phase A ships with a golden-file test that runs the
parser against all 57 real ADR files currently in the repository and asserts a clean,
successful parse on every one — not a handful of hand-picked "nice" files chosen because
they parse easily.

**Phase B — generate.** `AdrMetadata` plus the ADR's body text is transformed into a
`concept:adr-NNN-slug` KB node. Critically, this is not a flat dump of the file's prose into
a single opaque text blob. The generator emits **reciprocal, typed links**, reusing ADR-030's
already-shipped in-text link grammar (`[[REL_TYPE:NODE_ID][display]]`, parsed via
`classify_link`/`parse_typed_links` in `shared/kb/src/org.rs`) rather than inventing a
second, parallel link representation: an `Extends:` header entry becomes a generated
`rel=extends` link from the child node to the parent, and the parent's node correspondingly
gains an inbound `extended-by` edge, so the two ADRs are graph-navigable from either
direction without either file's author having had to write the reverse link by hand.
`Relates to:`, `Depends on:`, and `Supersedes:` each get their own reciprocal typed-link
pair the same way. This reciprocal-link generation is the actual "molecular structuring"
the founding vision asks for: an ADR node's value is not that its prose is now searchable
(it already was, as a file, via `project_search`/ripgrep) but that it becomes a first-class
node in the KB's typed relationship graph — traversable via `kb_links_from`/`kb_links_to`/
`kb_graph`, walkable via `kb_shortest_path`, and rankable in `kb_search_context`'s RAG
excerpts alongside every other kind of KB node.

**Phase C — wire into the build.** A new `make adr-kb` target is added as a direct sibling
of the two build targets that already do this exact job for other pre-built KBs —
`manual-kb` (`Makefile:378-382`, builds `assets/mae-manual.cozo` via `build-manual-kb`) and
`practices-kb` (`Makefile:391-395`, builds `assets/mae-practices.cozo` via
`build-practices-kb`) — and is wired into the same `install` target and the same CI/release
path those two already use. Per principle #8, this is a deliberate reuse of an existing,
proven pipeline (binary crate + `assets/*.cozo` artifact + checksum + `install-*` alias),
not a second, parallel packaging mechanism invented because the ADR-KB's content happens to
come from a different source directory.

**Phase D — migrate the existing four.** The four hand-authored nodes (ADR-002, ADR-005,
ADR-015, ADR-016) are migrated to generator output. This migration is diffed, not assumed:
hand-authored vs. generated output for each of the four is compared, and any information
present in the hand-authored version that the generator cannot reproduce from Phase A's
header vocabulary — for example, the hand-authored ADR-005 node's explicit three-phase
migration-path table, which is body content rather than header metadata, or the
hand-authored alternatives-rejected comparison table — is treated as a required extension
to Phase A's parsed structure (e.g., parsing an `## Alternatives rejected` section into a
structured field, or preserving inline tables verbatim in the generated body) rather than
an accepted, silent loss. Silent loss here would repeat exactly the failure this ADR exists
to prevent, just at migration time instead of at authoring time.

**Phase E — staleness gate.** A `verify-adr-kb-sync` CI check is added: if a commit changes
an ADR file's header (`Status:`, `Extends:`, `Relates to:`, `Depends on:`, `Supersedes:`)
without a corresponding `make adr-kb` re-run reflected in the same commit, CI fails. This
is principle #15 made concrete and enforced rather than aspirational — the drift between
"the ADR file now says Accepted" and "the KB node still says Proposed" stops being a thing
that silently reintroduces itself every time someone updates an ADR's status line, because
CI now refuses to let that drift merge unnoticed.

## Consequences

**Positive.** The KB gains full coverage of MAE's own real decision history — all 57 ADRs,
not 4 — with each one richly, reciprocally linked to the ADRs it extends, relates to,
depends on, or supersedes, making the KB's graph-traversal and RAG-retrieval tools (`kb_graph`,
`kb_shortest_path`, `kb_search_context`) meaningfully more useful for exactly the kind of
"why did we decide X" architectural question an AI peer is most likely to ask. New ADRs stop
requiring a second, easily-forgotten hand-authoring step — `make adr-kb` at build/release
time keeps the KB current as a mechanical consequence of the existing release pipeline, not
a separate discipline a contributor has to remember. The staleness gate (Phase E) converts
what was previously an invisible, indefinitely-accumulating source of drift into a CI-visible,
must-fix-before-merge signal.

**Costs (honest).** Phase A's header parser becomes a piece of infrastructure every future
ADR author is implicitly bound by — an ADR whose header deviates from the parseable
convention (a typo in a field name, a free-text `Extends:` value that doesn't resolve to a
real ADR number) either needs the parser to reject it loudly (the correct behavior, per
Phase A's adversarial tests below) or needs the author to fix the header, which is friction
that didn't exist when the header convention was purely a human-readable one. Phase C adds a
sixth pre-built KB artifact to the release pipeline (`assets/mae-adr.cozo`, presumably,
alongside `mae-manual.cozo` and `mae-practices.cozo`), which is a small but nonzero addition
to release build time and artifact count. Phase E's CI gate, if its header-vs-prose diffing
(see Verification) is not carefully scoped, risks becoming exactly the kind of
over-triggering check that frustrated contributors learn to route around or disable — its
correctness is load-bearing, not incidental.

## Alternatives rejected

- **Hand-authoring the remaining 53 ADRs as KB nodes, the same way the first 4 were done.**
  Rejected. This does not scale — 53 more hand-transcriptions is a large one-time cost with
  no mechanism preventing the exact same gap from reopening the moment ADR-058 or ADR-060
  merges. It is the precise staleness problem this ADR exists to close, not a fix for it:
  every future ADR would still need a human to remember, unprompted, to hand-author its KB
  counterpart, with nothing checking that they did.
- **Full-text AI-summarization as the generation mechanism**, i.e., feeding each ADR file to
  an LLM and asking it to produce a KB node. Rejected as the *primary* mechanism, though not
  rejected outright as a future enhancement. An AI summary would lose the precise,
  machine-verifiable `Status`/`Extends`/`Relates-to`/`Depends-on`/`Supersedes` structure that
  a deterministic parser preserves exactly, and would introduce non-determinism and
  unauditability into what should be a mechanical, reproducible transformation — the same
  file should always produce the same node. AI-driven enrichment of these deterministically-
  generated nodes (richer summaries, cross-ADR synthesis, gap analysis) is deferred to
  ADR-061, explicitly layered *on top of* the nodes this ADR generates rather than
  substituting for them. The generation mechanism itself must stay deterministic and
  auditable; enrichment is optional and additive.

## Verification

Each phase carries its own required adversarial test, per CLAUDE.md principle #14 — the
goal in each case is a test that tries to break the phase, not one that walks its intended
happy path and stops.

- **Phase A.** Malformed-header cases must each produce a structured parse error, never a
  silent partial parse and never an infinite loop: (1) a file missing its `**Status:**`
  field entirely; (2) a file whose `**Extends:**` line references an ADR number that does
  not exist in `docs/adr/` (a dangling reference); (3) a circular extends chain constructed
  across three real files (A extends B, B extends C, C extends A) — the parser must detect
  the cycle and error, not walk it forever. All three are exercised against real files
  written for the test, not just asserted as documented intent.
- **Phase B.** A round-trip test over all 57 real ADR files: for every `Extends`/`Relates
  to`/`Depends on`/`Supersedes` reference in the corpus, the generated *inbound* reciprocal
  edge on the referenced ADR's node (computed by walking the graph backward from that node)
  must be provably identical to the *outbound* edge generated directly from the referencing
  ADR's own header (computed forward). For example, ADR-056's own header extends ADR-051;
  the test asserts that ADR-056's generated node carries an outbound `extends` edge to
  ADR-051 *and* that ADR-051's generated node carries the matching inbound `extended-by`
  edge back, and that these two are derivable from each other regardless of which direction
  the test computes them from first. This is the round-trip/property-style test principle
  #14 favors over a single fixed-order linear check — it must hold over the entire real
  corpus, not one cherry-picked ADR pair chosen because its links are simple.
- **Phase C.** Two required tests, targeting two different failure modes: (1) CI must FAIL
  on a commit that edits an ADR's `Extends:` header field without a corresponding `make
  adr-kb` re-run in the same change; (2) CI must NOT fail on a commit that only edits an
  ADR's prose (its Context/Decision/Consequences body) without touching any header field —
  a check that cannot tell the difference between a header change and a prose-only change
  would over-trigger on ordinary editorial fixes and, per the standing failure mode this
  project has already seen with other CI gates, get disabled by frustrated contributors
  rather than fixed. **A required new adversarial test, directly reproducing the Obsidian
  failure class named in Context:** start a live collaborative session against a KB
  instance, then run the ADR-KB generator concurrently against the same instance, and
  assert that no silent data loss or unattributed overwrite occurs — the generator must
  either (a) cleanly refuse to run against a KB with an active live-sync session attached
  and report why, or (b) route its writes through the KB's normal CRDT write path
  (`KbStore`, the same path any other programmatic write already uses) rather than a direct
  file/DB write, so a concurrent live edit and a concurrent generator run converge instead
  of one silently discarding the other's write.
- **Phase D.** For each of the 4 existing hand-authored nodes, diff hand-authored output
  against generator output field-by-field and section-by-section. Any content present in
  the hand-authored version that the generator cannot reproduce is asserted to fail the
  test — the test's oracle is "zero unaccounted-for information loss," not "the generator
  ran without crashing." A failure here is resolved by extending Phase A's header/body
  parsing vocabulary to capture the missing structure, then re-running the diff, never by
  narrowing the test's assertions to accept the loss.
