//! Self-contained, offline-first HTML export of a KB subgraph.
//!
//! @ai-caution: [architecture-debt] ~6,205 lines, ~7.75x the 800-line source
//! ceiling — tracked in `.claude/commands/mae-audit.md`'s "Known exceptions"
//! list and `ROADMAP.md`'s "Architecture Debt" section. Folded back in-tree
//! from a standalone sibling project (`bilingual-kb-export`) during the
//! `feat/subgraph-html-export` integration; most of the size is two large
//! embedded string constants (`GRAPH_JS`, `STATIC_CSS`) plus this module's
//! own extensive test suite, not tangled Rust control flow. Not split this
//! pass -- see the audit doc's entry for the candidate seam (extracting the
//! JS/CSS constants to sibling asset files via `include_str!`).
//!
//! One Rust entry point (`HtmlGraphExporter::export`) returns one complete
//! HTML `String` — inline `<style>`, inline `<script>`, an embedded JSON
//! data payload, no external network requests, no bundler, no npm
//! dependency shipped in the output. This mirrors `crate::html::HtmlExporter`
//! (`html.rs`)'s "one function, one dependency-free HTML string" shape; it
//! is a separate type rather than an `Exporter` impl because its input
//! shape (a positioned node/edge graph, not `OrgMeta` + `Vec<OrgElement>`)
//! doesn't fit that trait's signature.
//!
//! `mae-export` stays a leaf crate here too (no `mae-kb`/`mae-canvas`
//! dependency) — mirrors `mae_canvas::kb_graph`'s own "leaf crate, caller
//! bridges the types" pattern (see that module's doc comment). The caller
//! (`crates/ai/src/tool_impls`, which already depends on both) converts
//! `mae_kb::SubgraphResult` + `mae_canvas`-baked layout positions into
//! [`GraphExportNode`]/[`GraphExportEdge`] before calling here.
//!
//! ## `is_seed` vs the exported subgraph's anchor node
//!
//! @ai-caution: [correctness] `GraphExportNode::is_seed` is a literal reuse
//! of `mae_kb`/`mae_canvas`'s REAL `is_seed` concept — `NodeSource::Seed`,
//! i.e. "this is MAE's own compiled-in manual content" (see
//! `mae_canvas::scene::SceneNode::is_seed`'s doc comment and
//! `mae_core::ai_residency::is_residency_exempt`). It has NOTHING to do
//! with "the node this subgraph export was centered on" — that is a
//! genuinely different concept with no existing reusable field anywhere in
//! this codebase, so it gets its own honestly-different name here:
//! [`GraphExportNode::is_anchor`]. Do not conflate the two: a curated
//! user-authored onboarding note (the typical anchor/seed-of-the-BFS node
//! for this exporter's real dogfooded use case) will almost always have
//! `is_seed: false` and `is_anchor: true`. The exported page's distinct
//! styling and Previous/Next reading-order walk are driven by `is_anchor`, not
//! `is_seed` — `is_seed` is exposed in the JSON payload purely because the
//! field genuinely exists on the source data and a reader may still find
//! it useful (e.g. to visually distinguish MAE's own built-in docs from
//! user notes within the same exported subgraph).
//!
//! ## Bilingual content (v1)
//!
//! Translation data is NOT part of `mae-kb`'s `Node` schema — it is a
//! purely additive overlay supplied by an external `{id: {title_es,
//! body_es}}` JSON file (see [`TranslationMap`]/[`load_translations`]),
//! applied by the caller before building [`GraphExportNode`]s. When no
//! translation exists for a node, its ES fields mirror the EN ones and the
//! page hides the EN/ES toggle entirely (see `has_translations` below).
//!
//! ## Mermaid diagrams
//!
//! `#+begin_src mermaid ... #+end_src` blocks inside a node body are
//! pre-rendered to an inline `<svg>` at export time via `npx
//! @mermaid-js/mermaid-cli` (`mmdc`), themed from the same
//! [`GruvboxPalette`] as the page CSS — see [`render_mermaid_block`]. If
//! `mmdc`/`npx` isn't available (or the render fails for any reason), that
//! ONE diagram falls back to a `<pre>` block of the raw mermaid source
//! (with a visible warning), not a failed export.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use crate::html::render_element as render_org_element;
use crate::{
    convert_inline_markup_str, html_escape, parse_org_document, parse_org_link_str, InlineTarget,
    OrgElement,
};

// ---------------------------------------------------------------------
// Bilingual translation overlay
// ---------------------------------------------------------------------

/// One node's translation overlay, loaded from an external `--translations`
/// JSON file — additive, never part of `mae_kb::Node`. See module docs.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct NodeTranslation {
    #[serde(default)]
    pub title_es: Option<String>,
    #[serde(default)]
    pub body_es: Option<String>,
}

/// `{node_id: NodeTranslation}` — the whole shape of a `--translations`
/// file.
pub type TranslationMap = HashMap<String, NodeTranslation>;

/// Load a `--translations` JSON file. Callers only invoke this when a path
/// was explicitly given (an OMITTED `--translations` flag is never an
/// error — see module docs); a path that WAS given but can't be read or
/// doesn't parse as the expected shape IS a real, clearly-reported error —
/// silently ignoring a typo'd path would produce a half-translated page
/// with no indication why.
pub fn load_translations(path: &Path) -> Result<TranslationMap, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "kb_export_subgraph_html: couldn't read translations file '{}': {e}",
            path.display()
        )
    })?;
    serde_json::from_str(&raw).map_err(|e| {
        format!(
            "kb_export_subgraph_html: translations file '{}' isn't valid JSON in the expected \
             `{{\"node-id\": {{\"title_es\": \"...\", \"body_es\": \"...\"}}}}` shape: {e}",
            path.display()
        )
    })
}

// ---------------------------------------------------------------------
// Gruvbox palette (sourced from crates/core/src/themes/gruvbox-{dark,light}.toml)
// ---------------------------------------------------------------------

/// Resolved gruvbox hex values for the exported page's CSS + the mermaid
/// diagram theme. These are a byte-for-byte copy of what
/// `crates/core/src/themes/gruvbox-dark.toml` / `gruvbox-light.toml`'s
/// `[palette]` table plus their `"ui.graph.*"` `[styles]` role-mapping
/// resolve to today — NOT hand-picked gruvbox lookalikes. If either theme
/// file's palette or `ui.graph.*` mapping changes, update the matching
/// `dark()`/`light()` constructor below to match.
#[derive(Debug, Clone, Copy)]
pub struct GruvboxPalette {
    pub name: &'static str,
    pub bg0: &'static str,
    pub bg1: &'static str,
    pub bg2: &'static str,
    pub bg3: &'static str,
    pub fg0: &'static str,
    pub fg1: &'static str,
    pub fg2: &'static str,
    pub fg3: &'static str,
    pub fg4: &'static str,
    pub gray: &'static str,
    /// `"ui.graph.node.<kind>"` resolved fill per `NodeKind`.
    pub node_index: &'static str,
    pub node_command: &'static str,
    pub node_concept: &'static str,
    pub node_key: &'static str,
    pub node_note: &'static str,
    pub node_project: &'static str,
    pub node_category: &'static str,
    pub node_lesson: &'static str,
    pub node_tutorial: &'static str,
    pub node_meta: &'static str,
    pub node_block: &'static str,
    pub node_scheme_api: &'static str,
    pub node_task: &'static str,
    pub node_view: &'static str,
    /// `"ui.graph.node.selected"` — reused here for the ANCHOR node (the
    /// most visually prominent role available), not for click-selection
    /// (the exported page has no separate "selected vs anchor" visual
    /// tier — see `render_css`).
    pub node_selected: &'static str,
    /// `"ui.graph.node.hover"`.
    pub node_hover: &'static str,
    /// `"ui.graph.edge"`.
    pub edge: &'static str,
    /// `"ui.graph.edge.boundary"` — unused by v1 (boundary links are
    /// dropped, see module docs) but kept for parity/future use.
    pub edge_boundary: &'static str,
    /// Single emphasis accent for the chord-diagram nav widget's current
    /// node + its incident edges (everything else in that widget renders
    /// muted, see `push_palette_vars`/`STATIC_CSS`). This is deliberately
    /// NOT one of the 14 `node_*` per-`NodeKind` hues above — those were
    /// validated (dataviz skill's categorical-palette checker, run against
    /// gruvbox's 7 accent hues as a 7-slot categorical set) to genuinely
    /// FAIL as a simultaneous-discrimination categorical palette (chroma-
    /// floor failures on `#83a598`/`#d3869b` in dark mode; a CVD ΔE of 1.9
    /// on the `#d3869b`↔`#83a598` adjacent pair; a normal-vision floor of
    /// 10.2 on `#fabd2f`↔`#b8bb26`, under the 15 threshold) — fine for the
    /// full native graph view's 14-way kind legend (a much larger canvas,
    /// point-read rather than simultaneous-discrimination), but not for a
    /// small widget that needs exactly ONE color to reliably pop against a
    /// muted field. `bright_orange` (dark) / `orange` (light) — gruvbox's
    /// own literal palette values, both independently re-derivable: WCAG
    /// contrast against this theme's own `bg0` is 5.84:1 (dark,
    /// `#fe8019` on `#282828`) / 3.41:1 (light, `#d65d0e` on `#fbf1c7`).
    pub accent: &'static str,
    /// Prose-link color (in-body `<a>`, popover links) — deliberately a
    /// *different* hue from `accent` (orange), which already carries a
    /// specific meaning elsewhere on the page ("this is the current node/
    /// edge"); reusing it for ordinary hyperlinks would blur that meaning.
    /// Gruvbox `bright_blue`/`blue` (the same hues already used for the
    /// `node_scheme_api` kind color, just promoted to a first-class role
    /// here), validated as a single-hue ordinal ramp (not part of the
    /// 7-slot categorical set that failed): 5.48:1 contrast on dark `bg0`
    /// (`#83a598` on `#282828`), 3.73:1 on light `bg0` (`#458588` on
    /// `#fbf1c7`) — both clear the 3:1 UI-component floor.
    pub link: &'static str,
}

impl GruvboxPalette {
    /// `gruvbox-dark.toml`.
    pub const fn dark() -> Self {
        Self {
            name: "dark",
            bg0: "#282828",
            bg1: "#3c3836",
            bg2: "#504945",
            bg3: "#665c54",
            fg0: "#fbf1c7",
            fg1: "#ebdbb2",
            fg2: "#d5c4a1",
            fg3: "#bdae93",
            fg4: "#a89984",
            gray: "#928374",
            node_index: "#fabd2f",      // bright_yellow
            node_command: "#cc241d",    // red
            node_concept: "#b8bb26",    // bright_green
            node_key: "#d3869b",        // bright_purple
            node_note: "#98971a",       // green
            node_project: "#fabd2f",    // bright_yellow
            node_category: "#8ec07c",   // bright_aqua
            node_lesson: "#b8bb26",     // bright_green
            node_tutorial: "#b8bb26",   // bright_green
            node_meta: "#fb4934",       // bright_red
            node_block: "#b16286",      // purple
            node_scheme_api: "#83a598", // bright_blue
            node_task: "#fb4934",       // bright_red
            node_view: "#ebdbb2",       // fg1
            node_selected: "#fabd2f",   // bright_yellow
            node_hover: "#8ec07c",      // bright_aqua
            edge: "#928374",            // gray
            edge_boundary: "#fb4934",   // bright_red
            accent: "#fe8019",          // bright_orange
            link: "#83a598",            // bright_blue
        }
    }

    /// `gruvbox-light.toml`.
    pub const fn light() -> Self {
        Self {
            name: "light",
            bg0: "#fbf1c7",
            bg1: "#ebdbb2",
            bg2: "#d5c4a1",
            bg3: "#bdae93",
            fg0: "#282828",
            fg1: "#3c3836",
            fg2: "#504945",
            fg3: "#665c54",
            fg4: "#7c6f64",
            gray: "#928374",
            node_index: "#d79921",      // yellow
            node_command: "#cc241d",    // red
            node_concept: "#98971a",    // green
            node_key: "#b16286",        // purple
            node_note: "#98971a",       // green
            node_project: "#d79921",    // yellow
            node_category: "#689d6a",   // aqua
            node_lesson: "#98971a",     // green
            node_tutorial: "#98971a",   // green
            node_meta: "#cc241d",       // red
            node_block: "#b16286",      // purple
            node_scheme_api: "#458588", // blue
            node_task: "#cc241d",       // red
            node_view: "#3c3836",       // fg1
            node_selected: "#d65d0e",   // orange
            node_hover: "#689d6a",      // aqua
            edge: "#928374",            // gray
            edge_boundary: "#cc241d",   // red
            accent: "#d65d0e",          // orange
            link: "#458588",            // blue
        }
    }
}

// ---------------------------------------------------------------------
// Mermaid diagram pre-rendering
// ---------------------------------------------------------------------

/// Shell out to `npx @mermaid-js/mermaid-cli` (`mmdc`) to render one
/// mermaid diagram to an inline SVG string, themed from `palette`. Mirrors
/// the invocation shape of `~/.claude/skills/org-kb-to-pdf/scripts/render_diagrams.sh`
/// (`npx mmdc -i in.mmd -o out.png -b white -s 4`), with `-o out.svg` for
/// SVG instead of PNG and a `-c` mermaid config JSON (`themeVariables`)
/// built from `palette` instead of a plain white background.
fn try_render_mermaid_svg(source: &str, palette: &GruvboxPalette) -> Result<String, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let scratch = std::env::temp_dir().join(format!(
        "mae-mermaid-{}-{}-{}",
        std::process::id(),
        nanos,
        source.len()
    ));
    std::fs::create_dir_all(&scratch)
        .map_err(|e| format!("couldn't create scratch dir {}: {e}", scratch.display()))?;
    let in_path = scratch.join("diagram.mmd");
    let out_path = scratch.join("diagram.svg");
    let config_path = scratch.join("mermaid-config.json");

    std::fs::write(&in_path, source).map_err(|e| format!("couldn't write mermaid source: {e}"))?;

    let config = serde_json::json!({
        "theme": "base",
        "themeVariables": {
            "background": palette.bg0,
            "primaryColor": palette.bg1,
            "primaryTextColor": palette.fg1,
            "primaryBorderColor": palette.node_scheme_api,
            "lineColor": palette.gray,
            "secondaryColor": palette.bg2,
            "tertiaryColor": palette.bg1,
            "textColor": palette.fg1,
            "nodeTextColor": palette.fg1,
            "mainBkg": palette.bg1,
            "edgeLabelBackground": palette.bg0,
            "clusterBkg": palette.bg2,
            "clusterBorder": palette.bg3,
            "fontFamily": "monospace",
        },
    });
    std::fs::write(
        &config_path,
        serde_json::to_string(&config).unwrap_or_default(),
    )
    .map_err(|e| format!("couldn't write mermaid theme config: {e}"))?;

    let result = Command::new("npx")
        .args(["--yes", "@mermaid-js/mermaid-cli"])
        .arg("-i")
        .arg(&in_path)
        .arg("-o")
        .arg(&out_path)
        .arg("-b")
        .arg(palette.bg0)
        .arg("-c")
        .arg(&config_path)
        .current_dir(&scratch)
        .output();

    let svg = match result {
        Ok(output) if output.status.success() => std::fs::read_to_string(&out_path)
            .map_err(|e| format!("mmdc reported success but the output SVG is unreadable: {e}")),
        Ok(output) => Err(format!(
            "mmdc exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(e) => Err(format!(
            "couldn't launch `npx` — is Node.js installed? ({e})"
        )),
    };

    let _ = std::fs::remove_dir_all(&scratch);
    svg
}

/// Render one `#+begin_src mermaid` block's source to HTML: an inline
/// `<svg>` on success, or (without failing the whole export) a warning +
/// `<pre>` of the raw source on failure. `source` is the raw mermaid text
/// (NOT html-escaped — the success path embeds mmdc's own SVG output
/// as-is, the failure path escapes it going into `<pre>`).
fn render_mermaid_block(source: &str, palette: &GruvboxPalette) -> String {
    match try_render_mermaid_svg(source, palette) {
        Ok(svg) => format!("<div class=\"mermaid-diagram\">{svg}</div>\n"),
        Err(reason) => {
            eprintln!(
                "kb_export_subgraph_html: mermaid diagram render failed, falling back to raw \
                 source ({reason})"
            );
            mermaid_fallback_html(source, &reason)
        }
    }
}

/// Pure (no subprocess) — the exact markup `render_mermaid_block` falls
/// back to on a render failure. Split out so the fallback shape is
/// unit-testable without depending on `mmdc`/`npx` being present in the
/// test environment.
fn mermaid_fallback_html(source: &str, reason: &str) -> String {
    format!(
        "<div class=\"mermaid-fallback\">\n\
         <p class=\"mermaid-fallback-warning\">⚠ Diagram could not be rendered ({}) — showing raw source:</p>\n\
         <pre><code class=\"language-mermaid\">{}</code></pre>\n\
         </div>\n",
        html_escape(reason),
        html_escape(source)
    )
}

// ---------------------------------------------------------------------
// Node body -> HTML (org source blocks get mermaid special-casing;
// everything else reuses crate::html's element renderer)
// ---------------------------------------------------------------------

/// Render a KB node's org-mode body to an HTML fragment safe to assign via
/// `element.innerHTML` client-side: every bit of the source node's own
/// text content is `html_escape`d (via `crate::html`'s existing element
/// renderer — see `render_org_element`'s doc comment), and `#+begin_src
/// mermaid` blocks are pre-rendered to inline SVG (or a safe raw-source
/// fallback) instead of being dumped as a generic `<pre><code>` block.
/// `mae_kb::Node::body` retains a `:PROPERTIES:...:END:` drawer verbatim
/// (properties are metadata, but the generic org parser this module reuses
/// — `parse_org_document`, written for exporting genuine org-file body
/// content — has no concept of a drawer and treats it as an ordinary
/// paragraph). Left unstripped, an exported node's body renders its own
/// raw `:ID:`/`:hash:` lines as visible prose. Bounded to the first ~500
/// chars so a `:PROPERTIES:`-looking string deep in real prose is never
/// mistaken for one — mirrors the same bounded-prefix convention
/// `shared/kb/src/activity.rs::body_hash` already uses for the same
/// drawer shape.
///
/// Splices the drawer OUT rather than requiring it to be the very first
/// thing in the body: a real KB node (a roadmap checklist item, tagged
/// `roadmap`/`phase`) reproduced a leak where its own first line of prose
/// ("upgrade fails partway.") precedes the drawer -- org only auto-hides
/// a `:PROPERTIES:` drawer when it immediately follows a headline; one
/// attached to a plain list item (this KB's roadmap-step convention,
/// documented in `org-kb-to-pdf`'s own gotcha list for the same drawer
/// shape) keeps whatever text came before it in the same body string. A
/// stricter "must be leading" check silently let that content through as
/// visible `:ID:`/`:END:` prose instead of being treated as metadata.
fn strip_leading_properties_drawer(body: &str) -> std::borrow::Cow<'_, str> {
    // `body.len().min(500)` alone is a byte OFFSET, not a char boundary --
    // a real, reproducible panic ("byte index N is not a char boundary")
    // when a multi-byte UTF-8 character (an em dash, `—`, 3 bytes) straddles
    // exactly byte 500. Walk backward to the nearest real char boundary
    // (str::floor_char_boundary is nightly-only, so this is the stable
    // equivalent) rather than assuming every byte offset is safe to slice at.
    let mut boundary = body.len().min(500);
    while boundary > 0 && !body.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let head = &body[..boundary];
    if let Some(props_start) = head.find(":PROPERTIES:") {
        // Real org drawer syntax requires the `:PROPERTIES:` marker to sit
        // alone on its own line (whitespace-only before it, back to the
        // start of that line) -- otherwise this would also match a body
        // that merely mentions ":PROPERTIES:" mid-sentence while discussing
        // org-mode syntax, which is real prose, not metadata to hide.
        let line_start = head[..props_start].rfind('\n').map_or(0, |i| i + 1);
        if head[line_start..props_start].trim().is_empty() {
            if let Some(end_rel) = head[props_start..].find(":END:") {
                let end = props_start + end_rel + ":END:".len();
                let tail = body[end..].trim_start_matches(['\n', '\r']);
                let mut spliced = String::with_capacity(props_start + tail.len());
                spliced.push_str(&body[..props_start]);
                spliced.push_str(tail);
                return std::borrow::Cow::Owned(spliced);
            }
        }
    }
    std::borrow::Cow::Borrowed(body)
}

fn render_node_body_html(body: &str, palette: &GruvboxPalette) -> String {
    let body = strip_leading_properties_drawer(body);
    let (meta, elements) = parse_org_document(&body);
    let mut html = String::with_capacity(body.len() * 2);
    for element in &elements {
        if let OrgElement::SrcBlock {
            language,
            body: src,
            ..
        } = element
        {
            if language.eq_ignore_ascii_case("mermaid") {
                html.push_str(&render_mermaid_block(src, palette));
                continue;
            }
        }
        render_org_element(&mut html, element, &meta.options);
    }
    html
}

/// A short, plain-text (no markup) preview of a node body for the hover
/// popover — org markup resolved away, collapsed to single-line whitespace,
/// truncated to `max_chars` (character-boundary-safe) with a trailing
/// ellipsis if truncated.
///
/// Each element's raw text is run through `convert_inline_markup_str(...,
/// InlineTarget::PlainText)` — the same parser `render_node_body_html`
/// (HTML) and `html.rs`'s `HtmlExporter` (Markdown) already use, just a
/// third output mode — rather than a separate hand-rolled stripper. The
/// previous version only filtered out `* / = ~` characters, which drops
/// emphasis markers but has no concept of a `[[target|label]]` link at
/// all: a body containing one showed the raw brackets/pipe/target text
/// verbatim in the hover popover (a real bug, not hypothetical -- visible
/// in this session's own earlier screenshot as a mangled, concatenated
/// GitHub URL). Reusing the real parser fixes that for free instead of
/// teaching a second, separate implementation about link syntax.
pub fn plain_text_preview(body: &str, max_chars: usize) -> String {
    let body = strip_leading_properties_drawer(body);
    let (_, elements) = parse_org_document(&body);
    let mut text = String::new();
    let push_plain = |s: &str, text: &mut String| {
        text.push_str(&convert_inline_markup_str(s, InlineTarget::PlainText));
        text.push(' ');
    };
    for element in &elements {
        match element {
            OrgElement::Paragraph(p) => push_plain(p, &mut text),
            OrgElement::Heading { title, .. } => push_plain(title, &mut text),
            OrgElement::Quote(q) => push_plain(q, &mut text),
            OrgElement::List { items, .. } => {
                for item in items {
                    push_plain(&item.content, &mut text);
                }
            }
            // Previously missing entirely -- a node whose body is a table
            // (e.g. the HCL cheatsheet's function-reference table) got an
            // empty or near-empty hover-popover preview, since no cell
            // content ever reached `text` at all.
            OrgElement::Table { rows, .. } => {
                for row in rows {
                    for cell in row {
                        push_plain(cell, &mut text);
                    }
                }
            }
            _ => {}
        }
    }
    // Collapse whitespace (newlines from multi-line paragraphs, etc.) into
    // single spaces -- markup/link resolution already happened above.
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(collapsed.trim(), max_chars)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

// ---------------------------------------------------------------------
// Exported graph data model
// ---------------------------------------------------------------------

/// One node ready for HTML export: a flattened, already-positioned,
/// already-rendered view over a `mae_kb::Node` (the caller in
/// `crates/ai/src/tool_impls` bridges the types — see module docs).
#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphExportNode {
    pub id: String,
    pub kind: String,
    pub x: f64,
    pub y: f64,
    /// `NodeSource::Seed` — see module docs' `is_seed` vs `is_anchor` note.
    pub is_seed: bool,
    /// The BFS starter/center node this subgraph export was rooted at —
    /// drives the page's distinct styling and the Previous/Next
    /// reading-order walk. See module docs.
    pub is_anchor: bool,
    /// A "guidance node" (kb/adrs/0004-guidance-nodes-colophon.org) —
    /// editorial/meta content (writing-style, review, accuracy standards)
    /// the exported guide was written against, not part of its subject
    /// matter. Always rendered in a distinct "About this guide" colophon
    /// section (see [`HtmlGraphExporter::export`]) and excluded from the
    /// interactive chord graph and the Previous/Next reading-order walk —
    /// this tool only DISPLAYS these nodes, per that ADR's decision; it
    /// never interprets or checks content against them. Set via
    /// [`build_guidance_node`], never alongside `is_anchor: true` (a node
    /// is either the guide's subject or its colophon, not both).
    pub is_guidance: bool,
    /// Explicit, authored Previous/Next reading-order chain (a project-local
    /// org convention — mae_kb has no first-class concept of this, it's
    /// just a `* Reading Order` heading with `Previous ::`/`Next ::` links
    /// in the body). `None` when absent or unresolvable — see
    /// [`parse_reading_order`]. Consumed client-side by `computeReadingOrder`
    /// (GRAPH_JS) to walk the authored chain in preference to the BFS-
    /// distance heuristic for nodes that have one.
    pub reading_order_prev: Option<String>,
    pub reading_order_next: Option<String>,
    /// The `Part ::` line from the same `* Reading Order` section (a plain
    /// text label, e.g. "Project-Scope Architecture Decisions" — the
    /// authored structural-context grouping this node sits in), rendered
    /// client-side as a "you are here" breadcrumb above the node title. See
    /// [`parse_reading_order_part`]. Unlike `reading_order_prev`/`_next`
    /// (id-linked navigation targets, language-agnostic), this IS
    /// translatable display text, so it's parsed once per language —
    /// `_es` falls back to `_en` when no Spanish translation exists, the
    /// same per-field fallback convention `title_es`/`body_es` already use.
    pub reading_order_part_en: Option<String>,
    pub reading_order_part_es: Option<String>,
    /// Org `#+filetags:`, verbatim from `mae_kb::Node.tags`. Empty for
    /// guidance nodes (out of scope for the interactive graph's tag filter,
    /// same as they're already excluded from `topicNodes`). Drives the
    /// header's tag-filter UI (GRAPH_JS `applyTagFilter`).
    pub tags: Vec<String>,
    pub title_en: String,
    pub body_en: String,
    pub preview_en: String,
    pub title_es: String,
    pub body_es: String,
    pub preview_es: String,
}

/// One internal edge (both endpoints present in the exported node set —
/// boundary links are dropped for v1, see module docs).
#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphExportEdge {
    pub source: String,
    pub target: String,
    pub rel_type: String,
    pub weight: f64,
}

/// Build a [`GraphExportNode`] from raw org-mode title/body content —
/// applies mermaid pre-rendering, the plain-text hover preview, and the
/// bilingual overlay (mirrors EN when no translation is present) in one
/// place, so callers (the real tool impl AND tests) never have to
/// duplicate this assembly.
#[allow(clippy::too_many_arguments)]
pub fn build_export_node(
    id: impl Into<String>,
    kind: impl Into<String>,
    x: f64,
    y: f64,
    is_seed: bool,
    is_anchor: bool,
    title: &str,
    body: &str,
    translation: Option<&NodeTranslation>,
    palette: &GruvboxPalette,
) -> GraphExportNode {
    let body_html = render_node_body_html(body, palette);
    let preview_en = plain_text_preview(body, 200);
    let title_es = translation
        .and_then(|t| t.title_es.clone())
        .unwrap_or_else(|| title.to_string());
    let es_body_raw = translation.and_then(|t| t.body_es.as_deref());
    let (body_es, preview_es) = match es_body_raw {
        Some(es_body) => (
            render_node_body_html(es_body, palette),
            plain_text_preview(es_body, 200),
        ),
        None => (body_html.clone(), preview_en.clone()),
    };
    let (reading_order_prev, reading_order_next) = parse_reading_order(body);
    let reading_order_part_en = parse_reading_order_part(body);
    let reading_order_part_es = es_body_raw
        .and_then(parse_reading_order_part)
        .or_else(|| reading_order_part_en.clone());
    GraphExportNode {
        id: id.into(),
        kind: kind.into(),
        x,
        y,
        is_seed,
        is_anchor,
        is_guidance: false,
        reading_order_prev,
        reading_order_next,
        reading_order_part_en,
        reading_order_part_es,
        tags: Vec::new(),
        title_en: title.to_string(),
        body_en: body_html,
        preview_en,
        title_es,
        body_es,
        preview_es,
    }
}

/// Extract an explicit, authored Previous/Next reading-order chain from a
/// node's raw org body, if present. This is a project-local org convention
/// (`* Reading Order` heading, `Previous ::`/`Next ::` list items, each
/// either `none` — chain start/end — or a `[[id:...][...]]` link) — mae_kb
/// has no first-class concept of it (no `rel_type` distinguishes these
/// links from any other), so it's parsed here from body text specifically
/// for `computeReadingOrder` (GRAPH_JS) to prefer over the BFS-distance
/// heuristic when it exists. Reuses this crate's own `parse_org_document`
/// (the single source of truth for org semantics here) and the same
/// bracket-link parser the body renderer already uses — not a second,
/// parallel text-scanning parser. Defensive: any missing/malformed shape
/// (no such heading, no matching list, a list item with no link) yields
/// `None` for that side, never a panic — this walks real, user-authored
/// prose, not a guaranteed-well-formed machine format.
fn parse_reading_order(body: &str) -> (Option<String>, Option<String>) {
    let (_, elements) = parse_org_document(body);
    let mut prev = None;
    let mut next = None;
    let mut in_section = false;
    for el in &elements {
        match el {
            OrgElement::Heading { title, .. } => {
                in_section = title == "Reading Order";
            }
            OrgElement::List { items, .. } if in_section => {
                for item in items {
                    if let Some(rest) = item.content.strip_prefix("Previous ::") {
                        prev = extract_first_link_id(rest.trim());
                    } else if let Some(rest) = item.content.strip_prefix("Next ::") {
                        next = extract_first_link_id(rest.trim());
                    }
                }
            }
            _ => {}
        }
    }
    (prev, next)
}

