# ADR-087: Four text-index domains, one owner each

**Status:** Accepted.
**Relates to:** CLAUDE.md principle #7 (no hardcoding — the width policy options), #8 (shared
computation — the single width helper), #15 (fix the drift, don't add a third implementation).
**Tracking:** audit epic #592; findings #594, #574, and the class listed under *Context*.

## Context

A pre-v0.15 audit found ~17 findings across 12 subsystems in one class: **an offset from one index
domain used where another was meant.** Symptoms are panics on non-ASCII input (`&s[..n]` landing
mid-UTF-8), truncation cutting a character in half, and selection/search highlighting that indexes a
display-column array with rope character columns.

The remediation plan said this class was "use the existing helper, not write one" —
`text_utils::truncate_end` and `truncate_start` are documented as width-safe and the GUI already
uses them. **That was wrong, and grounding it in prior art is what found the error.** MAE has *two*
`display_width` implementations:

- `crates/core/src/text_utils.rs:107` — `s.chars().map(|c| c.width().unwrap_or(0)).sum()`
- `crates/core/src/grapheme.rs:17` — `UnicodeWidthStr::width(s)`

The `unicode-width` crate documents these as **deliberately different**, listing emoji ZWJ
sequences, emoji modifier sequences, presentation sequences, and several scripts' ligatures as cases
where a string's width differs from the sum of its characters'. For a family ZWJ emoji the per-char
sum returns 8; the real width is 2. `truncate_end` computes its budget with the wrong one and then
iterates `char_indices()`, so it can also cut *between* a ZWJ and its base character. `truncate_start`
is worse: it iterates `char_indices().rev()`, walking into a cluster from the right, and can
accumulate a combining mark's width before its base.

So the helper the plan named as the cure is a source of the disease.

A second, larger finding: **`Window::cursor_col` has no declared domain and is read as four
different ones** — grapheme index, char index (two sites), and a raw cast into an LSP
`Position.character`. It is also persisted to disk undeclared. And MAE **never negotiates LSP
`positionEncoding`** anywhere, so it sends char indices where the specification's default is UTF-16
code units.

## The constraint that shapes everything below

**Display width is a property of the terminal, not of the text.** This is not a detail; it bounds
what any convention can promise.

UAX #11 says its East_Asian_Width property "is not intended for use by modern terminal emulators
without appropriate tailoring." Kitty's text-sizing protocol states the coordination problem is
unsolvable in principle — there is no shared width database between application and terminal, and
expecting one is "not realistic" — and inverts the relationship so the *application* declares width.
WezTerm defaults to Unicode 9 tables. Foot ships three width policies and Ghostty two, both
documenting cursor desync as the failure mode. Vim has probed the terminal with a cursor-position
report at startup for decades, so probing is sanctioned practice rather than a hack.

Two consequences:

1. An ADR that says "compute the display width correctly" is promising something unavailable. What
   is available is *consistency within MAE* and *containment* when the terminal disagrees.
2. **MAE's GUI backend does not have this problem** — it owns its cell grid, so the same helper is
   authoritative there and merely a good estimate in the TUI.

## Decision

**Four domains, each with exactly one owner, and every crossing is an explicit named conversion.**

| Domain | Representation | Produced by | Consumed by | Persistable / sendable? |
|---|---|---|---|---|
| **Byte offset** | `usize` into `str`/`RopeSlice` bytes | regex, tree-sitter, file I/O | slicing, storage, CRDT ops, KB, MCP, cursor/selection state | **Yes** — the canonical domain, and the only wire/disk one |
| **Char (scalar) index** | `usize`, ropey 1.x's address space | ropey 1.x API | ropey 1.x API only | Legacy; migrate away |
| **Grapheme boundary** | a byte offset *known* to sit on a cluster boundary | `unicode-segmentation` | cursor motion, delete, selection snapping, truncation cut points | **No** — UAX #29 boundaries are not version-stable |
| **Display column** | `usize` cells | the single width helper | layout, fit, cursor screen position, wrap | **No** — not a property of the text |

