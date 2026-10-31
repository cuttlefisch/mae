//! Renderers (ADR-112 D3): functions of the tree and its [`Semantics`].
//!
//! One builder ([`render`]) turns a document into a host-neutral element
//! tree ([`Node`]); HTML ([`to_html`]), hast JSON ([`to_hast`]) and search
//! text ([`text`]) are readings of that tree, so they cannot disagree about
//! what is shown. The org source a node is edited in is [`node_source`].
//!
//! The guarantees, each pinned by `tests/render_*.rs`:
//! - **Closure.** Every `href` is a [`Resolver`]-supplied target (validated:
//!   no scheme but `http`, `https` or `mailto`, never protocol-relative), an
//!   `http:`/`https:`/`mailto:` URL, or `#ID` of an id in the same output.
//!   Any other link renders as its description, or as
//!   [`Resolver::placeholder`] when it has none — never as its target. An
//!   `src` is an `http:`/`https:` URL, only with [`RenderOptions::images`].
//! - **Allowlist by construction.** Tags and attributes are the enums
//!   [`Tag`] and [`Attr`] ([`ATTRIBUTES`] says which tag carries which), and
//!   every attribute value is validated or built by this module. No raw
//!   HTML in any form passes: `#+HTML:`, export snippets and export blocks
//!   are never rendered, and affiliated `#+ATTR_*` keywords are not read.
//! - **Unrendered regions contribute nothing.** The renderer skips exactly
//!   what [`Semantics::unrendered`] lists (the decision `semantics()` made,
//!   reused, never re-derived), and draws footnotes only from
//!   [`Semantics::footnotes`].
//! - **Deterministic.** The same document, semantics, options and resolver
//!   give the same tree, byte for byte, natively and in WASM.
//!
//! Per construct (what is shown, and what is deliberately not):
//!
//! | Construct | Rendered as |
//! |---|---|
//! | headline | `<section class="org-section org-level-N">` with `<hN id>` (h6 below 6): TODO keyword `<span class="org-todo">` (+ `org-done`), priority `<span class="org-priority">`, title, tags `<span class="org-tags">` of `<span class="org-tag">` |
//! | section, zeroth section | its elements, unwrapped |
//! | paragraph | `<p>` |
//! | bold, italic, underline, strike-through | `<strong>`, `<em>`, `<span class="org-underline">`, `<del>` |
//! | code, verbatim | `<code>`, `<code class="org-verbatim">` (contents verbatim) |
//! | link | see Closure; a KB link carries `data-org-rel`/`-w`/`-c` |
//! | plain list | `<ul>`/`<ol>` of `<li>` (`value` from `[@N]`), `<dl>` of `<dt>`/`<dd>` when the first item has a tag; checkbox `<code class="org-checkbox">[X]</code>` |
//! | table | `<table>`; the first row group is `<thead>` (`<th scope="col">`) when a rule separates it from another group (Org's rule); each later group a `<tbody>`; `#+TBLFM` omitted; a table.el table `<pre class="org-table-el">` |
//! | quote, center, special, dynamic block | `<blockquote>`, `<div class="org-center">`, `<div class="org-special-block" data-org-block>`, `<div class="org-dynamic-block">` |
//! | verse block | `<p class="org-verse">`, lines joined by `<br>`, leading spaces as no-break spaces |
//! | src, example block, fixed width | `<pre class="org-src|org-example|org-fixed-width"><code>`, verbatim (Org's comma escapes removed; `: ` prefixes removed); src gets `language-L` for a valid language name. Never highlighted, never evaluated |
//! | caption (`#+CAPTION`) | a table's `<caption>`; around a paragraph or src/example block, `<figure class="org-figure">` with `<figcaption>` |
//! | horizontal rule | `<hr>` |
//! | footnote reference | `<sup class="org-footnote-ref"><a id="fnr-N" href="#fn-N">N</a></sup>`; definitions in `<section class="org-footnotes">` as `<div class="org-footnote" id="fn-N">`, numbered in order of first reference; a reference with no definition is its text |
//! | timestamp | `<time class="org-timestamp">` with its text |
//! | entity | its UTF-8 (`org-entities`) |
//! | subscript, superscript | `<sub>`, `<sup>` |
//! | line break | `<br>` |
//! | target, radio target | `<span class="org-target" id="t-…">` (empty), `<span class="org-radio-target" id="r-…">` text |
//! | radio link | `<a class="org-radio" href="#r-…">`, or its text if the target is not rendered |
//! | statistics cookie | `<span class="org-cookie">` |
//! | LaTeX fragment, environment | `<code class="org-latex">`, `<pre class="org-latex-environment"><code>`: the source as text, never MathML |
//! | inline src block | `<code class="org-inline-src">` with its body (+ `language-L`) |
//! | macro, citation | `<span class="org-macro">`, `<span class="org-citation">`: the source as text (not expanded) |
//! | `#+TRANSCLUDE:` | `<p class="org-transclusion">` holding a link to its member |
//! | property drawer | omitted unless [`RenderOptions::property_drawers`]: `<dl class="org-properties">`, values plain text with only bracket links as links; always in [`Rendered::nodes`] as data |
//! | omitted (as Org's export omits them by default) | keywords, planning, clock, diary sexp, babel call, inline babel call, export snippet, comment, comment and export blocks, non-property drawers, affiliated keywords other than CAPTION |
//! | omitted with a [`RenderDiagnostic`] | content past [`RENDER_MAX_DEPTH`], and text the parser left opaque (its nesting cap) or unplaced: unparsed text is never shown, as it may hold an unrendered region |