/// Extract the plain-text `Part ::` label from a node's `* Reading Order`
/// section, if present — the authored structural-context line (e.g. "Part
/// :: Project-Scope Architecture Decisions.") that groups a node within the
/// KB's larger authored structure, distinct from the `Previous ::`/
/// `Next ::` id-linked navigation targets [`parse_reading_order`] extracts
/// from the same section. Unlike those (language-agnostic ids), this IS
/// translatable display text, so callers parse it separately against
/// English vs. Spanish body text (see `build_export_node`) rather than
/// once. A trailing period — the consistent authored convention on every
/// real "Part ::" line seen in this KB — is stripped for a cleaner
/// breadcrumb label. Defensive like `parse_reading_order`: no heading/list/
/// prefix match yields `None`, never a panic.
fn parse_reading_order_part(body: &str) -> Option<String> {
    let (_, elements) = parse_org_document(body);
    let mut in_section = false;
    for el in &elements {
        match el {
            OrgElement::Heading { title, .. } => {
                in_section = title == "Reading Order";
            }
            OrgElement::List { items, .. } if in_section => {
                for item in items {
                    if let Some(rest) = item.content.strip_prefix("Part ::") {
                        let text = rest.trim().trim_end_matches('.').trim();
                        if !text.is_empty() {
                            return Some(text.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Find the first `[[...]]` link in `text` and return its target id
/// (`id:`-prefix stripped, matching how body links are normalized
/// elsewhere in this file). `None` if there's no link at all (the `Previous
/// :: none — first document...` chain-boundary case).
fn extract_first_link_id(text: &str) -> Option<String> {
    let pos = text.find("[[")?;
    let (_, target, _) = parse_org_link_str(text, pos)?;
    Some(target.strip_prefix("id:").unwrap_or(target).to_string())
}

/// Build a "guidance node" (kb/adrs/0004-guidance-nodes-colophon.org) — an
/// editorial/meta note (writing-style guide, review standards, accuracy
/// discipline) an exported guide was written against, always included and
/// rendered in a distinct colophon section regardless of BFS reachability
/// from the export's anchor. Same content assembly as [`build_export_node`]
/// (mermaid pre-rendering, bilingual overlay, plain-text preview all still
/// apply — a guidance node with no Spanish translation gets the exact same
/// ADR-0003 fallback notice as any other node when a reader opens it), just
/// with no graph position (never drawn in the interactive chord widget, so
/// `x`/`y` are unused) and never simultaneously the anchor.
pub fn build_guidance_node(
    id: impl Into<String>,
    kind: impl Into<String>,
    title: &str,
    body: &str,
    translation: Option<&NodeTranslation>,
    palette: &GruvboxPalette,
) -> GraphExportNode {
    let mut n = build_export_node(
        id,
        kind,
        0.0,
        0.0,
        false,
        false,
        title,
        body,
        translation,
        palette,
    );
    n.is_guidance = true;
    n
}

// ---------------------------------------------------------------------
// HTML assembly
// ---------------------------------------------------------------------

/// Tunable magic numbers behind the exported chord diagram's layout/
/// animation math and a couple of UI timing values -- previously hardcoded
/// literals baked directly into STATIC_CSS/GRAPH_JS. `Default` reproduces
/// every one of those original hardcoded values exactly, so
/// `ChordDiagramConfig::default()` round-trips through [`export`] and
/// [`export_with_config`] to byte-identical output (see the
/// `default_chord_config_produces_identical_output_to_export` test) --
/// callers who never touch this type see no behavior change. See
/// `kb/adrs/0005-chord-diagram-config.org` for the surface-level design
/// rationale.
///
/// [`export`]: HtmlGraphExporter::export
/// [`export_with_config`]: HtmlGraphExporter::export_with_config
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChordDiagramConfig {
    /// GRAPH_JS `HOVER_GROWTH_FACTOR`: multiplier applied to a wedge's
    /// world radius on hover. 1.0 disables the hover-growth effect.
    pub hover_growth_factor: f64,
    /// GRAPH_JS `strokeBuffer`: viewBox padding reserved for stroke width
    /// beyond the strict hover-growth radius.
    pub stroke_buffer_px: f64,
    /// GRAPH_JS `cosmeticCushion`: extra flat viewBox padding beyond the
    /// strict correctness minimum, purely for visual breathing room.
    pub cosmetic_cushion_px: f64,
    /// GRAPH_JS `minOnscreenRadiusPx`: the guaranteed minimum on-screen
    /// wedge thickness (hit-target floor).
    pub min_onscreen_radius_px: f64,
    /// GRAPH_JS's initial `pad` seed for the two-pass viewBox-fit loop.
    /// NOTE: this is a convergence *seed*, not the final padding value
    /// (the loop overwrites `pad` after fitting `maxOuterR`) -- a
    /// different seed only makes the two-pass approximation converge from
    /// a different starting point, it never changes the final result or
    /// breaks rendering.
    pub initial_pad_px: f64,
    /// GRAPH_JS edge `pullBack`: how far a chord's control point is pulled
    /// toward the ring center (0 = straight line, 1 = fully at center).
    pub edge_pull_back: f64,
    /// GRAPH_JS `wedgeGapRadians`: angular gap left between adjacent wedge
    /// slots (0 = flush, separated only by rounded corners).
    pub wedge_gap_radians: f64,
    /// GRAPH_JS `HISTORY_DEPTH_CAP`: how many visited nodes the history
    /// panel keeps before evicting the oldest.
    pub history_depth_cap: u32,
    /// GRAPH_JS wedge `cornerRadius`'s fraction of `halfThickness` (the
    /// "petal" rounding amount).
    pub wedge_corner_radius_fraction: f64,
    /// GRAPH_JS search-input debounce (ms) before hiding stale results.
    pub search_debounce_ms: u32,
    /// STATIC_CSS's dominant transition/animation duration (ms) --
    /// replaces every "200ms" occurrence, the majority of this file's
    /// timing rules. Deliberately does NOT touch the two 180ms
    /// micro-interaction rules or the 220ms fullscreen-enter asymmetry,
    /// which stay fixed regardless of this value (see
    /// `ui_transition_ms_override_does_not_touch_180ms_or_220ms_rules`) --
    /// this is one coarse knob, not a per-rule timing system.
    pub ui_transition_ms: u32,
}

impl Default for ChordDiagramConfig {
    fn default() -> Self {
        ChordDiagramConfig {
            hover_growth_factor: 1.6,
            stroke_buffer_px: 2.0,
            cosmetic_cushion_px: 16.0,
            min_onscreen_radius_px: 12.0,
            initial_pad_px: 40.0,
            edge_pull_back: 0.55,
            wedge_gap_radians: 0.0,
            history_depth_cap: 8,
            wedge_corner_radius_fraction: 0.6,
            search_debounce_ms: 150,
            ui_transition_ms: 200,
        }
    }
}

/// Applies `cfg` to `GRAPH_JS` via exact-substring replacement against
/// verified anchor literals -- GRAPH_JS is deliberately a plain raw string,
/// not `format!`-templated (see its own doc comment: with ~5000 lines of
/// JS full of `{`/`}`, escaping every literal brace for `format!` would be
/// invasive and bug-prone). Returns the const unchanged (zero allocation)
/// when `cfg` is the default, since a default-valued config always
/// formats back to the exact original literal text.
fn render_graph_js(cfg: &ChordDiagramConfig) -> std::borrow::Cow<'static, str> {
    if *cfg == ChordDiagramConfig::default() {
        return std::borrow::Cow::Borrowed(GRAPH_JS);
    }
    let mut js = GRAPH_JS.to_string();
    js = js.replacen(
        "var HOVER_GROWTH_FACTOR = 1.6;",
        &format!("var HOVER_GROWTH_FACTOR = {};", cfg.hover_growth_factor),
        1,
    );
    js = js.replacen(
        "var strokeBuffer = 2;",
        &format!("var strokeBuffer = {};", cfg.stroke_buffer_px),
        1,
    );
    js = js.replacen(
        "var cosmeticCushion = 16;",
        &format!("var cosmeticCushion = {};", cfg.cosmetic_cushion_px),
        1,
    );
    js = js.replacen(
        "var minOnscreenRadiusPx = 12;",
        &format!("var minOnscreenRadiusPx = {};", cfg.min_onscreen_radius_px),
        1,
    );
    js = js.replacen(
        "var pad = 40;",
        &format!("var pad = {};", cfg.initial_pad_px),
        1,
    );
    js = js.replacen(
        "var pullBack = 0.55;",
        &format!("var pullBack = {};", cfg.edge_pull_back),
        1,
    );
    js = js.replacen(
        "var wedgeGapRadians = 0;",
        &format!("var wedgeGapRadians = {};", cfg.wedge_gap_radians),
        1,
    );
    js = js.replacen(
        "var HISTORY_DEPTH_CAP = 8;",
        &format!("var HISTORY_DEPTH_CAP = {};", cfg.history_depth_cap),
        1,
    );
    js = js.replacen(
        "var cornerRadius = halfThickness * 0.6;",
        &format!(
            "var cornerRadius = halfThickness * {};",
            cfg.wedge_corner_radius_fraction
        ),
        1,
    );
    js = js.replacen(
        "window.setTimeout(function () { searchResults.hidden = true; }, 150);",
        &format!(
            "window.setTimeout(function () {{ searchResults.hidden = true; }}, {});",
            cfg.search_debounce_ms
        ),
        1,
    );
    std::borrow::Cow::Owned(js)
}

/// Applies `cfg.ui_transition_ms` to `STATIC_CSS` -- same exact-substring
/// approach and same default fast path as [`render_graph_js`], scoped to
/// just the one field STATIC_CSS actually exposes.
fn render_static_css(cfg: &ChordDiagramConfig) -> std::borrow::Cow<'static, str> {
    if cfg.ui_transition_ms == ChordDiagramConfig::default().ui_transition_ms {
        return std::borrow::Cow::Borrowed(STATIC_CSS);
    }
    std::borrow::Cow::Owned(STATIC_CSS.replace("200ms", &format!("{}ms", cfg.ui_transition_ms)))
}

/// Exports a positioned KB subgraph to one self-contained HTML page.
/// Mirrors `crate::html::HtmlExporter`'s "one function, one dependency-free
/// HTML string" shape (see module docs for why this isn't the same
/// `Exporter` trait).
pub struct HtmlGraphExporter;

impl HtmlGraphExporter {
    /// `page_title`: `<title>`/`<h1>` text (e.g. "Terraform Onboarding").
    /// `anchor_id`: the id of the node with `is_anchor: true` in `nodes` —
    /// used to drive the reading-order walk's starting point without the
    /// page having to re-scan `nodes` client-side for the flag.
    pub fn export(
        &self,
        nodes: &[GraphExportNode],
        edges: &[GraphExportEdge],
        anchor_id: &str,
        page_title: &str,
    ) -> String {
        self.export_with_config(
            nodes,
            edges,
            anchor_id,
            page_title,
            &ChordDiagramConfig::default(),
        )
    }

    /// Same as [`export`](Self::export), with the chord diagram's layout/
    /// animation/timing constants overridable via `config` instead of
    /// fixed at their hardcoded defaults.
    pub fn export_with_config(
        &self,
        nodes: &[GraphExportNode],
        edges: &[GraphExportEdge],
        anchor_id: &str,
        page_title: &str,
        config: &ChordDiagramConfig,
    ) -> String {
        let node_ids: std::collections::HashSet<&str> =
            nodes.iter().map(|n| n.id.as_str()).collect();
        // Defensive: drop any edge whose endpoint isn't in `nodes` — an
        // internal-caller-contract violation (boundary links are supposed
        // to already be filtered out before this point, see module docs),
        // not a user-facing error, so this stays a silent filter rather
        // than a panic/Result.
        let edges: Vec<&GraphExportEdge> = edges
            .iter()
            .filter(|e| {
                node_ids.contains(e.source.as_str()) && node_ids.contains(e.target.as_str())
            })
            .collect();

        let has_translations = nodes.iter().any(|n| n.title_es != n.title_en);

        let dark = GruvboxPalette::dark();
        let light = GruvboxPalette::light();

        let payload = serde_json::json!({
            "anchorId": anchor_id,
            "hasTranslations": has_translations,
            "nodes": nodes,
            "edges": edges,
        });
        let payload_json = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());

        let mut html = String::with_capacity(64 * 1024);
        html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
        html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
        html.push_str("<title>");
        html.push_str(&html_escape(page_title));
        html.push_str("</title>\n<style>\n");
        html.push_str(&render_css_variables(&dark, &light));
        html.push_str(&render_static_css(config));
        html.push_str("</style>\n</head>\n<body>\n");

        html.push_str("<header id=\"page-header\">\n<h1 id=\"page-title\">");
        html.push_str(&html_escape(page_title));
        html.push_str("</h1>\n");
        html.push_str(
            "<div class=\"search-group\">\n\
             <input id=\"node-search\" type=\"text\" placeholder=\"Search nodes\u{2026}\" autocomplete=\"off\">\n\
             <div id=\"search-results\" class=\"search-results\" hidden></div>\n\
             </div>\n\
             <div class=\"tag-filter-group\">\n\
             <button id=\"tag-picker-toggle\" type=\"button\">Tags \u{25be}</button>\n\
             <div id=\"tag-picker\" class=\"tag-picker\" hidden></div>\n\
             <div id=\"active-tag-chips\" class=\"active-tag-chips\"></div>\n\
             </div>\n",
        );
        html.push_str("<div class=\"controls\">\n");
        // Leftmost in .controls so it lands on the header's own row before
        // Home/Prev/Next when #page-header's flex-wrap breaks the group
        // onto a second line on a narrow viewport. Label/aria-expanded are
        // rewritten on every state change by GRAPH_JS (setSidebarOpen) --
        // unlike #theme-toggle's static label, "is the sidebar open" isn't
        // otherwise visible once it's off-canvas.
        html.push_str(
            "<button id=\"sidebar-toggle\" type=\"button\" aria-expanded=\"true\" aria-controls=\"sidebar\">\u{2630} Hide sidebar</button>\n",
        );
        html.push_str(
            "<button id=\"home-button\" type=\"button\" title=\"Jump to the spine/anchor node\">\u{2302} Home</button>\n",
        );
        html.push_str(
            "<button id=\"prev-button\" type=\"button\" disabled>\u{2190} Previous</button>\n",
        );
        html.push_str("<button id=\"next-button\" type=\"button\">Next \u{2192}</button>\n");
        html.push_str(
            "<button id=\"theme-toggle\" type=\"button\">\u{263D}/\u{2600} Theme</button>\n",
        );
        html.push_str("<button id=\"lang-toggle\" type=\"button\" hidden>EN / ES</button>\n");
        html.push_str("</div>\n</header>\n");

        // The node's own rendered content is the primary reading surface, not
        // the nav chrome around it -- `#main-content` is deliberately the
        // dominant flex child, with the chord diagram + outline demoted to a
        // narrow fixed-width `#sidebar`, stacked (chord above outline) rather
        // than side by side. Element ids are unchanged from the prior layout
        // (`#detail-panel-content`, `#graph-pane`, `#outline-panel`, etc.) --
        // every DOM query in GRAPH_JS below is by id, not DOM position, so
        // this restructure is pure markup/CSS, no JS changes needed for it.
        html.push_str(
            "<main id=\"app-main\">\n\
             <article id=\"main-content\">\n\
             <div id=\"detail-panel-content\">\n\
             <p class=\"hint\">Click a node in the graph to see its details here.</p>\n\
             </div>\n\
             </article>\n\
             <aside id=\"sidebar\">\n\
             <div id=\"graph-pane\">\n\
             <button id=\"graph-fullscreen-toggle\" type=\"button\" title=\"Expand diagram\" aria-label=\"Expand diagram\">\u{26F6}</button>\n\
             <div id=\"graph-caption\"></div>\n\
             <svg id=\"graph-svg\" xmlns=\"http://www.w3.org/2000/svg\"></svg>\n\
             <div id=\"popover\" class=\"popover\" hidden></div>\n\
             </div>\n\
             <nav id=\"outline-panel\">\n\
             <h3 id=\"outline-toggle\">On this page \u{25be}</h3>\n\
             <ul class=\"outline-list\" id=\"outline-list\"></ul>\n\
             </nav>\n\
             <nav id=\"history-panel\">\n\
             <h3>Visited</h3>\n\
             <div class=\"history-controls\">\n\
             <button id=\"history-back\" type=\"button\" title=\"Go back\">\u{2190} Back</button>\n\
             <button id=\"history-forward\" type=\"button\" title=\"Go forward\">Forward \u{2192}</button>\n\
             </div>\n\
             <ul class=\"history-list\" id=\"history-list\"></ul>\n\
             </nav>\n\
             </aside>\n\
             <div id=\"sidebar-backdrop\" hidden></div>\n\
             </main>\n",
        );

        html.push_str(&render_colophon(nodes));

        html.push_str("<script id=\"graph-data\" type=\"application/json\">");
        html.push_str(&escape_for_inline_script(&payload_json));
        html.push_str("</script>\n");

        html.push_str("<script>\n");
        html.push_str(&escape_for_inline_script(&render_graph_js(config)));
        html.push_str("\n</script>\n");

        html.push_str("</body>\n</html>\n");
        html
    }
}

/// Renders the "About this guide" colophon (kb/adrs/0004-guidance-nodes-
/// colophon.org) — a static, always-visible list of every `is_guidance`
/// node's title, each a button GRAPH_JS wires to `selectNode` (the SAME
/// click-to-open path chord nodes and in-body links already use, so
/// language toggling, the ADR-0003 translation-fallback notice, mermaid,
/// etc. all just work when a reader opens one). Returns an empty string —
/// no `<footer>` at all — when there are no guidance nodes, which is the
/// common case; most exports don't carry any.
fn render_colophon(nodes: &[GraphExportNode]) -> String {
    let guidance: Vec<&GraphExportNode> = nodes.iter().filter(|n| n.is_guidance).collect();
    if guidance.is_empty() {
        return String::new();
    }
    let mut html = String::new();
    html.push_str("<footer id=\"colophon\" class=\"colophon\">\n<h2>About this guide</h2>\n");
    html.push_str(
        "<p class=\"colophon-intro\">Standards this guide was written against — this tool \
         displays these notes but does not check the guide's content against them:</p>\n",
    );
    html.push_str("<ul class=\"colophon-list\">\n");
    for n in &guidance {
        html.push_str("<li><button type=\"button\" class=\"colophon-link\" data-node-id=\"");
        html.push_str(&html_escape(&n.id));
        html.push_str("\" data-title-en=\"");
        html.push_str(&html_escape(&n.title_en));
        html.push_str("\" data-title-es=\"");
        html.push_str(&html_escape(&n.title_es));
        html.push_str("\">");
        html.push_str(&html_escape(&n.title_en));
        html.push_str("</button></li>\n");
    }
    html.push_str("</ul>\n</footer>\n");
    html
}

/// Guard against a JSON string (or, defensively, the static JS constant)
/// containing a literal `</script`/`</style` sequence that would
/// prematurely close the surrounding `<script>` tag and let subsequent
/// markup escape into the page as raw HTML/JS. Escaping every `</`
/// occurrence to `<\/` is valid inside both a JSON string literal
/// (backslash-solidus is a legal JSON escape, decodes back to `/`) and a
/// `<script>` element's text content (browsers don't interpret `<\/` as a
/// tag close) — so this is safe to apply unconditionally, not just when a
/// dangerous substring is detected. One real trap this found: it is
/// applied to the whole GRAPH_JS constant too, not just user-content
/// JSON, so a hand-written regex literal in GRAPH_JS must never place its
/// OWN closing `/` immediately after a `<` (e.g. `/</g`) — that specific
/// `</` gets escaped exactly like any other, but for a regex literal that
/// strips its closing delimiter and corrupts the whole script. Write
/// `/[<]/` (a character class) instead wherever a bare `<` needs
/// matching.
fn escape_for_inline_script(s: &str) -> String {
    s.replace("</", "<\\/")
}

fn render_css_variables(dark: &GruvboxPalette, light: &GruvboxPalette) -> String {
    let mut css = String::new();
    css.push_str(":root {\n");
    push_palette_vars(&mut css, dark);
    css.push_str("}\n");
    css.push_str("@media (prefers-color-scheme: light) {\n:root {\n");
    push_palette_vars(&mut css, light);
    css.push_str("}\n}\n");
    css.push_str(":root[data-theme=\"dark\"] {\n");
    push_palette_vars(&mut css, dark);
    css.push_str("}\n:root[data-theme=\"light\"] {\n");
    push_palette_vars(&mut css, light);
    css.push_str("}\n");
    css
}

fn push_palette_vars(css: &mut String, p: &GruvboxPalette) {
    let vars: [(&str, &str); 24] = [
        ("--bg0", p.bg0),
        ("--bg1", p.bg1),
        ("--bg2", p.bg2),
        ("--bg3", p.bg3),
        ("--fg0", p.fg0),
        ("--fg1", p.fg1),
        ("--fg2", p.fg2),
        ("--fg3", p.fg3),
        ("--fg4", p.fg4),
        ("--gray", p.gray),
        ("--node-index", p.node_index),
        ("--node-command", p.node_command),
        ("--node-concept", p.node_concept),
        ("--node-key", p.node_key),
        ("--node-note", p.node_note),
        ("--node-project", p.node_project),
        ("--node-category", p.node_category),
        ("--node-lesson", p.node_lesson),
        ("--node-tutorial", p.node_tutorial),
        ("--node-meta", p.node_meta),
        ("--node-block", p.node_block),
        ("--node-scheme_api", p.node_scheme_api),
        ("--node-task", p.node_task),
        ("--node-view", p.node_view),
    ];
    for (name, value) in vars {
        css.push_str(name);
        css.push_str(": ");
        css.push_str(value);
        css.push_str(";\n");
    }
    css.push_str("--node-anchor: ");
    css.push_str(p.node_selected);
    css.push_str(";\n--node-hover: ");
    css.push_str(p.node_hover);
    css.push_str(";\n--edge: ");
    css.push_str(p.edge);
    css.push_str(";\n--edge-boundary: ");
    css.push_str(p.edge_boundary);
    css.push_str(";\n--accent: ");
    css.push_str(p.accent);
    css.push_str(";\n--link: ");
    css.push_str(p.link);
    css.push_str(";\n");
}

const STATIC_CSS: &str = r#"
* { box-sizing: border-box; }
html, body { margin: 0; padding: 0; height: 100%; }
body {
  background: var(--bg0);
  color: var(--fg1);
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
  display: flex;
  flex-direction: column;
  transition: background-color 200ms ease, color 200ms ease;
}
#page-header {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 0.5rem 0.75rem;
  padding: 0.75rem 1.25rem;
  background: var(--bg1);
  border-bottom: 1px solid var(--bg3);
  transition: background-color 200ms ease, border-color 200ms ease;
}
#page-title { margin: 0; font-size: 1.25rem; }
/* Pushes the Home/Prev/Next/Theme/Lang group to the row's far end,
   matching #page-header's prior space-between look, while the new search
   + tag-filter groups sit naturally between the title and it (dataviz
   skill: "filters sit in a single left-aligned row above the content they
   scope" -- here that's this same header row, not a second one, per this
   feature's own scoping decision). flex-wrap above lets the whole row
   break onto a second line on a narrow viewport instead of overflowing. */
.controls { margin-left: auto; display: flex; flex-wrap: wrap; gap: 0.5rem; }
/* #tag-picker-toggle lives in .tag-filter-group, NOT .controls (it sits
   between the search box and .controls in the header, see the markup
   above) -- a real reported bug: `.controls button` alone left it as a
   completely unstyled native <button>, an OS-default gray box clashing
   with every themed control around it, in both light and dark mode.
   Shares this exact rule rather than duplicating it, so the two families
   of header buttons can never visually drift apart again. */
.controls button, #tag-picker-toggle {
  background: var(--bg2);
  color: var(--fg1);
  border: 1px solid var(--bg3);
  border-radius: 4px;
  padding: 0.4rem 0.8rem;
  cursor: pointer;
  font-size: 0.9rem;
  /* >=24px hit target on every control, not just graph nodes. */
  min-height: 24px;
  transition: background-color 180ms ease, color 180ms ease, transform 180ms ease;
}
.controls button:hover, #tag-picker-toggle:hover { background: var(--bg3); }
.controls button:disabled {
  opacity: 0.4;
  cursor: default;
  background: var(--bg2);
}
.controls button#home-button { background: var(--accent); color: var(--bg0); border-color: var(--accent); }
.controls button#home-button:hover { transform: translateY(-1px); }

/* --- Header search: fuzzy jump-to-node, a dropdown of results below the
   input (same floating-panel treatment as .popover -- border + shadow, no
   pointer-events:none here since these rows ARE the interactive target). */
.search-group { position: relative; }
#node-search {
  background: var(--bg2);
  color: var(--fg1);
  border: 1px solid var(--bg3);
  border-radius: 4px;
  padding: 0.4rem 0.6rem;
  font-size: 0.9rem;
  min-height: 24px;
  width: 14rem;
  max-width: 40vw;
}
#node-search:focus { outline: 2px solid var(--accent); outline-offset: 1px; }
.search-results {
  position: absolute;
  top: calc(100% + 0.25rem);
  left: 0;
  min-width: 100%;
  max-width: 24rem;
  background: var(--bg1);
  border: 1px solid var(--bg3);
  border-radius: 6px;
  padding: 0.25rem;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
  z-index: 20;
}
.search-results button {
  display: block;
  width: 100%;
  text-align: left;
  background: none;
  border: none;
  color: var(--fg1);
  padding: 0.35rem 0.5rem;
  border-radius: 4px;
  cursor: pointer;
  font-size: 0.85rem;
  min-height: 24px;
}
.search-results button:hover, .search-results button.active { background: var(--bg2); }

/* --- Header tag filter: a picker dropdown (all tags, toggleable) plus
   removable chips for whichever are active. Filtering itself is applied to
   the chord ring (see .node.filtered-out / .edge.filtered-out below), not
   to any list here -- the graph IS the filtered view. */
/* [hidden] overrides here are load-bearing, not decoration: both rules
   below set `display` explicitly (flex, for their internal layout), which
   has the SAME CSS specificity as the browser's built-in
   `[hidden] { display: none }` UA-stylesheet rule -- author CSS appearing
   later in the cascade wins a specificity tie, so without this override
   these elements stayed visibly `display: flex` regardless of the
   `hidden` attribute/property GRAPH_JS sets on them. Confirmed as a real
   bug by the Layer 2 suite: an always-flex #tag-picker sat on top of
   (and ate clicks intended for) #next-button/#prev-button underneath it
   in the header. */
.tag-filter-group[hidden], .tag-picker[hidden] { display: none; }
.tag-filter-group { position: relative; display: flex; align-items: center; gap: 0.4rem; }
.tag-picker {
  position: absolute;
  top: calc(100% + 0.25rem);
  left: 0;
  min-width: 12rem;
  max-width: 20rem;
  background: var(--bg1);
  border: 1px solid var(--bg3);
  border-radius: 6px;
  padding: 0.4rem;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
  z-index: 20;
  display: flex;
  flex-wrap: wrap;
  gap: 0.3rem;
}
.tag-picker button, .active-tag-chips button {
  background: var(--bg2);
  color: var(--fg1);
  border: 1px solid var(--bg3);
  border-radius: 999px;
  padding: 0.2rem 0.6rem;
  cursor: pointer;
  font-size: 0.8rem;
  min-height: 24px;
}
.tag-picker button:hover { background: var(--bg3); }
.tag-picker button.active { background: var(--accent); color: var(--bg0); border-color: var(--accent); }
.active-tag-chips { display: flex; flex-wrap: wrap; gap: 0.3rem; }
.active-tag-chips button { color: var(--link); }
.active-tag-chips button:hover { background: var(--bg3); }
/* The node's rendered content is the primary reading surface -- #main-content
   is the dominant flex child; the chord diagram + outline are demoted to a
   narrow, fixed-width #sidebar (not a flex ratio -- a compact nav widget
   doesn't benefit from stretching wider on a wide viewport), stacked chord-
   above-outline rather than side by side. See the html_graph module doc
   comment for why this replaced an earlier layout where the graph took ~2/3
   of the page width and the outline was easy to lose below a long body. */
#app-main {
  flex: 1;
  display: flex;
  min-height: 0;
}
#main-content {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  padding: 1.5rem 2rem;
}
#sidebar {
  flex: 0 0 300px;
  display: flex;
  flex-direction: column;
  min-width: 0;
  /* min-height: 0 overrides the flex default (min-height: auto), which
     otherwise lets #sidebar's children push it taller than the cross-axis
     height align-items: stretch actually gives it -- confirmed necessary
     by the Layer 2 suite: without this, #graph-pane's own content-driven
     height (itself content-forced past its nominal 280px, a pre-existing
     characteristic this fix works around rather than chasing further) plus
     #history-panel's real min-height could push #sidebar's rendered
     content past #app-main's own bottom edge, with nothing to contain it
     -- it simply overflowed underneath (and unclickable behind) the
     #colophon footer that follows in paint order. overflow-y: auto makes
     #sidebar itself the containing scrollport of last resort if its
     children's combined minimums ever do exceed the space available,
     rather than leaving that spillover uncontained. */
  min-height: 0;
  overflow-y: auto;
  border-left: 1px solid var(--bg3);
}
/* #sidebar-toggle (in #page-header/.controls) drives BOTH the desktop
   collapse and the mobile drawer through one shared `data-sidebar`
   attribute on <html> (never a class -- "toggled" would mean opposite
   things per breakpoint: hide on desktop, show on mobile). Each breakpoint
   below only overrides its own non-default value, so a first-time visitor
   with nothing in localStorage renders correctly with zero JS. */
@media (max-width: 767px) {
  #sidebar { display: none; }
  html[data-sidebar="open"] #sidebar {
    /* flex/flex-direction/overflow-y/border-left above still apply --
       this only lifts it out of flow into a right-edge drawer. A FULL-
       viewport (inset:0) sidebar was tried first and rejected: it would
       sit at a higher z-index than #sidebar-backdrop and entirely cover
       it, leaving no exposed area for "tap outside to close" to ever
       actually hit -- min(85vw, 320px) always leaves real backdrop
       showing, even on the narrowest phone widths.

       top: var(--header-h) (JS-synced to #page-header's real rendered
       height, incl. its own flex-wrap onto a second row on narrow
       viewports -- see the ResizeObserver in GRAPH_JS) was also chosen
       after a real, caught bug: `top: 0` + z-index-boosting #page-header
       above the drawer to keep it clickable instead ALSO put the
       header's own hit-test region on top of the drawer's top strip,
       silently swallowing clicks on #graph-fullscreen-toggle (it lives
       near #graph-pane's own top edge). Starting the drawer below the
       header sidesteps the stacking conflict entirely -- no z-index
       tug-of-war needed between the header and the drawer at all. */
    display: flex;
    position: fixed;
    top: var(--header-h, 3.5rem);
    right: 0;
    bottom: 0;
    width: min(85vw, 320px);
    z-index: 900;
    background: var(--bg0);
    animation: sidebar-drawer-in 220ms ease;
  }
  #sidebar-backdrop { display: none; }
  html[data-sidebar="open"] #sidebar-backdrop {
    display: block;
    position: fixed;
    top: var(--header-h, 3.5rem);
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 899;
    background: rgba(0, 0, 0, 0.5);
  }
  /* Exit animation only: JS keeps data-sidebar="open" (so the fixed
     positioning above still applies) while this plays, then flips it to
     "closed" on animationend -- same two-phase approach as #graph-pane's
     fullscreen-anim-out below. Scoped to this breakpoint only: desktop's
     collapse (@media min-width:768px below) has no animation, it's an
     instant display:none. Placed after the "open" rule above so equal-
     specificity source order lets this win while data-sidebar is still
     "open". */
  html[data-sidebar-anim="out"] #sidebar { animation: sidebar-drawer-out 200ms ease; }
}
@media (min-width: 768px) {
  html[data-sidebar="closed"] #sidebar { display: none; }
}
@keyframes sidebar-drawer-in {
  from { transform: translateX(100%); }
  to { transform: translateX(0); }
}
@keyframes sidebar-drawer-out {
  from { transform: translateX(0); }
  to { transform: translateX(100%); }
}
#graph-pane {
  flex: 0 0 280px;
  display: flex;
  flex-direction: column;
  position: relative;
  min-width: 0;
  /* A hairline divider (one shade off the page surface, same treatment as
     a chart's own recessive gridlines) separates the chord widget from the
     outline below it -- no color inversion, no card border-radius/shadow.
     An earlier version rendered this panel in the *opposite* gruvbox mode
     with a rounded, drop-shadowed "card" look, on the theory that visual
     contrast would read as a deliberate focal point; in practice it read
     as a boxed-in prototype widget instead of an integrated part of the
     page. The widget now shares the sidebar's own surface color -- the
     dataviz anti-pattern list's guidance against "thick blocks, heavy
     chrome, no breathing room" applies here as much as it would to any
     other chart container. */
  border-bottom: 1px solid var(--bg3);
  background: var(--bg0);
  color: var(--fg1);
  transition: background-color 200ms ease, color 200ms ease;
}
#graph-svg { width: 100%; flex: 1; min-height: 0; display: block; }
#graph-fullscreen-toggle {
  position: absolute;
  top: 0.4rem;
  right: 0.4rem;
  z-index: 2;
  width: 28px;
  height: 28px;
  min-height: 24px;
  padding: 0;
  border: 1px solid var(--bg3);
  border-radius: 4px;
  background: var(--bg1);
  color: var(--fg2);
  font-size: 0.95rem;
  line-height: 1;
  cursor: pointer;
  transition: background-color 200ms ease, color 200ms ease;
}
#graph-fullscreen-toggle:hover { background: var(--bg2); color: var(--fg1); }
/* See the fullscreen JS block (GRAPH_JS) for why this is a `position:
   fixed` overlay rather than the native Fullscreen API, and why enter/
   exit are @keyframes animations rather than transitions. `flex: 1 1
   auto` lets the pane actually fill the fixed box (its normal `flex: 0 0
   280px` is a SIDEBAR-width constraint that would otherwise cap it). */
