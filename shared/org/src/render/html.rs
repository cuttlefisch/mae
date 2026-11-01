//! HTML: the element tree serialised. Text and attribute values are
//! escaped; every attribute value is double-quoted; void elements have no
//! end tag. Nothing else is written, so the output parses back (with an
//! HTML5 parser, `tests/render_html_parse.rs`) to the same tree.

use super::tree::{Element, Node};

/// `nodes` as HTML.
pub fn to_html(nodes: &[Node]) -> String {
    let mut out = String::new();
    for n in nodes {
        node(n, &mut out);
    }
    out
}

fn node(n: &Node, out: &mut String) {
    match n {
        Node::Text(t) => escape(t, false, out),
        Node::Element(e) => element(e, out),
    }
}

fn element(e: &Element, out: &mut String) {
    out.push('<');
    out.push_str(e.tag.as_str());
    for (a, v) in &e.attrs {
        out.push(' ');
        out.push_str(a.as_str());
        out.push_str("=\"");
        escape(v, true, out);
        out.push('"');
    }
    out.push('>');
    if e.tag.is_void() {
        return;
    }
    for c in &e.children {
        node(c, out);
    }
    out.push_str("</");
    out.push_str(e.tag.as_str());
    out.push('>');
}

/// `&`, `<` and `>` always; `"` in an attribute value.
fn escape(s: &str, attr: bool, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}
