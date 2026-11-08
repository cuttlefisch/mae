//! mae-org: MAE's org-mode parser (ADR-112).
//! @stability: experimental
//!
//! [`parse`] turns org text into a **lossless** syntax tree: every input byte
//! belongs to exactly one token, so printing an unedited tree reproduces the
//! input byte for byte — CRLF, a BOM and a missing final newline included.
//! Typed views ([`Headline`], [`PropertyDrawer`], [`Block`], [`Keyword`]) sit
//! on top and read only from the tree. [`parse_fragment`] parses a headline's
//! body; [`Document::contents`] gives each element's contents range.
//!
//! Element extents follow `org-element` (Org 9.7), the conformance oracle:
//! affiliated keywords and trailing blank lines live inside the element they
//! belong to, so the Emacs differential (`conformance/`) needs no mapping
//! layer. Intended differences are listed, with the test that pins each, in
//! `conformance/divergences.toml`.
//!
//! Elements (phase O1) and inline objects (O2: emphasis, links, radio
//! links, timestamps, entities, citations …) are both in the tree; the typed
//! [`Link`] view reports a link as `org-element` does. [`semantics`] is
//! MAE's KB projection of the tree (O2), and the renderers (O3: [`render`],
//! [`to_html`], [`to_hast`], [`text`], [`normalise`], [`node_source`])
//! draw only what it renders.
//!
//! **Source is not display text.** [`semantics`] reads titles, aliases,
//! property values, link descriptions and link fragments as plain SOURCE
//! text, so an inline
//! src block, a babel call or an export snippet the export hides is spelled
//! out in them. Anything shown to a reader (a page, a title in a list, a
//! search result) comes from the renderers ([`NodeMeta`]'s `title`,
//! `title_unlinked`, `aliases` and `properties`, [`node_text`], [`text`]),
//! which hide exactly what the export hides; never from [`semantics`].
//!
//! **Offsets.** Every span this crate reports, in Rust and in its JSON
//! ([`Span`], diagnostics, semantics, render diagnostics), is a UTF-8 BYTE
//! offset into the text that was parsed. A JavaScript consumer must slice
//! the UTF-8 bytes (`TextEncoder`), not the string, which counts UTF-16
//! code units.
//!
//! **Versioning.** The crate has its own version, independent of MAE's
//! workspace version, and [`API_VERSION`] is it: the renderers' output, the
//! JSON shapes and the WASM facade's ABI are the contract the
//! `mae-org-vX.Y.Z` tags name. Every option and result type is
//! `#[non_exhaustive]` and built from `Default` (see each type's `with_*`
//! setters), so adding a field is a minor version.
//!
//! Properties the crate guarantees, each pinned by tests:
//! - **Total:** `parse` never panics and never fails; malformed input
//!   becomes text plus [`Diagnostic`]s.
//! - **Linear-ish:** every forward search is a lookup in an index built in
//!   one pass, so hostile input cannot make the parse quadratic.
//! - **No recursion on input nesting:** containers are an explicit stack,
//!   capped by [`ParseOptions::max_depth`], itself clamped to
//!   [`MAX_DEPTH_CEILING`] because rowan drops and compares a tree
//!   recursively (one frame per level).
//! - **Emacs's reading of the bytes:** the line-ending convention
//!   ([`LineEnding`]), character classes and case folding are Emacs's own,
//!   the latter generated from Emacs by `conformance/gen-char-tables.el`.
//! - **Deterministic and configuration-as-data:** the tree is a function of
//!   the text and [`ParseOptions`] only (ADR-112 D4). No I/O.

mod blocks;
mod builder;
mod builders;
mod chars;
mod chartables;
mod diagnostic;
mod element;
mod entities;
mod green;
mod headline;
mod highlight;
pub use highlight::{
    highlights, section_highlights, sections, Highlight, NeedsFull, Section, SectionHighlights,
};
mod index;
mod inline;
mod json;
pub use json::json_string;
mod kind;
mod leaf;
mod link_grammar;
mod list;
mod options;
mod paragraph;
mod render;
mod scan;
mod semantics;
mod span;
mod text;
mod tokens;
mod views;

