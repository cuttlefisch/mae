//! Markdown (GFM) export: `mae-org`'s rendered tree ([`crate::document`])
//! written as Markdown.
//!
//! This reads the same tree the HTML export serialises, so what is published
//! is `mae-org`'s decision in both formats (ADR-112 D3): a `:noexport:` or
//! COMMENT subtree, a comment, a drawer or an export block is not in the tree
//! and so cannot reach the Markdown.
//!
//! Every tag `mae-org` can emit ([`mae_org::TAGS`]) has an explicit mapping
//! ([`kind`]; a test fails when a new one has none). Text is escaped so that
//! nothing in the document becomes Markdown structure it was not: every
//! ASCII punctuation character Markdown gives a meaning anywhere is
//! backslash-escaped, and line-initial block markers (`-`, `+`, `=`, `1.`)
//! too. Code (src, example, fixed-width blocks and inline code) is written
//! verbatim, fenced by a run of backticks longer than any inside it.
//!
//! No raw HTML from the document is emitted: Markdown export blocks are not
//! in the tree. The only HTML written is `<sub>`/`<sup>`, which Markdown has
//! no syntax for, around escaped text.

use mae_org::{text_of, Attr, Element, Node, Tag};

use crate::document::{has_class, Exported};

/// The whole Markdown document for `doc`: YAML front matter for the title,
/// author and date (each a JSON-quoted string, which YAML reads as a
/// double-quoted scalar, so no value can end the block), then the body.
pub fn to_markdown(doc: &Exported) -> String {
    let mut md = String::with_capacity(4096);
    let mut front = Vec::new();
    for (key, value, shown) in [
        ("title", &doc.title, true),
        ("author", &doc.author, doc.settings.with_author),
        ("date", &doc.date, doc.settings.with_date),
    ] {
        let text = text_of(value);
        if shown && !text.is_empty() {
            let quoted = serde_json::to_string(&text).unwrap_or_else(|_| "\"\"".to_string());
            front.push(format!("{key}: {quoted}"));
        }
    }
    if !front.is_empty() {
        md.push_str("---\n");
        for line in front {
            md.push_str(&line);
            md.push('\n');
        }
        md.push_str("---\n\n");
    }
    let body = blocks(&doc.body).join("\n\n");
    if !body.is_empty() {
        md.push_str(&body);
        md.push('\n');
    }
    md
}

/// How a tag is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Starts a block of its own.
    Block,
    /// Part of the surrounding text.
    Inline,
}

/// The mapping for every tag `mae-org` emits. `None` only for a tag added to
/// `mae-org` after this was written (`Tag` is `#[non_exhaustive]`); the test
/// over [`mae_org::TAGS`] fails then, so the new tag gets a deliberate
/// mapping. At run time such a tag is written as its children.
pub(crate) fn kind(tag: Tag) -> Option<Kind> {
    use Tag::*;
    Some(match tag {
        Blockquote | Caption | Dd | Div | Dl | Dt | Figcaption | Figure | H1 | H2 | H3 | H4
        | H5 | H6 | Hr | Li | Ol | P | Pre | Section | Table | Tbody | Td | Th | Thead | Tr
        | Ul => Kind::Block,
        A | Br | Code | Del | Em | Img | Span | Strong | Sub | Sup | Time => Kind::Inline,
        _ => return None,
    })
}

fn is_block(n: &Node) -> bool {
    match n {
        Node::Element(e) => kind(e.tag) != Some(Kind::Inline),
        _ => false,
    }
}

/// `nodes` as Markdown blocks (to be joined by a blank line). A run of inline
/// nodes between blocks becomes a paragraph of its own.
fn blocks(nodes: &[Node]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < nodes.len() {
        if is_block(&nodes[i]) {
            if let Node::Element(e) = &nodes[i] {
                out.extend(block(e));
            }
            i += 1;
            continue;
        }
        let start = i;
        while i < nodes.len() && !is_block(&nodes[i]) {
            i += 1;
        }
        push_paragraph(&mut out, inline_text(&nodes[start..i], Mode::Paragraph));
    }
    out
}

