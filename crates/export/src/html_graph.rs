//! Self-contained, offline-first HTML export of a KB subgraph.
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
//! `is_seed: false` and `is_anchor: true`. The exported page's "distinct
//! styling + Start here" affordance is driven by `is_anchor`, not
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
use crate::{convert_inline_markup_str, html_escape, parse_org_document, InlineTarget, OrgElement};

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
/// `mae_kb::Node::body` retains a leading `:PROPERTIES:...:END:` drawer
/// verbatim (properties are metadata, but the generic org parser this
/// module reuses — `parse_org_document`, written for exporting genuine
/// org-file body content — has no concept of a drawer and treats it as
/// an ordinary paragraph). Left unstripped, every exported node's body
/// literally opens with its own raw `:ID:`/`:hash:` lines rendered as
/// visible prose. Strips only a *leading* drawer (bounded to the first
/// ~500 chars so a `:PROPERTIES:`-looking string deep in real prose is
/// never mistaken for one) — mirrors the same bounded-prefix convention
/// `shared/kb/src/activity.rs::body_hash` already uses for the same
/// drawer shape.
fn strip_leading_properties_drawer(body: &str) -> &str {
    let head = &body[..body.len().min(500)];
    if let Some(props_start) = head.find(":PROPERTIES:") {
        if head[..props_start].trim().is_empty() {
            if let Some(end_rel) = head[props_start..].find(":END:") {
                let end = props_start + end_rel + ":END:".len();
                return body[end..].trim_start_matches(['\n', '\r']);
            }
        }
    }
    body
}

fn render_node_body_html(body: &str, palette: &GruvboxPalette) -> String {
    let body = strip_leading_properties_drawer(body);
    let (meta, elements) = parse_org_document(body);
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
    let (_, elements) = parse_org_document(body);
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
    /// drives the page's "distinct styling + Start here" affordance. See
    /// module docs.
    pub is_anchor: bool,
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
    let (body_es, preview_es) = match translation.and_then(|t| t.body_es.as_deref()) {
        Some(es_body) => (
            render_node_body_html(es_body, palette),
            plain_text_preview(es_body, 200),
        ),
        None => (body_html.clone(), preview_en.clone()),
    };
    GraphExportNode {
        id: id.into(),
        kind: kind.into(),
        x,
        y,
        is_seed,
        is_anchor,
        title_en: title.to_string(),
        body_en: body_html,
        preview_en,
        title_es,
        body_es,
        preview_es,
    }
}

// ---------------------------------------------------------------------
// HTML assembly
// ---------------------------------------------------------------------

/// Exports a positioned KB subgraph to one self-contained HTML page.
/// Mirrors `crate::html::HtmlExporter`'s "one function, one dependency-free
/// HTML string" shape (see module docs for why this isn't the same
/// `Exporter` trait).
pub struct HtmlGraphExporter;

