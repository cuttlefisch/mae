//! Headlines, lists, tables and the verbatim blocks.

use super::anchors::{encode_id, slug};
use super::build::{trim_text, Builder, Mode};
use super::tree::{push, text, Attr, Element, Node, Tag};
use super::HeadlineIds;
use crate::kind::SyntaxKind::*;
use crate::kind::{SyntaxNode, SyntaxToken};
use crate::semantics::plain::plain_text;
use crate::views::Headline;
use crate::Span;
use rowan::NodeOrToken;

fn tokens(n: &SyntaxNode) -> impl Iterator<Item = SyntaxToken> {
    n.children_with_tokens().filter_map(|e| e.into_token())
}

fn token_text(n: &SyntaxNode, kind: crate::SyntaxKind) -> Option<String> {
    tokens(n)
        .find(|t| t.kind() == kind)
        .map(|t| t.text().to_string())
}

impl Builder<'_> {
    /// The anchor of a headline, claimed now (same order in both passes).
    /// `title` is the title as written (the key `[[*Title]]` links find it
    /// by); `shown` is its rendered text, which the slug is made from, so
    /// an id never spells a link target or snippet the title does not show.
    fn headline_anchor(
        &mut self,
        n: &SyntaxNode,
        h: &Headline,
        (title, shown): (&str, &str),
    ) -> Option<String> {
        if self.mode == Mode::Meta {
            return None;
        }
        let start = Span::of(n).start;
        // An archived headline's drawer is not rendered (its section is
        // unrendered), so its CUSTOM_ID is not read either.
        let hidden = h.section().is_some_and(|s| self.skipped(&s));
        let custom = h.property_drawer().filter(|_| !hidden);
        let custom = custom.and_then(|d| d.get("CUSTOM_ID"));
        let custom = custom.filter(|c| !c.is_empty());
        let explicit = self.opts.headline_ids == HeadlineIds::Explicit;
        let node = self.node_ids.get(&start).copied();
        let mut id = None;
        if explicit {
            id = node.and_then(|k| self.ids.try_claim(format!("id-{}", encode_id(k))));
            if id.is_none() {
                id = custom
                    .as_deref()
                    .and_then(|c| self.ids.try_claim(format!("cid-{}", encode_id(c))));
            }
        }
        let id = id.unwrap_or_else(|| {
            let s = slug(shown);
            let base = if s.is_empty() {
                "h".to_string()
            } else {
                format!("h-{s}")
            };
            self.ids.claim(base)
        });
        if let Some(k) = node {
            self.found.nodes.entry(k.to_string()).or_insert(id.clone());
        }
        if let Some(c) = custom {
            self.found.custom_ids.entry(c).or_insert(id.clone());
        }
        self.found
            .titles
            .entry(title.to_string())
            .or_insert(id.clone());
        Some(id)
    }

    pub fn headline(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let Some(h) = Headline::cast(n.clone()) else {
            return;
        };
        let title_node = n.children().find(|c| c.kind() == HEADLINE_TITLE);
        let plain = title_node.as_ref().map(plain_text).unwrap_or_default();
        let mut title = Vec::new();
        if let Some(t) = &title_node {
            self.objects(t, &mut title);
        }
        let shown = super::text::text_of(&without_footnote_refs(&title));
        let anchor = self.headline_anchor(n, &h, (&plain, &shown));
        let mut head = Element::new(Tag::heading(h.level()));
        if let Some(id) = anchor {
            head = head.attr(Attr::Id, id);
        }
        let head = head.with(self.headline_line(&h, title));
        let level = format!("org-level-{}", h.level());
        let mut section = vec![head.into()];
        self.elements(n, &mut section);
        let wrapper = Element::new(Tag::Section)
            .class("org-section")
            .class(&level)
            .with(section);
        push(out, wrapper.into());
    }

    /// The headline's line: TODO keyword, priority, title and tags.
    fn headline_line(&mut self, h: &Headline, title: Vec<Node>) -> Vec<Node> {
        let mut line = Vec::new();
        if let Some(kw) = h.todo() {
            let mut span = Element::new(Tag::Span).class("org-todo");
            if self.done.contains(&kw) {
                span = span.class("org-done");
            }
            push(&mut line, span.with(vec![text(&kw)]).into());
            push(&mut line, text(" "));
        }
        if let Some(p) = token_text(h.syntax(), PRIORITY) {
            let span = Element::new(Tag::Span).class("org-priority");
            push(&mut line, span.with(vec![text(&p)]).into());
            push(&mut line, text(" "));
        }
        title.into_iter().for_each(|t| push(&mut line, t));
        trim_text(&mut line);
        let mut tags = Vec::new();
        for t in h.tags().iter().filter(|t| !t.is_empty()) {
            if !tags.is_empty() {
                push(&mut tags, text(" "));
            }
            let tag = Element::new(Tag::Span).class("org-tag").with(vec![text(t)]);
            push(&mut tags, tag.into());
        }
        if !tags.is_empty() {
            push(&mut line, text(" "));
            let tags = Element::new(Tag::Span).class("org-tags").with(tags);
            push(&mut line, tags.into());
        }
        line
    }

    pub fn list(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let items: Vec<SyntaxNode> = n.children().filter(|c| c.kind() == ITEM).collect();
        let Some(first) = items.first() else {
            return;
        };
        let descriptive = first.children().any(|c| c.kind() == ITEM_TAG_CONTENT);
        let bullet = token_text(first, BULLET).unwrap_or_default();
        let tag = match () {
            _ if descriptive => Tag::Dl,
            _ if matches!(bullet.as_str(), "-" | "+" | "*") => Tag::Ul,
            _ => Tag::Ol,
        };
        let mut children = Vec::new();
        for item in &items {
            if self.skipped(item) {
                continue;
            }
            self.deeper(item, |b| b.item(item, descriptive, &mut children));
        }
        push(out, Element::new(tag).with(children).into());
    }

    fn item_anchor(&mut self, item: &SyntaxNode) -> Option<String> {
        if self.mode == Mode::Meta {
            return None;
        }
        let id = self.node_ids.get(&Span::of(item).start).copied()?;
        let anchor = self.ids.try_claim(format!("id-{}", encode_id(id)))?;
        self.found
            .nodes
            .entry(id.to_string())
            .or_insert(anchor.clone());
        Some(anchor)
    }

    pub fn item(&mut self, item: &SyntaxNode, descriptive: bool, out: &mut Vec<Node>) {
        let anchor = self.item_anchor(item);
        let mut body = Vec::new();
        if let Some(c) = token_text(item, CHECKBOX) {
            let code = Element::new(Tag::Code).class("org-checkbox");
            push(&mut body, code.with(vec![text(&c)]).into());
            push(&mut body, text(" "));
        }
        if !descriptive {
            // A tag in a list that is not descriptive (its first item has
            // none) is shown as written, not dropped.
            if let Some(t) = item.children().find(|c| c.kind() == ITEM_TAG_CONTENT) {
                self.objects(&t, &mut body);
                push(&mut body, text(" :: "));
            }
        }
        self.elements(item, &mut body);
        if !descriptive {
            let mut li = Element::new(Tag::Li);
            if let Some(a) = anchor {
                li = li.attr(Attr::Id, a);
            }
            if let Some(v) = token_text(item, COUNTER).and_then(|c| counter_value(&c)) {
                li = li.attr(Attr::Value, v);
            }
            return push(out, li.with(body).into());
        }
        let mut dt = Element::new(Tag::Dt);
        if let Some(a) = anchor {
            dt = dt.attr(Attr::Id, a);
        }
        let mut tag = Vec::new();
        if let Some(t) = item.children().find(|c| c.kind() == ITEM_TAG_CONTENT) {
            self.objects(&t, &mut tag);
        }
        trim_text(&mut tag);
        push(out, dt.with(tag).into());
        push(out, Element::new(Tag::Dd).with(body).into());
    }

    pub fn table(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let rows: Vec<SyntaxNode> = n.children().filter(|c| c.kind() == TABLE_ROW).collect();
        if rows.is_empty() {
            return self.table_el(n, out);
        }
        // Org's special rows (column groups, cookies, marks) and special
        // column are instructions, not data: dropped before the header is
        // found, as ox drops them.
        let special_column = has_special_column(&rows);
        let rows: Vec<SyntaxNode> = rows
            .into_iter()
            .filter(|r| !is_special_row(r, special_column))
            .collect();
        let mut groups: Vec<Vec<&SyntaxNode>> = vec![Vec::new()];
        for r in &rows {
            if is_rule(r) {
                if groups.last().is_some_and(|g| !g.is_empty()) {
                    groups.push(Vec::new());
                }
            } else if let Some(g) = groups.last_mut() {
                g.push(r);
            }
        }
        groups.retain(|g| !g.is_empty());
        let mut children = Vec::new();
        let caption = self.caption(n);
        if !caption.is_empty() {
            push(
                &mut children,
                Element::new(Tag::Caption).with(caption).into(),
            );
        }
        let header = groups.len() >= 2;
        for (i, g) in groups.iter().enumerate() {
            let head = header && i == 0;
            let mut trs = Vec::new();
            for r in g {
                let tr = self.table_row(r, head, special_column);
                push(&mut trs, tr.into());
            }
            let wrap = if head { Tag::Thead } else { Tag::Tbody };
            push(&mut children, Element::new(wrap).with(trs).into());
        }
        push(out, Element::new(Tag::Table).with(children).into());
    }

    fn table_row(&mut self, row: &SyntaxNode, head: bool, special_column: bool) -> Element {
        let mut cells = Vec::new();
        let skip = usize::from(special_column);
        for c in row.children().filter(|c| c.kind() == TABLE_CELL).skip(skip) {
            let mut inner = Vec::new();
            for part in c.children_with_tokens() {
                match part {
                    NodeOrToken::Token(t) if t.kind() == MARKUP => {}
                    NodeOrToken::Token(t) => push(&mut inner, text(t.text())),
                    NodeOrToken::Node(o) => self.deeper(&o, |b| b.object(&o, &mut inner)),
                }
            }
            trim_text(&mut inner);
            let cell = if head {
                Element::new(Tag::Th).attr(Attr::Scope, "col")
            } else {
                Element::new(Tag::Td)
            };
            push(&mut cells, cell.with(inner).into());
        }
        Element::new(Tag::Tr).with(cells)
    }

    /// A table.el table: its text, verbatim.
    fn table_el(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let raw: String = tokens(n).map(|t| t.text().to_string()).collect();
        let code = Element::new(Tag::Code).with(vec![text(raw.trim_end_matches(['\n', '\r']))]);
        let pre = Element::new(Tag::Pre)
            .class("org-table-el")
            .with(vec![code.into()]);
        push(out, pre.into());
    }

    pub fn verse(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let parts: Vec<_> = n
            .children_with_tokens()
            .filter(|e| {
                // The blank lines after the closing line are the block's
                // post-blank, not verse.
                !matches!(
                    e.kind(),
                    BLOCK_BEGIN | BLOCK_END | AFFILIATED_KEYWORD | BLANK_LINE
                )
            })
            .collect();
        // The verse's own final line ending is not a break; one before
        // more content is.
        let last_newline = parts
            .last()
            .filter(|e| e.kind() == NEWLINE)
            .map(|_| parts.len() - 1);
        let mut inner = Vec::new();
        let mut at_bol = true;
        for (i, part) in parts.into_iter().enumerate() {
            match part {
                NodeOrToken::Token(t) if t.kind() == NEWLINE => {
                    if Some(i) != last_newline {
                        push(&mut inner, Element::new(Tag::Br).into());
                        push(&mut inner, text("\n"));
                    }
                    at_bol = true;
                }
                NodeOrToken::Token(t) => {
                    push(&mut inner, text(&verse_text(t.text(), at_bol)));
                    at_bol = false;
                }
                NodeOrToken::Node(o) => {
                    self.deeper(&o, |b| b.object(&o, &mut inner));
                    at_bol = false;
                }
            }
        }
        trim_text(&mut inner);
        let p = Element::new(Tag::P).class("org-verse").with(inner);
        push(out, p.into());
    }

    /// A src block `:exports` shows the code of (`element` skips the others).
    pub fn src(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let line = n
            .children()
            .find(|c| c.kind() == BLOCK_BEGIN)
            .map(|l| l.to_string())
            .unwrap_or_default();
        let (lang, _) = super::babel::src_line(&line);
        let lang = Some(lang).filter(|l| super::links::is_valid_language(l));
        let mut code = Element::new(Tag::Code);
        if let Some(l) = lang {
            code = code.class(&format!("language-{l}"));
        }
        let raw = token_text(n, BLOCK_CONTENT).unwrap_or_default();
        let code = code.with(vec![text(&unescape_code(&raw))]);
        let pre = Element::new(Tag::Pre)
            .class("org-src")
            .with(vec![code.into()]);
        push(out, pre.into());
    }

    /// An example block (or any lesser block shown verbatim).
    pub fn verbatim(&mut self, n: &SyntaxNode, class: &str, out: &mut Vec<Node>) {
        let raw = token_text(n, BLOCK_CONTENT).unwrap_or_default();
        let code = Element::new(Tag::Code).with(vec![text(&unescape_code(&raw))]);
        let pre = Element::new(Tag::Pre).class(class).with(vec![code.into()]);
        push(out, pre.into());
    }

    pub fn fixed_width(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let raw: String = tokens(n).map(|t| t.text().to_string()).collect();
        let body: String = raw
            .split_inclusive('\n')
            .map(|l| {
                let rest = l.trim_start_matches([' ', '\t']);
                let rest = rest.strip_prefix(':').unwrap_or(rest);
                rest.strip_prefix(' ').unwrap_or(rest).to_string()
            })
            .collect();
        let code = Element::new(Tag::Code).with(vec![text(body.trim_end_matches(['\n', '\r']))]);
        let pre = Element::new(Tag::Pre)
            .class("org-fixed-width")
            .with(vec![code.into()]);
        push(out, pre.into());
    }

    pub fn latex_environment(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let raw: String = n
            .children_with_tokens()
            .filter(|e| !matches!(e.kind(), AFFILIATED_KEYWORD | BLANK_LINE))
            .map(|e| e.to_string())
            .collect();
        let code = Element::new(Tag::Code).with(vec![text(raw.trim_end_matches(['\n', '\r']))]);
        let pre = Element::new(Tag::Pre)
            .class("org-latex-environment")
            .with(vec![code.into()]);
        push(out, pre.into());
    }
}