**Rule 1 — one conversion function per boundary, named for both ends.** `byte_to_col`,
`col_to_byte`, `char_to_byte`. No `usize` from one domain is reused in another without a call. The
live bug ("a display-column array indexed with rope char columns") is an *absent* conversion; the
two `display_width`s are a *duplicated* one. Any function accepting or returning a column carries
`visual`/`display` in its name, or it is a text-index column. This is Helix's discipline, adopted
deliberately.

**Rule 2 — truncation is a conjunction, with exactly one implementation.** Cut points accumulate
**per-grapheme-cluster** width (`grapheme_indices(true)` + `UnicodeWidthStr`), never per-`char`.
`text_utils::display_width` is deleted and re-exported from `grapheme.rs`; `truncate_end` and
`truncate_start` are rewritten over grapheme clusters. Everything downstream
(`centered_popup_dims`, the which-key column math) inherits the fix because it already calls
`display_width`.

**Rule 3 — every width policy choice is an OptionRegistry option, not a constant** (principle #7).
Every editor surveyed that lacks these has an open, years-old width bug; every one that has them
(Vim's `'ambiwidth'`/`setcellwidths()`/`'emoji'`, Emacs's `char-width-table`, Ghostty, WezTerm)
shipped them because there is no right answer. Minimum set: ambiguous-width resolution (default
narrow; implemented free by dispatching between `width()` and `width_cjk()`), control-character
width (currently an undocumented `.unwrap_or(0)`), tab width, and emoji presentation handling.
Note the width of a string containing a tab is undefined without a starting column — the helper
either takes one or documents that it does not handle tabs. **Do not assume width ≤ 2**: U+17D8
has width 3.

**Rule 4 — every stored position declares its domain, and it is byte.** `Window::cursor_col` is
the first. The declaration is the load-bearing part; the disagreeing call sites then get explicit
conversions. Byte rather than char, on evidence that arrived after the first draft of this rule:
ropey 2.0 inverts its own convention to byte-primary and makes **char indexing an opt-in feature
flag**; Helix is migrating the same way in the same language on the same rope; Zed and
rust-analyzer's `text-size` both chose byte offsets independently. Byte is the only domain that is
simultaneously version-stable, persistable, directly sliceable, and what every producer already
hands MAE.

Honest cost, priced rather than hidden: `Buffer::char_offset_at`, `display_region`'s `rope_col_map`,
and `edit_ops.rs` all compute in chars today, and the persisted `SessionBuffer::cursor_col` needs a
migration. If a phase scopes that out, it must still *declare* char explicitly and record byte as
the migration target — an undeclared field is the actual defect.

**Rule 5 — negotiate LSP `positionEncoding`, and treat every inbound position as unvalidated.**
Currently unhandled anywhere; the spec default is UTF-16 and MAE sends char indices. Negotiate
`utf-8` (free once Rule 4 lands). A position landing mid-sequence rounds to a boundary rather than
panicking. Adopt Zed's `Unclipped<T>` shape at the LSP/DAP/MCP boundary — one wrapper, one clip
function — because it encodes *"not yet validated"*, which the compiler can enforce. It does **not**
encode "counts bytes rather than chars", which the compiler cannot. Add a chokepoint validator that
every offset→slice conversion passes through: `debug_assert`-panics in debug, clamps and logs in
release. Roughly thirty lines, and it covers the entire panic class.

**Rule 6 — assume the terminal disagrees and contain the damage.** When rendering a cluster whose
width MAE knows is contested (EAW=Ambiguous, emoji presentation, ZWJ, Nerd-Font PUA), the TUI emits
absolute cursor positioning afterwards rather than relying on relative advance. This is Neovim's
pattern (`tui.c` forces repositioning after an ambiguous-width char). The goal is not correctness —
that is unavailable — but that a disagreement about one glyph does not desync the rest of the line.
It also gives property tests a meaningful oracle: assert **containment**, not correctness.

**Rule 7 — one crate owns the width call, at an exact-pinned version.** No crate outside `mae-core`
imports `unicode_width` or `unicode_segmentation` directly; `mae-renderer` and `mae-gui` go through
`mae_core`. Today 12 modules across 3 crates import it directly. Pin exactly, for the reason Helix
pins in its own manifest and the reason `unicode-width` gives itself: *"Relying on any character
producing a stable width in this crate is likely the sign of a bug."* Note ratatui does **not** pin,
which is why this matters.

## Enforcement — and what was rejected

Ranked by (catches real bugs × low noise). Most of this was **measured**, not argued.

**Adopt:**

1. **Delete the duplicate `display_width`, funnel through one crate, exact-pin.** Not enforcement,
   but it removes more bug surface than every tool below combined.
2. **Declare `Window::cursor_col`'s domain; negotiate `positionEncoding`.** The root cause. Nothing
   else matters while one `usize` means four things.
3. **The chokepoint validator** (Rule 5). ~30 lines, no call-site ceremony, covers the whole panic
   class. This is what Zed relies on *instead of* types.
4. **A named nasty-string corpus**, table-driven across every string API: ZWJ family emoji,
   combining marks, Hangul jamo, regional-indicator flags, skin-tone modifiers, viramas, bidi
   overrides, BOM, astral CJK, CRLF, wide and ambiguous. Measured: a 15-entry hand-curated list beat
   every general-purpose generator on grapheme, width, and ZWJ oracles — and its failures are
   *named* rather than an inscrutable generated string.
5. **proptest for the invariants** — never panics; output width ≤ N; output is a grapheme-boundary
   prefix; idempotence. With a caveat that changes how it is used: proptest's default `String`
   generator **structurally cannot produce ZWJ sequences** (category Cf is excluded), confirmed by
   two independent measurements. So proptest covers the panic invariants; the corpus covers ZWJ.
6. **`clippy::char_indices_as_byte_indices`** — already deny-by-default, and MAE already passes it.
   Free.

**Rejected, with reasons:**

- **`clippy::string_slice`.** Measured against the one comparable bug MAE has already shipped
  (`90b0cd51`) and found to have **inverted polarity**: silent on the defect, and it fires on the
  *fix*. Of 59 candidate sites, most are `String`/`Vec`/ASCII protocol data. It would train
  contributors to add `#[allow]` at exactly the sites that matter.
- **Newtypes for `ByteOffset`/`CharOffset`/`ColumnOffset`.** The highest-value question asked, and
  the answer is no. Zed spent roughly 4,500 diff lines on the approach, closed its own RFC
  `not_planned`, and then shipped *this exact bug class inside the newtype* — because a newtype
  encodes which space a value belongs to, not which unit it counts. Naming discipline (Rule 1) plus
  the chokepoint validator is the recommended pairing. The one wrapper that earns its keep is
  `Unclipped<T>` at the untrusted boundary, which encodes validation state — something the compiler
  genuinely can check.

## Consequences

**Positive.** The panic class is closed by ~30 lines rather than by a type-system refactor. The
width helper becomes honest about being an estimate in the TUI and authoritative in the GUI. Rule 4
resolves a field that four call sites currently disagree about, and Rule 5 fixes an LSP interop bug
that was never on the audit's list.

**Negative / Risks.** Rule 4 is the expensive one — char-computing call sites plus a session-file
migration. Rule 6 is new machinery with no analogue in MAE today. And this ADR cannot promise
correct rendering on every terminal, because that is not available; it promises internal consistency
and bounded damage, and any claim beyond that would be false.

## Alternatives considered

**Route everything through the existing `text_utils` helpers** (the original plan). Rejected: those
helpers are a source of the bug class, not the cure.

**Adopt typed index newtypes.** Rejected on measured evidence — see above.

**Rely on `clippy::string_slice` as the gate.** Rejected — inverted polarity on a real MAE bug.
