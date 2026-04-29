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

## DO NOT

- Pass `markup.heading` spans to conversation buffer layout — triggers heading scaling
- Pass different spans to `compute_layout()` vs `render_buffer_content()` — causes cursor drift
- Add heading scale to buffers without testing cursor + scroll behavior