/// The version of mae-org's public contract: the renderers' output, the
/// JSON shapes and the WASM facade's ABI. The `mae-org-vX.Y.Z` tags name it.
/// It IS the crate's own version (`shared/org/Cargo.toml`), which MAE's
/// release bump does not touch: the workspace version changes on every MAE
/// release whether or not this contract did.
pub const API_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The Emacs version the character classes and case folding were generated
/// from (`conformance/gen-char-tables.el`).
pub use chartables::EMACS_VERSION as CHAR_TABLES_EMACS_VERSION;
pub use diagnostic::{Diagnostic, DiagnosticKind};
pub use kind::{OrgLanguage, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};
pub use link_grammar::{
    classify_link, classify_link_with, LinkClass, LinkKind, LinkTypes, DEFAULT_REL, MAIL_SCHEMES,
    OTHER_SCHEMES, WEB_SCHEMES,
};
pub use options::{
    ParseOptions, TodoSequence, DEFAULT_LINK_TYPES, DEFAULT_MAX_DEPTH, MAX_DEPTH_CEILING,
};
pub use render::{
    encode_id, heading_anchor, is_valid_id, is_valid_language, node_source, node_text, normalise,
    render, render_hast, render_html, render_inline, replace_body, safe_external_url,
    safe_resolver_href, slug, text, text_of, to_hast, to_html, with_line_ending, Attr,
    BodyEditError, Element, HeadlineIds, MacroFailure, Node, NodeMeta, RenderDiagnostic,
    RenderOptions, Rendered, Resolved, Resolver, Tag, TreeFormat, ATTRIBUTES, CLASSES,
    DECLARATION_KEYWORDS, DEFAULT_PLACEHOLDER, RENDER_MAX_DEPTH, SLUG_MAX, TAGS,
};
/// The tree is a rowan tree; `node.text_range()` returns rowan types. The
/// re-export makes that coupling explicit and versioned: plain-data results
/// (diagnostics, contents) use [`Span`] instead, so a JSON/WASM consumer never
/// needs rowan.
pub use rowan;
pub use semantics::{
    semantics, semantics_with, ArchivedTrees, NodeOrigin, Region, RenderedFootnote,
    SemanticDiagnostic, SemanticLink, SemanticNode, Semantics, SemanticsOptions, Transclusion,
    Unrendered, LINK_EXCLUDE_KEYWORDS, ORG_EXPORT_SELECT_TAGS,
};
pub use span::Span;
pub use text::LineEnding;
pub use views::{
    Block, Headline, Keyword, Link, LinkAbbrevs, LinkFormat, LinkInfo, Property, PropertyDrawer,
};

use rowan::GreenNode;

/// A parsed org document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    green: GreenNode,
    diagnostics: Vec<Diagnostic>,
    todo_sequences: Vec<TodoSequence>,
    line_ending: LineEnding,
    contents: span::ContentsTable,
    radio_targets: Vec<String>,
    /// The node kinds the tree holds.
    kinds: kind::KindSet,
    /// How the text was read, so a text derived from it (its macros
    /// expanded) is read the same way.
    reading: Reading,
}

/// What [`Document::reparse`] reads a derived text with: the options, the
/// start (a document or a body) and the fragment context of the parse.
#[derive(Debug, Clone)]
struct Reading {
    options: ParseOptions,
    start: builder::Start,
    fragment: Option<FragmentContext>,
}

/// Not part of what a document IS: two parses with the same tree,
/// diagnostics and readings are equal however they were asked for.
impl PartialEq for Reading {
    fn eq(&self, _: &Reading) -> bool {
        true
    }
}

impl Eq for Reading {}