/// `nodes` without footnote references: a slug must not carry a note's
/// number, which an unrelated earlier note would change.
fn without_footnote_refs(nodes: &[Node]) -> Vec<Node> {
    nodes
        .iter()
        .filter(
            |n| !matches!(n, Node::Element(e) if e.get(Attr::Class) == Some("org-footnote-ref")),
        )
        .map(|n| match n {
            Node::Element(e) => {
                let mut e = e.clone();
                e.children = without_footnote_refs(&e.children);
                Node::Element(e)
            }
            t => t.clone(),
        })
        .collect()
}

/// `[@N]`'s N when it is a decimal number.
fn counter_value(counter: &str) -> Option<String> {
    let n = counter.strip_prefix("[@")?.strip_suffix(']')?;
    (!n.is_empty() && n.len() <= 9 && n.bytes().all(|b| b.is_ascii_digit()))
        .then(|| n.trim_start_matches('0').to_string())
        .map(|v| if v.is_empty() { "0".to_string() } else { v })
}

/// A cell's contents when they are one plain string, trimmed (`Some("")`
/// for an empty cell); `None` when they hold an object, which is never a
/// mark or a cookie (Org compares the parsed contents with a string).
fn cell_string(cell: &SyntaxNode) -> Option<String> {
    let mut s = String::new();
    for part in cell.children_with_tokens() {
        match part {
            NodeOrToken::Token(t) if t.kind() == MARKUP => {}
            NodeOrToken::Token(t) => s.push_str(t.text()),
            NodeOrToken::Node(_) => return None,
        }
    }
    Some(s.trim_matches([' ', '\t']).to_string())
}