fn push_paragraph(out: &mut Vec<String>, text: String) {
    let text = text.trim_end().to_string();
    if !text.trim().is_empty() {
        out.push(text);
    }
}

fn block(e: &Element) -> Vec<String> {
    use Tag::*;
    match e.tag {
        P => {
            let mut out = Vec::new();
            push_paragraph(&mut out, inline_text(&e.children, Mode::Paragraph));
            out
        }
        H1 | H2 | H3 | H4 | H5 | H6 => {
            let level = heading_level(e.tag);
            let text = inline_text(&e.children, Mode::Line);
            vec![format!("{} {}", "#".repeat(level), text.trim())]
        }
        Hr => vec!["---".to_string()],
        Pre => vec![fenced(e)],
        Blockquote => {
            let inner = blocks(&e.children).join("\n\n");
            if inner.is_empty() {
                return Vec::new();
            }
            vec![prefix_lines(&inner, "> ", ">")]
        }
        Ul | Ol => list(e),
        Dl => description_list(e),
        Table => table(e),
        Section if has_class(e, "org-footnotes") => footnotes(e),
        Caption | Figcaption => {
            let mut out = Vec::new();
            push_paragraph(&mut out, inline_text(&e.children, Mode::Paragraph));
            out
        }
        // Containers: their content, in place. Sections and center, special
        // and dynamic blocks are structure, not text Markdown can show.
        _ => blocks(&e.children),
    }
}

fn heading_level(tag: Tag) -> usize {
    match tag {
        Tag::H1 => 1,
        Tag::H2 => 2,
        Tag::H3 => 3,
        Tag::H4 => 4,
        Tag::H5 => 5,
        _ => 6,
    }
}

