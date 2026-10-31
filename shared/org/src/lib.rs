//! mae-org: MAE's org-mode parser (ADR-112).
//! @stability: experimental
//!
//! [`parse`] turns org text into a **lossless** syntax tree: every input byte
//! belongs to exactly one token, so printing an unedited tree reproduces the
//! input byte for byte — CRLF, a BOM and a missing final newline included.
//! Typed views ([`Headline`], [`PropertyDrawer`], [`Block`], [`Keyword`]) sit
//! on top and read only from the tree.
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
//!   capped by [`ParseOptions::max_depth`].
//! - **Deterministic and configuration-as-data:** the tree is a function of
//!   the text and [`ParseOptions`] only (ADR-112 D4). No I/O.

mod blocks;
mod builder;
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
mod text;
mod tokens;
mod views;

pub use diagnostic::{Diagnostic, DiagnosticKind};
pub use kind::{OrgLanguage, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};
pub use options::{ParseOptions, TodoSequence, DEFAULT_MAX_DEPTH};
pub use rowan::{TextRange, TextSize};
pub use views::{Block, Headline, Keyword, Property, PropertyDrawer};

use rowan::GreenNode;

/// A parsed org document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    green: GreenNode,
    diagnostics: Vec<Diagnostic>,
    todo_sequences: Vec<TodoSequence>,
}

/// Parses `text`. Total: any input gives a document whose tree prints back
/// to `text`, except inputs over `u32::MAX` bytes, which give an empty
/// document with an [`DiagnosticKind::InputTooLarge`] diagnostic.
pub fn parse(text: &str, options: &ParseOptions) -> Document {
    if exceeds_tree_limit(text.len()) {
        return Document::too_large();
    }
    let src = text::Source::new(text);
    let ix = index::Index::build(&src);
    let built = builder::build(&src, &ix, options);
    let todo = options::TodoKeywords::resolve(&built.in_file_todo, &options.todo_sequences);
    let green = green::to_green(text, &built.events, &todo);
    let mut diagnostics = built.diagnostics;
    diagnostics.sort_by_key(Diagnostic::order_key);
    diagnostics.dedup();
    let todo_sequences = if built.in_file_todo.is_empty() {
        options.todo_sequences.clone()
    } else {
        built.in_file_todo
    };
    Document {
        green,
        diagnostics,
        todo_sequences,
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
        }
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
