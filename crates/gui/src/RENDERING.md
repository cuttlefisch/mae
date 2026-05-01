# GUI Rendering Rules

## Critical Invariant: Span Parity

`compute_layout()` and `render_buffer_content()` MUST receive the same
`syntax_spans` parameter. Layout uses spans to compute heading scale via
`line_heading_scale()`. If the renderer sees different spans, line heights
will not match pixel positions, causing cursor misalignment and scroll jumps.

## Heading Scale Pipeline

1. `HighlightSpan` with `theme_key = "markup.heading"` triggers scaling
2. `line_heading_scale()` detects heading spans and returns scale factor
3. `compute_layout()` uses scale to compute `LineLayout.line_height` and `LineLayout.glyph_advance`
4. Renderer and cursor both consume `LineLayout` for positioning

## Buffer-Type Span Rules

| Buffer Type | Heading Spans | Inline Spans | Source |
|---|---|---|---|
| Normal (code) | tree-sitter | tree-sitter | `syntax_spans` map |
| Org | regex (compute_org_spans) | regex | `compute_org_spans()` |
| Markdown | tree-sitter | tree-sitter | standard pipeline |
| Help | manual `*`/`#` heading loop | `compute_markdown_style_spans()` | generated per-frame |
| Conversation | NONE (breaks layout) | `compute_markdown_style_spans()` | `highlight_spans_with_markup()` |
| Shell/Debug | none | none | dedicated renderers |

## Display Region Pipeline

`DisplayRegion` on `Buffer` provides link concealment (Emacs text-property `invisible` + `display`
equivalent). The pipeline:

1. `compute_link_regions()` detects md/org links, builds `DisplayRegion` list per buffer
2. `apply_display_regions_to_line()` builds a `display_map` (rope-col → display-col) and
   `display_chars` (replacement text) on `LineLayout`
3. `render_buffer_content()` draws `display_chars` instead of rope chars
4. `compute_cursor_position()` uses `display_map` for pixel-accurate cursor placement
5. `snap_past_regions()` skips cursor over concealed byte ranges on move-left/move-right

## Span Dedup Pattern

`render_common::spans::highlight_spans_for_buffer()` centralizes span selection for buffer kinds
using the standard text pipeline (Conversation, Help, GitStatus, *AI-Diff*). Both renderers call
this in their `_` arm — if `Some`, use shared spans; if `None`, use syntax spans. Specialized
renderers (Shell, Debug, Messages, Visual, FileTree) keep dedicated match arms.

## Line Counting: `line_count()` vs `display_line_count()`

Ropey adds a phantom empty line after a trailing `\n`. A 2-line file `"a\nb\n"` has
`line_count() == 3` but `display_line_count() == 2`. Using the wrong one causes the
cursor to land on an invisible line (ghost line bug).

### When to use `display_line_count()`

**All navigation and positioning** — anywhere cursor_row or target_row is being
set during movement, jumps, or external input:

- LSP go-to-definition / references / diagnostics
- Marks, jumplist, changelist navigation
- `:goto`, `:read`, `:g` cursor positioning
- Messages buffer scroll/cursor clamping
- Visual anchor clamping
- Nyan mode / scroll percentage display

### When to use `line_count()`

- **`clamp_cursor()`** — insert mode needs the phantom line (pressing Enter at EOF
  creates `"text\n"` where cursor must sit on the empty line below)
- Rope char/byte index lookups (`line_to_char`, `char_to_byte`)
- Search iteration over all rope lines (`:s`, `:g` line scanning)
- Any context that indexes into the rope (phantom line is a real rope line)

### Rule of thumb

> If you're **setting a cursor position**, use `display_line_count()`.
> If you're **iterating rope data** or in `clamp_cursor`, use `line_count()`.

## Layout Pixel Budget

`compute_layout()` accumulates `pixel_y` via repeated `+= cell_height` while
`pixel_y_limit` is computed as a single multiplication. After ~36+ lines,
floating-point drift (~0.002px) can cause the accumulated value to exceed the
limit by a fraction of a ULP.

**All overflow checks use a 0.5px tolerance** to absorb this drift:

```rust
if pixel_y + line_height > pixel_y_limit + 0.5 { break; }
```

0.5px is safe because the smallest possible line is ~14px (minimum font size),
so the tolerance can never admit an extra line. Without it, the layout emits
one fewer line than `ensure_scroll_wrapped` expects, creating a ghost line
where tildes render but no content is visible.

### If you modify the layout loop

- Never remove the `+ 0.5` tolerance without also switching to integer line counting
- If adding new `pixel_y` comparisons, include the same tolerance
- The tolerance covers FP drift only — it does NOT replace proper bounds checking
- Test with fractional `cell_height` values (e.g. 20.3) to stress the accumulation path

## DO NOT

- Pass `markup.heading` spans to conversation buffer layout — triggers heading scaling
- Pass different spans to `compute_layout()` vs `render_buffer_content()` — causes cursor drift
- Add heading scale to buffers without testing cursor + scroll behavior