/// Every line of `text` after `prefix` (`blank` for an empty line).
fn prefix_lines(text: &str, prefix: &str, blank: &str) -> String {
    text.split('\n')
        .map(|l| {
            if l.is_empty() {
                blank.to_string()
            } else {
                format!("{prefix}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A list item: `marker` and a space before the first line, the rest
/// indented to the content's column.
fn list_item(marker: &str, body: &str) -> String {
    let indent = " ".repeat(marker.chars().count() + 1);
    let mut out = String::new();
    for (i, line) in body.split('\n').enumerate() {
        if i == 0 {
            out.push_str(marker);
            if !line.is_empty() {
                out.push(' ');
                out.push_str(line);
            }
        } else {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(&indent);
                out.push_str(line);
            }
        }
    }
    out
}

/// An item's content: a leading run of inline nodes (a checkbox, a
/// description-less `term ::`) joins the item's first paragraph, as it sits
/// on the item's first line in the source.
fn item_body(lead: &str, children: &[Node]) -> String {
    let mut i = 0;
    while i < children.len() && !is_block(&children[i]) {
        i += 1;
    }
    let mut first = lead.to_string();
    first.push_str(&inline_text(&children[..i], Mode::Paragraph));
    if let Some(Node::Element(p)) = children.get(i) {
        if p.tag == Tag::P && !has_class(p, "org-verse") {
            first.push_str(&inline_text(&p.children, Mode::Paragraph));
            i += 1;
        }
    }
    let mut body = String::new();
    let first = first.trim_end();
    if !first.trim().is_empty() {
        body.push_str(first);
    }
    // A list right after the item's text stays tight (no blank line), as
    // it is written in Org; any other block is a paragraph of the item.
    for node in &children[i..] {
        let list = matches!(node, Node::Element(e) if matches!(e.tag, Tag::Ul | Tag::Ol | Tag::Dl));
        for b in blocks(std::slice::from_ref(node)) {
            if !body.is_empty() {
                body.push_str(if list { "\n" } else { "\n\n" });
            }
            body.push_str(&b);
        }
    }
    body
}

fn list(e: &Element) -> Vec<String> {
    let ordered = e.tag == Tag::Ol;
    let mut n: u64 = 1;
    let mut items = Vec::new();
    for c in &e.children {
        let Node::Element(li) = c else { continue };
        if let Some(v) = li.get(Attr::Value).and_then(|v| v.parse().ok()) {
            n = v;
        }
        let marker = if ordered {
            format!("{n}.")
        } else {
            "-".to_string()
        };
        n = n.saturating_add(1);
        items.push(list_item(&marker, &item_body("", &li.children)));
    }
    if items.is_empty() {
        Vec::new()
    } else {
        vec![items.join("\n")]
    }
}

/// `<dl>`: Markdown has no description list, so each term is an item whose
/// first line is the term in bold.
fn description_list(e: &Element) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    let mut term: Option<String> = None;
    for c in &e.children {
        let Node::Element(x) = c else { continue };
        match x.tag {
            Tag::Dt => {
                if let Some(t) = term.take() {
                    items.push(list_item("-", &t));
                }
                let text = inline_text(&x.children, Mode::Line);
                term = Some(format!("{}:", strong(text.trim())));
            }
            Tag::Dd => {
                let lead = term.take().map(|t| format!("{t} ")).unwrap_or_default();
                items.push(list_item("-", &item_body(&lead, &x.children)));
            }
            _ => {}
        }
    }
    if let Some(t) = term {
        items.push(list_item("-", &t));
    }
    if items.is_empty() {
        Vec::new()
    } else {
        vec![items.join("\n")]
    }
}

fn strong(text: &str) -> String {
    if text.is_empty() {
        String::new()
    } else {
        format!("**{text}**")
    }
}

/// The text of a `<pre>` as written (no trimming: code is byte-exact), and
/// its language from a `language-L` class on its `<code>`.
fn fenced(pre: &Element) -> String {
    let mut text = String::new();
    raw_text(&pre.children, &mut text);
    let language = pre.children.iter().find_map(|c| match c {
        Node::Element(code) if code.tag == Tag::Code => code
            .get(Attr::Class)?
            .split_ascii_whitespace()
            .find_map(|c| c.strip_prefix("language-"))
            .map(str::to_string),
        _ => None,
    });
    let fence = "`".repeat(longest_run(&text, '`').max(2) + 1);
    let mut out = format!("{fence}{}\n", language.unwrap_or_default());
    out.push_str(&text);
    if !text.is_empty() && !text.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&fence);
    out
}

fn raw_text(nodes: &[Node], out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) => raw_text(&e.children, out),
            _ => {}
        }
    }
}

