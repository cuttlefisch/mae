# ADR-080: A Configurable Surface for the Chord Diagram's Layout/Timing Constants (Superseded)

**Status:** Superseded by ADR-081. Ported here for the historical record — see ADR-081 for the
mechanism actually shipped in PR #567 (and why).
**Relates to:** ADR-077 (the module this ships as part of); ADR-081 (supersedes this ADR's
Decision).

*Ported from a downstream feature branch's own `kb/adrs/0005-chord-diagram-config-surface.org`
during PR #567's integration.*

## Context

The chord diagram's layout, hover-growth, and timing math was entirely hardcoded — every value
baked directly into the exported page's CSS/JS as a literal. This surface needed to become
genuinely configurable, from both human (`set-option!`/init.scm) and AI-peer (MCP tool arg) sides.

The binding constraint (as understood at the time this ADR was written): the CSS/JS were plain
raw-string Rust constants, not `format!`-templated — with several thousand lines of JS/CSS full of
literal `{`/`}`, converting either to a `format!` template would mean escaping every literal brace.
Any configurability mechanism had to work WITH that constraint, not force abandoning it.

## Decision (as originally accepted — see Status)

Add `ChordDiagramConfig` (one field per constant, `Default` set to the exact original hardcoded
values) and apply it via targeted, exact-substring `str::replacen`/`str::replace` against verified
anchor literals (e.g. `"var HOVER_GROWTH_FACTOR = 1.6;"`), falling back to returning the original
`&'static str` constant unchanged whenever the supplied config equals `Default`.

## Consequences (as originally assessed)

**Positive:** zero behavior change for existing call sites; no invasive rewrite of the raw-string
constants into a brace-escaped template.

**Negative / Risks — the reason this was superseded:** the exact-substring approach was coupled
to the CURRENT literal text of the CSS/JS constants. A future unrelated edit that reformats one of
these lines (adds a trailing comment, reflows the initializer) would silently break that one
field's `replacen` — the config value would be silently ignored rather than erroring, with no
signal beyond a per-field regression test happening to still exist and still pass. This is exactly
the failure mode a pre-merge architecture review flagged during PR #567's integration: "silently a
no-op — dead option, no error signal — the moment the literal text ever reformats."

## Why this was superseded, not just left as accepted debt

By the time this feature was integrated upstream (PR #567), the CSS/JS constants had already been
extracted from inline Rust string literals into real, checked-in `.js`/`.css` asset files (a
separate, independently-motivated fix — see ADR-077's integration notes and the file history of
`crates/export/assets/`). Once the JS/CSS lived in real files, real-browser test coverage
(`crates/export/tests/browser/`) became practical, which made the `replacen` approach's own
documented weakness both easier to fix AND cheaper to verify was fixed. ADR-081 documents the
replacement: real data injection via the same JSON payload the exported page already uses for
node/edge data, eliminating the text-patching fragility entirely rather than continuing to detect
it after the fact via anchor-literal regression tests.

## Alternatives Considered (as originally assessed)

**Convert the CSS/JS to real `format!` templates.** Rejected on the same grounds the original
raw-string design chose to avoid this in the first place.

**A generic string-keyed `HashMap<String, String>` override bag.** Rejected as strictly worse than
a typed struct: no compile-time field-name checking, no `Default` to diff against.
