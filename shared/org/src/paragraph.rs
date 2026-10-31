//! `org-element-paragraph-parser`: the default element.
//!
//! A paragraph runs until a line that really starts another element. A line
//! that only LOOKS like one (a `#+begin_X` with no `#+end_X` before the
//! limit, a drawer opener with no `:END:`) does not end it; those checks go
//! through the lookahead index, so the scan is linear in the lines consumed.

use crate::diagnostic::DiagnosticKind;
use crate::element::{Body, Ctx, Element};
use crate::kind::SyntaxKind::PARAGRAPH;
use crate::scan;

/// A paragraph whose affiliated keywords start at `begin` and whose text
/// starts at `cb` (mid-line inside an item or footnote definition).
pub(crate) fn paragraph(cx: &mut Ctx, begin: usize, cb: usize, limit: usize) -> Element {
    let src = cx.src;
    let n = src.lines.len();
    // The separator search starts at the end of the first line. Separators
    // are anchored at line starts, so only an EMPTY first line (end of line
    // == start of line) can be a separator itself: Emacs then makes a
    // paragraph of just the blank line(s) (Worg, blank line opening a block).
    let first = src.line_of(cb);
    let mut i = if src
        .lines
        .get(first)
        .is_some_and(|l| l.start == cb && l.end == cb)
    {
        first
    } else {
        first + 1
    };
    let before_blank = loop {
        if i >= n || src.lines[i].start >= limit {
            break limit;
        }
        if separates(cx, i, limit) {
            break src.lines[i].start;
        }
        i += 1;
    };
    // `skip-chars-backward " \r\t\n" contents-begin` then
    // `line-beginning-position 2`: for a paragraph of only blank lines this
    // is the line after its first, past the separator (oracle).
    let ce = src
        .after_last_nonblank(before_blank, cb)
        .max(cb)
        .min(limit.max(cb));
    let end = src.skip_blank(before_blank, limit).max(ce);
    Element {
        kind: PARAGRAPH,
        begin,
        post_aff: cb,
        head_end: cb,
        body_end: ce,
        end,
        body: Body::Text,
        list: None,
    }
}

/// Does line `i` end the paragraph above it?
fn separates(cx: &mut Ctx, i: usize, limit: usize) -> bool {
    let src = cx.src;
    let s = src.content(i);
    if scan::headline_level(s).is_some() {
        return true;
    }
    if !(scan::may_separate(s) || scan::list_like(s)) {
        return false;
    }
    if scan::drawer_name(s).is_some() {
        let closed = cx.ix.drawer_end(src, i + 1, limit).is_some();
        if !closed {
            cx.diagnose(
                DiagnosticKind::UnterminatedDrawer,
                src.lines[i].start,
                src.lines[i].end,
            );
        }
        return closed;
    }
    if let Some((a, b)) = scan::block_begin(s) {
        let closed = cx.ix.block_end(src, &s[a..b], i + 1, limit).is_some();
        if !closed {
            cx.diagnose(
                DiagnosticKind::UnterminatedBlock,
                src.lines[i].start,
                src.lines[i].end,
            );
        }
        return closed;
    }
    if let Some(name) = scan::latex_begin(s) {
        return cx.ix.latex_end(src, name, i, limit).is_some();
    }
    scan::bracketed_keyword_is_dual(s).unwrap_or(true)
}
