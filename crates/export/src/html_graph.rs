//! Self-contained, offline-first HTML export of a KB subgraph.
//!
//! @ai-caution: [architecture-debt] ~3,700 lines, still over the 800-line
//! source ceiling — tracked in `.claude/commands/mae-audit.md`'s "Known
//! exceptions" list and `ROADMAP.md`'s "Architecture Debt" section. Folded
//! shipped as a real in-tree module (see ADR-077) rather than a Scheme
//! reimplementation. The two large
//! embedded string constants (`GRAPH_JS`, `STATIC_CSS`) that originally made
//! up over 40% of this file's line count have been extracted to real,
//! lintable `assets/graph.js`/`assets/graph.css` files (loaded via
//! `include_str!`) -- found, during a pre-merge security/architecture
//! review, to have already let a real bug (a regex literal corrupted by the
//! escaper) through, since there was no `node --check`/CI syntax gate on an
//! inline Rust string. What remains here is the Rust assembly logic plus
//! this module's own extensive test suite (adversarial script-injection
//! escaping, wedge geometry, per-field `ChordDiagramConfig` override
//! tests) -- not a further asset-embedding seam, so no further split is
//! attempted this pass.
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
    ListItem, OrgElement,
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
///
/// @ai-caution: [architecture-debt] hand-copied theme snapshot, no automated
/// drift check against the source TOML files — see
/// https://github.com/cuttlefisch/mae/issues/568 and ROADMAP.md's
/// "Architecture Debt" section.
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
/// Recurses into `ListItem::children` (a nested sub-list) so a hover
/// preview doesn't silently drop nested-item text -- `plain_text_preview`
/// flattens everything into one collapsed-whitespace string anyway, so
/// nesting structure itself isn't meaningful here, only making sure no
/// content is missing.
fn push_plain_list_items(
    items: &[ListItem],
    push_plain: &impl Fn(&str, &mut String),
    text: &mut String,
) {
    for item in items {
        push_plain(&item.content, text);
        if !item.children.is_empty() {
            push_plain_list_items(&item.children, push_plain, text);
        }
    }
}

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
                push_plain_list_items(items, &push_plain, &mut text);
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
    /// A "guidance node" (ADR-079) —
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

/// Build a "guidance node" (ADR-079) — an
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
/// `ADR-081` for the surface-level design
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

/// `GRAPH_JS` is emitted verbatim -- every `ChordDiagramConfig` field it
/// reads flows through the `#graph-data` JSON payload's `chordConfig`
/// object instead (see `HtmlGraphExporter::export_with_config`), not
/// exact-substring text patching. `GRAPH_JS` reads `data.chordConfig` once
/// at load with hardcoded defaults (matching `ChordDiagramConfig::
/// default()` exactly) as its own fallback, so it stays independently
/// valid, `node --check`-able JS even without that payload.
///
/// `STATIC_CSS`'s one tunable (`ui_transition_ms`) is a real CSS custom
/// property (`--ui-transition-ms`, with a `200ms` fallback baked into
/// every `var(--ui-transition-ms, 200ms)` use in the stylesheet itself --
/// see graph.css) rather than text-patched -- [`render_transition_var_css`]
/// emits the one small `:root{...}` rule (already inside the page's single
/// `<style>` block, see its call site) that sets it to the real configured
/// value.
fn render_transition_var_css(cfg: &ChordDiagramConfig) -> String {
    format!(":root{{--ui-transition-ms:{}ms;}}\n", cfg.ui_transition_ms)
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
            // Real data injection, not the old exact-substring `.replacen()`
            // patching against literal JS text (which silently no-op'd --
            // dead option, no error signal -- the moment GRAPH_JS's literal
            // text ever reformatted). `graph.js` reads this object once at
            // load, with hardcoded defaults matching `ChordDiagramConfig::
            // default()` as its own fallback, so it stays independently
            // valid JS (and `node --check`-able) even without this payload.
            "chordConfig": {
                "hoverGrowthFactor": config.hover_growth_factor,
                "strokeBufferPx": config.stroke_buffer_px,
                "cosmeticCushionPx": config.cosmetic_cushion_px,
                "minOnscreenRadiusPx": config.min_onscreen_radius_px,
                "initialPadPx": config.initial_pad_px,
                "edgePullBack": config.edge_pull_back,
                "wedgeGapRadians": config.wedge_gap_radians,
                "historyDepthCap": config.history_depth_cap,
                "wedgeCornerRadiusFraction": config.wedge_corner_radius_fraction,
                "searchDebounceMs": config.search_debounce_ms,
            },
        });
        let payload_json = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());

        let mut html = String::with_capacity(64 * 1024);
        html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
        html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
        html.push_str("<title>");
        html.push_str(&html_escape(page_title));
        html.push_str("</title>\n<style>\n");
        html.push_str(&render_css_variables(&dark, &light));
        html.push_str(&render_transition_var_css(config));
        html.push_str(STATIC_CSS);
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
        html.push_str(&escape_for_inline_script(GRAPH_JS));
        html.push_str("\n</script>\n");

        html.push_str("</body>\n</html>\n");
        html
    }
}