fn longest_run(text: &str, c: char) -> usize {
    let (mut best, mut run) = (0, 0);
    for x in text.chars() {
        if x == c {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

/// A GFM table. Its header is the `<thead>` row, or the first row when there
/// is none (GFM requires one). A `<caption>` becomes a paragraph before it.
fn table(e: &Element) -> Vec<String> {
    let mut out = Vec::new();
    let mut header: Option<Vec<String>> = None;
    let mut rows: Vec<Vec<String>> = Vec::new();
    for c in &e.children {
        let Node::Element(x) = c else { continue };
        match x.tag {
            Tag::Caption => {
                push_paragraph(&mut out, inline_text(&x.children, Mode::Paragraph));
            }
            Tag::Thead => {
                for r in rows_of(x) {
                    if header.is_none() {
                        header = Some(r);
                    } else {
                        rows.push(r);
                    }
                }
            }
            Tag::Tbody => rows.extend(rows_of(x)),
            Tag::Tr => rows.push(cells_of(x)),
            _ => {}
        }
    }
    let header = match header {
        Some(h) => h,
        None if !rows.is_empty() => rows.remove(0),
        None => return out,
    };
    let width = rows
        .iter()
        .map(Vec::len)
        .chain(std::iter::once(header.len()))
        .max()
        .unwrap_or(0)
        .max(1);
    let line = |cells: &[String]| {
        let mut s = String::from("|");
        for i in 0..width {
            s.push(' ');
            s.push_str(cells.get(i).map(String::as_str).unwrap_or(""));
            s.push_str(" |");
        }
        s
    };
    let mut t = vec![line(&header)];
    t.push(format!("|{}", " --- |".repeat(width)));
    t.extend(rows.iter().map(|r| line(r)));
    out.push(t.join("\n"));
    out
}

fn rows_of(group: &Element) -> Vec<Vec<String>> {
    group
        .children
        .iter()
        .filter_map(|c| match c {
            Node::Element(tr) if tr.tag == Tag::Tr => Some(cells_of(tr)),
            _ => None,
        })
        .collect()
}

fn cells_of(tr: &Element) -> Vec<String> {
    tr.children
        .iter()
        .filter_map(|c| match c {
            Node::Element(cell) if matches!(cell.tag, Tag::Td | Tag::Th) => {
                Some(inline_text(&cell.children, Mode::Cell).trim().to_string())
            }
            _ => None,
        })
        .collect()
}

/// The footnotes section: each definition as a GFM footnote, `[^N]:` and its
/// content (continuation lines indented four spaces). The back link to the
/// reference is HTML navigation, not content, and is left out.
fn footnotes(section: &Element) -> Vec<String> {
    let mut out = Vec::new();
    for c in &section.children {
        let Node::Element(def) = c else { continue };
        let Some(label) = def.get(Attr::Id).and_then(|id| id.strip_prefix("fn-")) else {
            continue;
        };
        let content: Vec<Node> = def
            .children
            .iter()
            .filter(|n| !is_back_link(n))
            .cloned()
            .collect();
        let body = blocks(&content).join("\n\n");
        let mut s = format!("[^{label}]:");
        for (i, line) in body.split('\n').enumerate() {
            if i == 0 {
                s.push(' ');
                s.push_str(line);
            } else {
                s.push('\n');
                if !line.is_empty() {
                    s.push_str("    ");
                    s.push_str(line);
                }
            }
        }
        out.push(s);
    }
    out
}

fn is_back_link(n: &Node) -> bool {
    let Node::Element(e) = n else { return false };
    if e.tag == Tag::A && has_class(e, "org-footnote-back") {
        return true;
    }
    e.tag == Tag::Sup
        && e.children.iter().any(is_back_link)
        && e.children
            .iter()
            .all(|c| is_back_link(c) || matches!(c, Node::Text(t) if t.trim().is_empty()))
}

/// Where inline text goes, which decides what a line break becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A paragraph: a line break is a hard break (`\` at the line's end).
    Paragraph,
    /// One line (a heading, a term): a line break is a space.
    Line,
    /// A table cell: one line, and `|` escaped inside code spans too.
    Cell,
}

fn inline_text(nodes: &[Node], mode: Mode) -> String {
    let mut out = String::new();
    for n in nodes {
        inline(n, mode, &mut out);
    }
    out
}

fn inline(n: &Node, mode: Mode, out: &mut String) {
    match n {
        Node::Text(t) => escape_text(t, mode, out),
        Node::Element(e) => inline_element(e, mode, out),
        _ => {}
    }
}

fn inline_element(e: &Element, mode: Mode, out: &mut String) {
    use Tag::*;
    match e.tag {
        Strong => delimited(e, "**", mode, out),
        Em => delimited(e, "*", mode, out),
        Del => delimited(e, "~~", mode, out),
        Code if has_class(e, "org-checkbox") => {
            let mut t = String::new();
            raw_text(&e.children, &mut t);
            // GFM task-list markers; the space after comes from the text
            // that follows the checkbox in the tree.
            match t.as_str() {
                "[X]" | "[x]" => out.push_str("[x]"),
                "[ ]" => out.push_str("[ ]"),
                other => code_span(other, mode, out),
            }
        }
        Code => {
            let mut t = String::new();
            raw_text(&e.children, &mut t);
            code_span(&t, mode, out);
        }
        A => {
            let text = inline_text(&e.children, mode);
            match e.get(Attr::Href) {
                Some(href) if !text.trim().is_empty() => {
                    out.push('[');
                    out.push_str(&text);
                    out.push_str("](");
                    out.push_str(&destination(href));
                    out.push(')');
                }
                _ => out.push_str(&text),
            }
        }
        Img => {
            let alt = e.get(Attr::Alt).unwrap_or("");
            if let Some(src) = e.get(Attr::Src) {
                out.push_str("![");
                escape_text(alt, Mode::Line, out);
                out.push_str("](");
                out.push_str(&destination(src));
                out.push(')');
            }
        }
        Br => match mode {
            Mode::Paragraph => out.push_str("\\\n"),
            Mode::Line | Mode::Cell => out.push(' '),
        },
        Sup if has_class(e, "org-footnote-ref") => {
            let mut label = String::new();
            raw_text(&e.children, &mut label);
            let label = label.trim();
            if !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric()) {
                out.push_str(&format!("[^{label}]"));
            }
        }
        Sub | Sup => {
            let tag = e.tag.as_str();
            out.push_str(&format!("<{tag}>"));
            for c in &e.children {
                inline(c, mode, out);
            }
            out.push_str(&format!("</{tag}>"));
        }
        Span if has_class(e, "org-tags") => {
            let tags: Vec<String> = e
                .children
                .iter()
                .filter_map(|c| match c {
                    Node::Element(t) => Some(inline_text(&t.children, Mode::Line)),
                    _ => None,
                })
                .collect();
            if !tags.is_empty() {
                out.push(':');
                out.push_str(&tags.join(":"));
                out.push(':');
            }
        }
        // Text-level containers (spans of every class, time) and, for a
        // block tag met inline, its content.
        _ => {
            for c in &e.children {
                inline(c, mode, out);
            }
        }
    }
}

/// `delim`…`delim` around the element's text, with its leading and trailing
/// whitespace moved outside (Markdown does not open or close emphasis next
/// to a space).
fn delimited(e: &Element, delim: &str, mode: Mode, out: &mut String) {
    let inner = inline_text(&e.children, mode);
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        out.push_str(&inner);
        return;
    }
    let lead = &inner[..inner.len() - inner.trim_start().len()];
    let trail = &inner[inner.trim_end().len()..];
    out.push_str(lead);
    out.push_str(delim);
    out.push_str(trimmed);
    out.push_str(delim);
    out.push_str(trail);
}