mod anchors;
mod build;
mod hast;
mod html;
mod links;
mod meta;
mod notes;
mod objects;
mod source;
mod structure;
mod text;
mod tree;

pub use anchors::{encode_id, is_valid_id, slug, SLUG_MAX};
pub(crate) use hast::json_str;
pub use hast::to_hast;
pub use html::to_html;
pub use links::{is_valid_language, safe_external_url, safe_resolver_href};
pub use source::{node_source, normalise, with_line_ending};
pub use text::{node_text, text, text_of};
pub use tree::{Attr, Element, Node, Tag, ATTRIBUTES, CLASSES, TAGS};

use crate::{Document, Semantics, Span};
use std::collections::HashMap;

/// How headlines get their anchor ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HeadlineIds {
    /// A KB node's `:ID:` (`id-…`), else its `CUSTOM_ID` (`cid-…`), else a
    /// slug of its title (`h-…`).
    #[default]
    Explicit,
    /// A slug of the title (`h-…`) for every headline.
    Slug,
}

/// What the caller decides about the output.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct RenderOptions {
    pub headline_ids: HeadlineIds,
    /// Render a description-less `http:`/`https:` link to an image
    /// (`.png`, `.jpg`, `.jpeg`, `.gif`, `.svg`, `.webp`, `.avif`) as
    /// `<img>`. Default off. A local image is never an image.
    pub images: bool,
    /// Render property drawers as `<dl class="org-properties">`. Default
    /// off, as in Org's export; [`Rendered::nodes`] carries them either way.
    pub property_drawers: bool,
}

impl RenderOptions {
    pub fn new(headline_ids: HeadlineIds, images: bool, property_drawers: bool) -> RenderOptions {
        RenderOptions {
            headline_ids,
            images,
            property_drawers,
        }
    }
}

/// Where a KB link goes: caller-supplied data, never a callback (no
/// re-entrancy across WASM).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The link target. Validated with [`safe_resolver_href`]: a target
    /// it refuses leaves the link unresolved.
    pub href: String,
    /// The text of a link without a description.
    pub title: Option<String>,
}

/// KB node id → [`Resolved`], and the text of an unresolved link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolver {
    pub targets: HashMap<String, Resolved>,
    /// The text of a link that resolves nowhere and has no description.
    pub placeholder: String,
}

/// [`Resolver::placeholder`]'s default.
pub const DEFAULT_PLACEHOLDER: &str = "[?]";