#graph-pane.fullscreen {
  position: fixed;
  inset: 0;
  z-index: 1000;
  flex: 1 1 auto;
  border-bottom: none;
}
@keyframes graph-fullscreen-in {
  from { opacity: 0; transform: scale(0.96); }
  to { opacity: 1; transform: scale(1); }
}
@keyframes graph-fullscreen-out {
  from { opacity: 1; transform: scale(1); }
  to { opacity: 0; transform: scale(0.96); }
}
#graph-pane.fullscreen-anim-in { animation: graph-fullscreen-in 220ms ease; }
#graph-pane.fullscreen-anim-out { animation: graph-fullscreen-out 200ms ease; }
/* Shows the hovered node's title, falling back to the selected node's
   title when nothing is hovered (see updateCaption() in GRAPH_JS) -- a
   real, legible body-text size instead of the cramped 11px in-SVG label
   this replaced. Sits at the TOP of the graph viewport, not below the
   diagram: the hover popover is cursor-positioned and can land near the
   bottom of the ring, which would cover a caption placed underneath it --
   putting the caption above the ring keeps it clear of anywhere the
   popover can actually appear. min-height holds its layout slot even
   when briefly empty (before the very first hover/selection) so the ring
   below it doesn't shift. text-overflow handles a long title without
   wrapping to a second line and shrinking the ring to make room. */
#graph-caption {
  flex: 0 0 auto;
  min-height: 1.4em;
  padding: 0.4rem 0.75rem;
  text-align: center;
  font-size: 0.85rem;
  color: var(--fg1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
#main-content .hint { color: var(--fg3); font-style: italic; }
#main-content h2 { margin-top: 0; }
/* 70ch is the standard reading-measure heuristic (60-80 chars/line); only
   engages once #main-content's content box exceeds it, so mobile/narrow
   renders are unaffected without a media query. Capped here (the actual
   note-text box) rather than on #main-content itself, which stays the
   untouched flex/scroll container (overflow-y: auto, and the JS scrollTop
   reset at navigation time both key off #main-content unchanged). */
#detail-panel-content {
  transition: opacity 180ms ease;
  opacity: 1;
  max-width: 70ch;
  margin-left: auto;
  margin-right: auto;
}
#detail-panel-content.fading { opacity: 0; }
#main-content .kind-badge {
  display: inline-block;
  font-size: 0.75rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--bg0);
  background: var(--fg3);
  border-radius: 3px;
  padding: 0.1rem 0.4rem;
  margin-bottom: 0.5rem;
}
/* "Part ::" breadcrumb -- a small, muted structural label ("where am I in
   the guide"), not a call to action and not a link (see renderDetail's own
   comment on why it's plain text). Deliberately quieter than kind-badge
   (no background/box), sitting just above the title it's providing
   context for. */
#main-content .node-part-breadcrumb {
  font-size: 0.75rem;
  color: var(--fg3);
  text-transform: uppercase;
  letter-spacing: 0.03em;
  margin-bottom: 0.2rem;
}
#main-content .anchor-note {
  background: var(--bg1);
  border-left: 3px solid var(--node-anchor);
  padding: 0.4rem 0.6rem;
  margin-bottom: 0.75rem;
  font-size: 0.85rem;
}
/* kb/adrs/0003: deliberately quieter than .anchor-note (muted fg3 text,
   thinner accent-colored border, italic) -- this is informational, not a
   warning, and should read as "here's a heads-up," not compete for
   attention with the anchor-note's own visual weight. */
#main-content .translation-fallback-note {
  background: var(--bg1);
  border-left: 2px solid var(--link);
  padding: 0.35rem 0.6rem;
  margin-bottom: 0.75rem;
  font-size: 0.8rem;
  font-style: italic;
  color: var(--fg3);
}
/* kb/adrs/0004: a footer, not a sidebar/#main-content section -- always
   full page width, visually separate from both the curated topic content
   and the graph/outline nav chrome, so a reader can tell at a glance that
   these links are meta-content about the guide, not part of its subject
   matter (the ADR's own wording). Deliberately muted (fg3 text, thin
   top border only, no card/shadow) -- a colophon reads as a reference
   footnote, not a call to action. */
#colophon {
  flex: 0 0 auto;
  padding: 1rem 1.5rem;
  border-top: 1px solid var(--bg3);
  background: var(--bg1);
  color: var(--fg3);
  font-size: 0.85rem;
  transition: background-color 200ms ease, border-color 200ms ease, color 200ms ease;
}
#colophon h2 { margin: 0 0 0.25rem; font-size: 0.95rem; color: var(--fg2); }
#colophon .colophon-intro { margin: 0 0 0.5rem; font-style: italic; }
#colophon .colophon-list { list-style: none; padding: 0; margin: 0; display: flex; flex-wrap: wrap; gap: 0.5rem; }
#colophon .colophon-link {
  background: none;
  border: 1px solid var(--bg3);
  color: var(--link);
  border-radius: 4px;
  padding: 0.3rem 0.6rem;
  cursor: pointer;
  font-size: 0.85rem;
  min-height: 24px;
}
#colophon .colophon-link:hover { background: var(--bg2); }
#main-content .guidance-note {
  background: var(--bg1);
  border-left: 3px solid var(--link);
  padding: 0.4rem 0.6rem;
  margin-bottom: 0.75rem;
  font-size: 0.85rem;
  font-style: italic;
}
#main-content .link-list { list-style: none; padding: 0; margin: 0.5rem 0; }
#main-content .link-list li { margin-bottom: 0.3rem; }
#main-content .link-jump {
  background: none;
  border: 1px solid var(--bg3);
  color: var(--fg1);
  border-radius: 4px;
  padding: 0.25rem 0.5rem;
  cursor: pointer;
  text-align: left;
  width: 100%;
}
#main-content .link-jump:hover { background: var(--bg1); }
#main-content .external-link {
  color: var(--fg4);
  font-size: 0.85rem;
}
#main-content pre { background: var(--bg1); padding: 0.75rem; border-radius: 4px; overflow-x: auto; }
/* #+begin_example blocks (command transcripts/sample output) get a
   left-border treatment distinct from source-code <pre> blocks, matching
   how blockquote already reads as "quoted material" rather than "code" --
   .example is deliberately NOT a src-block language, so it never gets a
   language-* class or syntax highlighting. */
#main-content pre.example { border-left: 3px solid var(--fg4); }
#main-content pre.results { border-left: 3px solid var(--link); }
#main-content li input[type="checkbox"] { margin-right: 0.4em; vertical-align: middle; }
/* Real "- term :: definition" description lists (html.rs's render_list)
   -- a <dl>/<dt>/<dd> structure instead of a flat <li> with the literal
   " :: " still visible, and real nested <ul>/<ol>/<dl> for any list
   item's children (an indented sub-list previously flattened into
   siblings of its own parent, discarding the nesting entirely -- see
   parse_list_items's own doc comment in crates/export/src/lib.rs). */
#main-content dl { margin: 1rem 0; }
#main-content dt { font-weight: bold; color: var(--fg1); }
#main-content dd { margin: 0 0 0.75rem 1.5rem; }
#main-content ul ul, #main-content ol ol, #main-content ul ol, #main-content ol ul,
#main-content dl dl, #main-content dl ul, #main-content dl ol { margin-top: 0.3rem; }
/* Syntax-highlighting token classes -- applied client-side by
   `highlightCodeBlocks()` (GRAPH_JS) over each src/example block's own
   text after it lands in the DOM. Deliberately reuses only the theme
   colors already validated for both light/dark surfaces (--accent,
   --link, --fg2, --fg4) rather than introducing new unvalidated hues, so
   this needs no separate colorblind/contrast pass. */
#main-content .tok-kw { color: var(--accent); font-weight: 600; }
#main-content .tok-str { color: var(--link); }
#main-content .tok-com { color: var(--fg4); font-style: italic; }
#main-content .tok-num { color: var(--fg2); }
#main-content .tok-interp { color: var(--accent); }
#main-content .tok-prompt { color: var(--fg4); font-weight: 700; }
/* Inline code (=x=/~x~ spans) previously had only a monospace font, with
   nothing to set it apart from surrounding prose -- a background pill
   (matching how most rendered-markdown/org viewers treat inline code)
   fixes that. Block code (inside <pre>) already has its own background
   from the rule above, so the second rule below cancels the pill there to
   avoid a visibly doubled/nested box. */
#main-content code {
  font-family: "JetBrains Mono", "Fira Code", monospace;
  background: var(--bg2);
  padding: 0.1rem 0.35rem;
  border-radius: 3px;
  font-size: 0.9em;
}
#main-content pre code {
  background: none;
  padding: 0;
  border-radius: 0;
  font-size: 1em;
}
#main-content blockquote { border-left: 3px solid var(--bg3); margin-left: 0; padding-left: 0.75rem; color: var(--fg3); }
/* Prose links (in-body `<a>` from the org-link converter) previously had no
   rule at all and fell through to the browser's default blue/purple, which
   clashes with the theme. `--link` is deliberately a different hue from
   `--accent` (orange already means "current node/edge" elsewhere on the
   page) -- see `GruvboxPalette::link`'s doc comment for the validated
   contrast numbers. Underline at reduced opacity rather than the browser
   default full-strength double rule, so it reads as a link without
   competing with the accent-orange selected state used elsewhere. */
#main-content a,
.popover a {
  color: var(--link);
  text-decoration: underline;
  text-decoration-color: color-mix(in srgb, var(--link) 50%, transparent);
}
#main-content a:hover,
.popover a:hover {
  text-decoration-color: var(--link);
}
#main-content a:visited,
.popover a:visited {
  color: var(--link);
  opacity: 0.85;
}
.mermaid-diagram { margin: 0.75rem 0; }
.mermaid-diagram svg { max-width: 100%; height: auto; }
.mermaid-fallback-warning { color: var(--node-command); font-size: 0.85rem; }

.popover {
  position: fixed;
  max-width: 320px;
  background: var(--bg1);
  border: 1px solid var(--bg3);
  border-radius: 6px;
  padding: 0.5rem 0.75rem;
  font-size: 0.85rem;
  pointer-events: none;
  z-index: 10;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
}
.popover .popover-title { font-weight: bold; margin-bottom: 0.25rem; }
.popover .popover-body { color: var(--fg3); }

/* --- Chord nav widget: a small, muted-by-default view where exactly ONE
   accent color (--accent, validated for simultaneous-discrimination
   contrast — see GruvboxPalette::accent's doc comment) marks the current
   node + its incident edges. Per-kind hues (still used for the kind badge
   text in the detail panel) are deliberately NOT used for fill here — 14
   simultaneously-visible hues failed a real categorical-palette check,
   one accent against a muted field doesn't need to pass that check at
   all. `transform`/`filter`/`fill`/`stroke` all transition over
   150-250ms so navigation and hover read as motion, not a snap. */
/* Each node is an annular-sector <path> (a wedge of the ring), not a
   <circle> — kb/adrs/00XX (arc-slice redesign). Growth (hover/neighbor)
   is NOT done via CSS `transform: scale(...)`: an earlier version tried
   that (transform-origin pinned to the ring's own center via an SVG
   presentation attribute + `transform-box: view-box`) and it was
   confirmed broken empirically across BOTH engines this session's own
   Layer 2 harness targets — Firefox ignored `transform-box: view-box`
   for `getBoundingClientRect()` purposes entirely (zero measurable
   growth), and Chromium applied the transform but skewed the wedge's
   midpoint sideways rather than growing it radially outward, because a
   wedge's own local bounding box (still involved in some engines'
   resolution of transform-origin regardless of transform-box) isn't
   centered the way a circle's is. Instead, growth is real GEOMETRY: JS
   recomputes and re-sets the path's own `d` attribute on hover/neighbor/
   selection changes (see `refreshWedgeGrowth` in GRAPH_JS), and `d` is
   itself a transitionable CSS property in both target engines — `M`/`A`/
   `L`/`A`/`Z` structure never changes between the idle and grown shapes,
   only the outer-arc radius, which is what makes the transition interpolate
   smoothly instead of snapping. */
.node path {
  /* The theme's own primary foreground/text color (the same tone
     #main-content's body copy uses), not a separate muted gray -- a
     deliberate switch away from an earlier flat --fg4 fill the user
     called out as looking flat/uninteresting. Fully opaque (an earlier
     degree-driven fill-opacity ramp was tried and explicitly reverted per
     user request -- "reset...to just be opaque"). No stroke at all (user
     request): wedge-to-wedge separation comes from the rounded corners
     alone (GRAPH_JS arcPath's cornerRadius, a "petal" look, also a user
     request), not a drawn border or an angular gap -- state changes
     (below) are background-color changes, never border changes. */
  fill: var(--fg1);
  /* Kept at its own fixed 130ms, deliberately OUTSIDE the shared
     ui_transition_ms bucket (not a "200ms" literal) -- `d`-attribute path
     interpolation is real CPU work every frame, not a GPU-composited
     transform, so it reads as noticeably more sluggish than the rest of
     the page's 200ms UI chrome transitions at the same nominal duration;
     user-reported as "much slower" feeling and confirmed too slow at
     200ms, tightened here rather than folded into the shared knob so
     ui_transition_ms tuning elsewhere never re-introduces the sluggishness. */
  transition: d 130ms ease, filter 130ms ease, fill 130ms ease;
}
/* Node titles previously rendered as in-SVG <text> next to each circle --
   at an 11px font in a ~280px-wide widget, with only the anchor/selected/
   hovered node's label ever showing (see the removed comment this rule
   used to carry, on labeling density). Two real problems with that,
   reported directly: the text was too small to comfortably read, and
   reserving in-SVG room for it (even after fixing the reservation to be
   symmetric) ate a large fraction of the widget's small on-screen size as
   dead padding. Node titles now show in #graph-caption below the diagram
   instead, at a real body-text size -- see the caption rule and
   updateCaption() in GRAPH_JS. */
.node { cursor: pointer; }
/* Hover LIFTS (grows outward + drop-shadow), it does not recolor —
   recoloring is reserved entirely for `.selected` (the current node).
   This also means hover and selected no longer compete for the same
   visual channel, so both can be simultaneously true and simultaneously
   visible (a grown, accent-colored current node under the cursor) with
   no priority rule needed between them for color -- only `.selected`
   ever changes fill. The actual growth itself is applied via `d`
   (refreshWedgeGrowth, GRAPH_JS), not here -- this rule only owns the
   drop-shadow, a pure filter effect with no geometry dependency. */
.node.hovered path {
  filter: drop-shadow(0 2px 6px rgba(0, 0, 0, 0.45));
}
.node.selected path {
  fill: var(--accent);
}
.node.selected text { fill: var(--fg1); font-weight: bold; }
/* Directly-linked neighbor nodes of the current selection: a real,
   standing highlight for as long as the selection lasts (not transient
   like .hovered) -- a background-color change (user request: this used
   to be a colored stroke/border, which read as visual clutter rather
   than a clear signal once most of the ring shares the same idle fill).
   `--link` distinct from `--accent` (.selected's own color) so "this is
   the current node" and "this is connected to it" never look identical.
   Growth (via `d`, refreshWedgeGrowth) actually grows the real
   hit-tested area, not just the visual, the same mechanism .hovered
   already relies on -- confirmed a real reported gap: the OTHER endpoint
   of an already-highlighted incident edge looked identical to every
   unrelated node otherwise. Yields to .hovered's own larger growth
   factor when both are true at once (see refreshWedgeGrowth's
   precedence), so hover stays the most immediate feedback state. */
.node.neighbor path { fill: var(--link); }
/* Visited-node marker: an inner ~2/5-thickness band of the wedge itself
   (GRAPH_JS draw loop -- same angular span as the outer wedge, sharing
   its arcPath geometry helper, with a corner radius scaled down to the
   band's own thinner thickness), shown via opacity alone -- deliberately
   NOT a fill/stroke/geometry change on the OUTER wedge (`.node path`
   directly), so it never competes with hover/neighbor/selected, which
   already own those channels there. The current node never gets
   `.visited` at all (GRAPH_JS's classList.toggle, `tn.id !== id`) --
   selected styling already conveys "you are here" on its own, so no CSS
   override is needed here.

   --bg2 (not a darker foreground shade) after actually computing WCAG
   contrast for every --fg/--bg step against the wedge's own default
   --fg1 fill in both themes: the darker-foreground family fails the
   1.4.11 non-text 3:1 floor outright (best of that family, --fg4, is
   only 2.03:1 dark / 2.38:1 light) -- foreground steps are ALL close
   neighbors of --fg1 by construction (muted variants of the same ink),
   so none of them separate from it enough to read as a real different
   color, only a slightly duller one. --bg2 clears 3:1 comfortably in
   both themes (6.43:1 dark `#504945` on `#ebdbb2`, 6.76:1 light
   `#d5c4a1` on `#3c3836`) without going as stark as --bg1's
   near-page-background 8.45:1 (which would read as a literal cutout to
   the page behind rather than a deliberate inset color). Also the best
   all-around candidate against the ONE other fill a visited node can
   carry (`.neighbor`'s --link): 3.28:1 dark (clears 3:1), 2.47:1 light
   (short of 3:1, but the closest of any candidate tested, and a
   neighbor+visited node already carries a second signal via --link
   itself, so this compound case isn't the marker's only channel).

   Both rules below are qualified as `.node path.visited-inner-arc`,
   NOT the bare `.visited-inner-arc` class -- a real bug caught before
   shipping: `.node path` (specificity 0,1,1) and `.node.selected path` /
   `.node.neighbor path` (0,2,1) are ALL more specific than a bare single-
   class selector (0,1,0), so the generic wedge rules would silently win
   the cascade and either flatten the `transition` property (dropping the
   `opacity` transition `.node path`'s own d/filter/fill list doesn't
   include, so the marker would snap instead of fade) or, worse, override
   `fill` back to --fg1/--accent/--link on a neighbor+visited node,
   defeating the color choice above entirely. `.node path.visited-inner-
   arc` (0,2,1) and `.node.visited path.visited-inner-arc` (0,3,1) beat
   every one of those unconditionally, by specificity alone -- not by
   relying on this block's position in the source order, which a later
   edit could silently invalidate. */
.node path.visited-inner-arc { opacity: 0; transition: opacity 200ms ease; }
.node.visited path.visited-inner-arc { fill: var(--bg2); opacity: 1; }
/* Keyboard-focus ring: :focus-visible, not plain :focus -- a mouse click
   already shows selection via .selected's own fill/stroke recolor, so a
   focus ring on every click would be redundant visual noise. This only
   shows for real keyboard navigation (Tab landing on the roving tabindex
   stop, or Arrow-key movement), which is exactly when a visible focus
   indicator is actually needed. */
.node:focus-visible path { outline: 2px solid var(--link); outline-offset: 2px; }
.node:focus { outline: none; }

.edge {
  stroke: var(--gray);
  stroke-width: 1.25;
  opacity: 0.35;
  fill: none;
  transition: stroke 200ms ease, opacity 200ms ease, stroke-width 200ms ease;
}
.edge.incident {
  stroke: var(--accent);
  opacity: 0.9;
  stroke-width: 2;
  cursor: pointer;
}
/* Tag filter: dims (never removes/hides) non-matching marks, same
   transition timing as hover/select above, so toggling a filter reads as
   the same kind of motion as the rest of this widget's interactions. The
   graph itself IS the filtered view -- not a separate list. */
.node.filtered-out path { opacity: 0.15; }
.edge.filtered-out { opacity: 0.08; }

#outline-panel {
  flex: 1;
  padding: 0.5rem 1.25rem;
  min-height: 0;
  overflow-y: auto;
}
#outline-panel.collapsed .outline-list { display: none; }
/* Explicit font-weight (not the <h3> user-agent default -- don't rely on
   that holding) plus fg2 (not the more muted fg3 used for de-emphasized
   meta text elsewhere) so this reads clearly as a real section heading
   for the sidebar's contents, not another line of quiet chrome. */
#outline-panel h3 {
  margin: 0.25rem 0;
  font-size: 0.8rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--fg2);
  cursor: pointer;
  user-select: none;
}
.outline-list { list-style: none; padding: 0; margin: 0.25rem 0; }
.outline-list li { margin-bottom: 0.15rem; }
.outline-list button {
  background: none;
  border: none;
  color: var(--fg2);
  cursor: pointer;
  text-align: left;
  padding: 0.15rem 0;
  font-size: 0.85rem;
}
.outline-list button:hover { color: var(--accent); }

/* Pinned to the bottom of #sidebar (last flex child) and grows upward as
   visited entries accumulate, up to max-height -- flex-shrink: 1 and
   min-height: 0 (overriding the flex default of min-height: auto, which
   would otherwise force this panel to its content's natural minimum height
   regardless of how little room #sidebar actually has) let the WHOLE panel
   give ground when space is genuinely tight, deferring to #outline-panel
   the same way #outline-panel already yields to #graph-pane above it.
   #history-panel is itself a flex column so that giving ground doesn't mean
   the panel vanishes: the heading + Back/Forward controls below are
   flex: 0 0 auto (fixed, always shown/clickable at their natural size) and
   ONLY .history-list (flex: 1; min-height: 0; its own overflow-y: auto)
   is the part that shrinks toward zero and scrolls internally when space
   is short -- confirmed necessary by the Layer 2 suite: an earlier version
   let the whole panel (controls included) overflow past #sidebar's own
   bottom edge in a modest-height viewport, rendering underneath -- and
   therefore unclickable behind -- the #colophon footer that follows it in
   paint order; giving the whole panel flex-shrink without protecting the
   controls' own minimum size just moved the same problem from "overflows
   and gets covered" to "shrinks itself down to an unusably short sliver."
   #outline-panel (flex: 1; min-height: 0 already) keeps expanding downward
   to fill whatever's left above this panel, exactly as before.

   margin-top: auto is what actually pins this panel to #sidebar's bottom
   edge -- NOT #outline-panel's flex: 1 alone, which only pushes this panel
   down as a side effect of consuming all free space itself. renderOutline
   (GRAPH_JS) sets `outlinePanel.hidden = true` on any node with zero
   headings (a real, common case -- e.g. a body that's just a src/example
   block with no heading), which removes it from #sidebar's flex layout
   entirely: with no visible sibling left claiming a flex-grow share, this
   panel would otherwise flow immediately below #graph-pane instead of
   staying at the bottom, since flex-grow: 0 doesn't move it there on its
   own. An auto margin on a flex item absorbs whatever positive free space
   the main axis has left AFTER flex-grow/shrink resolve -- when
   #outline-panel is visible its flex: 1 already claims that free space
   (this margin then resolves to 0, so nothing changes from before), but
   when it's hidden this margin claims it instead, keeping this panel
   pinned to the bottom either way.

   min-height here is a REAL floor (not 0): confirmed necessary by the
   Layer 2 suite in Firefox specifically -- protecting the controls' own
   flex-shrink: 0 only governs space WITHIN #history-panel; with
   min-height: 0 at this outer level, #sidebar's own flex allocation still
   shrank the whole panel down to ~17px (room for nothing), and since this
   panel itself has no overflow clipping of its own (only .history-list
   does), the heading/controls/list simply rendered past the panel's own
   tiny box in normal document flow -- past #sidebar's bottom edge, and
   therefore underneath (and unclickable behind) #colophon. ~90px covers
   the heading + Back/Forward controls' real rendered height with a small
   margin; only .history-list gives up its own space below that floor. */
#history-panel {
  flex: 0 1 auto;
  margin-top: auto;
  display: flex;
  flex-direction: column;
  min-height: 90px;
  max-height: 40vh;
  padding: 0.5rem 1.25rem;
  border-top: 1px solid var(--bg3);
}
.history-controls {
  flex: 0 0 auto;
  display: flex;
  gap: 0.4rem;
  margin: 0.25rem 0 0.5rem;
}
.history-controls button {
  flex: 1;
  background: var(--bg2);
  color: var(--fg1);
  border: 1px solid var(--bg3);
  border-radius: 4px;
  padding: 0.3rem 0.4rem;
  cursor: pointer;
  font-size: 0.8rem;
  min-height: 24px;
}
.history-controls button:hover:not(:disabled) { background: var(--bg3); }
.history-controls button:disabled { opacity: 0.4; cursor: default; }
#history-panel h3 {
  flex: 0 0 auto;
  margin: 0.25rem 0;
  font-size: 0.8rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--fg2);
}
.history-list {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  list-style: none;
  padding: 0;
  margin: 0.25rem 0;
}
.history-list li { margin-bottom: 0.15rem; }
/* kb: no silent truncation -- the "N earlier" row when the depth cap has
   evicted entries, and the Back/Forward markers, are informational only
   (muted fg3), the same "heads-up, not a warning" tone as
   .translation-fallback-note. */
.history-list .history-truncated {
  color: var(--fg3);
  font-size: 0.8rem;
  font-style: italic;
  padding: 0.15rem 0;
}
.history-list button {
  background: none;
  border: 1px solid transparent;
  color: var(--link);
  cursor: pointer;
  text-align: left;
  width: 100%;
  padding: 0.2rem 0.4rem;
  border-radius: 4px;
  font-size: 0.85rem;
  min-height: 24px;
}
.history-list button:hover { background: var(--bg2); }
/* The current node: not a button (already on screen, nothing to click to
   get here), accent-bordered like .anchor-note's existing treatment. */
.history-list .history-current {
  border-left: 3px solid var(--node-anchor);
  padding: 0.2rem 0.4rem;
  font-size: 0.85rem;
  color: var(--fg1);
  font-weight: 600;
}
.history-list .history-marker {
  color: var(--fg3);
  font-size: 0.75rem;
  font-style: italic;
  margin-left: 0.4rem;
}
"#;