/// An inline code span, verbatim: delimited by a backtick run longer than any
/// inside, padded with a space when it starts or ends with a backtick.
fn code_span(text: &str, mode: Mode, out: &mut String) {
    let text = match mode {
        Mode::Paragraph => text.to_string(),
        Mode::Line => text.replace('\n', " "),
        // GFM splits a row on `|` before it parses code spans.
        Mode::Cell => text.replace('\n', " ").replace('|', "\\|"),
    };
    if text.is_empty() {
        return;
    }
    let ticks = "`".repeat(longest_run(&text, '`') + 1);
    let pad = text.starts_with('`') || text.ends_with('`');
    out.push_str(&ticks);
    if pad {
        out.push(' ');
    }
    out.push_str(&text);
    if pad {
        out.push(' ');
    }
    out.push_str(&ticks);
}

/// A link destination: `<…>` when it holds a character a bare destination
/// cannot (space, parentheses, angle brackets).
fn destination(href: &str) -> String {
    if href
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>'))
    {
        let escaped = href
            .replace('\\', "\\\\")
            .replace('<', "\\<")
            .replace('>', "\\>")
            .replace(['\n', '\r'], " ");
        format!("<{escaped}>")
    } else {
        href.to_string()
    }
}

/// Characters escaped wherever they are: each can start Markdown structure
/// (emphasis, code, links, HTML, tables, strike-through, entities, headings,
/// math) mid-line.
const ALWAYS: &[char] = &[
    '\\', '`', '*', '_', '[', ']', '<', '>', '|', '~', '&', '#', '$',
];