impl Default for Resolver {
    fn default() -> Self {
        Resolver {
            targets: HashMap::new(),
            placeholder: DEFAULT_PLACEHOLDER.to_string(),
        }
    }
}

/// Content the renderer left out rather than guess at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum RenderDiagnostic {
    /// Elements or objects nested deeper than [`RENDER_MAX_DEPTH`].
    TooDeep(Span),
    /// Text the parser left opaque (its nesting cap) or could not place.
    Unparsed(Span),
}

impl RenderDiagnostic {
    /// The name used in JSON.
    pub fn kind_name(&self) -> &'static str {
        match self {
            RenderDiagnostic::TooDeep(_) => "too_deep",
            RenderDiagnostic::Unparsed(_) => "unparsed",
        }
    }

    pub fn span(&self) -> Span {
        match self {
            RenderDiagnostic::TooDeep(s) | RenderDiagnostic::Unparsed(s) => *s,
        }
    }
}

/// How deep the renderer descends (elements and objects together); deeper
/// content is omitted with a [`RenderDiagnostic::TooDeep`]. Bounds the
/// renderer's recursion, so a hostile document cannot exhaust a stack.
pub const RENDER_MAX_DEPTH: usize = 256;

/// A KB node's rendered metadata: titles and aliases through the same inline
/// path as the body (inline elements only: no block element, no id, no
/// footnote, and no same-page link, since it may be shown on another page).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeMeta {
    pub id: String,
    /// The node's anchor id on this page; `None` for the file node.
    pub anchor: Option<String>,
    pub title: Vec<Node>,
    pub aliases: Vec<Vec<Node>>,
    /// The node's properties (canonical keys, as [`crate::SemanticNode`]),
    /// each value plain text in which only bracket links are links.
    pub properties: Vec<(String, Vec<Node>)>,
}

/// A rendered document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The page body: the children of the hast root.
    pub body: Vec<Node>,
    /// One per KB node, in [`Semantics::nodes`] order.
    pub nodes: Vec<NodeMeta>,
    /// Sorted by span, then kind.
    pub diagnostics: Vec<RenderDiagnostic>,
}

/// Which serialisation [`Rendered::to_json`] embeds trees as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeFormat {
    /// Each tree as a hast root object.
    Hast,
    /// Each tree as an HTML string.
    Html,
}

/// Renders `doc` (whose semantics are `sem`).
pub fn render(
    doc: &Document,
    sem: &Semantics,
    options: &RenderOptions,
    resolver: &Resolver,
) -> Rendered {
    let page = build::page(doc, sem, options, resolver, build::Mode::Page, None);
    let nodes = meta::nodes(doc, sem, options, resolver, &page.table);
    let mut diagnostics = page.diagnostics;
    diagnostics.sort();
    diagnostics.dedup();
    Rendered {
        body: page.body,
        nodes,
        diagnostics,
    }
}

/// The body of [`render`] as HTML.
pub fn render_html(
    doc: &Document,
    sem: &Semantics,
    options: &RenderOptions,
    resolver: &Resolver,
) -> String {
    to_html(&render(doc, sem, options, resolver).body)
}

/// The body of [`render`] as a hast root (JSON).
pub fn render_hast(
    doc: &Document,
    sem: &Semantics,
    options: &RenderOptions,
    resolver: &Resolver,
) -> String {
    to_hast(&render(doc, sem, options, resolver).body)
}

/// Text rendered inline-only, as [`NodeMeta::title`] is: `text` parsed as
/// Org objects (a title, an alias, any secondary string).
pub fn render_inline(text: &str, options: &RenderOptions, resolver: &Resolver) -> Vec<Node> {
    meta::inline(text, options, resolver)
}

impl Rendered {
    /// The whole result as JSON: `{"body": T, "nodes": [{"id", "anchor",
    /// "title": T, "aliases": [T], "properties": [{"key", "value": T}]}],
    /// "diagnostics": [{"kind", "start", "end"}]}`, each tree `T` in
    /// `format`.
    pub fn to_json(&self, format: TreeFormat) -> String {
        hast::rendered_json(self, format)
    }
}
