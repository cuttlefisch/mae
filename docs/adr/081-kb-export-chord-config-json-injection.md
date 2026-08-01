# ADR-081: Real JSON Data Injection for the Chord Diagram's Config, Not Exact-Substring Text Patching

**Status:** Accepted, implemented.
**Supersedes:** ADR-080 (the original exact-substring `replacen` mechanism).
**Depends on:** the extraction of `crates/export/assets/graph.js`/`graph.css` from inline Rust
string literals into real, checked-in files (same PR #567 integration pass) — this decision
assumes those files can be edited as real JS/CSS, which the extraction is what makes practical.

## Context

ADR-080's exact-substring `str::replacen` mechanism for applying `ChordDiagramConfig` overrides
to the generated JS/CSS had a documented, known weakness: it silently no-ops (dead option, no
error signal) the moment the target literal text (e.g. `"var HOVER_GROWTH_FACTOR = 1.6;"`) ever
reformats. A pre-merge architecture review flagged this directly during PR #567's integration —
dressed up as configurability, this was "the 'one-off fix... hardcoded workaround' principle #7
warns against."

By this point in the same integration pass, `GRAPH_JS`/`STATIC_CSS` had already been extracted
from inline Rust string literals into real, checked-in `crates/export/assets/graph.js`/`graph.css`
files (see the file history for that commit) — which removed the original constraint ADR-080 was
designed around (avoiding an invasive `format!`-template rewrite of an inline string literal).
With the JS/CSS now real files, a cleaner mechanism became straightforward.

## Decision

The 10 JS-facing `ChordDiagramConfig` fields now flow through the same `#graph-data` JSON payload
the exported page already uses for node/edge data — a new `chordConfig` object, always present.
`graph.js` reads it once at load (`var chordConfig = data.chordConfig || {};`) and initializes
each tunable variable from it using `??` (nullish coalescing, not `||` — `0` is a real, documented
value for `edge_pull_back`/`wedge_gap_radians`, not "unset") against hardcoded defaults matching
`ChordDiagramConfig::default()` exactly, so the file stays independently valid, `node --check`-able
JS even without that payload (e.g. a hand-built test fixture).

The one CSS-facing field (`ui_transition_ms`) becomes a real CSS custom property
(`--ui-transition-ms`, with a `200ms` fallback baked into every `var(--ui-transition-ms, 200ms)`
use in the stylesheet itself) set via one small `:root{}` rule emitted inline, rather than
literal-text substitution. The two deliberately-fixed exceptions (the 180ms micro-interaction
rules, the 220ms fullscreen-enter asymmetry) are untouched, same as before.

`render_graph_js` and its 12-arm `replacen` chain are deleted entirely.

## Consequences

**Positive**

- The class of bug ADR-080 could only detect after the fact (via a per-field anchor-literal
  regression test) is now structurally impossible: there is no literal text for a future edit to
  drift away from. A config field is either read from the payload or it isn't; there's no
  "did the anchor text survive verbatim" question at all.
- Fewer lines of Rust than the mechanism it replaces (no 12-arm `replacen` chain).
- Real, runtime-verified end to end: the Layer 2 browser suite added alongside this change
  confirms a `hover_growth_factor` override produces measurably different on-screen hover-growth
  geometry, not just different generated source text.

**Negative / Risks**

- `graph.js` now has two places a default value is asserted to match (`ChordDiagramConfig::
  default()` in Rust, and the JS's own `?? <default>` fallbacks) — a future change to one without
  the other would silently diverge for the "no payload at all" fallback path specifically (the
  normal, payload-present path is unaffected, since the real value always comes from Rust). Not
  mitigated further here beyond the existing `default_chord_config_produces_identical_output_to_
  export` Rust-side round-trip test, which would need extending to also assert the JS fallback
  literals match if this becomes a real concern.

## Alternatives Considered

**Keep `replacen`, just add a "does the anchor text still exist" assertion at build/CI time.**
Would catch drift, but only after it already happened, and only for anchor literals someone
remembered to write a corresponding test for — the underlying fragility (config value silently
dropped, not erroring) would remain the same, this would just shrink the detection window.
Rejected in favor of removing the fragility class entirely.

**A `format!`-templated JS/CSS file, now that they're real files.** Would work, but string-
templating several thousand lines of JS/CSS for the sake of ~10 interpolated numbers is a larger,
more invasive change than reading one JSON object at load — rejected as disproportionate to the
actual need.
