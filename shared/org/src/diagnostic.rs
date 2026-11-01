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
    /// opaque token (elements) or as plain text (objects: the span is the
    /// region, which holds at least one object Org would parse, possibly a
    /// radio target, which then makes no radio links).
    DepthLimit,
    /// Radio targets with whitespace other than spaces (a tab, a no-break
    /// space) next to spaces or at an end need a backtracking match per
    /// candidate; past a work budget proportional to the input those
    /// targets stop matching, so some radio links Org would make are plain
    /// text. The span is the document.
    RadioLinkBudget,
    /// In a fragment (`parse_fragment`), a headline at or above the
    /// fragment's level. In the file it would end the body, so the fragment
    /// is not one body. The span is the headline line.
    HeadlineOutsideFragment,
    /// The input is larger than `u32::MAX` bytes, the most a tree can address.
    /// The document is empty.
    InputTooLarge,
}

impl DiagnosticKind {
    /// The name used in JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            DiagnosticKind::UnterminatedBlock => "unterminated_block",
            DiagnosticKind::UnterminatedDrawer => "unterminated_drawer",
            DiagnosticKind::DepthLimit => "depth_limit",
            DiagnosticKind::RadioLinkBudget => "radio_link_budget",
            DiagnosticKind::HeadlineOutsideFragment => "headline_outside_fragment",
            DiagnosticKind::InputTooLarge => "input_too_large",
        }
    }
}

/// A diagnostic and the bytes it concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
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