impl HtmlGraphExporter {
    /// `page_title`: `<title>`/`<h1>` text (e.g. "Terraform Onboarding").
    /// `anchor_id`: the id of the node with `is_anchor: true` in `nodes` —
    /// used to drive "Start here" without the page having to re-scan
    /// `nodes` client-side for the flag.
    pub fn export(
        &self,
        nodes: &[GraphExportNode],
        edges: &[GraphExportEdge],
        anchor_id: &str,
        page_title: &str,
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
        html.push_str(&render_widget_inverse_theme_css(&dark, &light));
        html.push_str(STATIC_CSS);
        html.push_str("</style>\n</head>\n<body>\n");

        html.push_str("<header id=\"page-header\">\n<h1 id=\"page-title\">");
        html.push_str(&html_escape(page_title));
        html.push_str("</h1>\n<div class=\"controls\">\n");
        html.push_str(
            "<button id=\"home-button\" type=\"button\" title=\"Jump to the spine/anchor node\">\u{2302} Home</button>\n",
        );
        html.push_str(
            "<button id=\"prev-button\" type=\"button\" disabled>\u{2190} Previous</button>\n",
        );
        html.push_str("<button id=\"start-here\" type=\"button\">Start here \u{2192}</button>\n");
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
             <svg id=\"graph-svg\" xmlns=\"http://www.w3.org/2000/svg\"></svg>\n\
             <div id=\"popover\" class=\"popover\" hidden></div>\n\
             </div>\n\
             <nav id=\"outline-panel\">\n\
             <h3 id=\"outline-toggle\">On this page \u{25be}</h3>\n\
             <ul class=\"outline-list\" id=\"outline-list\"></ul>\n\
             </nav>\n\
             </aside>\n\
             </main>\n",
        );

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

/// Guard against a JSON string (or, defensively, the static JS constant)
/// containing a literal `</script`/`</style` sequence that would
/// prematurely close the surrounding `<script>` tag and let subsequent
/// markup escape into the page as raw HTML/JS. Escaping every `</`
/// occurrence to `<\/` is valid inside both a JSON string literal
/// (backslash-solidus is a legal JSON escape, decodes back to `/`) and a
/// `<script>` element's text content (browsers don't interpret `<\/` as a
/// tag close) — so this is safe to apply unconditionally, not just when a
/// dangerous substring is detected.
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

/// The chord nav widget (`#graph-pane`) deliberately renders in the *opposite*
/// gruvbox mode from the surrounding page — a light inset on a dark page, a
/// dark inset on a light page — so the widget reads as a distinct, deliberate
/// focal point rather than blending into the page chrome. Mirrors
/// [`render_css_variables`]'s own default/media-query/attribute structure
/// exactly, just inverted and scoped to `#graph-pane`: the attribute-selector
/// rules carry higher specificity than the bare/media-scoped ones, so an
/// explicit theme-toggle click always wins over the system preference, same
/// as it does for the rest of the page.
fn render_widget_inverse_theme_css(dark: &GruvboxPalette, light: &GruvboxPalette) -> String {
    let mut css = String::new();
    css.push_str("#graph-pane {\n");
    push_palette_vars(&mut css, light);
    css.push_str("}\n");
    css.push_str("@media (prefers-color-scheme: light) {\n#graph-pane {\n");
    push_palette_vars(&mut css, dark);
    css.push_str("}\n}\n");
    css.push_str(":root[data-theme=\"dark\"] #graph-pane {\n");
    push_palette_vars(&mut css, light);
    css.push_str("}\n:root[data-theme=\"light\"] #graph-pane {\n");
    push_palette_vars(&mut css, dark);
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
  justify-content: space-between;
  padding: 0.75rem 1.25rem;
  background: var(--bg1);
  border-bottom: 1px solid var(--bg3);
  transition: background-color 200ms ease, border-color 200ms ease;
}
#page-title { margin: 0; font-size: 1.25rem; }
.controls button {
  background: var(--bg2);
  color: var(--fg1);
  border: 1px solid var(--bg3);
  border-radius: 4px;
  padding: 0.4rem 0.8rem;
  margin-left: 0.5rem;
  cursor: pointer;
  font-size: 0.9rem;
  /* >=24px hit target on every control, not just graph nodes. */
  min-height: 24px;
  transition: background-color 180ms ease, color 180ms ease, transform 180ms ease;
}
.controls button:hover { background: var(--bg3); }
.controls button:disabled {
  opacity: 0.4;
  cursor: default;
  background: var(--bg2);
}
.controls button#home-button { background: var(--accent); color: var(--bg0); border-color: var(--accent); }
.controls button#home-button:hover { transform: translateY(-1px); }
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
  border-left: 1px solid var(--bg3);
}
#graph-pane {
  flex: 0 0 280px;
  position: relative;
  min-width: 0;
  border-bottom: 1px solid var(--bg3);
}
#graph-svg { width: 100%; height: 100%; display: block; }
#main-content .hint { color: var(--fg3); font-style: italic; }
#main-content h2 { margin-top: 0; }
#detail-panel-content { transition: opacity 180ms ease; opacity: 1; }
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
#main-content .anchor-note {
  background: var(--bg1);
  border-left: 3px solid var(--node-anchor);
  padding: 0.4rem 0.6rem;
  margin-bottom: 0.75rem;
  font-size: 0.85rem;
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
#main-content code { font-family: "JetBrains Mono", "Fira Code", monospace; }
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
#graph-pane {
  border-radius: 10px;
  overflow: hidden;
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.25);
}
.node circle {
  fill: var(--fg4);
  stroke: var(--bg0);
  stroke-width: 1.5;
  /* >=24px hit target regardless of the visible radius below this floor
     — handled by clamping the drawn radius itself in GRAPH_JS (simpler
     than a separate invisible hit-layer, and keeps click/hover geometry
     identical). */
  transition: transform 200ms ease, filter 200ms ease, fill 200ms ease;
  transform-box: fill-box;
  transform-origin: center;
}
/* Labels are selective, not persistent (dataviz skill's "selective direct
   labels" rule): with ~15-20 nodes crammed around a small chord ring,
   showing every label at once produces exactly the overlapping/clipped
   mess a real render surfaced (several titles ran into each other or
   were cut off past the diagram edge). Only the anchor, the currently
   selected node, and whatever's under the cursor show a label by
   default; every other node is an identifiable dot, its title one
   hover away via the existing popover -- this is a labeling-density
   fix, not a data-hiding one. */