/// Renders the "About this guide" colophon (ADR-079-guidance-nodes-
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

/// Static CSS for the exported page -- a real `.css` file
/// (`assets/graph.css`), not an inline Rust string literal, same
/// rationale as `GRAPH_JS` below.
const STATIC_CSS: &str = include_str!("../assets/graph.css");

/// Vanilla JS graph interaction layer -- no bundler, no external CDN, no
/// npm dependency shipped in the output. 100% static (every dynamic value
/// comes from the embedded `#graph-data` JSON payload read at runtime), so
/// this constant needs no `format!`/placeholder interpolation and can't
/// suffer brace-escaping bugs. A real `.js` file (`assets/graph.js`), not
/// an inline Rust string literal -- lintable/syntax-checkable with real JS
/// tooling (`node --check`, the browser test suite in `tests/browser/`)
/// instead of only via `html.contains("...")` source-text assertions.
const GRAPH_JS: &str = include_str!("../assets/graph.js");

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

    // --- ADR-078: untranslated-node fallback gets an explicit UI signal ---

    #[test]
    fn fallback_notice_logic_is_present_and_per_field() {
        // Regression guard for the original bug (see ADR-078): toggling the language on
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
        // Adversarial edge case per ADR-078's "per-field, not just
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
        // Found by this project's own Layer 2 browser suite -- exactly the
        // class of bug string-assertion tests structurally
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
    fn adversarial_begin_export_html_in_a_kb_node_body_is_escaped() {
        // kb_export_subgraph_html's whole purpose is producing a
        // shareable artifact from KB content that isn't necessarily
        // self-authored (a federated/shared KB) -- this path must NEVER
        // trust a #+begin_export html block the way org_export's opt-in
        // (ExportOptions::allow_raw_html_export_blocks) can, since
        // build_export_node/render_node_body_html always uses
        // ExportOptions::default() with no override.
        let evil_body = "Normal prose.\n#+begin_export html\n<img src=x onerror=alert(document.cookie)>\n#+end_export\n";
        let n = simple_node("a", "Title", evil_body, true);
        assert!(
            !n.body_en.contains("<img"),
            "a live <img> tag must never survive kb_export_subgraph_html's render path: {}",
            n.body_en
        );
        assert!(
            n.body_en.contains("&lt;img src=x onerror=alert(document.cookie)&gt;"),
            "expected the escaped form to be present: {}",
            n.body_en
        );
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

    // --- ADR-079: guidance nodes / colophon ---

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
        // check for ADR-079) -- the actual runtime behavior is covered
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
            html.contains(
                "var cornerRadius = halfThickness * (chordConfig.wedgeCornerRadiusFraction ?? 0.6);"
            ),
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
        assert_eq!(
            chord_config_json(&html)["wedgeGapRadians"],
            0.0,
            "expected zero angular gap between adjacent wedge slots at the default config: {html}"
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

    /// The 9 JS-side `ChordDiagramConfig` fields now flow through the
    /// `#graph-data` JSON payload's `chordConfig` object (real data
    /// injection), not exact-substring text patching against GRAPH_JS's
    /// source -- so the meaningful oracle is "the payload carries the
    /// overridden value," not "the JS source text changed."
    fn chord_config_json(html: &str) -> serde_json::Value {
        let start_marker = "<script id=\"graph-data\" type=\"application/json\">";
        let start = html
            .find(start_marker)
            .expect("expected a graph-data script tag")
            + start_marker.len();
        let end = html[start..]
            .find("</script>")
            .expect("expected graph-data script tag to close");
        let payload: serde_json::Value =
            serde_json::from_str(&html[start..start + end]).expect("graph-data must be valid JSON");
        payload["chordConfig"].clone()
    }

    #[test]
    fn hover_growth_factor_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            hover_growth_factor: 2.25,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert_eq!(chord_config_json(&html)["hoverGrowthFactor"], 2.25);
    }

    #[test]
    fn stroke_buffer_px_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            stroke_buffer_px: 5.0,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert_eq!(chord_config_json(&html)["strokeBufferPx"], 5.0);
    }

    #[test]
    fn cosmetic_cushion_px_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            cosmetic_cushion_px: 40.0,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert_eq!(chord_config_json(&html)["cosmeticCushionPx"], 40.0);
    }

    #[test]
    fn min_onscreen_radius_px_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            min_onscreen_radius_px: 20.0,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert_eq!(chord_config_json(&html)["minOnscreenRadiusPx"], 20.0);
    }

    #[test]
    fn initial_pad_px_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            initial_pad_px: 80.0,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert_eq!(chord_config_json(&html)["initialPadPx"], 80.0);
    }

    #[test]
    fn edge_pull_back_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            edge_pull_back: 0.1,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert_eq!(chord_config_json(&html)["edgePullBack"], 0.1);
    }

    #[test]
    fn edge_pull_back_of_exactly_zero_is_not_mistaken_for_unset() {
        // Adversarial: 0.0 is a real, documented, non-default value for
        // this field ("0 = straight line") -- a naive `value || default`
        // fallback in the JS reader would incorrectly treat it as "not
        // provided" and silently substitute the 0.55 default instead.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            edge_pull_back: 0.0,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert_eq!(chord_config_json(&html)["edgePullBack"], 0.0);
    }

    #[test]
    fn wedge_gap_radians_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            wedge_gap_radians: 0.05,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert_eq!(chord_config_json(&html)["wedgeGapRadians"], 0.05);
    }

    #[test]
    fn history_depth_cap_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            history_depth_cap: 15,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert_eq!(chord_config_json(&html)["historyDepthCap"], 15);
    }

    #[test]
    fn wedge_corner_radius_fraction_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            wedge_corner_radius_fraction: 0.3,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert_eq!(chord_config_json(&html)["wedgeCornerRadiusFraction"], 0.3);
    }

    #[test]
    fn search_debounce_ms_override_changes_generated_js() {
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            search_debounce_ms: 400,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert_eq!(chord_config_json(&html)["searchDebounceMs"], 400);
    }

    #[test]
    fn ui_transition_ms_override_does_not_touch_180ms_or_220ms_rules() {
        // ui_transition_ms is real CSS-custom-property injection now
        // (:root{--ui-transition-ms:...}), not exact-substring text
        // patching against STATIC_CSS -- the stylesheet's own rules
        // always read `var(--ui-transition-ms, 200ms)` verbatim
        // regardless of config; only the :root value that variable
        // resolves to changes.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let cfg = ChordDiagramConfig {
            ui_transition_ms: 350,
            ..ChordDiagramConfig::default()
        };
        let html = HtmlGraphExporter.export_with_config(&nodes, &[], "a", "T", &cfg);
        assert!(
            html.contains(":root{--ui-transition-ms:350ms;}"),
            "expected the overridden duration injected as a CSS custom property: {html}"
        );
        assert!(
            html.contains("transition: background-color var(--ui-transition-ms, 200ms) ease, color var(--ui-transition-ms, 200ms) ease;"),
            "the real STATIC_CSS rule must reference the custom property (with its own \
             200ms fallback), never a literal duration: {html}"
        );
        assert!(
            html.contains("180ms"),
            "the 180ms micro-interaction rules must stay fixed, not scale with this config: {html}"
        );
        assert!(
            html.contains("220ms"),
            "the 220ms fullscreen-enter asymmetry must stay fixed, not scale with this config: {html}"
        );
    }

    #[test]
    fn ui_transition_ms_default_still_resolves_to_200ms() {
        // Companion to the override test above: confirm the *unconfigured*
        // default path also injects the custom property (not skipped),
        // and every affected rule's var() fallback matches it exactly.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(
            html.contains(":root{--ui-transition-ms:200ms;}"),
            "expected the default duration injected as a CSS custom property: {html}"
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
