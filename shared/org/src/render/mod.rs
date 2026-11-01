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
//! - **What Org's export removes for babel's `:exports` is not rendered.**
//!   A src block, inline src block or existing results that `:exports`
//!   removes (resolved as Org resolves it: the block, `#+HEADER:`,
//!   inherited `header-args[:LANG]` properties and `#+PROPERTY:`; never
//!   evaluated) is skipped. This decision is the renderer's, as it is
//!   export's in Org: [`Semantics`] still reads such an element.
//! - **Hidden means hidden.** A skipped element takes everything attached
//!   to it: its affiliated keywords (a `#+CAPTION:`, and the targets and
//!   footnote references in one) are not rendered, anchored or numbered.
//! - **Deterministic.** The same document, semantics, options and resolver
//!   give the same tree, byte for byte, natively and in WASM.
//! - **Macros.** What renders is the document as Org's export reads it:
//!   its macros expanded as `org-export-as` expands them (COMMENT subtrees
//!   dropped first, templates collected, each macro replaced where Org
//!   finds one; after babel, each results macro it keeps replaced by its
//!   argument), then the expanded text parsed again. Whatever an expansion
//!   makes (a comment, a COMMENT headline, a `:noexport:` tag, an inline
//!   src block a later `}` completes) is read as Org reads it. Where Org's
//!   export would fail (an undefined macro, a circular expansion) or MAE
//!   cannot expand from the text alone (an `(eval …)` template, a lookup
//!   outside the file) or within a bound linear in the source, the render
//!   is EMPTY but for a [`RenderDiagnostic::MacroFailed`]. Every span still
//!   points into the source; heading anchors are made from the titles as
//!   written, every macro contributing nothing. [`Semantics`] stays the
//!   source's (registered: `macros`).
//!
//! Per construct (what is shown, and what is deliberately not):
//!
//! | Construct | Rendered as |
//! |---|---|
//! | headline | `<section class="org-section org-level-N">` (with `id` = the node's or custom id under [`HeadlineIds::Explicit`]) holding `<hN id="h-SLUG">` (`h` for a title with no letter or digit, a repeat `h-SLUG--2`, `--3`, …; h6 below 6; see [`heading_anchor`] and [`slug`]: the title AS WRITTEN rendered inline-only, babel objects and macros dropped (so no `#+MACRO:` line moves an anchor), its letters and digits of any script case-folded by Emacs's table): TODO keyword `<span class="org-todo">` (+ `org-done`), priority `<span class="org-priority">`, title, tags `<span class="org-tags">` of `<span class="org-tag">` |
//! | section, zeroth section | its elements, unwrapped |
//! | paragraph | `<p>` |
//! | bold, italic, underline, strike-through | `<strong>`, `<em>`, `<span class="org-underline">`, `<del>` |
//! | code, verbatim | `<code>`, `<code class="org-verbatim">` (contents verbatim) |
//! | link | see Closure; a KB link carries `data-org-rel`/`-w`/`-c` |
//! | plain list | `<ul>`/`<ol>` of `<li>` (`value` from `[@N]`), `<dl>` of `<dt>`/`<dd>` when the first item has a tag; checkbox `<code class="org-checkbox">[X]</code>` |
//! | table | `<table>`; the first row group is `<thead>` (`<th scope="col">`) when a rule separates it from another group (Org's rule); each later group a `<tbody>`; `#+TBLFM` omitted; a table.el table `<pre class="org-table-el">`. Org's special rows (a `/` column-group row, a row of only `<l>`/`<r>`/`<c>`/`<N>` cookies and empty cells, a `^ _ $ !` row in a table with a special column) and its special column (every first cell empty or one of `/ # ! $ * _ ^`) are dropped before the header is found, as ox drops them; column groups are not rendered as `<colgroup>` |
//! | quote, center, special, dynamic block | `<blockquote>`, `<div class="org-center">`, `<div class="org-special-block" data-org-block>`, `<div class="org-dynamic-block">` |
//! | verse block | `<p class="org-verse">`, lines joined by `<br>`; the indentation all its non-blank lines share removed (Org's `org-remove-indentation`), the rest as no-break spaces, one per column |
//! | src, example block, fixed width | `<pre class="org-src|org-example|org-fixed-width"><code>`, verbatim (Org's comma escapes removed; `: ` prefixes removed); src gets `language-L` for a valid language name. Never highlighted, never evaluated. A src block whose `:exports` is `none` or `results` is not rendered, nor are the existing results (`#+RESULTS:`) of one whose `:exports` is `none` or `code` |
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
//! | inline src block | `<code class="org-inline-src">` with its body (+ `language-L`) when its `:exports` is `code` or `both`; by Org's inline default (`results`) nothing. Its `{{{results(…)}}}` macro is removed unless `:exports` keeps results, and then (as any results macro babel keeps) replaced by its first argument before the page is parsed |
//! | macro | expanded as Org's export expands it, before anything is rendered (see **Macros** above); a macro left in the parse (in a property value, under a COMMENT headline) renders nothing |
//! | citation | `<span class="org-citation">`: the source as text; one holding an inline src block, babel call, export snippet, results macro or macro only its keys (fail closed) |
//! | `#+TRANSCLUDE:` | `<p class="org-transclusion">` holding a link to its member |
//! | property drawer | omitted unless [`RenderOptions::property_drawers`]: `<dl class="org-properties">`, one row per key with its value as [`Rendered::nodes`] reads it (`KEY` then every `KEY+` joined, `nil` none), plain text with only bracket links as links; always in [`Rendered::nodes`] as data |
//! | omitted (as Org's export omits them by default) | keywords, planning, clock, diary sexp, babel call, inline babel call, export snippets and blocks for other backends, comment, comment blocks |
//! | omitted (MAE's choice, unlike Org: see below) | every drawer but the property drawer (ADR-112 D2; Org's default exports all but `LOGBOOK`), `#+HTML:` and HTML export blocks and snippets (D3), affiliated keywords other than CAPTION (`#+ATTR_*` included) |
//! | omitted with a [`RenderDiagnostic`] | content past [`RENDER_MAX_DEPTH`], and text the parser left opaque (its nesting cap) or unplaced: unparsed text is never shown, as it may hold an unrendered region |

