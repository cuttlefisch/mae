//! Plain lists: `org-element--list-struct`, `org-element-plain-list-parser`
//! and `org-element-item-parser`.
//!
//! As in Org, the item structure of a whole list (every nesting level) is
//! computed once, when its top-level list is met, in one pass over its lines;
//! nested lists inside its items reuse it. Items end by indentation: a
//! non-blank line indented no deeper than an item's bullet ends that item, and
//! two blank lines end the list.

use crate::element::{Body, Ctx, Element};
use crate::kind::SyntaxKind::{ITEM, PLAIN_LIST};
use crate::scan;
use crate::text::{indent_column, ws_len};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ListItem {
    pub begin: usize,
    pub ind: usize,
    pub end: usize,
}

/// Every item of one top-level list, sorted by `begin`.
#[derive(Debug, Default)]
pub(crate) struct ListStruct {
    items: Vec<ListItem>,
}

impl ListStruct {
    fn find(&self, begin: usize) -> Option<ListItem> {
        self.items
            .binary_search_by_key(&begin, |it| it.begin)
            .ok()
            .map(|k| self.items[k])
    }
}

/// `org-element--list-struct` from line `first` (an item line).
fn build_struct(cx: &Ctx, first: usize, limit: usize) -> ListStruct {
    let src = cx.src;
    let mut open: Vec<ListItem> = Vec::new();
    let mut done: Vec<ListItem> = Vec::new();
    let mut i = first;
    loop {
        if i >= src.lines.len() || src.lines[i].start >= limit {
            let end = src.after_last_nonblank(cx.start(i), src.bom);
            close(&mut open, &mut done, 0, end);
            break;
        }
        if cx.ix.is_double_blank(i) {
            close(&mut open, &mut done, 0, cx.start(i));
            break;
        }
        let s = src.content(i);
        if scan::item_bullet(s).is_some() {
            let ind = indent_column(s);
            close(&mut open, &mut done, ind, cx.start(i));
            open.push(ListItem {
                begin: cx.start(i),
                ind,
                end: 0,
            });
        } else if !src.is_blank(i) {
            let end = src.after_last_nonblank(cx.start(i), src.bom);
            close(&mut open, &mut done, indent_column(s), end);
            if open.is_empty() {
                break;
            }
            i = skip_block_or_drawer(cx, i, limit);
        }
        i += 1;
    }
    done.sort_by_key(|it| it.begin);
    ListStruct { items: done }
}

/// Ends every open item indented at least `ind` columns deep, at `end`.
/// `ind == 0` ends them all.
fn close(open: &mut Vec<ListItem>, done: &mut Vec<ListItem>, ind: usize, end: usize) {
    while open.last().is_some_and(|it| ind <= it.ind) {
        if let Some(mut it) = open.pop() {
            it.end = end;
            done.push(it);
        }
    }
}

/// On a text line inside a list, jump over a whole block or drawer so lines
/// inside it cannot end items. Returns the line to continue after.
fn skip_block_or_drawer(cx: &Ctx, i: usize, limit: usize) -> usize {
    let src = cx.src;
    let s = src.content(i);
    let r = &s[ws_len(s)..];
    let end = if crate::text::starts_with_ci(r, "#+BEGIN:") {
        cx.ix.dynamic_end_colon(src, i + 1, limit)
    } else if let Some((a, b)) = scan::block_begin(s) {
        cx.ix.block_end(src, &s[a..b], i + 1, limit)
    } else if scan::drawer_name(s).is_some() {
        cx.ix.drawer_end(src, i, limit)
    } else {
        None
    };
    end.unwrap_or(i)
}

/// `org-element-plain-list-parser` on line `j`. A list nested in an item
/// inherits the item's structure (`inherited`); a top-level one builds its own.
pub(crate) fn plain_list(
    cx: &mut Ctx,
    j: usize,
    aff: usize,
    limit: usize,
    inherited: Option<usize>,
) -> Element {
    let cb = cx.start(j);
    let sid = match inherited.filter(|&sid| cx.lists[sid].find(cb).is_some()) {
        Some(sid) => sid,
        None => {
            let st = build_struct(cx, j, limit);
            cx.lists.push(st);
            cx.lists.len() - 1
        }
    };
    let st = &cx.lists[sid];
    let first = st.find(cb).unwrap_or(ListItem {
        begin: cb,
        ind: 0,
        end: cx.src.lines[j].next,
    });
    let mut pos = first.end;
    while let Some(it) = st.find(pos).filter(|it| it.ind == first.ind) {
        pos = it.end;
    }
    let ce = cx
        .src
        .after_last_nonblank(pos, cb)
        .min(limit)
        .max(cx.src.lines[j].next.min(limit));
    let end = skip_blank_to_limit(cx, ce, limit);
    Element {
        kind: PLAIN_LIST,
        begin: aff,
        post_aff: cb,
        head_end: cb,
        body_end: ce,
        end,
        body: Body::Children,
        list: Some(sid),
    }
}

/// `skip-chars-forward " \r\t\n" limit`, then the limit itself or the start
/// of the line reached.
fn skip_blank_to_limit(cx: &Ctx, pos: usize, limit: usize) -> usize {
    let b = cx.src.text.as_bytes();
    let mut q = pos;
    while q < limit && matches!(b[q], b' ' | b'\t' | b'\r' | b'\n') {
        q += 1;
    }
    if q >= limit {
        limit
    } else {
        cx.src.lines[cx.src.line_of(q)].start.max(pos)
    }
}