.node text {
  fill: var(--fg3);
  font-size: 11px;
  pointer-events: none;
  user-select: none;
  opacity: 0;
  /* Halo, not just a fill color: with up to 26 edges converging on one hub
     node in a star-shaped subgraph, a label can sit directly on top of
     several crossing lines at once -- raw fill-vs-solid-background contrast
     is irrelevant there, since the background right behind the glyphs often
     isn't the flat page background at all. `paint-order: stroke fill`
     draws a `--bg0`-colored outline behind the glyph before the fill, so
     the label stays legible regardless of what's underneath (edges, other
     nodes) -- the standard cartography/map-label technique for exactly
     this problem, not a color tweak that only helps against a flat bg. */
  stroke: var(--bg0);
  stroke-width: 3px;
  stroke-linejoin: round;
  paint-order: stroke fill;
  transition: fill 200ms ease, opacity 200ms ease;
}
.node-anchor text,
.node.selected text,
.node.hovered text {
  opacity: 1;
}
.node { cursor: pointer; }
/* Hover LIFTS (scale + drop-shadow), it does not recolor — recoloring is
   reserved entirely for `.selected` (the current node). This also means
   hover and selected no longer compete for the same visual channel, so
   both can be simultaneously true and simultaneously visible (a lifted,
   accent-colored current node under the cursor) with no priority rule
   needed between them for color -- only `.selected` ever changes fill. */
.node.hovered circle {
  transform: scale(1.25);
  filter: drop-shadow(0 2px 6px rgba(0, 0, 0, 0.45));
}
.node.selected circle {
  fill: var(--accent);
  stroke: var(--accent);
}
.node.selected text { fill: var(--fg1); font-weight: bold; }

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