//!
//! Every deliberate difference from Org's own HTML export
//! (`org-html-export-as-html`) in what is shown, hidden or linked is listed,
//! with its reason and pinning test, in
//! `conformance/render-divergences.toml` (checked by
//! `tests/render_divergences.rs`).

mod anchors;
mod babel;
mod build;
mod expand;
mod hast;
mod html;
mod links;
mod meta;
mod notes;
mod objects;
mod prepared;
mod source;
mod structure;
mod text;
mod tree;

pub use anchors::{encode_id, heading_anchor, is_valid_id, slug, SLUG_MAX};
pub(crate) use hast::json_str;
pub use hast::to_hast;
pub use html::to_html;
pub use links::{is_valid_language, safe_external_url, safe_resolver_href};
pub use source::{
    node_source, normalise, replace_body, with_line_ending, BodyEditError, DECLARATION_KEYWORDS,
};
pub use text::{node_text, text, text_of};
pub use tree::{Attr, Element, Node, Tag, ATTRIBUTES, CLASSES, TAGS};

use crate::{Document, Semantics, Span};
use std::collections::HashMap;

/// Which anchors a headline gets besides the one every rendered headline's
/// heading carries, `h-` + [`slug`] of its title ([`heading_anchor`]),
/// which is where a KB link's `#fragment` lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum HeadlineIds {
    /// The section also carries the KB node's `:ID:` (`id-…`), else the
    /// `CUSTOM_ID` (`cid-…`), when there is one: links to the node or the
    /// custom id land there.
    #[default]
    Explicit,
    /// Only the heading's `h-` anchor; links to the node or custom id land
    /// on it.
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

/// Where a KB link goes: caller-supplied data, never a callback (no
/// re-entrancy across WASM).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Resolved {
    /// The link target. Validated with [`safe_resolver_href`]: a target
    /// it refuses leaves the link unresolved.
    pub href: String,
    /// The text of a link without a description.
    pub title: Option<String>,
}

/// KB node id → [`Resolved`], and the text of an unresolved link.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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

/// Content the renderer left out rather than guess at, or substituted.
/// Spans are in the SOURCE text (the text the document was parsed from),
/// whatever its macros expanded to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum RenderDiagnostic {
    /// Elements or objects nested deeper than [`RENDER_MAX_DEPTH`].
    TooDeep(Span),
    /// Text the parser left opaque (its nesting cap) or could not place.
    Unparsed(Span),
    /// The document's macros could not be expanded as Org's export expands
    /// them (at the macro the span covers): as Org's export produces no
    /// output then, the render is EMPTY (no body, no node metadata) and
    /// this is its only diagnostic but [`RenderDiagnostic::MacroEnvironment`].
    MacroFailed(MacroFailure, Span),
    /// A `{{{time(…)}}}` or `{{{modification-time(…)}}}` macro expanded
    /// with a fixed time (1970-01-01 00:00:00 UTC), or an
    /// `{{{input-file}}}` with nothing: Org reads the clock or the file
    /// there, and MAE's output depends on the text only (ADR-112 D4).
    MacroEnvironment(Span),
}