/// `org-element-item-parser` at `pos`, using the inherited structure.
pub(crate) fn item(cx: &Ctx, pos: usize, limit: usize, sid: Option<usize>) -> Option<Element> {
    let src = cx.src;
    let it = cx.lists.get(sid?)?.find(pos)?;
    let li = src.line_of(pos);
    let head = item_head(src.content(li));
    let at_bol = src
        .lines
        .get(src.line_of(it.end))
        .is_some_and(|l| l.start == it.end)
        || it.end >= src.len();
    let end = if at_bol {
        it.end
    } else {
        src.next_line_start(it.end)
    }
    .min(limit);
    let q = skip_ws(cx, pos + head.contents_start, end);
    if q >= end {
        let head_end = src.lines[li].next.min(end);
        return Some(Element {
            list: sid,
            ..Element::leaf(ITEM, pos, pos, head_end, end)
        });
    }
    let ql = src.line_of(q);
    let cb = if ql == li { q } else { src.lines[ql].start };
    let ce = src.after_last_nonblank(end, pos).clamp(cb, end);
    Some(Element {
        kind: ITEM,
        begin: pos,
        post_aff: pos,
        head_end: cb,
        body_end: ce,
        end,
        body: Body::Children,
        list: sid,
    })
}

fn skip_ws(cx: &Ctx, pos: usize, limit: usize) -> usize {
    let b = cx.src.text.as_bytes();
    let mut q = pos;
    while q < limit && matches!(b[q], b' ' | b'\t' | b'\r' | b'\n') {
        q += 1;
    }
    q
}

/// The parts of an item's first line, per `org-list-full-item-re`, as byte
/// offsets within the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ItemHead {
    pub bullet: (usize, usize),
    pub counter: Option<(usize, usize)>,
    pub checkbox: Option<(usize, usize)>,
    /// The tag text, without its ` ::`.
    pub tag: Option<(usize, usize)>,
    /// Where the item's contents start (before skipping blanks).
    pub contents_start: usize,
}

pub(crate) fn item_head(s: &str) -> ItemHead {
    let Some(bullet) = scan::item_bullet(s) else {
        return ItemHead {
            contents_start: s.len(),
            ..ItemHead::default()
        };
    };
    let mut at = bullet.1 + ws_len(&s[bullet.1..]);
    let mut head = ItemHead {
        bullet,
        contents_start: at,
        ..ItemHead::default()
    };
    if at == bullet.1 && at < s.len() {
        return head; // `[ \t]+|$` failed: nothing more can match
    }
    if let Some(n) = counter_len(&s[at..]) {
        head.counter = Some((at, at + n));
        at += n + ws_len(&s[at + n..]);
    }
    if let Some(n) = checkbox_len(&s[at..]) {
        head.checkbox = Some((at, at + 3));
        at += n;
    }
    head.contents_start = at;
    if let Some((tag_end, after)) = tag_split(&s[at..]) {
        head.tag = Some((at, at + tag_end));
        let ordered = s[bullet.0..bullet.1].contains(['.', ')']);
        head.contents_start = if ordered { at } else { at + after };
    }
    head
}

/// `\[@(start:)?([0-9]+|[A-Za-z])\]`.
fn counter_len(s: &str) -> Option<usize> {
    let r = s.strip_prefix("[@")?;
    let r2 = r.strip_prefix("start:").unwrap_or(r);
    let digits = r2.bytes().take_while(u8::is_ascii_digit).count();
    let n = if digits > 0 {
        digits
    } else {
        usize::from(r2.bytes().next().is_some_and(|b| b.is_ascii_alphabetic()))
    };
    (n > 0 && r2[n..].starts_with(']')).then(|| 2 + (r.len() - r2.len()) + n + 1)
}

/// `(\[[ X-]\])([ \t]+|$)`: the checkbox plus its trailing blanks. Org
/// matches with `case-fold-search` on, so `[x]` is a checkbox too (corpus).
fn checkbox_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let ok =
        b.len() >= 3 && b[0] == b'[' && matches!(b[1], b' ' | b'X' | b'x' | b'-') && b[2] == b']';
    let ws = if ok { ws_len(&s[3..]) } else { 0 };
    (ok && (ws > 0 || s.len() == 3)).then_some(3 + ws)
}

/// `(.*)[ \t]+::([ \t]+|$)` with a greedy `.*`: the LAST qualifying `::`.
/// Returns (tag end, offset after the `::` and its blanks).
fn tag_split(s: &str) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let mut k = s.len();
    while let Some(at) = s[..k].rfind("::") {
        let before_ws = at > 0 && matches!(b[at - 1], b' ' | b'\t');
        let after = at + 2;
        let after_ok = after == s.len() || matches!(b[after], b' ' | b'\t');
        if before_ws && after_ok {
            let tag_end = s[..at].trim_end_matches([' ', '\t']).len();
            return Some((tag_end, after + ws_len(&s[after..])));
        }
        k = at;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_heads_split_like_org_list_full_item_re() {
        let h = item_head("- [@3] [X] term :: def");
        assert_eq!(h.bullet, (0, 1));
        assert_eq!(h.counter, Some((2, 6)));
        assert_eq!(h.checkbox, Some((7, 10)));
        assert_eq!(h.tag, Some((11, 15)));
        assert_eq!(h.contents_start, 19);
        // Ordered items keep the tag in their contents.
        assert_eq!(item_head("1. a :: b").contents_start, 3);
        assert_eq!(item_head("- [x] lower").checkbox, Some((2, 5)));
        // A bullet at the end of the line has nothing after it.
        assert_eq!(item_head("-").contents_start, 1);
        // `::` needs blank before it and blank or EOL after it.
        assert_eq!(item_head("- a::b").tag, None);
        assert_eq!(item_head("- a :: b :: c").tag, Some((2, 8)));
    }
}