/// Parses `text`. Total: any input gives a document whose tree prints back
/// to `text`, except inputs over `u32::MAX` bytes, which give an empty
/// document with an [`DiagnosticKind::InputTooLarge`] diagnostic.
pub fn parse(text: &str, options: &ParseOptions) -> Document {
    parse_with(
        text::Source::new(text),
        options,
        builder::Start::Document,
        None,
    )
}

/// What a fragment cannot know from its own text: where it sits in its file,
/// and what the rest of the file declares that its parse reads. Built from
/// the file's parse ([`FragmentContext::from_document`]) or field by field
/// ([`FragmentContext::new`] and the `with_*` setters).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct FragmentContext {
    /// The level of the headline whose body this is; 0 for the text before
    /// the first headline (the start of the file).
    pub level: usize,
    /// How many headlines the FILE has above the body, its own included
    /// (its ancestors are not every lower level: `* a` then `*** b` makes
    /// `b` the second). The file's nesting cap
    /// ([`ParseOptions::max_depth`]) counts them, so the fragment's
    /// contents nest exactly as there only with this set. 0 at level 0.
    pub ancestors: usize,
    /// The FILE's line-ending convention ([`LineEnding::detect`] on the whole
    /// file, or [`Document::line_ending`] of its parse). A fragment's own
    /// endings can say otherwise: a CRLF body in a file with one LF line is a
    /// unix file, in which every CR is content.
    pub line_ending: LineEnding,
    /// The FILE's radio targets ([`Document::radio_targets`]): text anywhere
    /// in the file matching one is a radio link, wherever the target is.
    /// `None`: the fragment's own targets only.
    pub radio_targets: Option<Vec<String>>,
    /// The TODO sequences in force in the FILE ([`Document::todo_sequences`]):
    /// they replace both `ParseOptions::todo_sequences` and any `#+TODO:` in
    /// the fragment, which the file's already include. `None`: the options',
    /// unless the fragment declares its own.
    pub todo_sequences: Option<Vec<TodoSequence>>,
}

impl FragmentContext {
    /// A body at `level` in a file with `line_ending`, nothing else known
    /// (as many ancestors as the level says: a headline whose every level
    /// above it is a headline).
    pub fn new(level: usize, line_ending: LineEnding) -> FragmentContext {
        FragmentContext {
            level,
            ancestors: level,
            line_ending,
            radio_targets: None,
            todo_sequences: None,
        }
    }

    /// The context of the body of `headline` (`None`: the text before the
    /// first headline) in `doc`, the parse of its whole file: its level and
    /// ancestors, and the file's line ending, radio targets and TODO
    /// sequences, so [`parse_fragment`] reads the body exactly as `doc`'s
    /// parse did.
    pub fn from_document(doc: &Document, headline: Option<&Headline>) -> FragmentContext {
        let (level, ancestors) = headline.map_or((0, 0), |h| {
            let n = h
                .syntax()
                .ancestors()
                .filter(|a| a.kind() == SyntaxKind::HEADLINE);
            (h.level(), n.count())
        });
        FragmentContext {
            level,
            ancestors,
            line_ending: doc.line_ending(),
            radio_targets: Some(doc.radio_targets().to_vec()),
            todo_sequences: Some(doc.todo_sequences().to_vec()),
        }
    }

    pub fn with_level(mut self, level: usize) -> Self {
        self.level = level;
        self
    }

    pub fn with_ancestors(mut self, ancestors: usize) -> Self {
        self.ancestors = ancestors;
        self
    }

    pub fn with_line_ending(mut self, line_ending: LineEnding) -> Self {
        self.line_ending = line_ending;
        self
    }

