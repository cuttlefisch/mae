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
//! Phase O1 builds block structure only. Inline content (emphasis, links,
//! timestamps) is a `TEXT` token per line until O2.
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
mod chars;
mod chartables;
mod diagnostic;
mod element;
mod green;
mod headline;
mod index;
mod kind;
mod leaf;
mod list;
mod options;
mod paragraph;
mod scan;
mod span;
mod text;
mod tokens;
mod views;

/// The Emacs version the character classes and case folding were generated
/// from (`conformance/gen-char-tables.el`).
pub use chartables::EMACS_VERSION as CHAR_TABLES_EMACS_VERSION;
pub use diagnostic::{Diagnostic, DiagnosticKind};
pub use kind::{OrgLanguage, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};
pub use options::{ParseOptions, TodoSequence, DEFAULT_MAX_DEPTH, MAX_DEPTH_CEILING};
/// The tree is a rowan tree; `node.text_range()` returns rowan types. The
/// re-export makes that coupling explicit and versioned: plain-data results
/// (diagnostics, contents) use [`Span`] instead, so a JSON/WASM consumer never
/// needs rowan.
pub use rowan;
pub use span::Span;
pub use text::LineEnding;
pub use views::{Block, Headline, Keyword, Property, PropertyDrawer};

use rowan::GreenNode;

/// A parsed org document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    green: GreenNode,
    diagnostics: Vec<Diagnostic>,
    todo_sequences: Vec<TodoSequence>,
    line_ending: LineEnding,
    contents: span::ContentsTable,
}

/// Parses `text`. Total: any input gives a document whose tree prints back
/// to `text`, except inputs over `u32::MAX` bytes, which give an empty
/// document with an [`DiagnosticKind::InputTooLarge`] diagnostic.
pub fn parse(text: &str, options: &ParseOptions) -> Document {
    parse_with(text::Source::new(text), options, builder::Start::Document)
}

/// What a fragment cannot know from its own text: where it sits in its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FragmentContext {
    /// The level of the headline whose body this is; 0 for the text before
    /// the first headline (the start of the file).
    pub level: usize,
    /// The FILE's line-ending convention ([`LineEnding::detect`] on the whole
    /// file, or [`Document::line_ending`] of its parse). A fragment's own
    /// endings can say otherwise: a CRLF body in a file with one LF line is a
    /// unix file, in which every CR is content.
    pub line_ending: LineEnding,
}

/// Parses `text` as the BODY of a headline: what follows the headline line in
/// its file (ADR-112 D1, M4). It parses exactly as the whole file would parse
/// that text — planning and a property drawer are recognised at its top —
/// with offsets relative to `text`. A headline at or above `context.level`
/// (one that would end the body in the file) is parsed and reported as
/// [`DiagnosticKind::HeadlineOutsideFragment`].
///
/// What a fragment cannot know, the caller supplies: the file's line-ending
/// convention (`context.line_ending`), and the TODO sequences in force in the
/// file (`options.todo_sequences`; an in-fragment `#+TODO:` still applies).
/// A body never starts its file, so a leading BOM is content, as it is
/// mid-file; at level 0 the fragment IS the start of the file and a BOM is
/// trivia, as in [`parse`].
pub fn parse_fragment(text: &str, options: &ParseOptions, context: &FragmentContext) -> Document {
    let start = match context.level {
        0 => builder::Start::Document,
        level => builder::Start::Fragment { level },
    };
    let src = text::Source::with(text, context.line_ending, context.level == 0);
    parse_with(src, options, start)
}

fn parse_with(src: text::Source, options: &ParseOptions, start: builder::Start) -> Document {
    let text = src.text;
    if exceeds_tree_limit(text.len()) {
        return Document::too_large();
    }
    let ix = index::Index::build(&src);
    let built = builder::build(&src, &ix, options, start);
    let declared = !built.in_file_todo.is_empty();
    let todo =
        options::TodoKeywords::resolve(declared, &built.in_file_todo, &options.todo_sequences);
    let green = green::to_green(text, &built.events, &todo);
    let mut diagnostics = built.diagnostics;
    diagnostics.sort_by_key(Diagnostic::order_key);
    diagnostics.dedup();
    let todo_sequences = if declared {
        built.in_file_todo
    } else {
        options.todo_sequences.clone()
    };
    Document {
        green,
        diagnostics,
        todo_sequences,
        line_ending: src.eol,
        contents: built.contents,
    }
}

/// rowan addresses text with `u32` offsets.
fn exceeds_tree_limit(len: usize) -> bool {
    u32::try_from(len).is_err()
}

impl Document {
    fn too_large() -> Document {
        let mut b = rowan::GreenNodeBuilder::new();
        b.start_node(<OrgLanguage as rowan::Language>::kind_to_raw(
            SyntaxKind::DOCUMENT,
        ));
        b.finish_node();
        let diagnostics = vec![Diagnostic::new(DiagnosticKind::InputTooLarge, 0, 0)];
        Document {
            green: b.finish(),
            diagnostics,
            todo_sequences: Vec::new(),
            line_ending: LineEnding::Lf,
            contents: span::ContentsTable::default(),
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
        let doc = Document::too_large();
        assert_eq!(doc.syntax().to_string(), "");
        assert_eq!(doc.diagnostics()[0].kind, DiagnosticKind::InputTooLarge);
    }
}
