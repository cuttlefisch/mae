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

## DO NOT

- Pass `markup.heading` spans to conversation buffer layout — triggers heading scaling
- Pass different spans to `compute_layout()` vs `render_buffer_content()` — causes cursor drift
- Add heading scale to buffers without testing cursor + scroll behavior