    pub fn with_radio_targets<S: Into<String>>(
        mut self,
        targets: impl IntoIterator<Item = S>,
    ) -> Self {
        self.radio_targets = Some(targets.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_todo_sequences(mut self, sequences: Vec<TodoSequence>) -> Self {
        self.todo_sequences = Some(sequences);
        self
    }
}

/// Parses `text` as the BODY of a headline: what follows the headline line in
/// its file (ADR-112 D1, M4). With [`FragmentContext::from_document`] it
/// parses exactly as the whole file parsed that text — planning and a
/// property drawer are recognised at its top, radio links to targets
/// anywhere in the file are made, the file's TODO keywords apply — with
/// offsets relative to `text`. A headline at or above `context.level` (one
/// that would end the body in the file) is parsed and reported as
/// [`DiagnosticKind::HeadlineOutsideFragment`].
///
/// What a fragment cannot know, the caller supplies in `context`: the file's
/// line-ending convention, its radio targets and its TODO sequences (see
/// each field). A body never starts its file, so a leading BOM is content,
/// as it is mid-file; at level 0 the fragment IS the start of the file and
/// a BOM is trivia, as in [`parse`]. Not reproduced: the radio work budget
/// is proportional to the text parsed, so a body near it may run out where
/// its file did not (or not where its file did).
pub fn parse_fragment(text: &str, options: &ParseOptions, context: &FragmentContext) -> Document {
    let start = match context.level {
        0 => builder::Start::Document,
        level => builder::Start::Fragment {
            level,
            ancestors: context.ancestors,
        },
    };
    let src = text::Source::with(text, context.line_ending, context.level == 0);
    parse_with(src, options, start, Some(context))
}

fn parse_with(
    src: text::Source,
    options: &ParseOptions,
    start: builder::Start,
    fragment: Option<&FragmentContext>,
) -> Document {
    let text = src.text;
    let reading = Reading {
        options: options.clone(),
        start,
        fragment: fragment.cloned(),
    };
    if exceeds_tree_limit(text.len()) {
        return Document::too_large(reading);
    }
    let ix = index::Index::build(&src);
    let built = builder::build(&src, &ix, options, start);
    let file_todo = fragment.and_then(|f| f.todo_sequences.as_deref());
    let declared = file_todo.is_some() || !built.in_file_todo.is_empty();
    let in_file = file_todo.unwrap_or(&built.in_file_todo);
    let todo = options::TodoKeywords::resolve(declared, in_file, &options.todo_sequences);
    let ix = inline::InlineIndex::build(&src);
    let mut link_types = options.link_types.clone();
    link_types.sort_by_key(|t| std::cmp::Reverse(t.len()));
    let inline_diagnostics = std::cell::RefCell::new(Vec::new());
    let cx = inline::InlineCx {
        src: &src,
        ix: &ix,
        link_types: &link_types,
        radio: None,
        radio_limit: options.radio_regexp_limit,
        diagnostics: &inline_diagnostics,
    };
    let mut contents = built.contents;
    let given = fragment.and_then(|f| f.radio_targets.as_deref());
    let (green, radio_targets) = green::to_green(&cx, &built.events, &todo, &mut contents, given);
    contents.finish();
    let mut diagnostics = built.diagnostics;
    diagnostics.extend(inline_diagnostics.into_inner());
    diagnostics.sort_by_key(Diagnostic::order_key);
    diagnostics.dedup();
    let todo_sequences = match (file_todo, declared) {
        (Some(t), _) => t.to_vec(),
        (None, true) => built.in_file_todo,
        (None, false) => options.todo_sequences.clone(),
    };
    Document {
        kinds: kind::KindSet::of(&green),
        green,
        diagnostics,
        todo_sequences,
        line_ending: src.eol,
        contents,
        radio_targets,
        reading,
    }
}

/// rowan addresses text with `u32` offsets.
fn exceeds_tree_limit(len: usize) -> bool {
    u32::try_from(len).is_err()
}

impl Document {
    /// Whether the tree holds a node of kind `k`.
    pub(crate) fn has_kind(&self, k: SyntaxKind) -> bool {
        self.kinds.has(k)
    }

    /// `text`, derived from this document's text, read as this document
    /// was: same options, same start, same line-ending convention.
    pub(crate) fn reparse(&self, text: &str) -> Document {
        let r = &self.reading;
        // A body never starts its file (a leading BOM is content there).
        let at_file_start = r.start == builder::Start::Document;
        let src = text::Source::with(text, self.line_ending, at_file_start);
        parse_with(src, &r.options, r.start, r.fragment.as_ref())
    }

    fn too_large(reading: Reading) -> Document {
        let mut b = rowan::GreenNodeBuilder::new();
        b.start_node(<OrgLanguage as rowan::Language>::kind_to_raw(
            SyntaxKind::DOCUMENT,
        ));
        b.finish_node();
        let diagnostics = vec![Diagnostic::new(DiagnosticKind::InputTooLarge, 0, 0)];
        let green = b.finish();
        Document {
            kinds: kind::KindSet::of(&green),
            green,
            diagnostics,
            todo_sequences: Vec::new(),
            line_ending: LineEnding::Lf,
            contents: span::ContentsTable::default(),
            radio_targets: Vec::new(),
            reading,
        }
    }

    /// The contents of an element node, as `org-element` defines them
    /// (`:contents-begin`/`:contents-end`): e.g. a block's lines between its
    /// delimiters, a paragraph without its trailing blank lines, an item after
    /// its bullet. `None` for elements Org gives no contents (keywords, lesser
    /// blocks, empty greater elements) and for nodes that are not elements.
    pub fn contents(&self, node: &SyntaxNode) -> Option<Span> {
        self.contents.get(node)
    }

    /// The line-ending convention the document was read with (decided once,
    /// as Emacs decides it; see [`LineEnding::detect`]).
    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// The root of the tree (a `DOCUMENT` node).
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// Diagnostics, sorted by position.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// The radio targets the parse made radio links of (`<<<…>>>` values as
    /// written, deduplicated, in Org's regexp order: longest first), or a
    /// fragment's file's ([`FragmentContext::radio_targets`]).
    pub fn radio_targets(&self) -> &[String] {
        &self.radio_targets
    }

    /// The TODO sequences the parse used: the file's own if it declares any,
    /// otherwise the configured ones.
    pub fn todo_sequences(&self) -> &[TodoSequence] {
        &self.todo_sequences
    }

    /// Every headline, in document order.
    pub fn headlines(&self) -> impl Iterator<Item = Headline> {
        self.syntax().descendants().filter_map(Headline::cast)
    }

    /// Every keyword element, in document order (not affiliated keywords, and
    /// not `#+` lines inside src/example blocks).
    pub fn keywords(&self) -> impl Iterator<Item = Keyword> {
        self.syntax().descendants().filter_map(Keyword::cast)
    }

    /// Every block, in document order.
    pub fn blocks(&self) -> impl Iterator<Item = Block> {
        self.syntax().descendants().filter_map(Block::cast)
    }

    /// The file-level property drawer: one at the top of the zeroth section.
    pub fn property_drawer(&self) -> Option<PropertyDrawer> {
        let first = self.syntax().children().next()?;
        (first.kind() == SyntaxKind::SECTION)
            .then_some(first)?
            .children()
            .find_map(PropertyDrawer::cast)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_over_u32_max_are_refused_by_the_size_check() {
        // A 4 GiB string cannot be allocated in a test; the check is the
        // function `parse` consults, pinned at its boundary.
        assert!(!exceeds_tree_limit(u32::MAX as usize));
        assert!(exceeds_tree_limit(u32::MAX as usize + 1));
        let doc = Document::too_large(Reading {
            options: ParseOptions::default(),
            start: builder::Start::Document,
            fragment: None,
        });
        assert_eq!(doc.syntax().to_string(), "");
        assert_eq!(doc.diagnostics()[0].kind, DiagnosticKind::InputTooLarge);
    }
}