/// The cells of a row (no rule) as [`cell_string`]s.
fn row_strings(row: &SyntaxNode) -> Vec<Option<String>> {
    row.children()
        .filter(|c| c.kind() == TABLE_CELL)
        .map(|c| cell_string(&c))
        .collect()
}

/// `org-export-table-has-special-column-p`: every non-rule row's first cell
/// is empty or one of `/ # ! $ * _ ^`, and one is not empty.
fn has_special_column(rows: &[SyntaxNode]) -> bool {
    let mut marked = false;
    for r in rows.iter().filter(|r| !is_rule(r)) {
        match row_strings(r).into_iter().next() {
            // No cell at all, or an empty one.
            None => {}
            Some(Some(s)) if s.is_empty() => {}
            Some(Some(s)) if matches!(s.as_str(), "/" | "#" | "!" | "$" | "*" | "_" | "^") => {
                marked = true
            }
            _ => return false,
        }
    }
    marked
}

/// `org-export-table-row-is-special-p`: a column-group row (`/` first), a
/// marked row (`^ _ $ !` first, in a table with a special column), or a
/// row of only cookies (`<[lrc]?N?>`, any case) and empty cells.
fn is_special_row(row: &SyntaxNode, special_column: bool) -> bool {
    if is_rule(row) {
        return false;
    }
    let cells = row_strings(row);
    let first = cells.first().cloned().flatten();
    match first.as_deref() {
        Some("/") => return true,
        Some("^" | "_" | "$" | "!") if special_column => return true,
        _ => {}
    }
    let mut cookie = false;
    for c in &cells {
        match c.as_deref() {
            Some("") => {}
            Some(s) if is_cookie(s) => cookie = true,
            _ => return false,
        }
    }
    cookie
}

