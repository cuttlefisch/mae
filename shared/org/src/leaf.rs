//! Headlines, sections and the elements that hold no other elements.
//! Each function mirrors the `org-element-*-parser` of the same name.

use crate::element::{Body, Ctx, Element};
use crate::kind::SyntaxKind::{self, *};
use crate::scan;

/// `org-element-headline-parser`: the subtree runs to the next headline of the
/// same or a higher level; contents start at the first non-blank line.
pub(crate) fn headline(cx: &Ctx, li: usize) -> Element {
    let src = cx.src;
    let begin = cx.start(li);
    let end = cx.start(cx.ix.subtree_end(li).unwrap_or(src.lines.len()));
    let line_end = src.lines[li].next.min(end);
    let cb = src.skip_blank(line_end, end);
    if cb >= end {
        return Element::leaf(HEADLINE, begin, begin, line_end, end);
    }
    Element {
        kind: HEADLINE,
        begin,
        post_aff: begin,
        head_end: cb,
        body_end: end,
        end,
        body: Body::Children,
        list: None,
    }
}

/// `org-element-section-parser`: from here to the next headline of any level.
pub(crate) fn section(cx: &Ctx, li: usize) -> Element {
    let begin = cx.start(li);
    let end = cx
        .ix
        .next_headline(li)
        .map_or(cx.src.len(), |h| cx.start(h));
    Element {
        kind: SECTION,
        begin,
        post_aff: begin,
        head_end: begin,
        body_end: end,
        end,
        body: Body::Children,
        list: None,
    }
}

/// `org-element-comment-parser`: consecutive `# ` lines.
pub(crate) fn comment(cx: &Ctx, li: usize, limit: usize) -> Element {
    let src = cx.src;
    let mut j = li + 1;
    while j < src.lines.len() && cx.start(j) < limit && scan::is_comment(src.content(j)) {
        j += 1;
    }
    let com_end = cx.start(j).min(limit.max(src.lines[li].next));
    let begin = cx.start(li);
    Element::leaf(
        COMMENT,
        begin,
        begin,
        com_end,
        src.skip_blank(com_end, limit),
    )
}

/// `org-element-fixed-width-parser`: consecutive `: ` lines.
pub(crate) fn fixed_width(cx: &Ctx, li: usize, aff: usize, limit: usize) -> Element {
    let src = cx.src;
    let mut j = li;
    while j < src.lines.len() && cx.start(j) < limit && scan::is_fixed_width(src.content(j)) {
        j += 1;
    }
    let area_end = cx.start(j);
    Element::leaf(
        FIXED_WIDTH,
        aff,
        cx.start(li),
        area_end,
        src.skip_blank(area_end, limit),
    )
}

/// Planning lines end at the next non-blank line, but keep a final
/// whitespace-only line without a line ending out (`skip_blank_bol`).
pub(crate) fn planning(cx: &Ctx, li: usize, limit: usize) -> Element {
    let next = cx.src.lines[li].next;
    let begin = cx.start(li);
    Element::leaf(
        PLANNING,
        begin,
        begin,
        next,
        cx.src.skip_blank_bol(next, limit),
    )
}

/// `org-element-clock-parser`: same blank-line rule as planning.
pub(crate) fn clock(cx: &Ctx, li: usize, limit: usize) -> Element {
    let next = cx.src.lines[li].next;
    let begin = cx.start(li);
    Element::leaf(
        CLOCK,
        begin,
        begin,
        next,
        cx.src.skip_blank_bol(next, limit),
    )
}

/// Keywords keep their affiliated keywords (`(or (car affiliated) (point))`).
pub(crate) fn keyword(cx: &Ctx, li: usize, aff: usize, limit: usize) -> Element {
    one_line(cx, KEYWORD, li, aff, limit)
}

/// A one-line element followed by blank lines (keyword, babel call,
/// horizontal rule, diary sexp).
pub(crate) fn one_line(cx: &Ctx, kind: SyntaxKind, li: usize, aff: usize, limit: usize) -> Element {
    let next = cx.src.lines[li].next;
    Element::leaf(
        kind,
        aff,
        cx.start(li),
        next,
        cx.src.skip_blank(next, limit),
    )
}

/// A one-line element with no trailing blank lines (table row, node
/// property): it ends where its line does.
pub(crate) fn single_line(cx: &Ctx, kind: SyntaxKind, li: usize, limit: usize) -> Element {
    let begin = cx.start(li);
    let end = cx.src.lines[li].next.min(limit.max(begin + 1));
    Element::leaf(kind, begin, begin, end, end)
}

/// `org-property-drawer-re`: `:PROPERTIES:`, property lines only, `:END:`.
/// `None` when any line in between is not a property line, or there is no
/// `:END:`; the caller then tries a plain drawer.
pub(crate) fn property_drawer(cx: &Ctx, li: usize, limit: usize) -> Option<Element> {
    let src = cx.src;
    if !scan::is_properties_open(src.content(li)) {
        return None;
    }
    let mut j = li + 1;
    loop {
        let line = src.lines.get(j)?;
        if line.end > limit {
            return None;
        }
        let s = src.content(j);
        if scan::is_drawer_end(s) {
            break;
        }
        if !scan::is_property_line(s) {
            return None;
        }
        j += 1;
    }
    let begin = cx.start(li);
    let cb = src.lines[li].next;
    let ce = cx.start(j);
    let end = src.skip_blank(src.lines[j].next, limit);
    let body = if ce > cb { Body::Children } else { Body::None };
    Some(Element {
        kind: PROPERTY_DRAWER,
        begin,
        post_aff: begin,
        head_end: cb,
        body_end: ce,
        end,
        body,
        list: None,
    })
}

/// `org-element-table-parser`. An org table's rows are children; a table.el
/// table is text. `#+TBLFM:` lines and blank lines form the tail.
pub(crate) fn table(cx: &Ctx, li: usize, aff: usize, limit: usize) -> Element {
    let src = cx.src;
    let table_el = !scan::is_table_row(src.content(li));
    let mut j = li + 1;
    while j < src.lines.len() && cx.start(j) < limit && !scan::ends_table(src.content(j), table_el)
    {
        j += 1;
    }
    let table_end = cx.start(j).min(limit);
    while j < src.lines.len() && cx.start(j) < limit && scan::is_tblfm(src.content(j)) {
        j += 1;
    }
    let before_blank = cx.start(j).min(limit.max(table_end));
    let end = src.skip_blank(before_blank, limit);
    let post_aff = cx.start(li);
    let body = if table_el { Body::Text } else { Body::Children };
    Element {
        kind: TABLE,
        begin: aff,
        post_aff,
        head_end: post_aff,
        body_end: table_end,
        end,
        body,
        list: None,
    }
}