/// Vanilla JS graph interaction layer — no bundler, no external CDN, no
/// npm dependency shipped in the output. 100% static (every dynamic value
/// comes from the embedded `#graph-data` JSON payload read at runtime), so
/// this constant needs no `format!`/placeholder interpolation and can't
/// suffer brace-escaping bugs.
const GRAPH_JS: &str = r#"
(function () {
  "use strict";
  var data = JSON.parse(document.getElementById("graph-data").textContent);
  var nodes = data.nodes;
  var edges = data.edges;
  var anchorId = data.anchorId;
  var hasTranslations = data.hasTranslations;

  var nodesById = {};
  nodes.forEach(function (n, i) { n._idx = i; nodesById[n.id] = n; });
  // kb/adrs/0004: guidance/colophon nodes are real entries in `nodes` (so
  // nodesById/selectNode/renderDetail all resolve them like any other node
  // when opened from the colophon), but never part of the interactive
  // chord graph or the reading-order walk -- topicNodes is what those two
  // things iterate over instead of `nodes` directly.
  var topicNodes = nodes.filter(function (n) { return !n.is_guidance; });

  var svg = document.getElementById("graph-svg");
  var popover = document.getElementById("popover");
  var graphCaption = document.getElementById("graph-caption");
  var mainContent = document.getElementById("main-content");
  var detailContent = document.getElementById("detail-panel-content");
  var outlinePanel = document.getElementById("outline-panel");
  var outlineList = document.getElementById("outline-list");
  var outlineToggle = document.getElementById("outline-toggle");
  var historyList = document.getElementById("history-list");
  var historyBackBtn = document.getElementById("history-back");
  var historyForwardBtn = document.getElementById("history-forward");
  var langToggle = document.getElementById("lang-toggle");
  var nextBtn = document.getElementById("next-button");
  var prevBtn = document.getElementById("prev-button");
  var homeBtn = document.getElementById("home-button");
  var themeToggle = document.getElementById("theme-toggle");
  var nodeSearch = document.getElementById("node-search");
  var searchResults = document.getElementById("search-results");
  var tagPickerToggle = document.getElementById("tag-picker-toggle");
  var tagPicker = document.getElementById("tag-picker");
  var activeTagChips = document.getElementById("active-tag-chips");
  var tagFilterGroup = document.querySelector(".tag-filter-group");
  var graphPane = document.getElementById("graph-pane");
  var fullscreenToggle = document.getElementById("graph-fullscreen-toggle");
  var sidebarEl = document.getElementById("sidebar");
  var sidebarToggle = document.getElementById("sidebar-toggle");
  var sidebarBackdrop = document.getElementById("sidebar-backdrop");
  var pageHeader = document.getElementById("page-header");

  // Keeps the mobile drawer/backdrop (STATIC_CSS: top: var(--header-h))
  // starting below the REAL header, whatever height it's actually
  // rendering at (its own flex-wrap can push it to a second row on a
  // narrow viewport) -- a real bug this fixes: a fixed guess or a
  // z-index-boosted header sitting ABOVE the drawer both put the
  // header's own hit-test region on top of the drawer's top strip,
  // silently swallowing clicks on controls near the sidebar's own top
  // edge (#graph-fullscreen-toggle). Re-synced on every header resize,
  // not just once on load.
  function syncHeaderHeightVar() {
    document.documentElement.style.setProperty("--header-h", pageHeader.getBoundingClientRect().height + "px");
  }
  syncHeaderHeightVar();
  if (window.ResizeObserver) {
    new ResizeObserver(syncHeaderHeightVar).observe(pageHeader);
  } else {
    window.addEventListener("resize", syncHeaderHeightVar);
  }

  var currentLang = "en";
  var selectedId = null;

  // --- Visited-node history panel state -- a shadow copy of what WE have
  // pushed via history.pushState, not a read of the browser's own history
  // (JS has no read access to that). Modeled the same way real session
  // history works: an ordered stack plus a position pointer, not just a
  // running log, so going Back then taking a NEW path correctly drops the
  // old "forward" entries instead of leaving a stale branch visible. ---
  var visitStack = [anchorId];
  var visitPos = 0;
  var visitDropped = 0; // oldest entries HISTORY_DEPTH_CAP has evicted -- rendered as "N earlier", never silently
  var HISTORY_DEPTH_CAP = 8;
  // Every node ever selected, for the ring's visited-node marker (below,
  // applySelection). Deliberately NOT derived from visitStack at render
  // time: visitStack is capped (HISTORY_DEPTH_CAP) and evicts its oldest
  // entries, but a "visited" mark on the ring should behave like a real
  // visited-link convention -- once seen, stays marked seen, regardless of
  // how much has been visited since.
  var visitedIds = {};
  visitedIds[anchorId] = true;

  if (hasTranslations) {
    langToggle.hidden = false;
  }

  // --- Layout: fit all node positions (chord-ring or force, whichever the
  // export baked in) into the SVG viewBox. Center is used both for the
  // viewBox fit AND as the pull-point for edge arcs below. Node titles no
  // longer render as in-SVG <text> at all (see the node-drawing loop and
  // #graph-caption below) -- they show, at a real legible size, in a
  // caption under the diagram instead.
  var minX = Math.min.apply(null, topicNodes.map(function (n) { return n.x; }));
  var maxX = Math.max.apply(null, topicNodes.map(function (n) { return n.x; }));
  var minY = Math.min.apply(null, topicNodes.map(function (n) { return n.y; }));
  var maxY = Math.max.apply(null, topicNodes.map(function (n) { return n.y; }));
  if (!isFinite(minX)) { minX = 0; maxX = 100; minY = 0; maxY = 100; }
  var w = Math.max(1, maxX - minX);
  var h = Math.max(1, maxY - minY);
  var centerX = (minX + maxX) / 2, centerY = (minY + maxY) / 2;

  function degreeOf(id) {
    var d = 0;
    edges.forEach(function (e) { if (e.source === id || e.target === id) d++; });
    return d;
  }

  // `renderedWidth` is governed by CSS (#graph-svg's fixed ~280-300px box),
  // not by the viewBox we're about to compute -- safe to read before the
  // viewBox is set.
  var renderedWidth = svg.getBoundingClientRect().width || 300;
  var minOnscreenRadiusPx = 12;

  // A wedge's OUTER arc corners sit at its own angle +/- a half-span (see
  // the draw loop below), NOT directly outward from the node's own (x, y).
  // For a node far out on the ring, that angular offset swings the corner
  // sideways by a real WORLD distance (~= outerR * halfSpan) that can
  // dwarf the radial growth itself -- confirmed as a real clipping bug
  // this session: padding sized only to cover "radial reach past the
  // node's own radius" clipped ordinary spoke wedges on hover, NOT the
  // anchor (despite the anchor having the largest thickness bonus),
  // because the anchor sits near center where that angular swing is tiny
  // while a far spoke's is not. Bounding the swing per-angle isn't worth
  // the complexity for a nav widget -- instead, require the viewBox to
  // contain a full circle of radius `maxOuterR` around the ring center.
  // Deliberately conservative (not the tightest possible pad), cheap to
  // compute, and correct no matter where in its span a corner lands.
  var maxNodeRadius = 0;
  topicNodes.forEach(function (n) {
    var r = Math.sqrt((n.x - centerX) * (n.x - centerX) + (n.y - centerY) * (n.y - centerY));
    maxNodeRadius = Math.max(maxNodeRadius, r);
  });
  // refreshWedgeGrowth's hover bonus is `minWorldRadius * (HOVER_GROWTH_FACTOR - 1)`
  // -- i.e. a hovered wedge's total outward reach past its own radius is
  // minWorldRadius * HOVER_GROWTH_FACTOR, not just minWorldRadius. Must
  // match refreshWedgeGrowth's formula exactly or the two drift out of
  // sync (a real bug found this session).
  var HOVER_GROWTH_FACTOR = 1.6;
  var strokeBuffer = 2;
  // Extra flat cushion beyond the strict correctness minimum, purely for
  // visual breathing room around the ring (user-requested) -- doesn't
  // affect centering, since pad is always applied symmetrically (see
  // centerX/centerY above, and viewBoxW below).
  var cosmeticCushion = 16;

  // pad and minWorldRadius are circularly defined (pad affects viewBoxW,
  // which affects worldToScreenScale, which affects minWorldRadius, which
  // pad must cover) -- two passes converges close enough: pad's effect on
  // scale is second-order once `w`/`h` dominate viewBoxW, true except at
  // very low node counts where minWorldRadius is tiny anyway. Not a
  // precision-critical fit, just a nav widget.
  var pad = 40;
  for (var fitPass = 0; fitPass < 2; fitPass++) {
    var viewBoxWGuess = w + pad * 2;
    var scaleGuess = renderedWidth / viewBoxWGuess;
    var minWorldRadiusGuess = minOnscreenRadiusPx / scaleGuess;
    // Every wedge's base thickness is exactly minWorldRadius (uniform --
    // see the draw loop's halfThickness, and its own comment on why a
    // per-node bonus was dropped), so there's no separate bonus term to
    // budget for here beyond the hover-growth reach.
    var maxOuterR = maxNodeRadius + minWorldRadiusGuess * HOVER_GROWTH_FACTOR + strokeBuffer;
    pad = Math.max(maxOuterR - w / 2, maxOuterR - h / 2, 0) + cosmeticCushion;
  }
  var viewBoxW = w + pad * 2;
  svg.setAttribute(
    "viewBox",
    (minX - pad) + " " + (minY - pad) + " " + viewBoxW + " " + (h + pad * 2)
  );
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");

  // Final scale/floor, from the FINAL (not provisional) viewBoxW -- see the
  // node-drawing loop below for why a flat world-space floor alone can't
  // guarantee a real on-screen hit target (preserveAspectRatio scales world
  // units down as the ring grows; a sufficient floor at 14 nodes silently
  // stops being sufficient at 50+).
  var worldToScreenScale = renderedWidth / viewBoxW;
  var minWorldRadius = minOnscreenRadiusPx / worldToScreenScale;

  var svgNS = "http://www.w3.org/2000/svg";
  function el(tag, attrs) {
    var n = document.createElementNS(svgNS, tag);
    for (var k in attrs) { n.setAttribute(k, attrs[k]); }
    return n;
  }
  // DOM (not SVG) element helper for the sidebar/detail panel, used
  // everywhere below instead of string-concatenated innerHTML so plain
  // text content only ever goes through textContent, never through
  // string interpolation into markup.
  function dom(tag, attrs, text) {
    var n = document.createElement(tag);
    for (var k in (attrs || {})) { n.setAttribute(k, attrs[k]); }
    if (text != null) { n.textContent = text; }
    return n;
  }

  // --- Draw edges first (under nodes), as arcs pulled toward the layout
  // center -- the chord-diagram convention -- rather than straight chords.
  // Each edge is independently clickable ("follow the link to the other
  // node"): clicking navigates to whichever endpoint ISN'T currently
  // selected (defaulting to the target when neither/both match). ---
  var edgeLayer = el("g", { id: "edge-layer" });
  svg.appendChild(edgeLayer);
  var edgePaths = [];
  edges.forEach(function (e) {
    var s = nodesById[e.source], t = nodesById[e.target];
    if (!s || !t) { return; }
    // kb/adrs/0004: a guidance node has no real chord position (x=0, y=0,
    // never drawn as a <g class="node"> below) -- an edge touching one
    // would otherwise draw a visually broken arc toward the coordinate
    // origin. Guidance nodes aren't expected to have topic-graph edges in
    // practice (they're pulled in as always-included meta content, not via
    // normal traversal), but this stays a real filter, not an assumption.
    if (s.is_guidance || t.is_guidance) { return; }
    var pullBack = 0.55; // 0 = straight line, 1 = fully at center
    var cx = s.x + (t.x - s.x) / 2 + (centerX - (s.x + t.x) / 2) * pullBack;
    var cy = s.y + (t.y - s.y) / 2 + (centerY - (s.y + t.y) / 2) * pullBack;
    // Land the vertex on each node's wedge INNER edge, not its raw (x, y)
    // -- which sits at nodeRadius, the wedge's mid-thickness point (every
    // wedge's halfThickness is exactly minWorldRadius; see above) -- so
    // the chord visually meets the slice instead of appearing to
    // originate from somewhere inside it. edgeVertexInset nudges a touch
    // further in so the vertex doesn't sit exactly on the boundary line
    // (avoids an anti-aliasing seam at that exact radius). The curve's
    // control point (cx, cy) above is left keyed to the nodes' own ring
    // positions -- only the endpoints move.
    var edgeVertexInset = 2;
    var sAngle = Math.atan2(s.y - centerY, s.x - centerX);
    var sRadius = Math.sqrt((s.x - centerX) * (s.x - centerX) + (s.y - centerY) * (s.y - centerY));
    var sVertex = polarPoint(centerX, centerY, sRadius - minWorldRadius - edgeVertexInset, sAngle);
    var tAngle = Math.atan2(t.y - centerY, t.x - centerX);
    var tRadius = Math.sqrt((t.x - centerX) * (t.x - centerX) + (t.y - centerY) * (t.y - centerY));
    var tVertex = polarPoint(centerX, centerY, tRadius - minWorldRadius - edgeVertexInset, tAngle);
    var d = "M " + sVertex[0] + " " + sVertex[1] + " Q " + cx + " " + cy + " " + tVertex[0] + " " + tVertex[1];
    var path = el("path", { d: d, class: "edge", "data-source": e.source, "data-target": e.target });
    path.addEventListener("click", function () {
      selectNode(selectedId === e.source ? e.target : e.source);
    });
    edgeLayer.appendChild(path);
    edgePaths.push(path);
  });

  // --- Draw nodes as arc slices (wedges) of the ring instead of
  // overlapping circles -- kb/adrs/00XX. Every topic node sits at the SAME
  // radius from center (chord_ring_positions, upstream in mae-canvas
  // outside this file) and gets an equal angular slot; the radial
  // thickness carries forward the EXACT same size formula circles used
  // (anchor/degree bonus on top of the >=24px on-screen floor), just
  // doubled to express thickness (both sides of the ring) instead of a
  // single-sided radius. The amount a wedge reaches beyond the ring's own
  // radius (thickness/2) is therefore numerically identical to the old
  // circle radius `r` -- exactly what the two-pass viewBox `pad` fit
  // above already budgets for, so that code needed zero changes.
  var ringNodeCount = topicNodes.length;
  var angleStep = ringNodeCount > 0 ? (2 * Math.PI / ringNodeCount) : 0;
  // No angular gap between adjacent wedges (user request) -- slots sit
  // flush against each other; the rounded corners below (cornerRadius)
  // are what visually separate one wedge from the next, the same way
  // adjacent flower petals read as distinct without a drawn gap between
  // them.
  var wedgeGapRadians = 0;

  function polarPoint(cx, cy, r, a) {
    return [cx + r * Math.cos(a), cy + r * Math.sin(a)];
  }

  // Annular-sector `d` path with optionally-rounded corners ("petal"
  // look, user request). a1 is always > a0 (both are angle +/- a
  // half-span). With cornerRadius 0 this is the plain sharp-cornered
  // sector: outer arc a0->a1 (sweep=1, increasing angle), straight in to
  // innerR, inner arc a1->a0 (sweep=0, decreasing angle), close -- the
  // standard annulus-sector construction (the same shape a d3.js-style
  // arc() generator produces).
  //
  // With cornerRadius > 0, each of the 4 corners (where a RADIAL edge
  // meets a CIRCULAR arc -- always a clean 90-degree corner, since a
  // radial line's direction is purely radial and a circle's tangent at
  // any point is purely perpendicular to its radius) is filleted: back
  // off `cr` along each adjoining edge and connect with a small arc of
  // radius `cr`. This is an approximation of the true circle-tangency
  // corner-radius algorithm (treating the arc edges as locally straight
  // right at the corner) -- accurate enough to look properly rounded for
  // any `cr` that's small relative to innerR/outerR, which every caller
  // here respects via the clamp below.
  // innerCornerRadius is OPTIONAL and defaults to outerCornerRadius --
  // every existing call site (the main wedge, both here and in
  // refreshWedgeGrowth) passes a single radius and gets the original
  // symmetric-corner behavior unchanged. The two-radius form exists for
  // visited-inner-arc below: that band's outer edge (innerArcOuterR) is
  // an artificial internal cut partway through the wedge, not a real
  // wedge boundary -- rounding it made the marker look like a separate
  // floating pill instead of a flush-sided slice of the petal it's
  // nested in (a real, reported visual mismatch). Its inner edge
  // (innerR) IS a real, SHARED boundary with the wedge itself, so that
  // corner still wants rounding, matching the wedge's own inner-corner
  // treatment there.
  function arcPath(cx, cy, innerR, outerR, a0, a1, outerCornerRadius, innerCornerRadius) {
    if (innerCornerRadius === undefined) { innerCornerRadius = outerCornerRadius; }
    var crOuter = outerCornerRadius || 0;
    var crInner = innerCornerRadius || 0;
    // Clamp each radius independently so opposite fillets on the SAME
    // edge (outer or inner) can never meet or cross -- otherwise a very
    // thin (small halfThickness) or very short (small angular span)
    // wedge would self-intersect into a degenerate shape instead of just
    // a less-rounded one. Both are also capped by the full radial
    // thickness so one edge's fillet can't bite into the other edge's
    // radius even when the two differ.
    crOuter = Math.max(
      0,
      Math.min(crOuter, (outerR - innerR) / 2 - 0.01, ((a1 - a0) * outerR) / 2 - 0.01)
    );
    crInner = Math.max(
      0,
      Math.min(
        crInner,
        (outerR - innerR) / 2 - 0.01,
        ((a1 - a0) * Math.max(innerR, 1)) / 2 - 0.01
      )
    );
    if (crOuter <= 0 && crInner <= 0) {
      var ox0 = polarPoint(cx, cy, outerR, a0), ox1 = polarPoint(cx, cy, outerR, a1);
      var ix1 = polarPoint(cx, cy, innerR, a1), ix0 = polarPoint(cx, cy, innerR, a0);
      return "M " + ox0[0] + " " + ox0[1] +
        " A " + outerR + " " + outerR + " 0 0 1 " + ox1[0] + " " + ox1[1] +
        " L " + ix1[0] + " " + ix1[1] +
        " A " + innerR + " " + innerR + " 0 0 0 " + ix0[0] + " " + ix0[1] +
        " Z";
    }
    // A zero radius here degenerates to a straight line to the same
    // endpoint the rounded case would have used (SVG's own rule for a
    // zero-radius arc segment) -- crOuter/crInner independently at 0
    // naturally produces a flush, unrounded edge on just that side with
    // no special-casing needed beyond the shared branch above.
    var dOuter = crOuter / outerR;
    var dInner = crInner / Math.max(innerR, 1);
    var pOuterStart = polarPoint(cx, cy, outerR, a0 + dOuter);
    var pOuterEnd = polarPoint(cx, cy, outerR, a1 - dOuter);
    var pSideEndOuter = polarPoint(cx, cy, outerR - crOuter, a1);
    var pSideEndInner = polarPoint(cx, cy, innerR + crInner, a1);
    var pInnerEnd = polarPoint(cx, cy, innerR, a1 - dInner);
    var pInnerStart = polarPoint(cx, cy, innerR, a0 + dInner);
    var pSideStartInner = polarPoint(cx, cy, innerR + crInner, a0);
    var pSideStartOuter = polarPoint(cx, cy, outerR - crOuter, a0);
    return "M " + pOuterStart[0] + " " + pOuterStart[1] +
      " A " + outerR + " " + outerR + " 0 0 1 " + pOuterEnd[0] + " " + pOuterEnd[1] +
      " A " + crOuter + " " + crOuter + " 0 0 1 " + pSideEndOuter[0] + " " + pSideEndOuter[1] +
      " L " + pSideEndInner[0] + " " + pSideEndInner[1] +
      " A " + crInner + " " + crInner + " 0 0 1 " + pInnerEnd[0] + " " + pInnerEnd[1] +
      " A " + innerR + " " + innerR + " 0 0 0 " + pInnerStart[0] + " " + pInnerStart[1] +
      " A " + crInner + " " + crInner + " 0 0 1 " + pSideStartInner[0] + " " + pSideStartInner[1] +
      " L " + pSideStartOuter[0] + " " + pSideStartOuter[1] +
      " A " + crOuter + " " + crOuter + " 0 0 1 " + pOuterStart[0] + " " + pOuterStart[1] +
      " Z";
  }

  var nodeLayer = el("g", { id: "node-layer" });
  svg.appendChild(nodeLayer);
  var nodeGroups = [];
  // Each real (non-guidance) node's REST geometry, keyed by id -- needed
  // after initial draw too, since hover/neighbor growth recomputes and
  // re-sets the wedge's own `d` attribute rather than using a CSS
  // transform (see the .node path doc comment in STATIC_CSS for why).
  var wedgeGeomById = {};
  nodes.forEach(function (n) {
    // kb/adrs/0004: guidance nodes never get a chord-graph <g> -- but
    // nodeGroups still gets a placeholder pushed for them (null, not
    // skipped) so it stays index-aligned with `nodes`/`n._idx`: groupFor(id)
    // below looks up nodeGroups[nodesById[id]._idx], and every call site
    // already null-checks its result (e.g. "if (g) { ... }"), so a
    // placeholder is a correct, silent no-op everywhere it's read.
    if (n.is_guidance) { nodeGroups.push(null); return; }
    // Every wedge is exactly minWorldRadius thick -- deliberately UNIFORM,
    // not anchor/degree-scaled (an earlier version added a per-node bonus
    // here, up to +5.4 world units). At real KB node counts that bonus was
    // a large fraction of the base thickness (confirmed on a real 168-node
    // export: thickness ranged 139-168, a ~20% swing) -- since every
    // wedge sits at the exact SAME radius (upstream chord_ring_positions
    // guarantees this), that swing reads as wedges bulging past their
    // neighbors, exactly the overlapping look the wedge redesign existed
    // to eliminate. Degree is still real, visible signal -- see the
    // fill-opacity below, which encodes it without touching geometry.
    var halfThickness = minWorldRadius;
    var nodeRadius = Math.sqrt(
      (n.x - centerX) * (n.x - centerX) + (n.y - centerY) * (n.y - centerY)
    );
    var angle = Math.atan2(n.y - centerY, n.x - centerX);
    // Always exactly the node's own nominal angular slot -- NEVER grown
    // past it to chase a bigger on-screen hit target. An earlier version
    // grew halfSpan up to a 24px-tangential-width floor (mirroring the
    // radial floor above) when a node's own slot was too thin; confirmed
    // as a real, severe bug on a real 168-node export: that floor forced
    // halfSpan past the nominal slot on 142 of 168 wedge boundaries (85%),
    // by MORE than a full slot width in places -- not "a touch of
    // overlap," systemic overlap that defeated the entire point of the
    // wedge redesign (eliminating the old overlapping-circles look). At
    // extreme node counts, a guaranteed minimum tangential hit target and
    // zero overlap are mutually exclusive in finite screen space -- this
    // file picks zero overlap as the hard invariant every time, and lets
    // the hit target degrade gracefully instead (the fullscreen toggle
    // exists specifically to claw some of that back by growing the ring's
    // on-screen size).
    var halfSpan = angleStep / 2 - wedgeGapRadians / 2;
    // "Flower petal" corner rounding (user request), scaled to the
    // wedge's own (rest-state) thickness so it looks proportionate at
    // any node count/ring size -- kept fixed across hover/neighbor growth
    // (refreshWedgeGrowth reuses this same value from wedgeGeomById
    // rather than rescaling it live) so the rounding doesn't visibly
    // change shape mid-transition, only the outer radius does.
    var cornerRadius = halfThickness * 0.6;
    wedgeGeomById[n.id] = {
      nodeRadius: nodeRadius,
      halfThickness: halfThickness,
      angle: angle,
      halfSpan: halfSpan,
      cornerRadius: cornerRadius,
    };
    var g = el("g", {
      class: "node" + (n.is_anchor ? " node-anchor" : ""),
      "data-idx": n._idx,
      "data-kind": n.kind,
      "data-id": n.id,
      // Roving tabindex (standard ARIA pattern for a set of related
      // items): only the currently-selected node is Tab-reachable, kept
      // in sync by updateRovingTabindex() below on every selection
      // change. -1 here is just the safe default until the first
      // applySelection() call sets the real state.
      tabindex: "-1",
      role: "button",
      "aria-label": n["title_" + currentLang],
    });
    var innerR = nodeRadius - halfThickness;
    var outerR = nodeRadius + halfThickness;
    var wedge = el("path", {
      d: arcPath(
        centerX, centerY, innerR, outerR, angle - halfSpan, angle + halfSpan, cornerRadius
      ),
    });
    g.appendChild(wedge);
    // Visited-node marker: an inner ~2/5-thickness band of the wedge
    // itself (a nested smaller arc, not a dot) -- a fill/opacity toggle
    // only, deliberately NOT the fill/stroke/geometry channels hover/
    // neighbor/selected already own on the OUTER wedge (see the .visited
    // CSS below), so it never competes with those. Same angular span as
    // the outer wedge (angle +/- halfSpan).
    //
    // Corner radii are asymmetric (arcPath's two-radius form), NOT the
    // wedge's own cornerRadius scaled down on both edges as a first
    // version tried and a real reported visual mismatch corrected: this
    // band's OUTER edge (innerArcOuterR) is an artificial cut partway
    // through the wedge, not a true wedge boundary -- rounding it made
    // the marker look like a separate rounded pill floating inside the
    // petal instead of lining up flush with the petal's own straight
    // sides. 0 there keeps that edge sharp and flush. Its INNER edge
    // (innerR) IS a real, shared boundary with the wedge itself, so it
    // reuses the wedge's own (unscaled) `cornerRadius` there, nesting
    // against the wedge's own rounded inner corner instead of a
    // mismatched smaller one.
    //
    // Positioned once at draw time since it doesn't move with growth
    // (growth only changes the OUTER wedge's outer radius, see
    // refreshWedgeGrowth).
    var innerArcOuterR = innerR + (outerR - innerR) * 0.4;
    var visitedArc = el("path", {
      class: "visited-inner-arc",
      d: arcPath(
        centerX, centerY, innerR, innerArcOuterR, angle - halfSpan, angle + halfSpan, 0, cornerRadius
      ),
    });
    g.appendChild(visitedArc);
    g.addEventListener("mouseenter", function () { onHover(n, true); });
    g.addEventListener("mousemove", movePopover);
    g.addEventListener("mouseleave", function () { onHover(n, false); });
    g.addEventListener("click", function () { selectNode(n.id); });
    nodeLayer.appendChild(g);
    nodeGroups.push(g);
  });

  // --- Keyboard navigation around the ring (roving tabindex) ---
  //
  // ringOrder: topic node ids in real angular order (NOT `nodes` array
  // order, which reflects layout/insertion order upstream, not position on
  // the ring) -- built from wedgeGeomById's own angle, the same value the
  // draw loop just used, so ArrowLeft/Right always match what's visually
  // adjacent on screen.
  var ringOrder = topicNodes
    .map(function (n) { return n.id; })
    .filter(function (id) { return wedgeGeomById[id]; })
    .sort(function (a, b) { return wedgeGeomById[a].angle - wedgeGeomById[b].angle; });

  // Standard ARIA roving-tabindex bookkeeping: exactly one node (the
  // current selection) is ever Tab-reachable at a time; every other node
  // drops out of the tab order entirely rather than requiring N tab-stops
  // to cross the ring. Called from applySelection on every real selection
  // change, so it's always in sync with `selectedId`.
  function updateRovingTabindex() {
    nodeGroups.forEach(function (ng) {
      if (!ng) { return; }
      ng.setAttribute("tabindex", ng.getAttribute("data-id") === selectedId ? "0" : "-1");
    });
  }

  // ArrowLeft/Right: move around the ring (both focus and selection
  // together -- there's no separate "focused but not yet selected" state
  // in this widget, unlike a typical roving-tabindex menu, since every
  // node is cheap to preview via its detail panel the instant it's
  // reached). ArrowUp/Down deliberately reuse the SAME Next/Previous
  // reading-order buttons Tab/click already drive (not a second,
  // competing way to move through the guide) -- "left/right" means
  // "around the ring," "up/down" means "forward/backward through the
  // guide," a deliberate split so the two never fight over the same keys.
  // Enter/Space activates the focused node the same way a click does --
  // almost always a no-op in practice (focus already mirrors selectedId
  // after every move above), kept anyway so this still behaves like a
  // real `role="button"` for a reader tabbing in fresh.
  nodeLayer.addEventListener("keydown", function (ev) {
    if (ev.key === "ArrowRight" || ev.key === "ArrowLeft") {
      var idx = ringOrder.indexOf(selectedId);
      if (idx === -1) { return; }
      var delta = ev.key === "ArrowRight" ? 1 : -1;
      var nextId = ringOrder[(idx + delta + ringOrder.length) % ringOrder.length];
      selectNode(nextId);
      var nextG = groupFor(nextId);
      if (nextG) { nextG.focus(); }
      ev.preventDefault();
    } else if (ev.key === "ArrowDown") {
      nextBtn.click();
      var afterNextG = groupFor(selectedId);
      if (afterNextG) { afterNextG.focus(); }
      ev.preventDefault();
    } else if (ev.key === "ArrowUp") {
      prevBtn.click();
      var afterPrevG = groupFor(selectedId);
      if (afterPrevG) { afterPrevG.focus(); }
      ev.preventDefault();
    } else if (ev.key === "Enter" || ev.key === " ") {
      selectNode(selectedId);
      ev.preventDefault();
    }
  });

  // Recomputes and re-sets a node's wedge `d` attribute for its CURRENT
  // combined hover/neighbor/rest state -- growth is real geometry (the
  // outer arc's radius), not a CSS transform (see the .node path doc
  // comment in STATIC_CSS for why that approach was tried and reverted).
  // Grows OUTWARD ONLY (inner radius and angular span stay fixed): the
  // wedge "pops out" of the ring rather than also widening sideways into
  // its neighbors' gaps, or inward into the ring's own hollow center.
  // Same precedence as the old CSS had (.hovered's larger growth wins
  // over .neighbor when both apply) -- now enforced by this one function
  // reading class state directly instead of two competing CSS selectors.
  function refreshWedgeGrowth(id) {
    var geom = wedgeGeomById[id];
    var g = groupFor(id);
    if (!geom || !g) { return; }
    // Growth is an ABSOLUTE bonus in world units, not a multiplier on
    // halfThickness itself -- halfThickness (the anchor/degree-scaled
    // reach beyond the ring) is often small relative to nodeRadius (the
    // ring's own large radius), so multiplying IT by 1.25 produced a
    // barely-perceptible change (confirmed empirically: <1% area growth
    // on this session's own fixture, nowhere near the old circles'
    // visibly-lifted hover state). minWorldRadius is the one quantity
    // already calibrated to a real, visible 12px on-screen floor
    // (see its own definition above) -- scaling growth off THAT instead
    // guarantees a real, visible "pop out" regardless of how thin a
    // given node's own halfThickness happens to be. The hover bonus
    // reuses HOVER_GROWTH_FACTOR (defined with the viewBox `pad` fit
    // above) rather than a second, independently-hardcoded 0.6 -- the pad
    // budget and the actual growth applied here MUST stay in sync, or a
    // hovered wedge can clip the viewBox edge (confirmed a real bug this
    // session when they drifted apart).
    var growthBonus = g.classList.contains("hovered")
      ? minWorldRadius * (HOVER_GROWTH_FACTOR - 1)
      : (g.classList.contains("neighbor") ? minWorldRadius * 0.35 : 0);
    var innerR = geom.nodeRadius - geom.halfThickness;
    var outerR = geom.nodeRadius + geom.halfThickness + growthBonus;
    var wedge = g.querySelector("path");
    if (wedge) {
      wedge.setAttribute(
        "d",
        arcPath(
          centerX, centerY, innerR, outerR,
          geom.angle - geom.halfSpan, geom.angle + geom.halfSpan,
          geom.cornerRadius
        )
      );
    }
  }

  function groupFor(id) {
    var n = nodesById[id];
    return n ? nodeGroups[n._idx] : null;
  }

  // Shows a node's title in #graph-caption, below the diagram, at a real
  // legible size -- see the CSS rule's comment for why this replaced
  // in-SVG label text. Falls back to the currently selected node (not
  // blank) so the caption reads as "what am I looking at" rather than
  // flickering empty every time the cursor leaves a node.
  function updateCaption(n) {
    graphCaption.textContent = n ? n["title_" + currentLang] : "";
  }
  // --- Hover popover (title via textContent, never innerHTML) ---
  function onHover(n, entering) {
    var g = groupFor(n.id);
    if (g) { g.classList.toggle("hovered", entering); }
    refreshWedgeGrowth(n.id);
    if (!entering) {
      popover.hidden = true;
      updateCaption(selectedId != null ? nodesById[selectedId] : null);
      return;
    }
    updateCaption(n);
    popover.textContent = "";
    popover.appendChild(dom("div", { class: "popover-title" }, n["title_" + currentLang]));
    popover.appendChild(dom("div", { class: "popover-body" }, n["preview_" + currentLang]));
    popover.hidden = false;
  }
  // Clamp to the viewport instead of always anchoring bottom-right of the
  // cursor: the chord widget sits in the right-hand sidebar, so a node on
  // the right half of the ring puts the cursor near the viewport's right
  // edge already -- an unclamped popover there rendered mostly off-screen.
  // Flip to whichever side of the cursor actually has room, per axis,
  // independently (a popover can need to flip horizontally, vertically,
  // both, or neither depending on where on the ring the cursor is).
  function movePopover(ev) {
    // onHover already set content + unhid the popover before this fires
    // (mouseenter -> onHover, then mousemove -> this), so its real
    // rendered size is already measurable -- no need to reposition or
    // toggle visibility just to read it.
    var rect = popover.getBoundingClientRect();
    var margin = 8;
    var left = ev.clientX + 14;
    if (left + rect.width + margin > window.innerWidth) {
      left = ev.clientX - rect.width - 14;
    }
    left = Math.max(margin, left);
    var top = ev.clientY + 14;
    if (top + rect.height + margin > window.innerHeight) {
      top = ev.clientY - rect.height - 14;
    }
    top = Math.max(margin, top);
    popover.style.left = left + "px";
    popover.style.top = top + "px";
  }

  // --- Hover-preview + click-to-navigate on in-body links (org-roam-ui-
  // style): every internal link the org-link converter produces inside a
  // rendered node body (an <a> whose href is a fragment-style internal
  // reference, not a real URL) gets the exact same hover popover chord-
  // diagram nodes already have -- same onHover/movePopover, same popover
  // element, same nodesById lookup, nothing new to build for that part.
  // Click actually opens the linked node (selectNode) instead of the
  // browser's default same-page fragment-scroll, which -- since no
  // element in this page actually has that id -- had no visible effect at
  // all; a real, reproducible bug (clicking any in-body link silently did
  // nothing), not just a missing nice-to-have. External https links never
  // match the fragment-prefix check, so they're excluded automatically,
  // and an internal reference that doesn't resolve in *this* curated
  // subgraph's nodesById (a real case -- not every link in a body's
  // source note happens to land inside whatever subgraph got exported)
  // is a silent no-op below for both hover and click, not an error.
  // A source note commonly links to more than what a depth-limited curated
  // export actually includes -- that's expected, not a bug in the curation
  // itself (see the "keep extraction opinionated" writing-style note). But
  // rendering those as normal `<a>` elements gave every unresolved link
  // the same blue, underlined, pointer-cursor appearance as a real one,
  // with nothing happening on click -- indistinguishable from a working
  // link until you actually try it. Unwrap unresolved links into plain
  // text (not just a "disabled-looking" style on the `<a>`) so there's no
  // false affordance at all: no color, no cursor, no focus stop.
  function wireBodyLinks(container) {
    var links = container.querySelectorAll("a[href^='#']");
    Array.prototype.forEach.call(links, function (a) {
      var n = nodesById[a.getAttribute("href").slice(1)];
      if (!n) {
        a.replaceWith(document.createTextNode(a.textContent));
        return;
      }
      a.addEventListener("mouseenter", function () { onHover(n, true); });
      a.addEventListener("mousemove", movePopover);
      a.addEventListener("mouseleave", function () { onHover(n, false); });
      a.addEventListener("click", function (ev) {
        ev.preventDefault();
        selectNode(n.id);
      });
    });
  }

  // --- Lightweight, self-contained syntax highlighting for src/example
  // blocks. This page ships zero external dependencies (no CDN script, no
  // bundled third-party highlighter) -- highlighting is a small
  // regex/scan-based tokenizer run over each block's OWN text after body
  // HTML lands in the DOM, not a real language grammar. It recognizes only
  // the token shapes that actually appear in this KB's HCL/Terraform and
  // shell content (comments, strings, numbers, keywords, `${...}`
  // interpolation) -- no speculative generality for languages this KB
  // doesn't use.
  var HL_KEYWORDS = {
    hcl: ["resource", "data", "variable", "output", "module", "provider", "terraform",
          "locals", "for_each", "count", "if", "else", "for", "in", "true", "false", "null"],
    terraform: ["resource", "data", "variable", "output", "module", "provider", "terraform",
                "locals", "for_each", "count", "if", "else", "for", "in", "true", "false", "null"],
    tf: ["resource", "data", "variable", "output", "module", "provider", "terraform",
         "locals", "for_each", "count", "if", "else", "for", "in", "true", "false", "null"],
    shell: ["if", "then", "else", "fi", "for", "in", "do", "done", "while", "case", "esac",
            "function", "return", "export", "local", "echo"],
    bash: ["if", "then", "else", "fi", "for", "in", "do", "done", "while", "case", "esac",
           "function", "return", "export", "local", "echo"],
    sh: ["if", "then", "else", "fi", "for", "in", "do", "done", "while", "case", "esac",
         "function", "return", "export", "local", "echo"]
  };

  function hlEscape(s) {
    // `/[<]/` (a character class), not the more obvious `/</` -- this
    // whole GRAPH_JS text gets a blanket `"</" -> "<\/"` pass
    // (`escape_for_inline_script`) as defense against embedded content
    // prematurely closing the page's own `<script>` tag. That pass is
    // safe inside string literals (`\/` is just an escaped `/`), but a
    // bare `/</g` regex literal has its OWN closing delimiter sitting
    // right after the `<` -- escaping THAT slash strips the regex's
    // closing delimiter and corrupts the whole script (found by actually
    // parsing the exported page's JS with `node --check`, not by
    // inspection -- a real parse failure, not a logic bug).
    return s.replace(/&/g, "&amp;").replace(/[<]/g, "&lt;").replace(/>/g, "&gt;");
  }

  // Tokenizes `src` (plain decoded text, NOT html-escaped) into an HTML
  // string with <span class="tok-*"> around comments/strings/numbers/
  // keywords/HCL `${...}` interpolation; everything else passes through
  // hlEscape()'d and unwrapped. One linear left-to-right scan. A string
  // literal is consumed as a single atomic token (its content is never
  // re-scanned for nested comments/interpolation), which means `${...}`
  // interpolation *inside* a string renders as part of that string's
  // color rather than its own span -- a real limitation of a lightweight
  // scanner, accepted rather than building a real recursive grammar for
  // it.
  function highlightSource(src, keywords) {
    var out = "";
    var i = 0;
    var n = src.length;
    while (i < n) {
      var ch = src[i];
      if (/^#$/.test(ch) || (ch === "/" && src[i + 1] === "/")) {
        var cEnd = src.indexOf("\n", i);
        if (cEnd === -1) { cEnd = n; }
        out += "<span class=\"tok-com\">" + hlEscape(src.slice(i, cEnd)) + "</span>";
        i = cEnd;
        continue;
      }
      if (ch === "/" && src[i + 1] === "*") {
        var bClose = src.indexOf("*/", i + 2);
        var bEnd = bClose === -1 ? n : bClose + 2;
        out += "<span class=\"tok-com\">" + hlEscape(src.slice(i, bEnd)) + "</span>";
        i = bEnd;
        continue;
      }
      if (ch === "\"") {
        var j = i + 1;
        while (j < n && src[j] !== "\"") {
          j += src[j] === "\\" ? 2 : 1;
        }
        j = Math.min(j + 1, n);
        out += "<span class=\"tok-str\">" + hlEscape(src.slice(i, j)) + "</span>";
        i = j;
        continue;
      }
      if (ch === "$" && src[i + 1] === "{") {
        var depth = 1;
        var k = i + 2;
        while (k < n && depth > 0) {
          if (src[k] === "{") { depth++; } else if (src[k] === "}") { depth--; }
          k++;
        }
        out += "<span class=\"tok-interp\">" + hlEscape(src.slice(i, k)) + "</span>";
        i = k;
        continue;
      }
      if (/[0-9]/.test(ch) && (i === 0 || !/[A-Za-z0-9_]/.test(src[i - 1]))) {
        var numMatch = /^[0-9]+(\.[0-9]+)?/.exec(src.slice(i))[0];
        out += "<span class=\"tok-num\">" + numMatch + "</span>";
        i += numMatch.length;
        continue;
      }
      if (/[A-Za-z_]/.test(ch)) {
        var word = /^[A-Za-z_][A-Za-z0-9_]*/.exec(src.slice(i))[0];
        out += keywords.indexOf(word) !== -1
          ? "<span class=\"tok-kw\">" + word + "</span>"
          : hlEscape(word);
        i += word.length;
        continue;
      }
      out += hlEscape(ch);
      i += 1;
    }
    return out;
  }

  // Runs over every `pre code[class^="language-"]` (skipping "mermaid" --
  // already replaced with real inline <svg> or a raw-source fallback, see
  // render_mermaid_block) and every `pre.example` -- the latter gets a
  // narrower treatment: only a leading "$ " shell-prompt marker per line
  // is styled, since example blocks are transcripts (mixed commands and
  // arbitrary output), not one known language.
  function highlightCodeBlocks(container) {
    var blocks = container.querySelectorAll("pre code[class^=\"language-\"]");
    Array.prototype.forEach.call(blocks, function (code) {
      var lang = code.className.slice("language-".length);
      if (lang === "mermaid") { return; }
      code.innerHTML = highlightSource(code.textContent, HL_KEYWORDS[lang] || []);
    });
    var examples = container.querySelectorAll("pre.example");
    Array.prototype.forEach.call(examples, function (pre) {
      var lines = pre.textContent.split("\n");
      pre.innerHTML = lines.map(function (line) {
        if (line.slice(0, 2) === "$ ") {
          return "<span class=\"tok-prompt\">$</span> " + hlEscape(line.slice(2));
        }
        return hlEscape(line);
      }).join("\n");
    });
  }

  // --- Selection / detail panel ---
  function outgoingLinks(id) {
    return edges.filter(function (e) { return e.source === id; })
      .map(function (e) { return { node: nodesById[e.target], rel: e.rel_type }; })
      .filter(function (x) { return x.node; });
  }
  function incomingLinks(id) {
    return edges.filter(function (e) { return e.target === id; })
      .map(function (e) { return { node: nodesById[e.source], rel: e.rel_type }; })
      .filter(function (x) { return x.node; });
  }

  function renderLinkList(container, title, links) {
    if (links.length === 0) { return; }
    container.appendChild(dom("h3", {}, title));
    var ul = dom("ul", { class: "link-list" });
    links.forEach(function (l) {
      var li = dom("li");
      var btn = dom("button", { type: "button", class: "link-jump" });
      btn.appendChild(document.createTextNode(l.node["title_" + currentLang] + " "));
      btn.appendChild(dom("span", { class: "external-link" }, "(" + (l.rel || "related_to") + ")"));
      btn.addEventListener("click", function () { selectNode(l.node.id); });
      li.appendChild(btn);
      ul.appendChild(li);
    });
    container.appendChild(ul);
  }

  // --- "On this page" outline: scanned from the ACTUAL rendered heading
  // elements inside .detail-body (not a second, possibly-diverging parse)
  // -- single source of truth is whatever really ended up in the DOM. ---
  function renderOutline(bodyEl) {
    outlineList.textContent = "";
    var headings = bodyEl.querySelectorAll("h1, h2, h3, h4, h5, h6");
    if (headings.length === 0) { outlinePanel.hidden = true; return; }
    outlinePanel.hidden = false;
    headings.forEach(function (h, i) {
      var id = "outline-h-" + i;
      h.id = id;
      var li = dom("li");
      var btn = dom("button", { type: "button" }, h.textContent);
      btn.style.paddingLeft = (Math.max(0, (parseInt(h.tagName.substring(1), 10) - 1)) * 0.75) + "rem";
      btn.addEventListener("click", function () {
        h.scrollIntoView({ behavior: "smooth", block: "start" });
      });
      li.appendChild(btn);
      outlineList.appendChild(li);
    });
  }
  outlineToggle.addEventListener("click", function () {
    outlinePanel.classList.toggle("collapsed");
  });

  // --- Visited-node history panel: renders visitStack/visitPos, kept in
  // sync by selectNode() (new navigation) and the popstate listener
  // (Back/Forward replay) above. Entries are real <button>s wired to
  // selectNode -- clicking any past (or, after a Back, future) entry
  // jumps straight there, same real-navigation path as everything else on
  // the page. The current node is not a button (nothing to click to get
  // somewhere already on screen), just an accent-bordered row. ---
  function renderHistoryPanel() {
    historyList.textContent = "";
    if (visitDropped > 0) {
      historyList.appendChild(dom(
        "li", { class: "history-truncated" },
        "⋯ " + visitDropped + " earlier"
      ));
    }
    visitStack.forEach(function (id, i) {
      var n = nodesById[id];
      if (!n) { return; }
      var li = dom("li");
      if (i === visitPos) {
        var row = dom("span", { class: "history-current" }, n["title_" + currentLang]);
        li.appendChild(row);
      } else {
        var btn = dom("button", { type: "button" }, n["title_" + currentLang]);
        btn.addEventListener("click", function () { selectNode(id); });
        li.appendChild(btn);
        if (i === visitPos - 1) {
          li.appendChild(dom("span", { class: "history-marker" }, "← Back"));
        } else if (i === visitPos + 1) {
          li.appendChild(dom("span", { class: "history-marker" }, "Forward →"));
        }
      }
      historyList.appendChild(li);
    });
    historyBackBtn.disabled = visitPos <= 0;
    historyForwardBtn.disabled = visitPos >= visitStack.length - 1;
  }
  // Back/Forward buttons replay through the REAL browser history (not a
  // second, hand-rolled navigation path) -- history.back()/forward()
  // trigger the SAME popstate listener above that already reconciles
  // visitStack/visitPos, so there is exactly one place that logic lives.
  historyBackBtn.addEventListener("click", function () { history.back(); });
  historyForwardBtn.addEventListener("click", function () { history.forward(); });

  function renderDetail(n) {
    detailContent.classList.add("fading");
    window.setTimeout(function () {
      detailContent.textContent = "";
      detailContent.appendChild(dom("span", { class: "kind-badge" }, n.kind));
      // "Part ::" is a label from the node's own authored Reading Order
      // section (parse_reading_order_part, Rust side) -- structural
      // context ("where am I in the guide"), not a navigable link, so it
      // renders as plain muted text, never wrapped in an <a>. Per-language
      // like title/body (falls back to English server-side when no
      // translation exists -- see build_export_node), absent entirely for
      // nodes with no Reading Order section at all.
      var partLabel = n["reading_order_part_" + currentLang];
      if (partLabel) {
        detailContent.appendChild(dom("div", { class: "node-part-breadcrumb" }, partLabel));
      }
      detailContent.appendChild(dom("h2", { class: "detail-title" }, n["title_" + currentLang]));
      if (n.is_anchor) {
        detailContent.appendChild(dom(
          "p", { class: "anchor-note" },
          "Starting point of this exported subgraph."
        ));
      }
      // kb/adrs/0004: a reader can also open a guidance/colophon node
      // directly (colophon button, or an ordinary in-body link to one) --
      // this note orients them the same way anchor-note does for the
      // anchor, since nothing else on this screen says "you left the
      // guide's own topic content."
      if (n.is_guidance) {
        detailContent.appendChild(dom(
          "p", { class: "guidance-note" },
          "Guidance note — a standard this guide was written against, not part of its topic content."
        ));
      }
      // kb/adrs/0003-untranslated-node-fallback-signal.org: the language
      // toggle is a real, working GLOBAL preference -- it must keep
      // applying even on a node with no Spanish translation, so it's
      // never disabled per-node. But title_es/body_es mirroring title_en/
      // body_en exactly (the deliberate fallback for "no translation
      // exists") previously gave no visible signal at all when a reader
      // toggled to Spanish on one of those nodes: the button's own label
      // changed, the content didn't, and a reader clicking it repeatedly
      // reasonably concluded the switch was broken. Surface the fallback
      // per field (title/body can be translated independently) rather
      // than only per-node, so a partial translation doesn't silently
      // read as complete either.
      if (currentLang === "es") {
        var titleFallback = n.title_es === n.title_en;
        var bodyFallback = n.body_es === n.body_en;
        var fallbackMsg = null;
        if (titleFallback && bodyFallback) {
          fallbackMsg = "This note isn't translated yet — showing English.";
        } else if (titleFallback) {
          fallbackMsg = "This note's title isn't translated yet — showing the English title.";
        } else if (bodyFallback) {
          fallbackMsg = "This note's text isn't translated yet — showing the English text.";
        }
        if (fallbackMsg) {
          detailContent.appendChild(dom(
            "p", { class: "translation-fallback-note" }, fallbackMsg
          ));
        }
      }
      var body = dom("div", { class: "detail-body" });
      // n.body_en / n.body_es are pre-escaped HTML produced server-side by
      // mae-export's org renderer (crate::html_escape on every bit of real
      // node content, plus pre-rendered mermaid <svg>) -- this is the ONE
      // deliberate innerHTML assignment in this file; every other piece of
      // text above/below goes through textContent/dom() instead.
      body.innerHTML = n["body_" + currentLang];
      wireBodyLinks(body);
      highlightCodeBlocks(body);
      detailContent.appendChild(body);
      renderLinkList(detailContent, "Links to", outgoingLinks(n.id));
      renderLinkList(detailContent, "Linked from", incomingLinks(n.id));
      renderOutline(body);
      detailContent.classList.remove("fading");
    }, 120);
  }

  // Applies a selection to the DOM only -- no history side effect. Used by
  // both real navigation (selectNode, below) and the popstate handler
  // (browser back/forward), which must NOT push a new entry for a
  // navigation the browser is already replaying.
  function applySelection(id) {
    var n = nodesById[id];
    if (!n) { return; }
    // Every real navigation starts at the top of the new node's content --
    // #main-content is the actual scrolling container (overflow-y: auto),
    // not the window, so a plain anchor-jump/scrollIntoView wouldn't do
    // this on its own. Confirmed as a real reported bug: following an
    // in-body link while scrolled partway down the current node left the
    // reader at that same scroll offset on the newly-loaded node's
    // content, which reads as "did this even navigate?" if the new
    // content happens to be shorter than the old scroll position.
    if (mainContent) { mainContent.scrollTop = 0; }
    // A click-to-navigate (chord node or in-body link) doesn't reliably
    // fire the hovered element's mouseleave -- clicking a body link
    // replaces #main-content's DOM (including the very <a> under the
    // cursor) as part of this call, and the popover was observed staying
    // on screen indefinitely afterward. Navigating away always ends
    // whatever hover context produced the popover, regardless of why the
    // browser didn't fire mouseleave for it.
    popover.hidden = true;
    if (selectedId != null) {
      var prevG = groupFor(selectedId);
      if (prevG) { prevG.classList.remove("selected"); }
    }
    selectedId = id;
    visitedIds[id] = true;
    // Persist the current node so reopening this same exported file later
    // resumes here instead of always restarting at the anchor (user
    // request) -- same try/catch-wrapped, per-file-path localStorage
    // pattern the theme preference below already uses; see its own
    // comment for why some privacy modes throw here. Updated on every
    // real selection (including Back/Forward replays via popstate calling
    // applySelection directly), not just forward navigation, since
    // "resume where you left off" should reflect wherever the reader
    // actually ended up.
    try { localStorage.setItem("mae-guide-last-node", id); } catch (e) { /* ignore */ }
    updateCaption(n);
    var g = groupFor(id);
    if (g) { g.classList.add("selected"); }
    updateRovingTabindex();
    // Visited marker: every node ever selected gets `.visited`, EXCEPT
    // the currently-selected one -- selected already owns the
    // fill/stroke/geometry channels, so showing the visited dot on top of
    // it would just be visual noise for information the selected styling
    // already conveys on its own.
    topicNodes.forEach(function (tn) {
      var tg = groupFor(tn.id);
      if (!tg) { return; }
      tg.classList.toggle("visited", !!visitedIds[tn.id] && tn.id !== id);
    });
    // Directly-linked neighbor nodes get their own highlight (bigger hit
    // target via the same transform: scale mechanism .hovered already
    // uses, plus a distinct ring color) -- not just their connecting
    // edges. Confirmed a real, reported gap: in a dense ring the OTHER
    // endpoint of a highlighted edge looked identical to every unrelated
    // node, hard to both spot and precisely click. Cleared from every
    // node first (simpler and just as correct as tracking the previous
    // neighbor set) then reapplied for the new selection.
    nodeGroups.forEach(function (ng) { if (ng) { ng.classList.remove("neighbor"); } });
    edgePaths.forEach(function (p) {
      var src = p.getAttribute("data-source"), tgt = p.getAttribute("data-target");
      var incident = src === id || tgt === id;
      p.classList.toggle("incident", incident);
      if (incident) {
        var neighborGroup = groupFor(src === id ? tgt : src);
        if (neighborGroup) { neighborGroup.classList.add("neighbor"); }
      }
    });
    // Refresh EVERY topic node's wedge growth, not just the ones that
    // changed -- simpler and just as correct as tracking a precise delta
    // (a node that stopped being a neighbor needs to shrink back too, and
    // this is cheap at the node counts this widget targets).
    topicNodes.forEach(function (tn) { refreshWedgeGrowth(tn.id); });
    renderDetail(n);
  }
  // Real navigation (chord click, body-link click, Home/Previous/Next):
  // pushes a history entry so the browser's own Back/Forward buttons work
  // -- the one navigation UX every reader already knows, and the actual
  // gap Home/Previous/Next (a linear reading-order walk) doesn't cover on
  // its own: following links freely through the graph has no "undo" of
  // its own otherwise. A no-op re-selection of the already-open node
  // (e.g. clicking a link back to the current page) doesn't push a
  // duplicate entry.
  function selectNode(id) {
    if (id === selectedId) { return; }
    if (!nodesById[id]) { return; }
    applySelection(id);
    // Record this as new forward navigation in the visited-history stack,
    // unconditionally -- regardless of whether the pushState call just
    // below succeeds or throws (see its own comment): the navigation
    // itself really happened (applySelection already ran), so the shadow
    // history the panel renders from should reflect that either way.
    // If we're not already at the tail (the reader went Back and is now
    // taking a NEW path), drop everything after the current position first
    // -- the same forward-history invalidation a real browser does the
    // moment you navigate somewhere new after Back.
    if (visitPos < visitStack.length - 1) {
      visitStack = visitStack.slice(0, visitPos + 1);
    }
    visitStack.push(id);
    visitPos = visitStack.length - 1;
    while (visitStack.length > HISTORY_DEPTH_CAP) {
      visitStack.shift();
      visitPos -= 1;
      visitDropped += 1;
    }
    renderHistoryPanel();
    // Single-quoted deliberately, not double: this whole script is a Rust
    // raw string delimited by double-quote-hash, and that exact two-char
    // sequence anywhere in the JS source closes it early (a real compile
    // break hit while writing this).
    //
    // Regression: pushState was called unconditionally, with nothing
    // catching a throw. Firefox rate-limits History API calls under
    // file:// -- clicking through even a modest number of nodes (Next
    // repeatedly, or a few body links) throws a real, reproducible
    // SecurityError ("the operation is insecure") once the limit is hit.
    // An uncaught throw here aborts selectNode() -- and, critically,
    // whatever the CALLER does *after* calling it: nextBtn/prevBtn's
    // click handlers call updateWalkButtons() right after selectNode(),
    // so a thrown pushState left Previous/Next's disabled state stale
    // and, on some navigations, effectively stuck. Content itself still
    // updates correctly (applySelection already ran above) -- only the
    // history entry is lost when this throws, which degrades gracefully
    // to "Back/Forward won't undo this one step" instead of breaking
    // navigation entirely.
    try {
      history.pushState({ nodeId: id }, "", '#' + id);
    } catch (e) { /* ignore -- see comment above */ }
  }
  window.addEventListener("popstate", function (ev) {
    var id = (ev.state && ev.state.nodeId) || anchorId;
    applySelection(id);
    // Keep Previous/Next's position (and disabled state) consistent with
    // whatever Back/Forward just landed on -- every node is present in
    // readingOrder, so this always finds a real index. Without this, a
    // Next click after a Back would continue from wherever walkIndex was
    // left by the last Previous/Next click instead of from the node
    // actually on screen.
    var idx = readingOrder.indexOf(id);
    if (idx !== -1) { walkIndex = idx; }
    updateWalkButtons();
    // Replaying history (native Back/Forward), not making new history --
    // move visitPos to match, never push/truncate. Search outward from the
    // current position first (nearest match is almost always right, and
    // handles a ring or repeat-visited node appearing more than once in
    // visitStack) before falling back to any occurrence.
    var foundAt = -1;
    for (var d = 0; d < visitStack.length && foundAt === -1; d++) {
      if (visitStack[visitPos - d] === id) { foundAt = visitPos - d; }
      else if (visitStack[visitPos + d] === id) { foundAt = visitPos + d; }
    }
    if (foundAt !== -1) {
      visitPos = foundAt;
    } else {
      // Not found at all -- state lost (e.g. a reload) or evicted past the
      // depth cap earlier. Degrade visibly, not silently: reseed from just
      // this one node rather than showing a stale/wrong stack, the same
      // "never break, degrade visibly" posture the pushState try/catch
      // above already takes.
      visitStack = [id];
      visitPos = 0;
      visitDropped = 0;
    }
    renderHistoryPanel();
  });
  // Regression found by this project's Layer 2 browser suite (kb/adrs/
  //0001): Home previously only called selectNode(anchorId), never
  // resetting walkIndex. readingOrder[0] is always the anchor, so after
  // walking forward to some position N via Next, clicking Home visually
  // returns to the anchor -- but a subsequent Next click resumed from the
  // stale walkIndex (N + 1), not from position 1 (the real "next after
  // home"), landing on an unexpected node with no visible sign anything
  // was wrong. Home is conceptually "jump to position 0," so it must
  // reset walkIndex the same way the popstate handler above resyncs it
  // for Back/Forward.
  homeBtn.addEventListener("click", function () {
    selectNode(anchorId);
    walkIndex = anchorWalkIndex();
    updateWalkButtons();
  });

  // --- Reading order: an explicit, authored Previous/Next chain when the
  // source KB has one (a project-local org convention -- see
  // parse_reading_order's Rust doc comment; mae_kb has no first-class
  // concept of this), falling back to BFS-distance-from-anchor + degree +
  // alphabetical tiebreak for any node that isn't part of one. Chain-linked
  // nodes come first, in chain order; everything else is appended after. ---
  function computeReadingOrder() {
    // kb/adrs/0004: guidance/colophon nodes never enter the Previous/Next
    // walk -- topicNodes, not nodes, both seeds `dist` (so a guidance node
    // is never a possible destination) and produces the final order.
    var topicIds = {};
    topicNodes.forEach(function (n) { topicIds[n.id] = true; });
    function validPrev(n) { return n.reading_order_prev && topicIds[n.reading_order_prev] ? n.reading_order_prev : null; }
    function validNext(n) { return n.reading_order_next && topicIds[n.reading_order_next] ? n.reading_order_next : null; }

    var visited = {};
    var order = [];
    topicNodes.forEach(function (n) {
      if (visited[n.id] || (!validPrev(n) && !validNext(n))) { return; }
      // Walk backward to this chain segment's start (guarded: real,
      // user-authored data, not a guaranteed-acyclic machine format).
      var startId = n.id, guard = 0;
      while (true) {
        var p = validPrev(nodesById[startId]);
        if (!p || visited[p] || ++guard > topicNodes.length) { break; }
        startId = p;
      }
      // Then forward from the start, collecting the whole segment once.
      var cur = startId, guard2 = 0;
      while (cur && !visited[cur] && guard2++ <= topicNodes.length) {
        visited[cur] = true;
        order.push(cur);
        cur = validNext(nodesById[cur]);
      }
    });

    var adjacency = {};
    topicNodes.forEach(function (n) { adjacency[n.id] = []; });
    edges.forEach(function (e) {
      if (adjacency[e.source]) { adjacency[e.source].push(e.target); }
      if (adjacency[e.target]) { adjacency[e.target].push(e.source); }
    });
    var dist = {};
    topicNodes.forEach(function (n) { dist[n.id] = Infinity; });
    if (dist[anchorId] !== undefined) {
      dist[anchorId] = 0;
      var queue = [anchorId];
      while (queue.length) {
        var cur2 = queue.shift();
        (adjacency[cur2] || []).forEach(function (next) {
          if (dist[next] === Infinity) { dist[next] = dist[cur2] + 1; queue.push(next); }
        });
      }
    }
    var rest = topicNodes.filter(function (n) { return !visited[n.id]; }).sort(function (a, b) {
      if (dist[a.id] !== dist[b.id]) { return dist[a.id] - dist[b.id]; }
      var degA = degreeOf(a.id), degB = degreeOf(b.id);
      if (degA !== degB) { return degB - degA; }
      return a.id < b.id ? -1 : (a.id > b.id ? 1 : 0);
    });
    return {
      ids: order.concat(rest.map(function (n) { return n.id; })),
      // Which ids were actually chain-walked (`visited`, reused directly --
      // a real KB can have MORE THAN ONE independent authored chain, e.g.
      // gitlab-migration's own main project-wide sequence PLUS a separate
      // local one inside gitlab-platform/gitlab-host's own ADRs; both get
      // walked and concatenated into `order`, so a single "chain ends at
      // index N" boundary is wrong -- confirmed on that exact 167-node
      // export: it stopped Next one click too late, at the node just past
      // the MAIN chain's real end, because that node happened to belong to
      // the second chain). See updateWalkButtons below for how this is
      // used: per-node, not as a single prefix-length boundary.
      isChainNode: visited,
      topicIds: topicIds,
    };
  }
  // Previous/Next share one position in `readingOrder`, clamped (not
  // modulo-wrapped) at both ends -- Next stops at the last node instead of
  // silently wrapping back to the start, so the two controls behave like
  // ordinary pagination, each disabled exactly when it has nowhere to go.
  //
  // walkIndex starts at the ANCHOR's own position in readingOrder, not a
  // hardcoded 0: when the KB has an explicit authored chain, readingOrder
  // follows THAT order first (see computeReadingOrder above), and the
  // anchor -- whichever node the export was actually rooted at -- can
  // legitimately sit anywhere within it, not just at the start. The anchor
  // is already auto-selected on page load (see selectNode(anchorId) below),
  // so walkIndex must point at wherever it really landed for position 0 to
  // mean "what's already on screen" -- exactly the invariant that made
  // starting at a hardcoded 0 correct back when readingOrder[0] was always
  // the anchor by construction (pure BFS distance from itself is always
  // zero); that invariant no longer holds unconditionally, so this is
  // computed instead of assumed.
  var readingOrderResult = computeReadingOrder();
  var readingOrder = readingOrderResult.ids;
  var isChainNode = readingOrderResult.isChainNode;
  var readingOrderTopicIds = readingOrderResult.topicIds;
  function anchorWalkIndex() {
    var i = readingOrder.indexOf(anchorId);
    return i === -1 ? 0 : i;
  }
  var walkIndex = anchorWalkIndex();
  // Next stops at the authored chain's real end (a genuine "Next :: none"
  // boundary, per the KB's own Reading Order data) rather than spilling
  // into unrelated BFS-fallback content -- confirmed as a real, jarring UX
  // gap on a real 167-node export (walking off "README" straight into
  // unrelated roadmap/ADR material with no signal anything had changed).
  // Checked per-node, not via a single fixed boundary: a KB can have more
  // than one independent authored chain (that same export has a second,
  // separate one inside gitlab-platform/gitlab-host's own ADRs) -- Next
  // follows whichever chain the CURRENT node is actually on to ITS OWN
  // real end, only stopping there, rather than stopping at wherever the
  // FIRST-discovered chain happened to end. A node never on any chain
  // (isChainNode false) still gets ordinary end-of-list pagination,
  // unaffected -- BFS-fallback nodes remain reachable directly (chord
  // ring, search, colophon), just not via Next once the current chain
  // (or, for chain-less nodes, the whole list) is done.
  function atChainEnd() {
    var n = nodesById[readingOrder[walkIndex]];
    if (!n || !isChainNode[n.id]) { return false; }
    return !(n.reading_order_next && readingOrderTopicIds[n.reading_order_next]);
  }
  function updateWalkButtons() {
    prevBtn.disabled = walkIndex <= 0;
    var done = walkIndex >= readingOrder.length - 1 || atChainEnd();
    nextBtn.textContent = done ? "✓ Done" : "Next →";
    nextBtn.disabled = done;
  }
  nextBtn.addEventListener("click", function () {
    walkIndex = Math.min(walkIndex + 1, readingOrder.length - 1);
    selectNode(readingOrder[walkIndex]);
    updateWalkButtons();
  });
  prevBtn.addEventListener("click", function () {
    if (walkIndex <= 0) { return; }
    walkIndex -= 1;
    selectNode(readingOrder[walkIndex]);
    updateWalkButtons();
  });
  updateWalkButtons();

  // --- Header search: hand-rolled subsequence fuzzy match (no external
  // lib -- this page ships zero dependencies) against each topic node's
  // CURRENT-language title. A match requires every query character to
  // appear in target order; score rewards consecutive-character runs and
  // word-start matches so "gitlab ci" beats a scattered same-length match.
  // A single, distinct effect (jump-to via a dropdown) -- deliberately not
  // entangled with the tag filter's chord-ring dimming below. ---
  function fuzzyScore(query, target) {
    if (!query) { return null; }
    var q = query.toLowerCase(), t = target.toLowerCase();
    var qi = 0, score = 0, consecutive = 0;
    for (var ti = 0; ti < t.length && qi < q.length; ti++) {
      if (t[ti] === q[qi]) {
        consecutive++;
        score += 1 + consecutive;
        if (ti === 0 || /[\s\-_/]/.test(t[ti - 1])) { score += 3; }
        qi++;
      } else {
        consecutive = 0;
      }
    }
    return qi === q.length ? score : null;
  }
  function renderSearchResults(query) {
    searchResults.textContent = "";
    if (!query) { searchResults.hidden = true; return; }
    var scored = topicNodes.map(function (n) {
      return { node: n, score: fuzzyScore(query, n["title_" + currentLang]) };
    }).filter(function (x) { return x.score !== null; })
      .sort(function (a, b) { return b.score - a.score; })
      .slice(0, 8);
    if (scored.length === 0) { searchResults.hidden = true; return; }
    scored.forEach(function (x) {
      var btn = dom("button", { type: "button" }, x.node["title_" + currentLang]);
      btn.addEventListener("click", function () {
        selectNode(x.node.id);
        nodeSearch.value = "";
        searchResults.hidden = true;
      });
      searchResults.appendChild(btn);
    });
    searchResults.hidden = false;
  }
  nodeSearch.addEventListener("input", function () {
    renderSearchResults(nodeSearch.value.trim());
  });
  nodeSearch.addEventListener("keydown", function (ev) {
    if (ev.key === "Escape") { searchResults.hidden = true; nodeSearch.blur(); }
  });
  // Delayed hide on blur (not immediate) -- a click on a result row blurs
  // the input just before its own click handler would otherwise fire;
  // hiding synchronously on blur would remove the button from the DOM
  // before that click registers.
  nodeSearch.addEventListener("blur", function () {
    window.setTimeout(function () { searchResults.hidden = true; }, 150);
  });

  // --- Header tag filter: dims (never removes) non-matching nodes/edges
  // in the chord ring -- the graph itself becomes the filtered view, no
  // separate list. OR semantics across active tags (the standard choice
  // for one flat facet -- AND would too easily produce an empty result on
  // sparse tag combinations). Guidance nodes are already excluded from
  // topicNodes (kb/adrs/0004), so they're never part of this either. ---
  var allTags = [];
  (function () {
    var seen = {};
    topicNodes.forEach(function (n) {
      (n.tags || []).forEach(function (t) {
        if (!seen[t]) { seen[t] = true; allTags.push(t); }
      });
    });
    allTags.sort();
  })();
  if (allTags.length === 0) { tagFilterGroup.hidden = true; }
  var activeTagFilters = {};
  function nodeMatchesTagFilter(n) {
    var active = Object.keys(activeTagFilters);
    if (active.length === 0) { return true; }
    return (n.tags || []).some(function (t) { return activeTagFilters[t]; });
  }
  function applyTagFilter() {
    nodes.forEach(function (n, i) {
      var g = nodeGroups[i];
      if (!g) { return; }
      g.classList.toggle("filtered-out", !nodeMatchesTagFilter(n));
    });
    edgePaths.forEach(function (p) {
      var s = nodesById[p.getAttribute("data-source")];
      var t = nodesById[p.getAttribute("data-target")];
      var bothMatch = s && t && nodeMatchesTagFilter(s) && nodeMatchesTagFilter(t);
      p.classList.toggle("filtered-out", !bothMatch);
    });
  }
  function renderTagPicker() {
    tagPicker.textContent = "";
    allTags.forEach(function (t) {
      var btn = dom("button", { type: "button" }, t);
      if (activeTagFilters[t]) { btn.classList.add("active"); }
      btn.addEventListener("click", function () { toggleTagFilter(t); });
      tagPicker.appendChild(btn);
    });
  }
  function renderActiveTagChips() {
    activeTagChips.textContent = "";
    Object.keys(activeTagFilters).forEach(function (t) {
      var btn = dom("button", { type: "button" }, t + " ×");
      btn.addEventListener("click", function () { toggleTagFilter(t); });
      activeTagChips.appendChild(btn);
    });
  }
  function toggleTagFilter(t) {
    if (activeTagFilters[t]) { delete activeTagFilters[t]; } else { activeTagFilters[t] = true; }
    renderTagPicker();
    renderActiveTagChips();
    applyTagFilter();
  }
  tagPickerToggle.addEventListener("click", function () {
    tagPicker.hidden = !tagPicker.hidden;
  });
  document.addEventListener("click", function (ev) {
    if (!tagFilterGroup.contains(ev.target)) { tagPicker.hidden = true; }
  });
  renderTagPicker();
  renderActiveTagChips();
  applyTagFilter();

  // --- Chord diagram fullscreen: an in-page expand, not the native
  // browser Fullscreen API (requestFullscreen()) -- that API needs a
  // user-activation gesture with quirky cross-engine/file:// behavior and
  // hands over the WHOLE SCREEN (including the OS chrome disappearing),
  // which is more than this needs. A `position: fixed` overlay gives the
  // same "big, easy-to-read ring" result while staying simple and
  // reliably testable. #graph-svg's own preserveAspectRatio="xMidYMid
  // meet" already scales the existing viewBox to fill whatever size its
  // container becomes -- no viewBox/geometry recompute needed here, the
  // ring just renders bigger (a real usability win: larger hit targets)
  // once its container grows.
  //
  // Clicking a node while fullscreen still just calls selectNode() (the
  // per-node click listener is unchanged) and does NOT auto-exit --
  // exploring several nodes at the enlarged size is the whole point;
  // making every click bounce back to the small view would defeat it. An
  // explicit toggle (this same button, now showing X) or Escape is the
  // only way out.
  //
  // Enter/exit both use a CSS @keyframes animation (STATIC_CSS,
  // graph-fullscreen-in/out) rather than a transition: `position: fixed`
  // itself isn't an animatable property, and the pane's opacity/transform
  // don't otherwise change between its normal and fullscreen layouts, so
  // there's no "before" state for a transition to interpolate from --
  // this file's small-motion convention (200ms ease) needs a real
  // self-contained animation here instead, not just a transition.
  var isGraphFullscreen = false;
  function setGraphFullscreen(next) {
    if (next === isGraphFullscreen) { return; }
    isGraphFullscreen = next;
    graphPane.classList.remove("fullscreen-anim-in", "fullscreen-anim-out");
    if (next) {
      graphPane.classList.add("fullscreen", "fullscreen-anim-in");
    } else {
      // Stays positioned fullscreen (`.fullscreen` not removed yet) while
      // the shrink-out animation plays, then drops out of fixed
      // positioning once it's actually finished -- removing `.fullscreen`
      // immediately would snap it back to the sidebar's small layout
      // before the animation had anything to animate.
      graphPane.classList.add("fullscreen-anim-out");
    }
    fullscreenToggle.textContent = next ? "✕" : "⛶";
    var label = next ? "Exit fullscreen" : "Expand diagram";
    fullscreenToggle.title = label;
    fullscreenToggle.setAttribute("aria-label", label);
  }
  graphPane.addEventListener("animationend", function (ev) {
    if (ev.animationName === "graph-fullscreen-out") {
      graphPane.classList.remove("fullscreen", "fullscreen-anim-out");
    }
  });
  fullscreenToggle.addEventListener("click", function () {
    setGraphFullscreen(!isGraphFullscreen);
  });
  // The Escape handler for THIS overlay is merged with the sidebar
  // drawer's below into one listener (search "ev.key === \"Escape\"") --
  // two separate `keydown` listeners each reading isGraphFullscreen raced
  // against each other: this one flips it to false, then the sidebar's
  // listener (registered second, so it runs second on the SAME keydown
  // event) reads the ALREADY-flipped value and wrongly concludes nothing
  // was fullscreen, closing the drawer too on the very same press. A
  // single listener that checks fullscreen first and returns early is the
  // only way to peel back one overlay per press.

  // --- #sidebar-toggle: one shared boolean drives BOTH the desktop
  // collapse (instant, #main-content reclaims the width) and the mobile
  // drawer (an off-canvas overlay, same fixed-position-overlay pattern as
  // #graph-pane's own fullscreen above) -- see the STATIC_CSS comment
  // above #sidebar for why a single `data-sidebar` attribute on <html>,
  // not a class, is what makes one control correct at both breakpoints.
  var SIDEBAR_MOBILE_QUERY = "(max-width: 767px)";
  function sidebarIsOpen() {
    var explicit = document.documentElement.getAttribute("data-sidebar");
    if (explicit === "open") { return true; }
    if (explicit === "closed") { return false; }
    // Nothing explicit yet: the plain per-breakpoint CSS default applies
    // (open on desktop, closed on mobile) -- matches sidebarIsOpen()'s own
    // read exactly, so the button's label/aria stay in sync even before
    // any click or stored preference exists.
    return !(window.matchMedia && window.matchMedia(SIDEBAR_MOBILE_QUERY).matches);
  }
  function updateSidebarToggleLabel(open) {
    var label = open ? "Hide sidebar" : "Show sidebar";
    sidebarToggle.textContent = "☰ " + label;
    sidebarToggle.setAttribute("aria-expanded", open ? "true" : "false");
  }
  function setSidebarOpen(open) {
    // Only the mobile drawer, closing from an already-EXPLICITLY-open
    // state, needs the two-phase animate-then-flip dance -- e.g. on
    // initial load with a stored "closed" preference, data-sidebar was
    // never explicitly "open" (nothing was visibly showing), so there's
    // nothing to animate away from; flip straight to closed.
    var wasExplicitlyOpen = document.documentElement.getAttribute("data-sidebar") === "open";
    if (open) {
      document.documentElement.removeAttribute("data-sidebar-anim");
      document.documentElement.setAttribute("data-sidebar", "open");
    } else if (wasExplicitlyOpen && window.matchMedia && window.matchMedia(SIDEBAR_MOBILE_QUERY).matches) {
      // Mobile close: keep data-sidebar="open" (so the fixed/inset:0
      // positioning stays) while the slide-out plays, then the
      // animationend listener below flips it to "closed" -- same
      // two-phase approach as setGraphFullscreen's fullscreen-anim-out.
      document.documentElement.setAttribute("data-sidebar-anim", "out");
    } else {
      // Desktop close has no animation (STATIC_CSS's min-width:768px
      // rule is a plain instant display:none) -- flip immediately.
      document.documentElement.setAttribute("data-sidebar", "closed");
    }
    updateSidebarToggleLabel(open);
    try {
      localStorage.setItem("mae-guide-sidebar-collapsed", open ? "false" : "true");
    } catch (e) { /* ignore */ }
  }
  sidebarEl.addEventListener("animationend", function (ev) {
    if (ev.animationName === "sidebar-drawer-out") {
      document.documentElement.setAttribute("data-sidebar", "closed");
      document.documentElement.removeAttribute("data-sidebar-anim");
    }
  });
  sidebarToggle.addEventListener("click", function () {
    setSidebarOpen(!sidebarIsOpen());
  });
  sidebarBackdrop.addEventListener("click", function () {
    setSidebarOpen(false);
  });
  // One merged Escape handler for BOTH overlays (see the comment above
  // fullscreenToggle's click listener for why splitting this across two
  // `keydown` listeners is a real, caught bug, not just a style choice):
  // fullscreen is checked FIRST and returns immediately, so a single
  // Escape press with both overlays open closes only the topmost one
  // (fullscreen); a second press is needed to close the drawer.
  document.addEventListener("keydown", function (ev) {
    if (ev.key !== "Escape") { return; }
    if (isGraphFullscreen) {
      setGraphFullscreen(false);
      return;
    }
    if (sidebarIsOpen() && window.matchMedia && window.matchMedia(SIDEBAR_MOBILE_QUERY).matches) {
      setSidebarOpen(false);
    }
  });
  // Sync the button's label/aria to whatever's in effect on first paint --
  // a stored preference (read alongside the theme preference below) may
  // still call setSidebarOpen() again after this, which is fine
  // (idempotent for the label/aria, and re-applying "open"/"closed" is a
  // no-op if it's already that value).
  updateSidebarToggleLabel(sidebarIsOpen());

  // --- Colophon (kb/adrs/0004): each button opens its guidance node via
  // the SAME selectNode() real navigation (chord nodes, in-body links, and
  // Home/Previous/Next all already funnel through it) -- language toggle,
  // ADR-0003's translation-fallback notice, mermaid, Back/Forward history,
  // all just work with no separate code path to keep in sync. ---
  var colophonLinks = document.querySelectorAll(".colophon-link");
  Array.prototype.forEach.call(colophonLinks, function (btn) {
    btn.addEventListener("click", function () {
      selectNode(btn.getAttribute("data-node-id"));
    });
  });

  // --- EN/ES toggle: swaps all visible text in place, instantly ---
  function applyLanguage() {
    updateCaption(selectedId != null ? nodesById[selectedId] : null);
    if (selectedId != null) { renderDetail(nodesById[selectedId]); }
    topicNodes.forEach(function (tn) {
      var tg = groupFor(tn.id);
      if (tg) { tg.setAttribute("aria-label", tn["title_" + currentLang]); }
    });
    langToggle.textContent = currentLang === "en" ? "EN / ES → ES" : "ES / EN → EN";
    Array.prototype.forEach.call(colophonLinks, function (btn) {
      btn.textContent = btn.getAttribute("data-title-" + currentLang);
    });
    renderHistoryPanel();
  }
  langToggle.addEventListener("click", function () {
    currentLang = currentLang === "en" ? "es" : "en";
    applyLanguage();
  });

  // --- Dark/light theme toggle: overrides prefers-color-scheme via
  // documentElement[data-theme], which CSS already defines at matching
  // specificity (render_css_variables) -- background/color/fill/stroke
  // all carry a 180-200ms transition, so this reads as a smooth cross-
  // fade rather than a snap.
  //
  // The chosen theme persists across reopening this same exported file
  // via localStorage (file:// origins persist it per-path in Chromium/
  // Firefox, which matches the real use case here -- no server needed).
  // Reads/writes are wrapped in try/catch: some browser privacy modes
  // throw on localStorage access rather than just returning null, and a
  // reader's theme preference isn't worth a page-load error over. A
  // stored preference needs data-theme set explicitly on load (not just
  // inside the click handler, which is all that existed before) -- the
  // page otherwise relies purely on the prefers-color-scheme media query
  // until the first click, which would silently ignore anything stored. ---
  var themeOrder = ["dark", "light"];
  var storedTheme = null;
  try { storedTheme = localStorage.getItem("mae-guide-theme"); } catch (e) { /* ignore */ }
  var themeIdx = themeOrder.indexOf(storedTheme);
  if (themeIdx === -1) {
    themeIdx = (window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches) ? 1 : 0;
  } else {
    document.documentElement.setAttribute("data-theme", themeOrder[themeIdx]);
  }
  themeToggle.addEventListener("click", function () {
    themeIdx = (themeIdx + 1) % themeOrder.length;
    document.documentElement.setAttribute("data-theme", themeOrder[themeIdx]);
    try { localStorage.setItem("mae-guide-theme", themeOrder[themeIdx]); } catch (e) { /* ignore */ }
  });

  // Only call setSidebarOpen() if a preference was actually stored --
  // leaving data-sidebar unset otherwise means the plain per-breakpoint
  // CSS default (open on desktop, closed on mobile) applies with zero
  // flash for a first-ever visit. A returning visitor with an explicit
  // stored value still gets a brief flash-then-correct, same tradeoff the
  // theme preference above already accepts (this script runs at the end
  // of <body>, not a synchronous anti-FOUC <head> script).
  var storedSidebarCollapsed = null;
  try { storedSidebarCollapsed = localStorage.getItem("mae-guide-sidebar-collapsed"); } catch (e) { /* ignore */ }
  if (storedSidebarCollapsed === "true") {
    setSidebarOpen(false);
  } else if (storedSidebarCollapsed === "false") {
    setSidebarOpen(true);
  }

  applyLanguage();
  // Resume the reader's last-open node (user request), falling back to
  // the anchor/spine node the same way this always worked before --
  // matching "Home" as a real default rather than an empty-state page
  // when there's no stored node, or it no longer exists in this export
  // (e.g. a stale value from a differently-scoped export at the same
  // path, or a node that was pruned). Uses replaceState, not selectNode's
  // pushState, so the page's very first load establishes the starting
  // history entry instead of creating a second one under it -- Back from
  // the first real navigation should leave the page, not land on an
  // invisible duplicate of itself.
  var storedLastNode = null;
  try { storedLastNode = localStorage.getItem("mae-guide-last-node"); } catch (e) { /* ignore */ }
  var initialNodeId = (storedLastNode && nodesById[storedLastNode]) ? storedLastNode : anchorId;
  applySelection(initialNodeId);
  // Keep Previous/Next's position consistent with wherever the restored
  // node actually lands in the reading order -- same resync the popstate
  // handler above already does for Back/Forward, needed here for the same
  // reason: walkIndex otherwise defaults to the anchor's own position
  // regardless of what was actually just selected.
  var initialWalkIdx = readingOrder.indexOf(initialNodeId);
  if (initialWalkIdx !== -1) { walkIndex = initialWalkIdx; }
  updateWalkButtons();
  try {
    history.replaceState({ nodeId: initialNodeId }, "", '#' + initialNodeId);
  } catch (e) { /* ignore -- see the try/catch in selectNode() above */ }
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> GruvboxPalette {
        GruvboxPalette::dark()
    }

    fn simple_node(id: &str, title: &str, body: &str, is_anchor: bool) -> GraphExportNode {
        build_export_node(
            id,
            "note",
            0.0,
            0.0,
            false,
            is_anchor,
            title,
            body,
            None,
            &palette(),
        )
    }

    // --- Translation loading ---

    #[test]
    fn load_translations_parses_valid_file() {
        let dir = std::env::temp_dir().join(format!("mae-html-graph-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("translations.json");
        std::fs::write(
            &path,
            r#"{"node-a": {"title_es": "Título", "body_es": "Cuerpo"}}"#,
        )
        .unwrap();
        let map = load_translations(&path).unwrap();
        assert_eq!(map["node-a"].title_es.as_deref(), Some("Título"));
        assert_eq!(map["node-a"].body_es.as_deref(), Some("Cuerpo"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_translations_fails_clearly_on_missing_file() {
        let path = std::path::Path::new("/nonexistent/path/translations.json");
        let err = load_translations(path).unwrap_err();
        assert!(err.contains("translations file"), "{err}");
        assert!(err.contains("nonexistent"), "{err}");
    }

    #[test]
    fn load_translations_fails_clearly_on_malformed_json() {
        let dir =
            std::env::temp_dir().join(format!("mae-html-graph-test-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("translations.json");
        std::fs::write(&path, "not json at all").unwrap();
        let err = load_translations(&path).unwrap_err();
        assert!(err.contains("valid JSON"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- Bilingual mirroring ---

    #[test]
    fn node_without_translation_mirrors_en_into_es() {
        let n = simple_node("a", "Title", "Body text.", false);
        assert_eq!(n.title_es, n.title_en);
        assert_eq!(n.body_es, n.body_en);
        assert_eq!(n.preview_es, n.preview_en);
    }

    #[test]
    fn node_with_translation_carries_es_fields() {
        let t = NodeTranslation {
            title_es: Some("Título ES".to_string()),
            body_es: Some("Cuerpo ES.".to_string()),
        };
        let n = build_export_node(
            "a",
            "note",
            0.0,
            0.0,
            false,
            false,
            "Title EN",
            "Body EN.",
            Some(&t),
            &palette(),
        );
        assert_eq!(n.title_es, "Título ES");
        assert!(n.body_es.contains("Cuerpo ES"));
        assert_ne!(n.body_es, n.body_en);
    }

    // --- kb/adrs/0003: untranslated-node fallback gets an explicit UI signal ---

    #[test]
    fn fallback_notice_logic_is_present_and_per_field() {
        // Regression guard for the bug that triggered this project's
        // extraction (kb/adrs/0001, kb/adrs/0003): toggling the language on
        // a node with no real Spanish translation previously flipped
        // currentLang and the toggle button's own label with zero visible
        // change to the content, which read as "the switch is broken."
        // Guards that the fix -- a per-field fallback check, not just a
        // per-node one, so a *partial* translation doesn't silently read
        // as complete either -- is present in the generated JS.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("if (currentLang === \"es\") {"));
        assert!(html.contains("var titleFallback = n.title_es === n.title_en;"));
        assert!(html.contains("var bodyFallback = n.body_es === n.body_en;"));
        assert!(html.contains("translation-fallback-note"));
        // Must not fire when currentLang is "en": showing English while in
        // English mode is normal, not a fallback -- the notice text itself
        // should only ever be appended inside the es branch.
        let es_branch = html
            .split("if (currentLang === \"es\") {")
            .nth(1)
            .and_then(|s| s.split("var body = dom(\"div\"").next())
            .expect("es-only fallback branch present");
        assert!(es_branch.contains("isn't translated yet"));
    }

    #[test]
    fn empty_string_translation_is_treated_as_a_real_translation_not_a_fallback() {
        // Adversarial edge case per kb/adrs/0003's "per-field, not just
        // per-node" fix: a NodeTranslation with title_es/body_es explicitly
        // present but set to "" is a different case from no translation at
        // all (where they're absent and mirror EN). An empty string is not
        // equal to a real EN title/body, so the client-side `=== ` fallback
        // check correctly does NOT treat it as a fallback -- this is
        // surprising enough (an empty title is a real, if unhelpful,
        // "translation") that it deserves an explicit, named test rather
        // than being left as accidental behavior nobody decided on.
        let t = NodeTranslation {
            title_es: Some(String::new()),
            body_es: Some(String::new()),
        };
        let n = build_export_node(
            "a",
            "note",
            0.0,
            0.0,
            false,
            false,
            "Title EN",
            "Body EN.",
            Some(&t),
            &palette(),
        );
        assert_eq!(n.title_es, "");
        assert_ne!(
            n.title_es, n.title_en,
            "an explicit empty-string translation must not equal the real EN title, \
             so the client-side fallback check (title_es === title_en) doesn't \
             mistake it for a missing translation"
        );
    }

    #[test]
    fn partial_translation_title_only_leaves_body_as_a_real_fallback() {
        // Adversarial edge case: a translation with only title_es set (no
        // body_es) must not silently mix a translated title with an
        // untranslated body with no signal -- body_es should fall back to
        // mirroring body_en (existing behavior), and the fallback-detection
        // JS's per-field check (not per-node) is what makes that visible.
        let t = NodeTranslation {
            title_es: Some("Título ES".to_string()),
            body_es: None,
        };
        let n = build_export_node(
            "a",
            "note",
            0.0,
            0.0,
            false,
            false,
            "Title EN",
            "Body EN.",
            Some(&t),
            &palette(),
        );
        assert_eq!(n.title_es, "Título ES");
        assert_ne!(n.title_es, n.title_en);
        assert_eq!(
            n.body_es, n.body_en,
            "missing body_es must mirror body_en exactly, so the client-side \
             bodyFallback === check fires for this node"
        );
    }

    #[test]
    fn adversarial_translation_json_cannot_break_out_of_script_or_style_tags() {
        // Extends the existing adversarial_title_and_body_cannot_break_out_
        // of_script_or_style_tags coverage to the translation path
        // specifically, which that test doesn't exercise at all -- a
        // translation is user-controlled data (a separate JSON file, not
        // the KB content itself) and deserves the same adversarial
        // treatment, not an exemption because it's a secondary input.
        let t = NodeTranslation {
            title_es: Some("</script><script>alert(1)</script>".to_string()),
            body_es: Some("</style><style>body{display:none}</style>".to_string()),
        };
        let nodes = vec![build_export_node(
            "a",
            "note",
            0.0,
            0.0,
            false,
            true,
            "Title EN",
            "Body EN.",
            Some(&t),
            &palette(),
        )];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(!html.contains("</script><script>alert(1)</script>"));
        assert!(!html.contains("</style><style>body{display:none}</style>"));
    }

    // --- Mermaid fallback (pure, no subprocess) ---

    #[test]
    fn mermaid_fallback_escapes_source_and_reason() {
        let html = mermaid_fallback_html("graph TD; A-->B;", "npx not found <script>");
        assert!(html.contains("graph TD; A--&gt;B;"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
    }

    // --- plain_text_preview ---

    #[test]
    fn plain_text_preview_strips_markup_and_truncates() {
        let body = "A *bold* paragraph with /italic/ text. ".repeat(20);
        let preview = plain_text_preview(&body, 50);
        assert!(preview.chars().count() <= 51); // 50 + ellipsis
        assert!(!preview.contains('*'));
        assert!(!preview.contains('/'));
    }

    #[test]
    fn plain_text_preview_includes_table_content() {
        // Regression: push_plain's match over OrgElement had no arm for
        // Table at all, so a node body dominated by a table (e.g. a
        // function-reference cheatsheet) got an empty or near-empty
        // hover-popover preview -- every cell's text was silently dropped.
        let body = "* Common functions\n\n\
                     | Function | Does |\n\
                     |----------+------|\n\
                     | length(x) | Number of elements |\n";
        let preview = plain_text_preview(body, 200);
        assert!(
            preview.contains("length(x)"),
            "expected table cell content in the preview, got: {preview:?}"
        );
        assert!(
            preview.contains("Number of elements"),
            "expected every cell's text, not just the first column, got: {preview:?}"
        );
    }

    #[test]
    fn plain_text_preview_resolves_links_instead_of_showing_raw_syntax() {
        // Regression: the popover showed raw "[[UUID|label]]" (mae_kb's
        // internal link storage form) or "[[id:UUID][label]]" (raw org-file
        // form) verbatim, brackets/pipe/target and all.
        let pipe_form = plain_text_preview(
            "See [[1bc667b2-1d9a-402e-a2ea-eab6fd7d81e3|state]] for detail.",
            200,
        );
        assert_eq!(pipe_form, "See state for detail.");

        let bracket_form = plain_text_preview(
            "See [[id:1bc667b2-1d9a-402e-a2ea-eab6fd7d81e3][state]] for detail.",
            200,
        );
        assert_eq!(bracket_form, "See state for detail.");

        let unlabeled = plain_text_preview("See [[1bc667b2-1d9a-402e-a2ea-eab6fd7d81e3]].", 200);
        assert_eq!(unlabeled, "See 1bc667b2-1d9a-402e-a2ea-eab6fd7d81e3.");
    }

    #[test]
    fn plain_text_preview_short_body_not_truncated() {
        let preview = plain_text_preview("Short body.", 200);
        assert_eq!(preview, "Short body.");
    }

    #[test]
    fn strip_leading_properties_drawer_survives_a_multibyte_char_at_the_500_byte_boundary() {
        // Real, reproducible panic hit exporting a genuine KB node
        // ("byte index 500 is not a char boundary; it is inside '—'"):
        // strip_leading_properties_drawer used to slice at
        // `body.len().min(500)`, a plain BYTE offset, with no check that
        // 500 actually lands on a char boundary. An em dash is 3 UTF-8
        // bytes -- placing one so it straddles byte offset 500 exactly
        // reproduces the crash unless the fix (walk back to the nearest
        // real char boundary) is in place. This must not panic, for any
        // body content, not just the one KB node that happened to trigger it.
        let padding = "a".repeat(498);
        let body = format!("{padding}— more text after the dash, well past the old 500-char bound");
        assert!(
            body.as_bytes().get(500).is_some(),
            "test body must be long enough to exercise the boundary"
        );
        assert!(
            !body.is_char_boundary(500),
            "test body must actually straddle byte 500 with a multi-byte char"
        );
        // The call itself not panicking IS the assertion -- also confirm
        // the em dash survived intact (not corrupted/truncated mid-byte).
        let result = strip_leading_properties_drawer(&body);
        assert!(
            result.contains('—'),
            "expected the multi-byte character to survive intact: {result}"
        );
    }

    #[test]
    fn strip_leading_properties_drawer_strips_a_drawer_preceded_by_real_prose() {
        // Real, reproducible leak from a genuine onprem-iac KB roadmap-step
        // node (tagged roadmap/phase): a plain LIST ITEM's own properties
        // drawer, which org only auto-hides when it immediately follows a
        // HEADLINE (see org-kb-to-pdf's identical gotcha for the LaTeX
        // pipeline) -- so the ingested body kept the item's own leading
        // prose line before the drawer, and the old "must be leading"
        // check let the whole :PROPERTIES:/:ID:/:END: block through as
        // visible text in the exported HTML.
        let body = "upgrade fails partway.\n:PROPERTIES:\n:ID: db87b07f-2f87-4f0d-b1dc-4f398313bf73\n:END:\nValidated by: cross-checked against the runbook.";
        let result = strip_leading_properties_drawer(body);
        assert!(
            !result.contains(":PROPERTIES:") && !result.contains(":END:"),
            "drawer must be stripped even when real prose precedes it: {result}"
        );
        assert!(
            result.contains("upgrade fails partway."),
            "the list item's own leading prose must survive, not just the drawer's removal: {result}"
        );
        assert!(
            result.contains("Validated by: cross-checked against the runbook."),
            "content after the drawer must survive: {result}"
        );
    }

    #[test]
    fn strip_leading_properties_drawer_leaves_a_midsentence_mention_alone() {
        // Adversarial guard on the fix above: ":PROPERTIES:" appearing
        // mid-line (not alone on its own line, per real org drawer syntax)
        // is prose ABOUT drawers, not an actual drawer -- must not be
        // mistaken for one and spliced out.
        let body = "This note explains what a :PROPERTIES: drawer is and how :END: closes it.";
        let result = strip_leading_properties_drawer(body);
        assert_eq!(result, body, "mid-sentence mention must be left untouched");
    }

    // --- HtmlGraphExporter::export: serialization / structure ---

    #[test]
    fn node_radius_is_computed_from_real_screen_scale_not_a_flat_svg_unit_floor() {
        // Regression: an earlier version floored node radius at a flat
        // world-space constant ("Math.max(12, ...)"), which only produces
        // a real 24px on-screen hit target by coincidence of the specific
        // node count/viewBox size it happened to be tested against.
        // Confirmed empirically this session (headless Chromium +
        // Puppeteer, not just this string check): a synthetic 50-node star
        // rendered circles as small as ~3px on-screen radius under the old
        // flat-floor code, vs. a real ~12px floor after this fix, at both
        // 14 nodes (this repo's real guide) and 50 (synthetic). This test
        // only confirms the fix's code path is present in every export
        // (getBoundingClientRect-derived scale, not a bare constant) --
        // the actual on-screen-pixel behavior needs a real browser to
        // verify and was checked that way, not by this test alone.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("getBoundingClientRect().width"));
        assert!(html.contains("minWorldRadius"));
        assert!(html.contains("worldToScreenScale"));
        // The actual thickness expression must reference the computed
        // floor, not fall back to a bare constant (a doc comment above
        // legitimately mentions the old "Math.max(12, ...)" pattern for
        // context, so this checks the real assignment specifically rather
        // than the whole page for that substring). Nodes render as arc-
        // slice wedges, not circles (kb/adrs/00XX) -- `halfThickness`
        // replaced the old `r` (circle radius). Deliberately uniform (no
        // anchor/degree bonus) -- see that assignment's own comment for
        // why a per-node bonus here caused a real bulging/overlapping
        // look on a dense real-world export.
        assert!(html.contains("var halfThickness = minWorldRadius;"));
    }

    #[test]
    fn chord_ring_has_no_in_svg_label_overflow_padding() {
        // Earlier versions reserved viewBox width for in-SVG node-label
        // text (first one-sided, later symmetric) -- either way, dead
        // padding around a widget that's already small on screen. Node
        // titles now show in #graph-caption below/above the ring instead
        // (see graph_caption_shows_hovered_then_falls_back_to_selected),
        // so the viewBox only needs to fit the node wedges themselves.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("var viewBoxW = w + pad * 2;"),
            "expected the viewBox to budget only wedge padding, no labelPad term"
        );
        assert!(
            !html.contains("labelPad"),
            "labelPad should be fully removed now that labels don't render in the SVG"
        );
        assert!(
            !html.contains("el(\"text\""),
            "node titles should no longer be constructed as in-SVG <text> elements at all"
        );
    }

    #[test]
    fn graph_caption_shows_hovered_then_falls_back_to_selected() {
        // Regression: node titles used to render as tiny (11px) in-SVG
        // text next to each circle. Moved to a real-sized caption element
        // instead -- this guards both that the caption exists and that its
        // update function falls back to the *selected* node (not blank) on
        // mouseleave, so the caption always reads as "what's on screen"
        // rather than flickering empty between hovers.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("id=\"graph-caption\""));
        assert!(html.contains("function updateCaption(n)"));
        assert!(
            html.contains("updateCaption(selectedId != null ? nodesById[selectedId] : null);"),
            "expected onHover's mouseleave path to fall back to the selected node, not clear the caption"
        );
    }

    #[test]
    fn export_produces_well_formed_standalone_html() {
        let nodes = vec![
            simple_node("a", "Node A", "Body A.", true),
            simple_node("b", "Node B", "Body B, see [[id:a][A]].", false),
        ];
        let edges = vec![GraphExportEdge {
            source: "a".to_string(),
            target: "b".to_string(),
            rel_type: "explains".to_string(),
            weight: 1.0,
        }];
        let html = HtmlGraphExporter.export(&nodes, &edges, "a", "Test Subgraph");

        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.trim_end().ends_with("</html>"));
        assert!(html.contains("<title>Test Subgraph</title>"));
        assert!(html.contains("id=\"graph-data\""));
        // Both nodes' ids appear in the embedded JSON payload.
        assert!(html.contains("\"id\":\"a\""));
        assert!(html.contains("\"id\":\"b\""));
        // No external network requests of any kind.
        assert!(!html.contains("<script src="));
        // The SVG namespace URI legitimately contains "http://" — that's
        // not a fetch, just an XML namespace string, so only the actual
        // network-fetch shapes are disallowed.
        assert!(!html.contains("<script src=\"http"));
        assert!(!html.contains("<link rel=\"stylesheet\" href=\"http"));
        assert!(!html.contains("cdn."));
    }

    #[test]
    fn export_contains_no_external_script_or_style_references() {
        let nodes = vec![simple_node("a", "Node A", "Body.", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "Title");
        assert!(!html.contains("<script src=\"http"));
        assert!(!html.contains("<link "));
        assert!(!html.contains("<script src=\"https"));
    }

    // --- Real-content regressions: found by actually running the export
    // against a live KB (RoamNotes) rather than only against hand-built
    // fixtures, which never happened to exercise the mae_kb-normalized
    // `[[UUID|label]]` link form or a real properties drawer ---

    #[test]
    fn internal_pipe_style_links_render_as_clean_split_href_and_label() {
        // mae_kb's org parser canonicalizes every `[[id:UUID][label]]`
        // link into `[[UUID|label]]` before storage (see
        // `parse_org_link_str`'s doc comment) -- assert that form renders
        // as a real split link, not a garbled "UUID|label" dumped as both
        // href and visible text (the actual bug: RoamNotes-sourced bodies
        // rendered `<a href="1bc667b2-...|state">1bc667b2-...|state</a>`
        // instead of `<a href="#1bc667b2-...">state</a>`). Uses
        // `build_export_node` directly (its `.body_en` is plain HTML, not
        // JSON-string-escaped the way a full page export's embedded
        // payload is) so the assertion reads the real markup directly.
        let n = build_export_node(
            "a",
            "note",
            0.0,
            0.0,
            true,
            true,
            "A",
            "See [[1bc667b2-1d9a-402e-a2ea-eab6fd7d81e3|state]] here.",
            None,
            &palette(),
        );
        assert_eq!(
            n.body_en,
            "<p>See <a href=\"#1bc667b2-1d9a-402e-a2ea-eab6fd7d81e3\">state</a> here.</p>\n"
        );
    }

    #[test]
    fn raw_org_file_two_bracket_links_still_render_correctly() {
        // Non-regression: the OTHER exporter (`html.rs`'s `HtmlExporter`)
        // feeds this same shared parser genuine raw org-FILE text, where
        // `id:` prefixes are still present and `][` (not `|`) is the
        // label separator -- confirm that form is unaffected.
        let n = build_export_node(
            "a",
            "note",
            0.0,
            0.0,
            true,
            true,
            "A",
            "See [[id:1bc667b2-1d9a-402e-a2ea-eab6fd7d81e3][state]] here.",
            None,
            &palette(),
        );
        assert_eq!(
            n.body_en,
            "<p>See <a href=\"#1bc667b2-1d9a-402e-a2ea-eab6fd7d81e3\">state</a> here.</p>\n"
        );
    }

    #[test]
    fn external_links_keep_their_real_url_as_href() {
        let n = build_export_node(
            "a",
            "note",
            0.0,
            0.0,
            true,
            true,
            "A",
            "See [[https://example.com/x|the docs]] here.",
            None,
            &palette(),
        );
        assert_eq!(
            n.body_en,
            "<p>See <a href=\"https://example.com/x\">the docs</a> here.</p>\n"
        );
    }

    #[test]
    fn leading_properties_drawer_never_appears_in_rendered_body_or_preview() {
        // The actual bug: every RoamNotes-sourced node's rendered body
        // opened with its own raw `:PROPERTIES: :ID: ... :hash: ... :END:`
        // drawer dumped as visible prose, because `parse_org_document`
        // (written for org BODY content) has no concept of a drawer.
        let body = ":PROPERTIES:\n:ID:       a\n:hash:     deadbeef\n:END:\nReal content here.";
        let n = build_export_node(
            "a",
            "note",
            0.0,
            0.0,
            true,
            true,
            "A",
            body,
            None,
            &palette(),
        );
        assert!(
            !n.body_en.contains(":PROPERTIES:"),
            "properties drawer leaked into the exported page: {}",
            n.body_en
        );
        assert!(!n.body_en.contains(":hash:"));
        assert!(n.body_en.contains("Real content here."));
        assert!(!n.preview_en.contains("PROPERTIES"));
        assert!(n.preview_en.contains("Real content here."));
    }

    // --- Chord nav widget additions: accent palette, home/outline/theme
    // controls, arc (not straight-line) edges ---

    #[test]
    fn dark_and_light_accent_are_the_real_gruvbox_bright_orange_and_orange() {
        // Independently re-derivable (see GruvboxPalette::accent's doc
        // comment): WCAG contrast against each theme's own bg0 is
        // 5.84:1 (dark) / 3.41:1 (light) -- both above the 3:1 UI-
        // component floor, and both literal values in gruvbox's own
        // upstream palette (bright_orange / orange), not hand-picked.
        assert_eq!(GruvboxPalette::dark().accent, "#fe8019");
        assert_eq!(GruvboxPalette::light().accent, "#d65d0e");
    }

    #[test]
    fn css_defines_the_accent_variable_for_both_themes() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("--accent: #fe8019"));
        assert!(html.contains("--accent: #d65d0e"));
    }

    #[test]
    fn body_links_actually_navigate_on_click_not_just_preview_on_hover() {
        // Regression: wireBodyLinks (formerly wireBodyLinkPreviews, renamed
        // to match what it actually does now) only wired hover listeners.
        // The <a> kept its bare "#UUID" href, so a click fell through to
        // the browser's default same-page fragment-scroll -- since no
        // element in the page actually has that id, this had NO visible
        // effect at all. Confirmed empirically (headless Chromium) both
        // before (click did nothing) and after (click opens the node) this
        // fix. This test checks the click handler + preventDefault are
        // present in the generated JS; the on-screen behavior was verified
        // separately with a real browser.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("function wireBodyLinks(container)"));
        assert!(html.contains("ev.preventDefault();"));
        assert!(html.contains("selectNode(n.id);"));
    }

    #[test]
    fn unresolved_body_links_get_unwrapped_not_left_looking_clickable() {
        // A source note commonly links to more than a depth-limited curated
        // export actually includes -- expected, not a bug in the curation
        // itself. But an unresolved link previously kept its normal <a>
        // styling (theme link color, underline, pointer cursor) with
        // nothing happening on click: indistinguishable from a working
        // link until a reader actually tried it. wireBodyLinks now unwraps
        // any href whose target isn't in nodesById into plain text instead
        // of leaving a dead-but-styled <a> in place.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("a.replaceWith(document.createTextNode(a.textContent));"),
            "expected unresolved body links to be unwrapped into plain text"
        );
    }

    #[test]
    fn prose_links_get_a_theme_link_color_distinct_from_accent() {
        // Regression: in-body <a> had no CSS rule at all and fell through
        // to the browser's default blue/purple, clashing with the theme.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("--link: #83a598"));
        assert!(html.contains("--link: #458588"));
        assert!(html.contains("#main-content a"));
        assert_ne!(
            GruvboxPalette::dark().link,
            GruvboxPalette::dark().accent,
            "link color must stay distinct from the graph-emphasis accent"
        );
    }

    #[test]
    fn next_button_does_not_re_select_the_already_visible_anchor() {
        // Regression: walkIndex started at -1 with a "Start here" label,
        // but readingOrder[0] is always the anchor (BFS distance 0 from
        // itself) and the anchor is already auto-selected on page load --
        // so the first click just re-selected the node already on screen,
        // a confusing no-op. walkIndex now starts at 0 (reflecting "we're
        // already at the first stop"), and the button always reads
        // "Next"/"Done", never "Start here". walkIndex is now computed via
        // anchorWalkIndex() (the anchor's own position in readingOrder,
        // which is 0 for a single-node export like this one either way --
        // see the gitlab-migration reading-order-chain tests below for why
        // it isn't hardcoded to 0 anymore in general), not a literal `= 0`.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("var walkIndex = anchorWalkIndex();"));
        assert!(
            html.contains(">Next \u{2192}<"),
            "button's initial static label should be Next, not Start here"
        );
        assert!(
            !html.contains("id=\"start-here\""),
            "the button id should reflect what it actually does now"
        );
    }

    #[test]
    fn graph_pane_actually_paints_with_the_page_background() {
        // Regression guard for a real bug: #graph-pane needs an explicit
        // `background: var(--bg0)` or it stays transparent and whatever
        // sits behind it in the DOM shows through instead.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("flex: 0 0 280px;"));
        let rule = html
            .split("flex: 0 0 280px;")
            .nth(1)
            .and_then(|s| s.split('}').next())
            .expect("#graph-pane sizing rule present");
        assert!(
            rule.contains("background: var(--bg0)"),
            "expected #graph-pane to actually paint with a background: {rule}"
        );
    }

    #[test]
    fn graph_pane_shares_the_page_root_theme_not_an_inverted_one() {
        // An earlier version rendered the chord widget in the *opposite*
        // gruvbox mode from the page (a distinct light/dark "card" inset),
        // scoped via `:root[data-theme] #graph-pane { ... }` blocks with
        // their own flipped --accent/--bg0/etc. That read as a boxed-in
        // prototype widget rather than an integrated part of the page (see
        // the STATIC_CSS comment above `#graph-pane`), so it was removed --
        // the widget now inherits the same theme variables as everything
        // else. Guard against the inverted, widget-scoped blocks coming
        // back.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(!html.contains(":root[data-theme=\"dark\"] #graph-pane {"));
        assert!(!html.contains(":root[data-theme=\"light\"] #graph-pane {"));
        assert!(!html.contains("box-shadow: 0 2px 10px"));
    }

    #[test]
    fn outline_heading_is_bold_and_not_the_most_muted_ink() {
        // The "On this page" heading previously had no explicit
        // font-weight (relying on the <h3> user-agent default) and used
        // the same muted fg3 ink as de-emphasized meta text elsewhere,
        // which undercut it reading as a real section heading at a
        // glance.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        let rule = html
            .split("#outline-panel h3 {")
            .nth(1)
            .and_then(|s| s.split('}').next())
            .expect("#outline-panel h3 rule present");
        assert!(
            rule.contains("font-weight: 600"),
            "expected an explicit bold weight, got: {rule}"
        );
        assert!(
            rule.contains("color: var(--fg2)"),
            "expected fg2 (not the more muted fg3), got: {rule}"
        );
    }

    #[test]
    fn theme_preference_persists_via_local_storage() {
        // Regression: the theme toggle previously had no persistence at
        // all -- themeIdx was recomputed purely from prefers-color-scheme
        // on every load, and data-theme was only ever set inside the
        // click handler, never on initial load. A reader's explicit
        // choice (as opposed to their OS/browser default) didn't survive
        // reopening the file.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("localStorage.getItem(\"mae-guide-theme\")"));
        assert!(html.contains("localStorage.setItem(\"mae-guide-theme\", themeOrder[themeIdx])"));
        // The stored-preference branch must set data-theme immediately on
        // load, not only inside the click handler, or a stored choice
        // would never actually apply until the reader clicked once.
        let stored_branch = html
            .split("var themeIdx = themeOrder.indexOf(storedTheme);")
            .nth(1)
            .and_then(|s| s.split("themeToggle.addEventListener").next())
            .expect("theme-init branch present");
        assert!(
            stored_branch.contains("document.documentElement.setAttribute(\"data-theme\""),
            "expected data-theme to be set on load when a stored preference exists: {stored_branch}"
        );
    }

    #[test]
    fn history_api_calls_are_guarded_against_a_throw() {
        // Real, reproducible bug: Firefox rate-limits History API calls
        // under file:// -- clicking through even a modest number of nodes
        // (repeated Next, or a few body links) throws a real SecurityError
        // ("the operation is insecure") once the limit is hit. selectNode()
        // previously called history.pushState() with nothing catching that
        // throw, which aborted selectNode() itself AND whatever the caller
        // did immediately after it -- nextBtn/prevBtn's click handlers call
        // updateWalkButtons() right after selectNode(), so a thrown
        // pushState left Previous/Next's disabled state stale. Reported
        // by a real user as "Next doesn't keep the page in Spanish, then
        // the EN/ES toggle stops working" -- content itself was fine
        // (applySelection already ran before the throw), it was
        // subsequent UI state that got left inconsistent.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains(
                "try {\n      history.pushState({ nodeId: id }, \"\", '#' + id);\n    } catch (e)"
            ),
            "expected pushState in selectNode() to be try/catch-guarded"
        );
        assert!(
            html.contains(
                "try {\n    history.replaceState({ nodeId: initialNodeId }, \"\", '#' + initialNodeId);\n  } catch (e)"
            ),
            "expected the initial replaceState call to be try/catch-guarded too"
        );
    }

    #[test]
    fn home_button_resets_walk_index_not_just_the_selection() {
        // Found by this project's own Layer 2 browser suite (kb/adrs/0001)
        // -- exactly the class of bug string-assertion tests structurally
        // can't catch on their own, though the fix is verifiable here once
        // known. Home previously only called selectNode(anchorId), never
        // touching walkIndex: after walking forward via Next to some
        // position N, clicking Home visually returned to the anchor, but
        // walkIndex stayed at N -- so the next Next click resumed from
        // N + 1 instead of position 1 (the real "next after home"),
        // landing on an unrelated node with no sign anything was wrong.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        let handler = html
            .split("homeBtn.addEventListener(\"click\", function () {")
            .nth(1)
            .and_then(|s| s.split("});").next())
            .expect("home button click handler present");
        assert!(
            handler.contains("walkIndex = anchorWalkIndex();"),
            "expected the Home handler to reset walkIndex to the anchor's own reading-order \
             position (not always 0 -- see the reading-order-chain tests below for why), got: \
             {handler}"
        );
        assert!(
            handler.contains("updateWalkButtons();"),
            "expected the Home handler to refresh Previous/Next's disabled state after resetting walkIndex, got: {handler}"
        );
    }

    #[test]
    fn export_includes_home_outline_and_theme_toggle_controls() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("id=\"home-button\""));
        assert!(html.contains("id=\"theme-toggle\""));
        assert!(html.contains("id=\"outline-panel\""));
        assert!(html.contains("id=\"outline-toggle\""));
        assert!(html.contains("id=\"outline-list\""));
    }

    #[test]
    fn export_includes_sidebar_toggle_control() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("id=\"sidebar-toggle\""));
        assert!(html.contains("aria-controls=\"sidebar\""));
        assert!(html.contains("aria-expanded=\"true\""));
    }

    #[test]
    fn export_includes_sidebar_backdrop_element() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("<div id=\"sidebar-backdrop\" hidden></div>"));
    }

    #[test]
    fn static_css_includes_mobile_sidebar_media_query() {
        // Regression guard: this file previously had zero width-based
        // @media rules at all (only prefers-color-scheme) -- pins both
        // breakpoints so a future edit can't silently drop the mobile
        // drawer / desktop collapse behavior.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("@media (max-width: 767px)"));
        assert!(html.contains("@media (min-width: 768px)"));
        assert!(html.contains("html[data-sidebar=\"open\"] #sidebar {"));
        assert!(html.contains("html[data-sidebar=\"closed\"] #sidebar { display: none; }"));
    }

    #[test]
    fn detail_panel_content_has_reading_width_cap() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        let rule = html
            .split("#detail-panel-content {")
            .nth(1)
            .and_then(|s| s.split('}').next())
            .expect("#detail-panel-content rule present");
        assert!(
            rule.contains("max-width: 70ch"),
            "expected a 70ch reading-measure cap, got: {rule}"
        );
        assert!(
            rule.contains("margin-left: auto") && rule.contains("margin-right: auto"),
            "expected the capped content to be centered, got: {rule}"
        );
    }

    #[test]
    fn sidebar_escape_handler_does_not_double_close_with_fullscreen() {
        // Real conflict this guards against: opening the mobile drawer,
        // then the chord diagram's own fullscreen overlay from inside it,
        // then pressing Escape once. An earlier version registered two
        // SEPARATE `keydown` listeners (one per overlay) that each read
        // isGraphFullscreen -- the fullscreen listener (registered first)
        // flipped it to false, and the sidebar listener (running second,
        // same event) then read the already-flipped value and wrongly
        // closed the drawer too on the same press. Caught by this
        // project's own Layer 2 suite, not by source inspection -- fixed
        // by merging both checks into ONE listener that checks fullscreen
        // first and returns early, so only that assertion is guaranteed to
        // exist just once, not per-overlay.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert_eq!(
            html.matches("document.addEventListener(\"keydown\"")
                .count(),
            1
        );
        let handler = html
            .split("document.addEventListener(\"keydown\", function (ev) {")
            .nth(1)
            .and_then(|s| s.split("});").next())
            .expect("keydown handler present");
        assert!(handler.contains("if (ev.key !== \"Escape\") { return; }"));
        assert!(handler.contains("if (isGraphFullscreen) {"));
        assert!(handler.contains("setGraphFullscreen(false);"));
        assert!(handler.contains("setSidebarOpen(false);"));
    }

    #[test]
    fn sidebar_preference_persists_via_local_storage() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("localStorage.getItem(\"mae-guide-sidebar-collapsed\")"));
        assert!(html.contains(
            "localStorage.setItem(\"mae-guide-sidebar-collapsed\", open ? \"false\" : \"true\")"
        ));
    }

    #[test]
    fn edges_render_as_curved_arcs_not_straight_lines() {
        // The chord-diagram convention: edges are quadratic-bezier <path>
        // elements pulled toward the layout center, not <line> elements --
        // this is a JS-side (GRAPH_JS) property, so assert on the emitted
        // script source rather than the (server-rendered) DOM shell.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("\"path\""),
            "expected SVG <path> edges: {html}"
        );
        assert!(
            !html.contains("el(\"line\""),
            "must not still be constructing <line> edges: {html}"
        );
    }

    #[test]
    fn edge_vertices_land_on_the_wedge_inner_edge_not_the_ring_midpoint() {
        // Source-text check only -- real rendered vertex position is Layer
        // 2 territory. Confirms the path `d` is built from a computed
        // inner-edge point (nodeRadius - minWorldRadius - inset), NOT the
        // node's raw (x, y), which sits at nodeRadius -- the wedge's
        // mid-thickness point -- and would visually land the chord vertex
        // inside the slice instead of on its inner edge.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("var edgeVertexInset = 2;"),
            "expected a small inward offset so the vertex doesn't sit exactly on the boundary: {html}"
        );
        assert!(
            html.contains("sRadius - minWorldRadius - edgeVertexInset"),
            "expected the source vertex to be placed at the wedge's inner edge: {html}"
        );
        assert!(
            html.contains("tRadius - minWorldRadius - edgeVertexInset"),
            "expected the target vertex to be placed at the wedge's inner edge: {html}"
        );
        assert!(
            !html.contains("\"M \" + s.x + \" \" + s.y"),
            "must not still start the path at the node's raw ring-midpoint (x, y): {html}"
        );
        assert!(
            !html.contains("+ t.x + \" \" + t.y"),
            "must not still end the path at the node's raw ring-midpoint (x, y): {html}"
        );
    }

    #[test]
    fn export_expected_node_count_reflected_in_payload() {
        let nodes = vec![
            simple_node("a", "A", "body", true),
            simple_node("b", "B", "body", false),
            simple_node("c", "C", "body", false),
        ];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        // Crude but effective: three distinct `"id":"..."` occurrences.
        let count = html.matches("\"id\":\"").count();
        assert_eq!(count, 3);
    }

    #[test]
    fn export_drops_edges_with_endpoints_outside_the_node_set() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let edges = vec![GraphExportEdge {
            source: "a".to_string(),
            target: "ghost".to_string(),
            rel_type: "references".to_string(),
            weight: 1.0,
        }];
        let html = HtmlGraphExporter.export(&nodes, &edges, "a", "T");
        assert!(!html.contains("ghost"));
    }

    // --- Adversarial: node title/body containing </script>, <style>, quotes ---

    #[test]
    fn adversarial_title_and_body_cannot_break_out_of_script_or_style_tags() {
        let evil_title = "Bad\"</script><style>body{display:none}</style><script>alert(1)";
        let evil_body = "See </script> and <style>*{color:red}</style> and \"quotes\" and 'ticks'.";
        let n = simple_node("a", evil_title, evil_body, true);
        let html = HtmlGraphExporter.export(&[n], &[], "a", "Adversarial Test");

        // The meaningful invariant isn't "no literal '<script' substring
        // anywhere" — an adversarial title's raw text legitimately ends up
        // inside the JSON payload's string content, and browsers only
        // treat a CASE-INSENSITIVE "</script"/"</style" CLOSING sequence
        // as ending a `<script>`/`<style>` element's raw-text content (a
        // bare, slash-less "<script>" embedded in that text is inert). So
        // the real oracle is: every "</script"/"</style" occurrence in the
        // document must be one of the ones WE emitted (exactly 2 real
        // `</script>` closes — graph-data + the JS body — and exactly 1
        // real `</style>` close), never one smuggled in via node content.
        let lower = html.to_lowercase();
        assert_eq!(
            lower.matches("</script").count(),
            2,
            "adversarial content must not introduce an extra real script-close: {html}"
        );
        assert_eq!(
            lower.matches("</style").count(),
            1,
            "adversarial content must not introduce an extra real style-close: {html}"
        );
        // And the escaped form must be present where the adversarial
        // content landed (proving the guard actually fired, not that the
        // content was simply absent).
        assert!(html.contains("<\\/script>"), "{html}");
        assert!(html.contains("<\\/style>"), "{html}");
    }

    #[test]
    fn adversarial_quotes_in_title_do_not_break_json() {
        let evil_title = r#"Title with "quotes" and \backslash\ and 'ticks'"#;
        let n = simple_node("a", evil_title, "body", false);
        let html = HtmlGraphExporter.export(&[n], &[], "a", "T");
        // Extract the JSON payload and confirm it round-trips.
        let start = html
            .find("<script id=\"graph-data\" type=\"application/json\">")
            .unwrap();
        let start = start + "<script id=\"graph-data\" type=\"application/json\">".len();
        let end = html[start..].find("</script>").unwrap() + start;
        let raw_json = &html[start..end];
        // Undo the </ -> <\/ guard before parsing, exactly like the page's
        // own JS does implicitly (JSON.parse decodes \/ back to /).
        let parsed: serde_json::Value = serde_json::from_str(raw_json).expect("valid JSON");
        assert_eq!(parsed["nodes"][0]["title_en"].as_str().unwrap(), evil_title);
    }

    #[test]
    fn html_escape_guard_neutralizes_literal_close_script_sequences() {
        let s = "</script>alert(1)</style>";
        let escaped = escape_for_inline_script(s);
        assert!(!escaped.contains("</script>"));
        assert!(!escaped.contains("</style>"));
        assert!(escaped.contains("<\\/script>"));
    }

    #[test]
    fn no_translations_hides_toggle_flag_in_payload() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("\"hasTranslations\":false"));
    }

    #[test]
    fn translations_present_sets_flag_true() {
        let t = NodeTranslation {
            title_es: Some("Título".to_string()),
            body_es: None,
        };
        let n = build_export_node(
            "a",
            "note",
            0.0,
            0.0,
            false,
            true,
            "Title",
            "body",
            Some(&t),
            &palette(),
        );
        let html = HtmlGraphExporter.export(&[n], &[], "a", "T");
        assert!(html.contains("\"hasTranslations\":true"));
    }

    // --- is_seed vs is_anchor are genuinely independent ---

    #[test]
    fn is_seed_and_is_anchor_are_independent_fields() {
        let n = build_export_node(
            "a",
            "concept",
            0.0,
            0.0,
            true,
            false,
            "Builtin Concept",
            "body",
            None,
            &palette(),
        );
        assert!(n.is_seed);
        assert!(!n.is_anchor);
    }

    // --- Mermaid block routed through render_node_body_html ---

    #[test]
    fn non_mermaid_src_block_is_rendered_as_plain_code_not_mermaid_path() {
        let body = "#+begin_src python\nprint(1)\n#+end_src\n";
        let html = render_node_body_html(body, &palette());
        assert!(html.contains("<pre><code class=\"language-python\">print(1)</code></pre>"));
        assert!(!html.contains("mermaid"));
    }

    #[test]
    fn example_block_renders_verbatim_through_render_node_body_html() {
        let body = "Intro.\n\n#+begin_example\n$ terraform plan\nNo changes.\n#+end_example\n";
        let html = render_node_body_html(body, &palette());
        assert!(html.contains("<pre class=\"example\">$ terraform plan\nNo changes.</pre>"));
        // Regression guard: this must NOT be the old bug's shape (the
        // literal "#+begin_example" text visible as prose).
        assert!(!html.contains("#+begin_example"));
    }

    #[test]
    fn example_block_css_rule_is_present_in_the_stylesheet() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("pre.example"));
    }

    // --- Mermaid happy path (real subprocess: npx + @mermaid-js/mermaid-cli).
    // Network/Node.js-dependent, so `#[ignore]`d by default — run explicitly
    // with `cargo test -p mae-export -- --ignored mermaid_diagram_renders`
    // in an environment known to have `npx` + network access. ---

    #[test]
    #[ignore = "shells out to `npx @mermaid-js/mermaid-cli`; requires Node.js + network"]
    fn mermaid_diagram_renders_to_real_inline_svg_via_mmdc() {
        let body =
            "Some intro text.\n\n#+begin_src mermaid\ngraph TD;\nA-->B;\n#+end_src\n\nOutro.\n";
        let html = render_node_body_html(body, &palette());
        assert!(
            html.contains("<svg"),
            "expected a real inline <svg> from mmdc, got: {html}"
        );
        assert!(
            !html.contains("mermaid-fallback"),
            "should not have fallen back: {html}"
        );
    }

    // --- Integration: export a small fixture to a real file ---

    #[test]
    fn integration_exports_fixture_subgraph_to_a_real_file() {
        let nodes = vec![
            simple_node(
                "root",
                "Root Node",
                "The root. Links: [[id:child1][Child 1]].",
                true,
            ),
            simple_node("child1", "Child One", "First child body.", false),
            simple_node("child2", "Child Two", "Second child body.", false),
            simple_node(
                "child3",
                "Child Three",
                "Third child, unlinked to root directly.",
                false,
            ),
        ];
        let edges = vec![
            GraphExportEdge {
                source: "root".into(),
                target: "child1".into(),
                rel_type: "explains".into(),
                weight: 1.0,
            },
            GraphExportEdge {
                source: "root".into(),
                target: "child2".into(),
                rel_type: "related_to".into(),
                weight: 1.0,
            },
            GraphExportEdge {
                source: "child2".into(),
                target: "child3".into(),
                rel_type: "extends".into(),
                weight: 1.0,
            },
        ];
        let html = HtmlGraphExporter.export(&nodes, &edges, "root", "Fixture Subgraph");

        let dir = tempfile::tempdir().unwrap();
        let out_path = dir.path().join("export.html");
        std::fs::write(&out_path, &html).unwrap();

        let read_back = std::fs::read_to_string(&out_path).unwrap();
        assert!(read_back.starts_with("<!DOCTYPE html>"));
        assert!(read_back.trim_end().ends_with("</html>"));
        assert_eq!(read_back.matches("\"id\":\"").count(), 4);
        assert!(!read_back.contains("<script src=\"http"));
        assert!(!read_back.contains("<script src=\"https"));
    }

    // --- kb/adrs/0004: guidance nodes / colophon ---

    #[test]
    fn build_guidance_node_sets_the_flag_and_no_seed_or_anchor_status() {
        let n = build_guidance_node(
            "style-guide",
            "note",
            "Style Guide",
            "body",
            None,
            &palette(),
        );
        assert!(n.is_guidance);
        assert!(!n.is_seed);
        assert!(!n.is_anchor);
        assert_eq!(n.x, 0.0);
        assert_eq!(n.y, 0.0);
    }

    #[test]
    fn guidance_node_translation_and_fallback_behavior_matches_any_other_node() {
        // build_guidance_node reuses build_export_node's assembly wholesale
        // -- a guidance node with no translation should mirror EN into ES
        // exactly like a topic node does (ADR-0003's fallback notice logic
        // in GRAPH_JS keys off title_es == title_en / body_es == body_en,
        // regardless of is_guidance).
        let n = build_guidance_node(
            "style-guide",
            "note",
            "Style Guide",
            "body text",
            None,
            &palette(),
        );
        assert_eq!(n.title_es, n.title_en);
        assert_eq!(n.body_es, n.body_en);
    }

    #[test]
    fn no_guidance_nodes_means_no_colophon_section_at_all() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            !html.contains("id=\"colophon\""),
            "an export with zero guidance nodes should render no colophon footer: {html}"
        );
    }

    #[test]
    fn guidance_node_renders_a_colophon_entry_with_both_language_titles() {
        let anchor = simple_node("a", "A", "body", true);
        let mut guidance = build_guidance_node(
            "style-guide",
            "practice",
            "Writing Style Guide",
            "Guidance body",
            None,
            &palette(),
        );
        // Give it a real ES title so the colophon's bilingual data
        // attributes are exercised, not just the EN-mirrors-ES fallback.
        guidance.title_es = "Guía de Estilo".to_string();
        let html = HtmlGraphExporter.export(&[anchor, guidance], &[], "a", "T");

        assert!(
            html.contains("id=\"colophon\""),
            "expected a colophon footer: {html}"
        );
        assert!(html.contains("data-node-id=\"style-guide\""));
        assert!(html.contains("data-title-en=\"Writing Style Guide\""));
        assert!(html.contains("data-title-es=\"Guía de Estilo\""));
        // The guidance node is still a real entry in the embedded JSON
        // payload (so nodesById/selectNode resolve it when a colophon
        // button or an in-body link opens it) -- both nodes present.
        assert_eq!(html.matches("\"id\":\"").count(), 2);
        assert!(html.contains("\"is_guidance\":true"));
    }

    #[test]
    fn guidance_node_is_excluded_from_the_reading_order_walk_and_chord_graph() {
        // Rust-side, this can only assert on the emitted GRAPH_JS/markup
        // shape (topicNodes-based filtering), not runtime DOM behavior --
        // the real browser-level guarantee (no <g class="node"> drawn, not
        // reachable via Next/Previous) is covered by the Layer 2 suite.
        let anchor = simple_node("a", "A", "body", true);
        let guidance = build_guidance_node(
            "style-guide",
            "practice",
            "Style Guide",
            "body",
            None,
            &palette(),
        );
        let html = HtmlGraphExporter.export(&[anchor, guidance], &[], "a", "T");
        assert!(
            html.contains(
                "var topicNodes = nodes.filter(function (n) { return !n.is_guidance; });"
            ),
            "computeReadingOrder/the chord draw loop must iterate topicNodes, not the raw nodes \
             list: {html}"
        );
    }

    #[test]
    fn adversarial_guidance_title_cannot_break_out_of_the_colophon_attribute_or_json() {
        let anchor = simple_node("a", "A", "body", true);
        let guidance = build_guidance_node(
            "style-guide",
            "practice",
            "\"><script>alert(1)</script>",
            "body",
            None,
            &palette(),
        );
        let html = HtmlGraphExporter.export(&[anchor, guidance], &[], "a", "T");
        assert!(
            !html.contains("<script>alert(1)</script>"),
            "adversarial guidance title must not survive unescaped in colophon markup or JSON: \
             {html}"
        );
        assert!(html.contains("&quot;&gt;&lt;script&gt;"));
    }

    // --- Visited-node history panel ---

    #[test]
    fn history_panel_markup_and_controls_are_present() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("id=\"history-panel\""));
        assert!(html.contains("id=\"history-list\""));
        assert!(html.contains("id=\"history-back\""));
        assert!(html.contains("id=\"history-forward\""));
    }

    #[test]
    fn history_panel_stays_pinned_to_the_bottom_even_when_outline_is_hidden() {
        // Regression: #history-panel was only ever visually pinned to
        // #sidebar's bottom edge as a SIDE EFFECT of #outline-panel's own
        // flex: 1 consuming all free space above it. renderOutline
        // (GRAPH_JS) hides #outline-panel entirely (`hidden = true`) on
        // any node with zero real headings -- a common case, not an edge
        // case -- which removes it from the flex layout and left
        // #history-panel flowing right below #graph-pane instead of
        // staying at the bottom. `margin-top: auto` on the flex item
        // fixes this unconditionally: it absorbs whatever free space is
        // left after flex-grow resolves, whether that's #outline-panel's
        // flex: 1 claiming it first (outline visible, this margin then
        // resolves to 0 -- no change from before) or nothing else claiming
        // it at all (outline hidden -- this margin claims it instead).
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("#history-panel {\n  flex: 0 1 auto;\n  margin-top: auto;"),
            "expected #history-panel to pin itself to the sidebar's bottom edge via its own \
             margin, not rely on a sibling's flex: 1 to push it there: {html}"
        );
    }

    #[test]
    fn history_panel_sits_after_outline_panel_in_the_sidebar() {
        // kb: placement decision -- chord graph -> outline -> history, so
        // outline stays adjacent to the chord widget it complements and
        // history (a session log, checked less often) goes last.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        let outline_pos = html
            .find("id=\"outline-panel\"")
            .expect("outline panel present");
        let history_pos = html
            .find("id=\"history-panel\"")
            .expect("history panel present");
        assert!(
            outline_pos < history_pos,
            "expected #outline-panel to precede #history-panel in the markup: {html}"
        );
    }

    #[test]
    fn history_panel_js_implements_forward_truncation_and_depth_cap() {
        // Source-text presence checks (this file's existing convention for
        // asserting on GRAPH_JS behavior, e.g. the topicNodes-filtering
        // check for kb/adrs/0004) -- the actual runtime behavior is covered
        // by the Layer 2 browser suite.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("visitStack = visitStack.slice(0, visitPos + 1);"),
            "expected forward-history truncation on new navigation: {html}"
        );
        assert!(
            html.contains("while (visitStack.length > HISTORY_DEPTH_CAP)"),
            "expected depth-cap eviction logic: {html}"
        );
        assert!(
            html.contains("visitDropped += 1;"),
            "expected the eviction count to be tracked, not silently dropped: {html}"
        );
        assert!(
            html.contains("history.back();") && html.contains("history.forward();"),
            "expected the Back/Forward buttons to replay through REAL browser history, not a \
             second hand-rolled navigation path: {html}"
        );
    }

    #[test]
    fn history_panel_renders_via_dom_helper_not_raw_innerhtml_for_titles() {
        // Node titles are untrusted content (sourced from the KB) -- this
        // file's established rule is textContent/dom(), never string-
        // concatenated innerHTML, for exactly that reason (see
        // escape_for_inline_script's doc comment and wireBodyLinks). Confirm
        // renderHistoryPanel follows the same rule as renderOutline/
        // renderLinkList rather than introducing a second, riskier pattern.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        let fn_start = html
            .find("function renderHistoryPanel()")
            .expect("renderHistoryPanel present");
        let fn_end = html[fn_start..]
            .find("historyBackBtn.addEventListener")
            .map(|i| fn_start + i)
            .expect("end of renderHistoryPanel");
        let fn_body = &html[fn_start..fn_end];
        assert!(
            !fn_body.contains(".innerHTML"),
            "renderHistoryPanel must build titles via dom()/textContent, not innerHTML: {fn_body}"
        );
    }

    // --- Authored reading-order chain extraction (gitlab-migration scale work) ---

    #[test]
    fn parse_reading_order_extracts_both_ids_from_a_real_section() {
        // Real shape, copied from gitlab-migration's own onboarding docs.
        let body = "Some prose about onboarding.\n\n\
* Reading Order\n\
- Part :: Onboarding Materials.\n\
- Previous :: [[id:0e320309-3373-4ed5-9a77-4eaac24c80fd][Bilingual Terminology Index (EN/ES)]].\n\
- Next :: [[id:54a41e36-f1cd-4081-ac1b-8d8eb126cea2][GitLab CI/CD Primer: What Changes For You]].\n";
        let (prev, next) = parse_reading_order(body);
        assert_eq!(
            prev.as_deref(),
            Some("0e320309-3373-4ed5-9a77-4eaac24c80fd")
        );
        assert_eq!(
            next.as_deref(),
            Some("54a41e36-f1cd-4081-ac1b-8d8eb126cea2")
        );
    }

    #[test]
    fn parse_reading_order_none_previous_yields_none() {
        let body = "* Reading Order\n\
- Part :: Executive Overview & Milestones.\n\
- Previous :: none — first document in the reading order.\n\
- Next :: [[id:d4e7af90-1c7a-4d60-8c5f-04c51eb626c8][GitLab CE Self-Hosted Migration]].\n";
        let (prev, next) = parse_reading_order(body);
        assert_eq!(prev, None);
        assert_eq!(
            next.as_deref(),
            Some("d4e7af90-1c7a-4d60-8c5f-04c51eb626c8")
        );
    }

    #[test]
    fn parse_reading_order_no_heading_at_all_yields_none_none() {
        let (prev, next) = parse_reading_order(
            "Just an ordinary node body.\n\n** Some Other Heading\nMore text.\n",
        );
        assert_eq!(prev, None);
        assert_eq!(next, None);
    }

    #[test]
    fn parse_reading_order_list_item_with_no_link_is_none_not_a_panic() {
        let body = "* Reading Order\n\
- Previous :: some unlinked plain text, not a real link.\n\
- Next :: also plain text.\n";
        let (prev, next) = parse_reading_order(body);
        assert_eq!(prev, None);
        assert_eq!(next, None);
    }

    #[test]
    fn parse_reading_order_ignores_a_list_before_the_heading() {
        // A list elsewhere in the body (e.g. a normal bullet list in the
        // prose) must not be mistaken for the Reading Order section just
        // because it also contains "Previous ::"-shaped text -- only a
        // list that follows the literal "Reading Order" heading counts.
        let body = "- Previous :: [[id:decoy][Decoy]].\n\n\
* Some Other Section\n\
More prose, no reading order here.\n";
        let (prev, next) = parse_reading_order(body);
        assert_eq!(prev, None);
        assert_eq!(next, None);
    }

    #[test]
    fn build_export_node_populates_reading_order_fields() {
        let body = "* Reading Order\n\
- Previous :: [[id:aaa][A]].\n\
- Next :: [[id:bbb][B]].\n";
        let n = build_export_node(
            "n1",
            "note",
            0.0,
            0.0,
            false,
            false,
            "Title",
            body,
            None,
            &palette(),
        );
        assert_eq!(n.reading_order_prev.as_deref(), Some("aaa"));
        assert_eq!(n.reading_order_next.as_deref(), Some("bbb"));
    }

    // --- "Part ::" breadcrumb extraction ---

    #[test]
    fn parse_reading_order_part_extracts_the_label_and_strips_trailing_period() {
        let body = "* Reading Order\n\
- Part :: Project-Scope Architecture Decisions.\n\
- Previous :: none.\n\
- Next :: none.\n";
        assert_eq!(
            parse_reading_order_part(body).as_deref(),
            Some("Project-Scope Architecture Decisions")
        );
    }

    #[test]
    fn parse_reading_order_part_absent_yields_none() {
        let body = "* Reading Order\n\
- Previous :: none.\n\
- Next :: none.\n";
        assert_eq!(parse_reading_order_part(body), None);
    }

    #[test]
    fn parse_reading_order_part_no_heading_at_all_yields_none() {
        assert_eq!(
            parse_reading_order_part("Just an ordinary node body, no Reading Order section.\n"),
            None
        );
    }

    #[test]
    fn build_export_node_populates_part_per_language_with_fallback() {
        let body = "* Reading Order\n\
- Part :: English Part Label.\n\
- Previous :: none.\n\
- Next :: none.\n";
        // No translation at all -> Spanish falls back to the English label,
        // the same per-field fallback convention title_es/body_es already use.
        let n = build_export_node(
            "n1",
            "note",
            0.0,
            0.0,
            false,
            false,
            "Title",
            body,
            None,
            &palette(),
        );
        assert_eq!(
            n.reading_order_part_en.as_deref(),
            Some("English Part Label")
        );
        assert_eq!(
            n.reading_order_part_es.as_deref(),
            Some("English Part Label")
        );

        // A real Spanish translation with its own "Parte ::"-shaped section
        // -- note the parser matches the literal "Part ::" prefix, so a
        // genuinely Spanish-authored section uses "Parte ::" and this
        // regression guard confirms that does NOT match (falls back to
        // English), while an explicit "Part ::" inside the translated body
        // (mirroring the fixture-style content this session's own real
        // gitlab-migration translations use) is picked up independently
        // from the English side.
        let es_body = "* Reading Order\n\
- Part :: Etiqueta de Parte en Español.\n\
- Previous :: none.\n\
- Next :: none.\n";
        let translation = NodeTranslation {
            title_es: None,
            body_es: Some(es_body.to_string()),
        };
        let n2 = build_export_node(
            "n2",
            "note",
            0.0,
            0.0,
            false,
            false,
            "Title",
            body,
            Some(&translation),
            &palette(),
        );
        assert_eq!(
            n2.reading_order_part_en.as_deref(),
            Some("English Part Label")
        );
        assert_eq!(
            n2.reading_order_part_es.as_deref(),
            Some("Etiqueta de Parte en Español")
        );
    }

    // --- Tags ---

    #[test]
    fn build_export_node_defaults_tags_to_empty() {
        let n = simple_node("a", "A", "body", true);
        assert!(n.tags.is_empty());
    }

    #[test]
    fn tags_flow_into_the_json_payload() {
        let mut n = simple_node("a", "A", "body", true);
        n.tags = vec!["infra".to_string(), "security".to_string()];
        let html = HtmlGraphExporter.export(&[n], &[], "a", "T");
        assert!(html.contains("\"tags\":[\"infra\",\"security\"]"));
    }

    #[test]
    fn compute_reading_order_walks_the_authored_chain_before_the_bfs_fallback() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("if (visited[n.id] || (!validPrev(n) && !validNext(n))) { return; }"),
            "expected the chain-walk-first pass to run before the BFS-distance fallback: {html}"
        );
        assert!(
            html.contains("var rest = topicNodes.filter(function (n) { return !visited[n.id]; })"),
            "expected non-chain nodes to be appended after every chain-walked node: {html}"
        );
    }

    #[test]
    fn chord_viewbox_pad_scales_with_node_count_not_a_flat_constant() {
        // kb: root-caused this session against a real 167-node export --
        // a flat `pad = 40` stopped covering minWorldRadius (which itself
        // scales UP with node count) past roughly n=66, cropping every
        // outer-ring node. Source-text check only (Rust tests can't
        // execute GRAPH_JS) -- confirms the two-pass fit replaced the flat
        // constant, not that it's numerically correct at any given n
        // (that's the real-browser stress test's job).
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("for (var fitPass = 0; fitPass < 2; fitPass++)"),
            "expected the two-pass pad/viewBox fit: {html}"
        );
        assert!(
            !html.contains("var pad = 40;\n  var minX"),
            "the old flat, never-scaling pad must actually be gone, not just supplemented: {html}"
        );
    }

    #[test]
    fn part_breadcrumb_renders_per_language_above_the_title_when_present() {
        // Source-text check only (Rust tests can't execute GRAPH_JS) --
        // the Rust-side parser (parse_reading_order_part) has its own
        // dedicated unit tests above; this confirms the JS renders it as
        // plain per-language text, not a link, and only when present.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains(r#"var partLabel = n["reading_order_part_" + currentLang];"#),
            "expected the breadcrumb to follow the same per-language field convention as title/body: {html}"
        );
        assert!(
            html.contains(
                r#"detailContent.appendChild(dom("div", { class: "node-part-breadcrumb" }, partLabel));"#
            ),
            "expected the breadcrumb to render as plain text (dom/textContent), never through innerHTML: {html}"
        );
        assert!(
            html.contains("#main-content .node-part-breadcrumb {"),
            "expected a muted, non-link breadcrumb style: {html}"
        );
    }

    #[test]
    fn visited_node_marking_state_and_toggle_logic_is_present() {
        // Source-text check only (Rust tests can't execute GRAPH_JS) --
        // confirms the visitedIds set is seeded from the anchor, grown on
        // every real selection, and never marks the currently-selected
        // node as visited (selected already owns that "you are here"
        // signal). The real-browser behavior (dot appears after
        // navigating away, persists across further selections) is Layer
        // 2's job.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("var visitedIds = {};"),
            "expected a persistent visited-node set, independent of the capped visitStack: {html}"
        );
        assert!(
            html.contains("visitedIds[anchorId] = true;"),
            "expected the anchor to be seeded as visited on load: {html}"
        );
        assert!(
            html.contains("visitedIds[id] = true;"),
            "expected every real selection to mark that node visited: {html}"
        );
        assert!(
            html.contains("tg.classList.toggle(\"visited\", !!visitedIds[tn.id] && tn.id !== id);"),
            "expected the visited class to be withheld from the currently-selected node: {html}"
        );
        assert!(
            html.contains("var innerArcOuterR = innerR + (outerR - innerR) * 0.4;"),
            "expected the visited marker to be an inner ~2/5-thickness band of the wedge, not a dot: {html}"
        );
        assert!(
            html.contains(".node.visited path.visited-inner-arc { fill: var(--bg2); opacity: 1; }"),
            "expected the visited marker to be an opacity-only channel (properly specificity-qualified against .node path/.node.selected path/.node.neighbor path), not fill/stroke/geometry: {html}"
        );
        // Regression: an earlier version scaled the WEDGE's own
        // cornerRadius down and applied it symmetrically to both edges of
        // the inner arc, which rounded its OUTER edge (an artificial cut
        // partway through the wedge, not a real boundary) -- that made the
        // marker look like a separate floating pill instead of a flush
        // slice of the petal it's nested in (reported, fixed by
        // switching to arcPath's independent-radius form: 0 on the outer
        // edge so it lines up flush with the wedge's own straight sides,
        // the wedge's real (unscaled) cornerRadius on the inner edge,
        // which IS a true shared boundary with the wedge).
        assert!(
            html.contains(
                "d: arcPath(\n        centerX, centerY, innerR, innerArcOuterR, angle - halfSpan, angle + halfSpan, 0, cornerRadius\n      ),"
            ),
            "expected the visited-arc's outer edge to be unrounded (flush with the petal's straight sides) and its inner edge to reuse the wedge's own real cornerRadius: {html}"
        );
    }

    #[test]
    fn keyboard_ring_navigation_roving_tabindex_and_key_handling_is_present() {
        // Source-text check only (Rust tests can't execute GRAPH_JS) --
        // real focus/keydown behavior across both target engines is
        // Layer 2's job.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("role: \"button\","),
            "expected every node group to carry an accessible role: {html}"
        );
        assert!(
            html.contains(r#""aria-label": n["title_" + currentLang],"#),
            "expected a per-language accessible label: {html}"
        );
        assert!(
            html.contains("function updateRovingTabindex() {"),
            "expected roving-tabindex bookkeeping to exist as its own function, called on every selection change: {html}"
        );
        assert!(
            html.contains(
                "ng.setAttribute(\"tabindex\", ng.getAttribute(\"data-id\") === selectedId ? \"0\" : \"-1\");"
            ),
            "expected exactly one node (the current selection) to be Tab-reachable at a time: {html}"
        );
        assert!(
            html.contains(r#"nodeLayer.addEventListener("keydown", function (ev) {"#),
            "expected a delegated keydown listener on the node layer: {html}"
        );
        assert!(
            html.contains(r#"if (ev.key === "ArrowRight" || ev.key === "ArrowLeft") {"#),
            "expected ArrowLeft/Right to move around the ring: {html}"
        );
        assert!(
            html.contains(r#"} else if (ev.key === "ArrowDown") {"#)
                && html.contains("nextBtn.click();"),
            "expected ArrowDown to reuse the existing Next button, not a second competing path: {html}"
        );
        assert!(
            html.contains(r#"} else if (ev.key === "ArrowUp") {"#)
                && html.contains("prevBtn.click();"),
            "expected ArrowUp to reuse the existing Previous button: {html}"
        );
        assert!(
            html.contains(r#"} else if (ev.key === "Enter" || ev.key === " ") {"#),
            "expected Enter/Space to activate the focused node like a click: {html}"
        );
        assert!(
            html.contains(".node:focus-visible path {"),
            "expected a real :focus-visible ring, not plain :focus (which would also fire on mouse clicks): {html}"
        );
    }

    #[test]
    fn wedge_corners_are_rounded_and_slots_have_no_angular_gap() {
        // Source-text check only -- real rendered curvature is Layer 2/
        // visual-inspection territory (the fillet math is an approximation,
        // not something a substring check can validate geometrically).
        // Confirms: (1) arcPath accepts and clamps a real cornerRadius
        // parameter (the "petal" look, user request), (2) both the initial
        // draw AND refreshWedgeGrowth pass the SAME per-node cornerRadius
        // (stored once in wedgeGeomById) -- critical for the `d` CSS
        // transition to keep interpolating smoothly across hover/neighbor
        // growth, since the command structure must stay identical between
        // states, and (3) there's no angular gap between slots anymore
        // (the rounding itself is what visually separates adjacent
        // wedges, not a drawn-apart gap).
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains(
                "function arcPath(cx, cy, innerR, outerR, a0, a1, outerCornerRadius, innerCornerRadius) {"
            ),
            "expected arcPath to accept independent outer/inner corner-radius parameters: {html}"
        );
        assert!(
            html.contains("var cornerRadius = halfThickness * 0.6;"),
            "expected a real, thickness-proportional corner radius, not a flat magic constant: {html}"
        );
        assert!(
            html.contains("cornerRadius: cornerRadius,"),
            "expected the corner radius to be stored in wedgeGeomById for growth to reuse: {html}"
        );
        assert!(
            html.contains("geom.cornerRadius"),
            "expected refreshWedgeGrowth to reuse the SAME stored corner radius, not recompute a \
             different one (which would break the `d` transition's command-structure match): {html}"
        );
        assert!(
            html.contains("var wedgeGapRadians = 0;"),
            "expected zero angular gap between adjacent wedge slots: {html}"
        );
    }

    #[test]
    fn wedge_angular_span_never_exceeds_its_nominal_slot() {
        // Source-text check only -- confirms the angular hit-target floor
        // that used to let halfSpan grow past a node's own slot (a real,
        // severe overlap bug: 142/168 boundary pairs on a real export)
        // has been removed entirely, not just tuned down. Real angular
        // measurement across many nodes is Layer 2's job.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("var halfSpan = angleStep / 2 - wedgeGapRadians / 2;"),
            "expected halfSpan to be exactly the nominal per-node slot: {html}"
        );
        assert!(
            !html.contains("minHalfSpan"),
            "expected the angular-growth-past-nominal-slot floor to be fully removed: {html}"
        );
    }

    #[test]
    fn wedge_styling_uses_background_color_not_borders() {
        // Source-text check only -- confirms the wedge restyle (user
        // request): no stroke/border anywhere, the neighbor highlight is
        // a fill-color change (not a stroke-color change), and the rest
        // state uses a real theme foreground color instead of a flat
        // muted gray.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains(".node path {") && html.contains("fill: var(--fg1);"),
            "expected the rest-state wedge fill to be the theme's real foreground color: {html}"
        );
        // Edges legitimately have their own stroke (.edge.incident uses
        // --accent for a highlighted connecting LINE, not a wedge) -- this
        // checks the specific old wedge-stroke declarations are gone,
        // rather than a blanket "stroke" search that would also (wrongly)
        // flag those.
        assert!(
            !html.contains("stroke: var(--bg0)"),
            "expected the old idle-state wedge border to be fully removed: {html}"
        );
        assert!(
            !html.contains(".node.neighbor path { stroke:"),
            "expected the neighbor highlight to no longer set any stroke: {html}"
        );
        assert!(
            !html.contains(".node.selected path {\n  fill: var(--accent);\n  stroke:"),
            "expected the selected wedge to no longer set a stroke alongside its fill: {html}"
        );
        assert!(
            html.contains(".node.neighbor path { fill: var(--link); }"),
            "expected the neighbor highlight to be a background-color change, not a border: {html}"
        );
    }

    #[test]
    fn wedges_render_fully_opaque_with_uniform_thickness() {
        // Source-text check only -- real per-node pixel behavior is Layer
        // 2's job. Confirms: (1) thickness is uniform (the overlap
        // regression this section fixed), (2) no fill-opacity ramp of any
        // kind -- wedges are fully opaque (an earlier degree-driven
        // opacity ramp was tried here and explicitly reverted per user
        // request, so this also guards against silently reintroducing it).
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("var halfThickness = minWorldRadius;"),
            "expected every wedge to share exactly the same thickness: {html}"
        );
        assert!(
            !html.contains("n.is_anchor ? 3 : 0"),
            "expected the anchor/degree thickness bonus to be fully removed, not just reduced: {html}"
        );
        assert!(
            !html.contains("fill-opacity:") && !html.contains("fillOpacity"),
            "expected no fill-opacity ramp of any kind -- wedges must render fully opaque: {html}"
        );
    }

    #[test]
    fn wedge_hover_growth_transition_is_snappy_and_independent_of_ui_transition_ms() {
        // Regression: `.node path`'s d/filter/fill transition was 200ms,
        // sharing the same literal as the rest of the page's UI chrome
        // transitions (governed by ui_transition_ms). d-attribute path
        // interpolation is real per-frame CPU work, not a GPU-composited
        // transform, so it read as noticeably more sluggish than a plain
        // 200ms transform transition -- user-reported as "much slower".
        // Tightened to a fixed 130ms, deliberately kept OUTSIDE the
        // ui_transition_ms bucket (not a "200ms" literal) so tuning that
        // shared knob can never accidentally re-introduce the slowdown.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("transition: d 130ms ease, filter 130ms ease, fill 130ms ease;"),
            "expected the wedge growth transition to be a fast, fixed 130ms: {html}"
        );
        // Confirm ui_transition_ms overrides genuinely don't touch it.
        let cfg = ChordDiagramConfig {
            ui_transition_ms: 500,
            ..ChordDiagramConfig::default()
        };
        let html2 = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html2.contains("transition: d 130ms ease, filter 130ms ease, fill 130ms ease;"),
            "wedge growth transition must stay fixed even when ui_transition_ms is overridden: {html2}"
        );
    }

    #[test]
    fn default_chord_config_produces_identical_output_to_export() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let a = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        let b = HtmlGraphExporter.export_with_config(
            &nodes,
            &[],
            "a",
            "T",
            &ChordDiagramConfig::default(),
        );
        assert_eq!(
            a, b,
            "a default-valued ChordDiagramConfig must round-trip to byte-identical output"
        );
    }

    #[test]
    fn hover_growth_factor_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            hover_growth_factor: 2.25,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html.contains("var HOVER_GROWTH_FACTOR = 2.25;"),
            "expected the overridden value in the generated JS: {html}"
        );
        assert!(
            !html.contains("var HOVER_GROWTH_FACTOR = 1.6;"),
            "must not still contain the hardcoded default: {html}"
        );
    }

    #[test]
    fn stroke_buffer_px_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            stroke_buffer_px: 5.0,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html.contains("var strokeBuffer = 5;"),
            "expected the overridden value in the generated JS: {html}"
        );
        assert!(!html.contains("var strokeBuffer = 2;"), "{html}");
    }

    #[test]
    fn cosmetic_cushion_px_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            cosmetic_cushion_px: 40.0,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html.contains("var cosmeticCushion = 40;"),
            "expected the overridden value in the generated JS: {html}"
        );
        assert!(!html.contains("var cosmeticCushion = 16;"), "{html}");
    }

    #[test]
    fn min_onscreen_radius_px_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            min_onscreen_radius_px: 20.0,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html.contains("var minOnscreenRadiusPx = 20;"),
            "expected the overridden value in the generated JS: {html}"
        );
        assert!(!html.contains("var minOnscreenRadiusPx = 12;"), "{html}");
    }

    #[test]
    fn initial_pad_px_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            initial_pad_px: 80.0,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html.contains("var pad = 80;"),
            "expected the overridden value in the generated JS: {html}"
        );
        assert!(!html.contains("var pad = 40;"), "{html}");
    }

    #[test]
    fn edge_pull_back_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            edge_pull_back: 0.1,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html.contains("var pullBack = 0.1;"),
            "expected the overridden value in the generated JS: {html}"
        );
        assert!(!html.contains("var pullBack = 0.55;"), "{html}");
    }

    #[test]
    fn wedge_gap_radians_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            wedge_gap_radians: 0.05,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html.contains("var wedgeGapRadians = 0.05;"),
            "expected the overridden value in the generated JS: {html}"
        );
        assert!(!html.contains("var wedgeGapRadians = 0;"), "{html}");
    }

    #[test]
    fn history_depth_cap_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            history_depth_cap: 15,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html.contains("var HISTORY_DEPTH_CAP = 15;"),
            "expected the overridden value in the generated JS: {html}"
        );
        assert!(!html.contains("var HISTORY_DEPTH_CAP = 8;"), "{html}");
    }

    #[test]
    fn wedge_corner_radius_fraction_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            wedge_corner_radius_fraction: 0.3,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html.contains("var cornerRadius = halfThickness * 0.3;"),
            "expected the overridden value in the generated JS: {html}"
        );
        assert!(
            !html.contains("var cornerRadius = halfThickness * 0.6;"),
            "{html}"
        );
    }

    #[test]
    fn search_debounce_ms_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            search_debounce_ms: 400,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html.contains("searchResults.hidden = true; }, 400);"),
            "expected the overridden value in the generated JS: {html}"
        );
        assert!(
            !html.contains("searchResults.hidden = true; }, 150);"),
            "{html}"
        );
    }

    #[test]
    fn ui_transition_ms_override_does_not_touch_180ms_or_220ms_rules() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            ui_transition_ms: 350,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html.contains("350ms"),
            "expected the overridden duration to appear: {html}"
        );
        assert!(
            html.contains("180ms"),
            "the 180ms micro-interaction rules must stay fixed, not scale with this config: {html}"
        );
        assert!(
            html.contains("220ms"),
            "the 220ms fullscreen-enter asymmetry must stay fixed, not scale with this config: {html}"
        );
        // A blanket `!html.contains("200ms")` would false-positive on
        // GRAPH_JS's own prose comments describing this convention (e.g.
        // "this file's small-motion convention (200ms ease)") -- those
        // aren't real CSS values, so check a known, real STATIC_CSS rule
        // instead of the whole page.
        assert!(
            !html.contains("transition: background-color 200ms ease, color 200ms ease;"),
            "expected this real STATIC_CSS rule's 200ms to have been replaced: {html}"
        );
        assert!(
            html.contains("transition: background-color 350ms ease, color 350ms ease;"),
            "expected the overridden duration in a real STATIC_CSS rule: {html}"
        );
    }

    #[test]
    fn last_open_node_persists_via_local_storage_and_restores_on_load() {
        // Source-text check only -- real localStorage read/write behavior
        // across a real reload is Layer 2's job.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains(r#"localStorage.setItem("mae-guide-last-node", id);"#),
            "expected every real selection to persist as the last-open node: {html}"
        );
        assert!(
            html.contains(r#"localStorage.getItem("mae-guide-last-node")"#),
            "expected the initial load to read the stored last-open node: {html}"
        );
        assert!(
            html.contains(
                "var initialNodeId = (storedLastNode && nodesById[storedLastNode]) ? storedLastNode : anchorId;"
            ),
            "expected a stale/missing stored node to fall back to the anchor, not crash or show a blank page: {html}"
        );
        assert!(
            html.contains("applySelection(initialNodeId);"),
            "expected the restored node to actually be selected on load: {html}"
        );
    }

    #[test]
    fn navigation_resets_scroll_to_top_of_main_content() {
        // Source-text check only -- real scroll-position behavior across a
        // real navigation is Layer 2's job.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains(r#"var mainContent = document.getElementById("main-content");"#),
            "expected a cached reference to the actual scrolling container: {html}"
        );
        assert!(
            html.contains("if (mainContent) { mainContent.scrollTop = 0; }"),
            "expected every real selection (applySelection) to reset scroll to top: {html}"
        );
    }

    #[test]
    fn chord_diagram_fullscreen_toggle_markup_and_logic_is_present() {
        // Source-text check only (Rust tests can't execute GRAPH_JS) --
        // real animation/keyboard behavior across both target engines is
        // Layer 2's job.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains(r#"<button id="graph-fullscreen-toggle""#),
            "expected a fullscreen toggle button inside #graph-pane: {html}"
        );
        assert!(
            html.contains("function setGraphFullscreen(next) {"),
            "expected a single source-of-truth toggle function: {html}"
        );
        // The fullscreen-Escape check now lives inside the single merged
        // keydown handler shared with the sidebar drawer (see
        // sidebar_escape_handler_does_not_double_close_with_fullscreen) --
        // asserted here as "checked first, inside an isGraphFullscreen
        // branch", not as the old standalone one-liner, which a prior
        // version of this merge would have duplicated across two
        // listeners and reintroduced the double-close bug that test
        // guards against.
        assert!(
            html.contains(
                "if (isGraphFullscreen) {\n      setGraphFullscreen(false);\n      return;\n    }"
            ),
            "expected Escape to exit fullscreen (checked first, in the merged handler): {html}"
        );
        assert!(
            html.contains("#graph-pane.fullscreen {")
                && html.contains("position: fixed;")
                && html.contains("@keyframes graph-fullscreen-in {")
                && html.contains("@keyframes graph-fullscreen-out {"),
            "expected a real animated enter/exit, not an instant snap: {html}"
        );
    }

    #[test]
    fn fuzzy_search_and_tag_filter_logic_is_present() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("function fuzzyScore(query, target)"));
        assert!(html.contains("function applyTagFilter()"));
        assert!(
            html.contains("g.classList.toggle(\"filtered-out\", !nodeMatchesTagFilter(n));"),
            "expected tag filtering to dim non-matching nodes, not remove them: {html}"
        );
    }

    #[test]
    fn search_and_tag_filter_header_markup_is_present() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("id=\"node-search\""));
        assert!(html.contains("id=\"search-results\""));
        assert!(html.contains("id=\"tag-picker-toggle\""));
        assert!(html.contains("id=\"tag-picker\""));
        assert!(html.contains("id=\"active-tag-chips\""));
    }

    #[test]
    fn hidden_attribute_overrides_are_present_for_every_display_flex_dropdown() {
        // Regression: .tag-picker/.tag-filter-group both set `display: flex`
        // explicitly, which has the SAME CSS specificity as the browser's
        // built-in `[hidden] { display: none }` rule -- author CSS later in
        // the cascade wins the tie, so without an explicit [hidden]
        // override these stayed visibly flex regardless of the `hidden`
        // attribute GRAPH_JS sets, and sat on top of (eating clicks from)
        // whatever header control was underneath. Found by the Layer 2
        // suite, not by inspection -- worth a standing regression guard.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains(".tag-filter-group[hidden], .tag-picker[hidden] { display: none; }"),
            "expected an explicit [hidden] override for every display:flex dropdown: {html}"
        );
    }

    #[test]
    fn tag_picker_toggle_shares_the_controls_button_theme_styling() {
        // Real reported bug: #tag-picker-toggle lives in .tag-filter-group
        // (a sibling of .controls in the header markup, sitting between
        // the search box and the Home/Previous/Next/theme buttons), so
        // the `.controls button { ... }` rule that themes every other
        // header button never matched it -- it rendered as a completely
        // unstyled native <button>, an OS-default box clashing with the
        // themed page around it in both light and dark mode.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains(".controls button, #tag-picker-toggle {"),
            "expected #tag-picker-toggle to share the real themed button styling, not render unstyled: {html}"
        );
        assert!(
            html.contains(
                ".controls button:hover, #tag-picker-toggle:hover { background: var(--bg3); }"
            ),
            "expected the hover state to also be shared, not just the base state: {html}"
        );
    }

    #[test]
    fn syntax_highlighting_js_and_css_are_present() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("function highlightCodeBlocks(container)"));
        assert!(html.contains("function highlightSource(src, keywords)"));
        assert!(html.contains("highlightCodeBlocks(body);"));
        assert!(html.contains(".tok-kw { color: var(--accent)"));
        assert!(html.contains(".tok-str { color: var(--link)"));
        assert!(html.contains(".tok-com { color: var(--fg4)"));
        assert!(html.contains(".tok-prompt { color: var(--fg4)"));
    }

    #[test]
    fn hl_escape_lt_regex_survives_the_close_script_tag_escaper_intact() {
        // Regression: `.replace(/</g, ...)` written the "obvious" way
        // parses fine as Rust/standalone JS, but this whole GRAPH_JS
        // constant also passes through `escape_for_inline_script`'s
        // blanket `"</" -> "<\/"` pass before being embedded -- which
        // strips that regex literal's own closing delimiter (the `/`
        // immediately after `<`) and corrupts the entire exported script.
        // Only caught by actually parsing the exported page's JS (`node
        // --check`), not by any Rust-side string check -- this is the
        // cheapest guard against it regressing: assert the safe `/[<]/g`
        // form survived, and the broken shape never appears.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("replace(/[<]/g, \"&lt;\")"));
        assert!(!html.contains("replace(/<\\/g"));
    }

    #[test]
    fn syntax_highlighting_skips_mermaid_language_blocks() {
        // Mermaid blocks are already replaced with real inline <svg> (or a
        // raw-source fallback) by render_mermaid_block before
        // highlightCodeBlocks ever runs -- re-tokenizing "mermaid" as a
        // generic language would be at best wasted work and at worst
        // corrupt an SVG's own markup if one somehow still carried the
        // language-mermaid class.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("if (lang === \"mermaid\") { return; }"));
    }

    #[test]
    fn next_stops_at_the_authored_chain_end_not_bfs_fallback_content() {
        // Real-world regression, found in two stages against a real
        // 167-node gitlab-migration export: (1) Next correctly followed
        // the authored chain but then silently spilled into unrelated
        // BFS-fallback content once the chain's real end (README's own
        // "Next :: none") was reached; (2) a first fix using a single
        // "chain-walked prefix length" boundary stopped Next ONE node too
        // late, because that KB has more than one independent authored
        // chain (a second, separate one inside gitlab-platform/gitlab-
        // host's own ADRs) concatenated after the main one -- the boundary
        // needs to be checked per-node (does THIS node have a valid next),
        // not as a single fixed length. Source-presence check only, the
        // actual walk behavior is covered by the Layer 2 suite.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains("isChainNode: visited,"));
        assert!(
            html.contains("function atChainEnd() {")
                && html.contains("if (!n || !isChainNode[n.id]) { return false; }")
                && html.contains(
                    "return !(n.reading_order_next && readingOrderTopicIds[n.reading_order_next]);"
                ),
            "expected Next's disabled state to check the CURRENT node's own next pointer, not a \
             single fixed chain-length boundary: {html}"
        );
        assert!(html.contains("var done = walkIndex >= readingOrder.length - 1 || atChainEnd();"));
    }

    #[test]
    fn neighbor_nodes_of_the_selection_get_a_standing_highlight() {
        // Real, reported gap: in a dense ring, the OTHER endpoint of an
        // already-highlighted incident edge looked identical to every
        // unrelated node -- hard to spot AND hard to click precisely.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains("var neighborGroup = groupFor(src === id ? tgt : src);"),
            "expected applySelection to compute and highlight the OTHER endpoint of each \
             incident edge: {html}"
        );
        assert!(html.contains("neighborGroup.classList.add(\"neighbor\");"));
        assert!(
            html.contains(
                "nodeGroups.forEach(function (ng) { if (ng) { ng.classList.remove(\"neighbor\"); } });"
            ),
            "expected the previous selection's neighbor highlight to be cleared before \
             recomputing it, not left to accumulate: {html}"
        );
        assert!(
            html.contains("topicNodes.forEach(function (tn) { refreshWedgeGrowth(tn.id); });"),
            "expected applySelection to refresh every topic node's wedge growth after \
             recomputing neighbors, so a node that stopped being a neighbor shrinks back: {html}"
        );
        assert!(
            html.contains("function refreshWedgeGrowth(id)")
                && html.contains("g.classList.contains(\"hovered\")")
                && html.contains("minWorldRadius * 0.35 : 0"),
            "expected the neighbor highlight to actually grow the real hit-tested area (via \
             real wedge geometry, not just a color change), and to yield to .hovered's own \
             larger growth bonus when both apply: {html}"
        );
    }
}