/// `\`<[lrc]?\([0-9]+\)?>\'`, case-insensitive as Emacs matches it.
fn is_cookie(s: &str) -> bool {
    let Some(inner) = s.strip_prefix('<').and_then(|r| r.strip_suffix('>')) else {
        return false;
    };
    let digits = inner
        .strip_prefix(['l', 'r', 'c', 'L', 'R', 'C'])
        .unwrap_or(inner);
    digits.bytes().all(|b| b.is_ascii_digit())
}

fn is_rule(row: &SyntaxNode) -> bool {
    tokens(row)
        .find(|t| t.kind() != WHITESPACE)
        .is_some_and(|t| t.kind() == TEXT && t.text().trim_start().starts_with("|-"))
}

/// A verse line's leading spaces as no-break spaces (Org's verse export).
fn verse_text(s: &str, at_bol: bool) -> String {
    if !at_bol {
        return s.to_string();
    }
    let rest = s.trim_start_matches(' ');
    let lead = s.len() - rest.len();
    format!("{}{rest}", "\u{a0}".repeat(lead))
}

/// `org-unescape-code-in-string`: a comma at the start of a line (after
/// blanks) before `*`, `,*`, `#+` or `,#+` is removed.
fn unescape_code(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.split_inclusive('\n') {
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        let rest = &line[indent..];
        let escaped = rest.strip_prefix(',').is_some_and(|r| {
            r.starts_with('*') || r.starts_with(",*") || r.starts_with("#+") || r.starts_with(",#+")
        });
        out.push_str(&line[..indent]);
        out.push_str(if escaped { &rest[1..] } else { rest });
    }
    out.trim_end_matches(['\n', '\r']).to_string()
}
