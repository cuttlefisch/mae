//! Diagnostics are data: malformed input becomes text plus a diagnostic,
//! never an error or a panic (ADR-112 D1).

use crate::span::Span;

/// What a diagnostic reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DiagnosticKind {
    /// A `#+begin_X` (or `#+BEGIN:`) line with no matching closing line
    /// before the end of its container — for a section, the next headline.
    /// The line is paragraph text.
    UnterminatedBlock,
    /// A `:NAME:` drawer opener with no `:END:` before the end of its
    /// container. The line is paragraph text.
    UnterminatedDrawer,
    /// Contents nested deeper than `ParseOptions::max_depth`, kept as one
    /// opaque token. The span is the unparsed contents.
    DepthLimit,
    /// In a fragment (`parse_fragment`), a headline at or above the
    /// fragment's level. In the file it would end the body, so the fragment
    /// is not one body. The span is the headline line.
    HeadlineOutsideFragment,
    /// The input is larger than `u32::MAX` bytes, the most a tree can address.
    /// The document is empty.
    InputTooLarge,
}

/// A diagnostic and the bytes it concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub span: Span,
    pub kind: DiagnosticKind,
}

impl Diagnostic {
    /// Sort key: position, then kind.
    pub(crate) fn order_key(&self) -> (Span, DiagnosticKind) {
        (self.span, self.kind)
    }

    /// Offsets beyond `u32::MAX` saturate; `parse` refuses such inputs first.
    pub(crate) fn new(kind: DiagnosticKind, start: usize, end: usize) -> Diagnostic {
        Diagnostic {
            span: Span::new(start, end),
            kind,
        }
    }
}
