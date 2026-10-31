//! The element recogniser: `org-element--current-element`, transcribed.
//!
//! Given a position, a container limit and a mode, it decides which element
//! starts there and computes that element's extent in `org-element`'s
//! representation (affiliated keywords and trailing blank lines inside the
//! element), without building anything. The builder turns extents into tree
//! events. Nothing here recurses on input nesting: a greater element's
//! contents are parsed later, by the builder's explicit stack.

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::index::Index;
use crate::kind::SyntaxKind::{self, *};
use crate::list::ListStruct;
use crate::text::Source;
use crate::{blocks, leaf, list, paragraph, scan};

/// What `org-element` expects next in a container (`org-element--next-mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    FirstSection,
    Section,
    TopComment,
    Planning,
    PropertyDrawer,
    Item,
    NodeProperty,
    TableRow,
    Normal,
}

impl Mode {
    /// Mode for the children of an element of `kind` parsed in mode `self`.
    pub fn for_children(self, kind: SyntaxKind) -> Mode {
        match kind {
            DOCUMENT => Mode::FirstSection,
            HEADLINE => Mode::Section,
            SECTION if self == Mode::FirstSection => Mode::TopComment,
            SECTION => Mode::Planning,
            PLAIN_LIST => Mode::Item,
            PROPERTY_DRAWER => Mode::NodeProperty,
            TABLE => Mode::TableRow,
            _ => Mode::Normal,
        }
    }

    /// Mode for the next sibling after an element of `kind`.
    pub fn after(self, kind: SyntaxKind) -> Mode {
        match (self, kind) {
            (Mode::Item, _) => Mode::Item,
            (Mode::NodeProperty, _) => Mode::NodeProperty,
            (Mode::TableRow, _) => Mode::TableRow,
            (Mode::Planning, PLANNING) => Mode::PropertyDrawer,
            (Mode::TopComment, COMMENT) => Mode::PropertyDrawer,
            _ => Mode::Normal,
        }
    }
}

/// How an element's contents are represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Body {
    /// No contents between head and tail.
    None,
    /// Elements, parsed by the builder in a child frame.
    Children,
    /// Raw contents (lesser blocks): one `BLOCK_CONTENT` token.
    Opaque,
    /// Text lines (paragraphs, table.el tables): `TEXT` tokens until O2.
    Text,
}

/// An element's extent, in byte offsets:
/// `begin ≤ post_aff ≤ head_end ≤ body_end ≤ end`.
///
/// `[begin, post_aff)` affiliated keywords, `[post_aff, head_end)` the
/// opening part, `[head_end, body_end)` the contents, `[body_end, end)` the
/// closing line (if the kind has one) and trailing blank lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Element {
    pub kind: SyntaxKind,
    pub begin: usize,
    pub post_aff: usize,
    pub head_end: usize,
    pub body_end: usize,
    pub end: usize,
    pub body: Body,
    /// The list structure item and plain-list children inherit.
    pub list: Option<usize>,
}

impl Element {
    /// An element made of whole lines `[post_aff, head_end)` and blank lines
    /// up to `end`.
    pub fn leaf(
        kind: SyntaxKind,
        begin: usize,
        post_aff: usize,
        head_end: usize,
        end: usize,
    ) -> Element {
        Element {
            kind,
            begin,
            post_aff,
            head_end,
            body_end: head_end,
            end,
            body: Body::None,
            list: None,
        }
    }
}

/// Parser state shared by the recognisers.
pub(crate) struct Ctx<'a> {
    pub src: &'a Source<'a>,
    pub ix: &'a Index,
    pub lists: Vec<ListStruct>,
    pub diagnostics: Vec<Diagnostic>,
    /// The first non-blank line: where the zeroth section starts.
    pub first_nonblank: usize,
    /// The text is a headline's body, so its first line follows a headline.
    pub after_headline: bool,
}