#outline-panel {
  flex: 1;
  padding: 0.5rem 1.25rem;
  min-height: 0;
  overflow-y: auto;
}
#outline-panel.collapsed .outline-list { display: none; }
#outline-panel h3 {
  margin: 0.25rem 0;
  font-size: 0.8rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--fg3);
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

  var svg = document.getElementById("graph-svg");
  var popover = document.getElementById("popover");
  var detailContent = document.getElementById("detail-panel-content");
  var outlinePanel = document.getElementById("outline-panel");
  var outlineList = document.getElementById("outline-list");
  var outlineToggle = document.getElementById("outline-toggle");
  var langToggle = document.getElementById("lang-toggle");
  var startHereBtn = document.getElementById("start-here");
  var prevBtn = document.getElementById("prev-button");
  var homeBtn = document.getElementById("home-button");
  var themeToggle = document.getElementById("theme-toggle");

  var currentLang = "en";
  var selectedId = null;

  if (hasTranslations) {
    langToggle.hidden = false;
  }

  // --- Layout: fit all node positions (chord-ring or force, whichever the
  // export baked in) into the SVG viewBox. Center is used both for the
  // viewBox fit AND as the pull-point for edge arcs below. Node labels
  // sit to the RIGHT of their circle (`x: n.x + r + 3`) and can run up to
  // 29 chars (28 + an ellipsis) at an 11px font -- padding just the node
  // *positions* by a flat amount clipped exactly this text in a real
  // render, since it never accounted for label width at all. `labelPad`
  // is a deliberately generous per-character estimate (real glyph widths
  // vary; overshooting costs empty margin, undershooting clips text
  // again -- overshoot), added only on the right/bottom where labels
  // actually extend. ---
  var pad = 60;
  var labelPad = 29 * 7;
  var minX = Math.min.apply(null, nodes.map(function (n) { return n.x; }));
  var maxX = Math.max.apply(null, nodes.map(function (n) { return n.x; }));
  var minY = Math.min.apply(null, nodes.map(function (n) { return n.y; }));
  var maxY = Math.max.apply(null, nodes.map(function (n) { return n.y; }));
  if (!isFinite(minX)) { minX = 0; maxX = 100; minY = 0; maxY = 100; }
  var w = Math.max(1, maxX - minX);
  var h = Math.max(1, maxY - minY);
  var centerX = (minX + maxX) / 2, centerY = (minY + maxY) / 2;
  svg.setAttribute(
    "viewBox",
    (minX - pad) + " " + (minY - pad) + " " + (w + pad * 2 + labelPad) + " " + (h + pad * 2)
  );
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");

  function degreeOf(id) {
    var d = 0;
    edges.forEach(function (e) { if (e.source === id || e.target === id) d++; });
    return d;
  }

  // Chord-diagram node labels stay short even for the few that show by
  // default (anchor/selected/hovered, see the `.node text` CSS) -- the
  // full title is always one hover away via the popover, the detail
  // panel, and the outline, so truncating here loses nothing a reader
  // can't get instantly from the same click/hover that revealed the
  // label in the first place. Character count, not byte length --
  // titles can contain non-ASCII text (the ES translations).
  function truncateLabel(s, maxChars) {
    var chars = Array.from(s);
    if (chars.length <= maxChars) { return s; }
    return chars.slice(0, maxChars).join("") + "…";
  }

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
    var pullBack = 0.55; // 0 = straight line, 1 = fully at center
    var cx = s.x + (t.x - s.x) / 2 + (centerX - (s.x + t.x) / 2) * pullBack;
    var cy = s.y + (t.y - s.y) / 2 + (centerY - (s.y + t.y) / 2) * pullBack;
    var d = "M " + s.x + " " + s.y + " Q " + cx + " " + cy + " " + t.x + " " + t.y;
    var path = el("path", { d: d, class: "edge", "data-source": e.source, "data-target": e.target });
    path.addEventListener("click", function () {
      selectNode(selectedId === e.source ? e.target : e.source);
    });
    edgeLayer.appendChild(path);
    edgePaths.push(path);
  });

  // --- Draw nodes. Radius floors at 12 (>=24px hit target diameter). ---
  var nodeLayer = el("g", { id: "node-layer" });
  svg.appendChild(nodeLayer);
  var nodeGroups = [];
  nodes.forEach(function (n) {
    var deg = degreeOf(n.id);
    var r = Math.max(12, (n.is_anchor ? 14 : 9) + Math.min(deg, 6) * 0.8);
    var g = el("g", {
      class: "node" + (n.is_anchor ? " node-anchor" : ""),
      "data-idx": n._idx,
      "data-kind": n.kind,
    });
    var circle = el("circle", { cx: n.x, cy: n.y, r: r });
    var text = el("text", { x: n.x + r + 3, y: n.y + 4 });
    text.textContent = truncateLabel(n["title_" + currentLang], 28);
    g.appendChild(circle);
    g.appendChild(text);
    g.addEventListener("mouseenter", function () { onHover(n, true); });
    g.addEventListener("mousemove", movePopover);
    g.addEventListener("mouseleave", function () { onHover(n, false); });
    g.addEventListener("click", function () { selectNode(n.id); });
    nodeLayer.appendChild(g);
    nodeGroups.push(g);
  });

  function groupFor(id) {
    var n = nodesById[id];
    return n ? nodeGroups[n._idx] : null;
  }

  // --- Hover popover (title via textContent, never innerHTML) ---
  function onHover(n, entering) {
    var g = groupFor(n.id);
    if (g) { g.classList.toggle("hovered", entering); }
    if (!entering) { popover.hidden = true; return; }
    popover.textContent = "";
    popover.appendChild(dom("div", { class: "popover-title" }, n["title_" + currentLang]));
    popover.appendChild(dom("div", { class: "popover-body" }, n["preview_" + currentLang]));
    popover.hidden = false;
  }
  function movePopover(ev) {
    popover.style.left = (ev.clientX + 14) + "px";
    popover.style.top = (ev.clientY + 14) + "px";
  }

  // --- Hover-preview on in-body links (org-roam-ui-style): every internal
  // link the org-link converter produces inside a rendered node body (an
  // <a> whose href is a fragment-style internal reference, not a real
  // URL) gets the exact same hover popover chord-diagram nodes already
  // have -- same onHover/movePopover, same popover element, same
  // nodesById lookup, nothing new to build. External https links never
  // match the fragment-prefix check, so they're excluded automatically,
  // and an internal reference that doesn't resolve in *this* curated
  // subgraph's nodesById (a real case -- not every link in a body's
  // source note happens to land inside whatever subgraph got exported)
  // is a silent no-op below, not an error.
  function wireBodyLinkPreviews(container) {
    var links = container.querySelectorAll("a[href^='#']");
    Array.prototype.forEach.call(links, function (a) {
      var n = nodesById[a.getAttribute("href").slice(1)];
      if (!n) { return; }
      a.addEventListener("mouseenter", function () { onHover(n, true); });
      a.addEventListener("mousemove", movePopover);
      a.addEventListener("mouseleave", function () { onHover(n, false); });
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

  function renderDetail(n) {
    detailContent.classList.add("fading");
    window.setTimeout(function () {
      detailContent.textContent = "";
      detailContent.appendChild(dom("span", { class: "kind-badge" }, n.kind));
      detailContent.appendChild(dom("h2", { class: "detail-title" }, n["title_" + currentLang]));
      if (n.is_anchor) {
        detailContent.appendChild(dom(
          "p", { class: "anchor-note" },
          "Starting point of this exported subgraph."
        ));
      }
      var body = dom("div", { class: "detail-body" });
      // n.body_en / n.body_es are pre-escaped HTML produced server-side by
      // mae-export's org renderer (crate::html_escape on every bit of real
      // node content, plus pre-rendered mermaid <svg>) -- this is the ONE
      // deliberate innerHTML assignment in this file; every other piece of
      // text above/below goes through textContent/dom() instead.
      body.innerHTML = n["body_" + currentLang];
      wireBodyLinkPreviews(body);
      detailContent.appendChild(body);
      renderLinkList(detailContent, "Links to", outgoingLinks(n.id));
      renderLinkList(detailContent, "Linked from", incomingLinks(n.id));
      renderOutline(body);
      detailContent.classList.remove("fading");
    }, 120);
  }

  function selectNode(id) {
    var n = nodesById[id];
    if (!n) { return; }
    if (selectedId != null) {
      var prevG = groupFor(selectedId);
      if (prevG) { prevG.classList.remove("selected"); }
    }
    selectedId = id;
    var g = groupFor(id);
    if (g) { g.classList.add("selected"); }
    edgePaths.forEach(function (p) {
      var incident = p.getAttribute("data-source") === id || p.getAttribute("data-target") === id;
      p.classList.toggle("incident", incident);
    });
    renderDetail(n);
  }
  homeBtn.addEventListener("click", function () { selectNode(anchorId); });

  // --- Suggested reading order (BFS distance from the anchor node) +
  // "Start here" walk ---
  function computeReadingOrder() {
    var adjacency = {};
    nodes.forEach(function (n) { adjacency[n.id] = []; });
    edges.forEach(function (e) {
      if (adjacency[e.source]) { adjacency[e.source].push(e.target); }
      if (adjacency[e.target]) { adjacency[e.target].push(e.source); }
    });
    var dist = {};
    nodes.forEach(function (n) { dist[n.id] = Infinity; });
    if (dist[anchorId] !== undefined) {
      dist[anchorId] = 0;
      var queue = [anchorId];
      while (queue.length) {
        var cur = queue.shift();
        (adjacency[cur] || []).forEach(function (next) {
          if (dist[next] === Infinity) { dist[next] = dist[cur] + 1; queue.push(next); }
        });
      }
    }
    var order = nodes.slice().sort(function (a, b) {
      if (dist[a.id] !== dist[b.id]) { return dist[a.id] - dist[b.id]; }
      var degA = degreeOf(a.id), degB = degreeOf(b.id);
      if (degA !== degB) { return degB - degA; }
      return a.id < b.id ? -1 : (a.id > b.id ? 1 : 0);
    });
    return order.map(function (n) { return n.id; });
  }
  // Previous/Next share one position in `readingOrder`, clamped (not
  // modulo-wrapped) at both ends -- Next stops at the last node instead of
  // silently wrapping back to the start, so the two controls behave like
  // ordinary pagination, each disabled exactly when it has nowhere to go.
  var readingOrder = computeReadingOrder();
  var walkIndex = -1;
  function updateWalkButtons() {
    prevBtn.disabled = walkIndex <= 0;
    startHereBtn.textContent = walkIndex === -1
      ? "Start here →"
      : (walkIndex >= readingOrder.length - 1 ? "✓ Done" : "Next →");
    startHereBtn.disabled = walkIndex >= readingOrder.length - 1;
  }
  startHereBtn.addEventListener("click", function () {
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

  // --- EN/ES toggle: swaps all visible text in place, instantly ---
  function applyLanguage() {
    nodeGroups.forEach(function (g, i) {
      var n = nodes[i];
      var t = g.querySelector("text");
      if (t) { t.textContent = truncateLabel(n["title_" + currentLang], 28); }
    });
    if (selectedId != null) { renderDetail(nodesById[selectedId]); }
    langToggle.textContent = currentLang === "en" ? "EN / ES → ES" : "ES / EN → EN";
  }
  langToggle.addEventListener("click", function () {
    currentLang = currentLang === "en" ? "es" : "en";
    applyLanguage();
  });

  // --- Dark/light theme toggle: overrides prefers-color-scheme via
  // documentElement[data-theme], which CSS already defines at matching
  // specificity (render_css_variables) -- background/color/fill/stroke
  // all carry a 180-200ms transition, so this reads as a smooth cross-
  // fade rather than a snap. ---
  var themeOrder = ["dark", "light"];
  var themeIdx = (window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches) ? 1 : 0;
  themeToggle.addEventListener("click", function () {
    themeIdx = (themeIdx + 1) % themeOrder.length;
    document.documentElement.setAttribute("data-theme", themeOrder[themeIdx]);
  });

  applyLanguage();
  // Auto-select the anchor/spine node on load so the accent + detail panel
  // are populated immediately, matching "Home" as a real default rather
  // than an empty-state page.
  selectNode(anchorId);
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

    // --- HtmlGraphExporter::export: serialization / structure ---

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
    fn graph_pane_theme_is_inverted_from_the_page_root() {
        // The chord widget deliberately renders in the opposite gruvbox mode
        // from the surrounding page -- assert both inverted, #graph-pane-
        // scoped blocks are present, and that the attribute-selector form
        // (which wins on specificity over the page root's own attribute
        // rule) pairs dark-page-root with the *light* accent inside the
        // widget, and vice versa.
        let nodes = vec![simple_node("a", "A", "body", true)];
        let html = HtmlGraphExporter.export(&nodes, &[], "a", "T");
        assert!(html.contains(":root[data-theme=\"dark\"] #graph-pane {"));
        assert!(html.contains(":root[data-theme=\"light\"] #graph-pane {"));

        let dark_root_widget_block = html
            .split(":root[data-theme=\"dark\"] #graph-pane {")
            .nth(1)
            .and_then(|s| s.split('}').next())
            .expect("dark-root widget block present");
        assert!(
            dark_root_widget_block.contains("--accent: #d65d0e"),
            "page in dark mode should give the widget the LIGHT-validated accent, got: {dark_root_widget_block}"
        );

        let light_root_widget_block = html
            .split(":root[data-theme=\"light\"] #graph-pane {")
            .nth(1)
            .and_then(|s| s.split('}').next())
            .expect("light-root widget block present");
        assert!(
            light_root_widget_block.contains("--accent: #fe8019"),
            "page in light mode should give the widget the DARK-validated accent, got: {light_root_widget_block}"
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
}