/// Writes text so Markdown reads it as text. At the start of a line, leading
/// blanks are dropped (four would make a code block) and a block marker
/// (`-`, `+`, `=`, `1.`, `1)`) is escaped.
fn escape_text(text: &str, mode: Mode, out: &mut String) {
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let c = if c == '\n' && mode != Mode::Paragraph {
            ' '
        } else {
            c
        };
        let at_line_start = out.is_empty() || out.ends_with('\n');
        // A blank line would end the paragraph: never write one.
        if c == '\n' && at_line_start {
            continue;
        }
        if at_line_start && mode == Mode::Paragraph {
            if c == ' ' || c == '\t' {
                continue;
            }
            if matches!(c, '-' | '+' | '=') {
                out.push('\\');
                out.push(c);
                continue;
            }
            if c.is_ascii_digit() {
                let digits: String = text[i..].chars().take_while(char::is_ascii_digit).collect();
                let after = text[i + digits.len()..].chars().next();
                if matches!(after, Some('.' | ')')) {
                    out.push_str(&digits);
                    out.push('\\');
                    out.push(after.unwrap_or('.'));
                    for _ in 0..digits.len() {
                        chars.next();
                    }
                    continue;
                }
            }
        }
        if ALWAYS.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tag `mae-org` can emit has a deliberate mapping. A tag added to
    /// `mae-org` later fails here instead of falling through to "write its
    /// children" unnoticed.
    #[test]
    fn every_mae_org_tag_has_a_markdown_mapping() {
        let unmapped: Vec<&str> = mae_org::TAGS
            .iter()
            .filter(|t| kind(**t).is_none())
            .map(|t| t.as_str())
            .collect();
        assert!(
            unmapped.is_empty(),
            "mae-org tags with no Markdown mapping: {unmapped:?}"
        );
        // And the mapping is a real decision, not one answer for all: the
        // block/inline split must match HTML's for the tags whose role is
        // unambiguous.
        assert_eq!(kind(Tag::P), Some(Kind::Block));
        assert_eq!(kind(Tag::Pre), Some(Kind::Block));
        assert_eq!(kind(Tag::Strong), Some(Kind::Inline));
        assert_eq!(kind(Tag::A), Some(Kind::Inline));
    }

    #[test]
    fn text_escaping_neutralises_markdown_structure() {
        let mut out = String::new();
        escape_text(
            "# not *a* [heading](x) <b> | `c` ~d~ &amp; $m$",
            Mode::Paragraph,
            &mut out,
        );
        assert_eq!(
            out,
            "\\# not \\*a\\* \\[heading\\](x) \\<b\\> \\| \\`c\\` \\~d\\~ \\&amp; \\$m\\$"
        );
        for (line, want) in [
            ("- item", "\\- item"),
            ("+ item", "\\+ item"),
            ("12. item", "12\\. item"),
            ("3) item", "3\\) item"),
            ("    code?", "code?"),
            ("===", "\\==="),
        ] {
            let mut out = String::new();
            escape_text(line, Mode::Paragraph, &mut out);
            assert_eq!(out, want, "line {line:?}");
        }
        // Mid-line, the same characters are text.
        let mut out = String::new();
        escape_text("a - b + c = 1. d", Mode::Paragraph, &mut out);
        assert_eq!(out, "a - b + c = 1. d");
    }

    #[test]
    fn a_code_span_is_never_closed_by_its_own_backticks() {
        let mut out = String::new();
        code_span("a``b", Mode::Paragraph, &mut out);
        assert_eq!(out, "```a``b```");
        let mut out = String::new();
        code_span("`x`", Mode::Paragraph, &mut out);
        assert_eq!(out, "`` `x` ``");
        let mut out = String::new();
        code_span("a|b", Mode::Cell, &mut out);
        assert_eq!(out, "`a\\|b`");
    }
}