impl<'a> Ctx<'a> {
    pub fn new(src: &'a Source<'a>, ix: &'a Index) -> Ctx<'a> {
        // `org-skip-whitespace` skips " \t\n\r", so in a unix-convention
        // file a line holding only a content CR counts as blank here, though
        // not for `[ \t]*$` (oracle: eol-cr-leading-blank.org).
        let q = src.skip_blank(src.bom, src.len());
        let first_nonblank = if q >= src.len() {
            src.lines.len()
        } else {
            src.line_of(q)
        };
        Ctx {
            src,
            ix,
            lists: Vec::new(),
            diagnostics: Vec::new(),
            first_nonblank,
            after_headline: false,
        }
    }

    pub fn start(&self, line: usize) -> usize {
        self.src.lines.get(line).map_or(self.src.len(), |l| l.start)
    }

    pub fn diagnose(&mut self, kind: DiagnosticKind, start: usize, end: usize) {
        self.diagnostics.push(Diagnostic::new(kind, start, end));
    }
}

/// The element starting at `pos`, parsed within `limit` in `mode`.
pub(crate) fn recognize(
    cx: &mut Ctx,
    pos: usize,
    limit: usize,
    mode: Mode,
    list: Option<usize>,
) -> Element {
    let li = cx.src.line_of(pos);
    let at_bol = pos == cx.start(li);
    match mode {
        Mode::Item => {
            if let Some(e) = list::item(cx, pos, limit, list) {
                return e;
            }
        }
        Mode::TableRow => return leaf::single_line(cx, TABLE_ROW, li, limit),
        Mode::NodeProperty => return leaf::single_line(cx, NODE_PROPERTY, li, limit),
        _ => {}
    }
    if !at_bol {
        // Inside an item or a footnote definition, after its bullet/label.
        return paragraph::paragraph(cx, pos, pos, limit);
    }
    recognize_at_bol(cx, li, limit, mode, list)
}

fn recognize_at_bol(
    cx: &mut Ctx,
    li: usize,
    limit: usize,
    mode: Mode,
    list: Option<usize>,
) -> Element {
    let s = cx.src.content(li);
    if scan::headline_level(s).is_some() {
        return leaf::headline(cx, li);
    }
    if matches!(mode, Mode::Section | Mode::FirstSection) {
        return leaf::section(cx, li);
    }
    if scan::is_comment(s) {
        return leaf::comment(cx, li, limit);
    }
    if mode == Mode::Planning && previous_line_is_starred(cx, li) && scan::is_planning(s) {
        return leaf::planning(cx, li, limit);
    }
    if property_drawer_allowed(cx, li, mode) {
        if let Some(e) = leaf::property_drawer(cx, li, limit) {
            return e;
        }
    }
    if scan::is_clock(s) {
        return leaf::clock(cx, li, limit);
    }
    with_affiliated(cx, li, limit, list)
}

fn previous_line_is_starred(cx: &Ctx, li: usize) -> bool {
    if li == 0 {
        return cx.after_headline;
    }
    cx.src.content(li - 1).starts_with('*')
}

/// Where a property drawer may start, per `org-element--current-element`:
/// right after a headline (or its planning line), or at the top of the
/// zeroth section (possibly after comments).
fn property_drawer_allowed(cx: &Ctx, li: usize, mode: Mode) -> bool {
    match mode {
        Mode::Planning => previous_line_is_starred(cx, li),
        Mode::PropertyDrawer | Mode::TopComment => {
            li == 0 || !cx.src.is_blank(li - 1) || li == cx.first_nonblank
        }
        _ => false,
    }
}

/// Collects affiliated keywords, then parses the element they belong to.
/// Orphaned ones (before a blank line, a comment, a clock line, a headline, or
/// running into the limit) are plain keywords instead.
fn with_affiliated(cx: &mut Ctx, li: usize, limit: usize, list: Option<usize>) -> Element {
    let begin = cx.start(li);
    // `(while (and (< (point) limit) (looking-at affiliated-re)) (forward-line))`
    let j = cx
        .ix
        .affiliated_run_end(li)
        .min(first_line_at(cx, limit, li));
    if j > li && orphaned(cx, j, limit) {
        return leaf::keyword(cx, li, begin, limit);
    }
    element_at(cx, j, begin, limit, list)
}

/// The first line whose start is at or after `limit`, but not before `from`.
fn first_line_at(cx: &Ctx, limit: usize, from: usize) -> usize {
    let lines = &cx.src.lines;
    let k = lines.partition_point(|l| l.start < limit);
    k.max(from)
}

fn orphaned(cx: &Ctx, j: usize, limit: usize) -> bool {
    if j >= cx.src.lines.len() || cx.start(j) >= limit {
        return true;
    }
    let s = cx.src.content(j);
    cx.src.is_blank(j)
        || scan::is_comment(s)
        || scan::is_clock_prefix(s)
        || scan::headline_level(s).is_some()
}

/// The element on line `j`, whose affiliated keywords (if any) start at
/// `aff`. This is the single-regexp dispatch of `org-element--current-element`.
fn element_at(cx: &mut Ctx, j: usize, aff: usize, limit: usize, list: Option<usize>) -> Element {
    let s = cx.src.content(j);
    if let Some(name) = scan::latex_begin(s) {
        return blocks::latex_environment(cx, j, aff, limit, name);
    }
    if scan::drawer_name(s).is_some() {
        return blocks::drawer(cx, j, aff, limit);
    }
    if scan::is_fixed_width(s) {
        return leaf::fixed_width(cx, j, aff, limit);
    }
    if scan::dynamic_begin(s).is_some() {
        return blocks::dynamic_block(cx, j, aff, limit);
    }
    if let Some(e) = hash_plus_element(cx, j, aff, limit) {
        return e;
    }
    if scan::footnote_definition(s).is_some() {
        return blocks::footnote_definition(cx, j, aff, limit);
    }
    if scan::is_horizontal_rule(s) || scan::is_diary_sexp(s) {
        let kind = if scan::is_diary_sexp(s) {
            DIARY_SEXP
        } else {
            HORIZONTAL_RULE
        };
        return leaf::one_line(cx, kind, j, aff, limit);
    }
    if scan::is_table_row(s) || is_tableel_table(cx, j, limit) {
        return leaf::table(cx, j, aff, limit);
    }
    if scan::item_bullet(s).is_some() {
        return list::plain_list(cx, j, aff, limit, list);
    }
    paragraph::paragraph(cx, aff, cx.start(j), limit)
}

/// `#+BEGIN_…`, `#+CALL:` and `#+KEY:` lines.
fn hash_plus_element(cx: &mut Ctx, j: usize, aff: usize, limit: usize) -> Option<Element> {
    let s = cx.src.content(j);
    if let Some(name) = scan::block_begin(s) {
        return Some(blocks::block(cx, j, aff, limit, name));
    }
    if scan::is_babel_call(s) {
        return Some(leaf::one_line(cx, BABEL_CALL, j, aff, limit));
    }
    scan::keyword_key(s).map(|_| leaf::keyword(cx, j, aff, limit))
}

/// The table.el heuristic: a full rule line, more lines before the limit,
/// and the run of table-like lines ending on a full rule. The run's end comes
/// from a precomputed index, so a rejected candidate costs O(1) rather than a
/// rescan (a rescan per line is quadratic on a run of near-tables).
fn is_tableel_table(cx: &Ctx, j: usize, limit: usize) -> bool {
    let src = cx.src;
    if !scan::is_tableel_rule(src.content(j)) || src.lines[j].next >= limit {
        return false;
    }
    let m = cx.ix.non_tableel_from(j + 1);
    let last = if m < src.lines.len() && src.lines[m].start < limit {
        if m == j + 1 {
            return false;
        }
        m - 1
    } else {
        // No non-table line before the limit: the last line before it.
        first_line_at(cx, limit, j + 1).saturating_sub(1)
    };
    scan::is_tableel_rule(src.content(last))
}
