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
    let kind = line_kind(s);
    if let LineKind::Headline(_) = kind {
        return leaf::headline(cx, li);
    }
    if matches!(mode, Mode::Section | Mode::FirstSection) {
        return leaf::section(cx, li);
    }
    if kind == LineKind::Comment {
        return leaf::comment(cx, li, limit);
    }
    if kind == LineKind::Planning && mode == Mode::Planning && previous_line_is_starred(cx, li) {
        return leaf::planning(cx, li, limit);
    }
    if property_drawer_allowed(cx, li, mode) {
        if let Some(e) = leaf::property_drawer(cx, li, limit) {
            return e;
        }
    }
    if kind == LineKind::Clock {
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
/// Orphaned ones (before the end of the buffer, a blank line, a comment, a
/// clock line or a headline) are dropped and the first line is parsed on its
/// own; ones that run into the limit otherwise are a keyword.
fn with_affiliated(cx: &mut Ctx, li: usize, limit: usize, list: Option<usize>) -> Element {
    let begin = cx.start(li);
    // `(while (and (< (point) limit) (looking-at affiliated-re)) (forward-line))`
    let j = cx
        .ix
        .affiliated_run_end(li)
        .min(first_line_at(cx, limit, li));
    if j > li {
        if orphaned(cx, j) {
            // `org-element--collect-affiliated-keywords' moves back to the
            // first one and drops them; the current-element regexp then
            // reads that line on its own: a keyword only if it is one
            // (`#+KEY:`), so `#+CAPTION[s b]: x' is a paragraph.
            return element_at(cx, li, begin, limit, list);
        }
        if cx.start(j) >= limit {
            // Kept but off-limits: Org calls the keyword parser regardless.
            return leaf::keyword(cx, li, begin, limit);
        }
    }
    element_at(cx, j, begin, limit, list)
}

/// The first line whose start is at or after `limit`, but not before `from`.
fn first_line_at(cx: &Ctx, limit: usize, from: usize) -> usize {
    let lines = &cx.src.lines;
    let k = lines.partition_point(|l| l.start < limit);
    k.max(from)
}

/// Org's orphan test at the line after the keywords: the end of the buffer,
/// a blank line, a comment, a clock line or a headline.
fn orphaned(cx: &Ctx, j: usize) -> bool {
    if j >= cx.src.lines.len() {
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
    match element_start(s) {
        Start::LatexEnvironment(name) => blocks::latex_environment(cx, j, aff, limit, name),
        Start::Drawer(_) => blocks::drawer(cx, j, aff, limit),
        Start::FixedWidth => leaf::fixed_width(cx, j, aff, limit),
        Start::DynamicBlock(_) => blocks::dynamic_block(cx, j, aff, limit),
        Start::Block(name) => blocks::block(cx, j, aff, limit, name),
        Start::BabelCall => leaf::one_line(cx, BABEL_CALL, j, aff, limit),
        Start::Keyword(_) => leaf::keyword(cx, j, aff, limit),
        Start::FootnoteDefinition(_) => blocks::footnote_definition(cx, j, aff, limit),
        Start::HorizontalRule => leaf::one_line(cx, HORIZONTAL_RULE, j, aff, limit),
        Start::DiarySexp => leaf::one_line(cx, DIARY_SEXP, j, aff, limit),
        Start::TableRow => leaf::table(cx, j, aff, limit),
        Start::TableElRule if is_tableel_table(cx, j, limit) => leaf::table(cx, j, aff, limit),
        Start::Item(_) => list::plain_list(cx, j, aff, limit, list),
        Start::TableElRule | Start::Text => paragraph::paragraph(cx, aff, cx.start(j), limit),
    }
}

/// What a line is to the recogniser out of context: the first of the
/// regexps `org-element--current-element` tries, in its order, that the
/// line matches, before the mode, the limit and the lines around it decide
/// (a block begin is a block only if its end follows: an opener is a kind
/// of line whether or not it is closed, so a line becoming one is a
/// change). The recogniser dispatches on it; the macro expansion compares
/// a line's kind before and after an edit with it, so both read lines one
/// way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineKind<'a> {
    Blank,
    Headline(usize),
    Comment,
    Planning,
    Clock,
    /// An affiliated keyword: its `#+KEY[…]:`.
    Affiliated(&'a str),
    Element(Start<'a>),
}

/// The element a line starts by the single-regexp dispatch, with the name
/// or key it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Start<'a> {
    LatexEnvironment(&'a str),
    Drawer(&'a str),
    FixedWidth,
    DynamicBlock(&'a str),
    Block(&'a str),
    BabelCall,
    Keyword(&'a str),
    FootnoteDefinition(&'a str),
    HorizontalRule,
    DiarySexp,
    TableRow,
    /// A table.el rule: a table if the lines after it make one.
    TableElRule,
    /// A list item: its bullet.
    Item(&'a str),
    Text,
}

/// The kind of line `s` (its content, no line ending).
pub(crate) fn line_kind(s: &str) -> LineKind<'_> {
    if crate::text::is_blank(s) {
        LineKind::Blank
    } else if let Some(level) = scan::headline_level(s) {
        LineKind::Headline(level)
    } else if scan::is_comment(s) {
        LineKind::Comment
    } else if scan::is_planning(s) {
        LineKind::Planning
    } else if scan::is_clock(s) {
        LineKind::Clock
    } else if let Some(e) = scan::affiliated(s) {
        LineKind::Affiliated(&s[..e])
    } else {
        LineKind::Element(element_start(s))
    }
}

/// The element line `s` starts by the single-regexp dispatch.
pub(crate) fn element_start(s: &str) -> Start<'_> {
    let at = |(a, b): (usize, usize)| &s[a..b];
    if let Some(name) = scan::latex_begin(s) {
        Start::LatexEnvironment(name)
    } else if let Some(n) = scan::drawer_name(s) {
        Start::Drawer(at(n))
    } else if scan::is_fixed_width(s) {
        Start::FixedWidth
    } else if let Some(n) = scan::dynamic_begin(s) {
        Start::DynamicBlock(at(n))
    } else if let Some(n) = scan::block_begin(s) {
        Start::Block(at(n))
    } else if scan::is_babel_call(s) {
        Start::BabelCall
    } else if let Some(k) = scan::keyword_key(s) {
        Start::Keyword(at(k))
    } else if let Some(e) = scan::footnote_definition(s) {
        Start::FootnoteDefinition(&s[..e])
    } else if scan::is_horizontal_rule(s) {
        Start::HorizontalRule
    } else if scan::is_diary_sexp(s) {
        Start::DiarySexp
    } else if scan::is_table_row(s) {
        Start::TableRow
    } else if scan::is_tableel_rule(s) {
        Start::TableElRule
    } else if let Some(b) = scan::item_bullet(s) {
        Start::Item(at(b))
    } else {
        Start::Text
    }
}

/// Whether line `after` is read as line `before` is, where `after` is
/// `before` with its bytes `at..at + old` replaced by `new` bytes: the same
/// kind with the same names, the same indentation, the same answer to
/// whether it ends a paragraph or a table above it, and, for a list item,
/// the same parts (bullet, counter, checkbox, tag, contents) moved by the
/// edit. What the line closes is not compared: a line closes only an
/// opener above it, and an opener above an expansion that a later line
/// closed would hold the expansion's macro, which then was no macro; so
/// what a new closer encloses is all before the expansion, already done.
pub(crate) fn same_line(before: &str, after: &str, at: usize, old: usize, new: usize) -> bool {
    let (kb, ka) = (line_kind(before), line_kind(after));
    if kb != ka
        || crate::text::ws_len(before) != crate::text::ws_len(after)
        || paragraph::may_end(before) != paragraph::may_end(after)
        || scan::ends_table(before, true) != scan::ends_table(after, true)
    {
        return false;
    }
    if !matches!(kb, LineKind::Element(Start::Item(_))) {
        return true;
    }
    let moved = |p: usize| {
        if p <= at {
            Some(p)
        } else if p >= at + old {
            Some(p - old + new)
        } else {
            None
        }
    };
    let pair = |r: Option<(usize, usize)>| r.map(|(a, b)| (moved(a), moved(b)));
    let (hb, ha) = (list::item_head(before), list::item_head(after));
    let same = |x: Option<(usize, usize)>, y: Option<(usize, usize)>| {
        pair(x) == y.map(|(a, b)| (Some(a), Some(b)))
    };
    same(Some(hb.bullet), Some(ha.bullet))
        && same(hb.counter, ha.counter)
        && same(hb.checkbox, ha.checkbox)
        && same(hb.tag, ha.tag)
        && moved(hb.contents_start) == Some(ha.contents_start)
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