/// Why a document's macros could not be expanded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum MacroFailure {
    /// A macro with no template (`Undefined Org macro`, as in Org).
    Undefined,
    /// The same macro, with the same arguments, at the same place again
    /// (`Circular macro expansion`, as in Org).
    Circular,
    /// An `(eval …)` template: Org evaluates Lisp, MAE never does.
    Eval,
    /// A macro whose value MAE cannot know from the text: `property` with
    /// a search location or of a special property, `date` with a format,
    /// a `time` format outside the supported directives, `keyword` or
    /// `property` without a name, a counter past `u64`.
    Unsupported,
    /// The expansion would do more work, or grow larger, than a bound
    /// linear in the source allows (or nest deeper than it).
    Budget,
}

impl RenderDiagnostic {
    /// The name used in JSON.
    pub fn kind_name(&self) -> &'static str {
        match self {
            RenderDiagnostic::TooDeep(_) => "too_deep",
            RenderDiagnostic::Unparsed(_) => "unparsed",
            RenderDiagnostic::MacroFailed(f, _) => match f {
                MacroFailure::Undefined => "macro_undefined",
                MacroFailure::Circular => "macro_circular",
                MacroFailure::Eval => "macro_eval",
                MacroFailure::Unsupported => "macro_unsupported",
                MacroFailure::Budget => "macro_budget",
            },
            RenderDiagnostic::MacroEnvironment(_) => "macro_environment",
        }
    }

    pub fn span(&self) -> Span {
        match self {
            RenderDiagnostic::TooDeep(s)
            | RenderDiagnostic::Unparsed(s)
            | RenderDiagnostic::MacroFailed(_, s)
            | RenderDiagnostic::MacroEnvironment(s) => *s,
        }
    }

    /// The same diagnostic at span `s`.
    pub(crate) fn at(self, s: Span) -> RenderDiagnostic {
        match self {
            RenderDiagnostic::TooDeep(_) => RenderDiagnostic::TooDeep(s),
            RenderDiagnostic::Unparsed(_) => RenderDiagnostic::Unparsed(s),
            RenderDiagnostic::MacroFailed(f, _) => RenderDiagnostic::MacroFailed(f, s),
            RenderDiagnostic::MacroEnvironment(_) => RenderDiagnostic::MacroEnvironment(s),
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
#[non_exhaustive]
pub struct NodeMeta {
    pub id: String,
    /// The node's anchor id on this page; `None` for the file node.
    pub anchor: Option<String>,
    pub title: Vec<Node>,
    /// The title with every link replaced by what it shows (its
    /// description, else its resolved title or the placeholder), and no
    /// `<a>` anywhere: for a consumer that wraps the title in its own link,
    /// which must not hold another (an HTML parser would split them).
    pub title_unlinked: Vec<Node>,
    pub aliases: Vec<Vec<Node>>,
    /// The node's properties (canonical keys, as [`crate::SemanticNode`]),
    /// each value plain text in which only bracket links are links.
    pub properties: Vec<(String, Vec<Node>)>,
}

/// A rendered document.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
#[non_exhaustive]
pub enum TreeFormat {
    /// Each tree as a hast root object.
    Hast,
    /// Each tree as an HTML string.
    Html,
}

/// Renders `doc` (whose semantics are `sem`): its macros expanded as
/// Org's export expands them, then the expanded text parsed and rendered
/// (see `expand`). When they cannot be, the render is empty but for a
/// [`RenderDiagnostic::MacroFailed`].
pub fn render(
    doc: &Document,
    sem: &Semantics,
    options: &RenderOptions,
    resolver: &Resolver,
) -> Rendered {
    let p = prepared::prepare(doc, sem);
    let mut diagnostics = p.notes.clone();
    let (body, nodes) = match p.failed {
        Some(f) => {
            diagnostics.push(f);
            (Vec::new(), Vec::new())
        }
        None => {
            let cx = p.cx();
            let page = build::page_with(&p.doc, cx, options, resolver, build::Mode::Page, None);
            let nodes = meta::nodes(&p, sem, options, resolver, &page.table);
            diagnostics.extend(page.diagnostics.into_iter().map(|d| p.to_source(d)));
            (page.body, nodes)
        }
    };
    diagnostics.sort();
    diagnostics.dedup();
    Rendered {
        body,
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
    /// "title": T, "title_unlinked": T, "aliases": [T], "properties":
    /// [{"key", "value": T}]}],
    /// "diagnostics": [{"kind", "start", "end"}]}`, each tree `T` in
    /// `format`.
    pub fn to_json(&self, format: TreeFormat) -> String {
        hast::rendered_json(self, format)
    }
}
