//! hast JSON (the unified/rehype syntax tree): `{"type": "root"|"element"|
//! "text", "tagName", "properties", "children", "value"}`. Keys are written
//! in a fixed order and properties in the element's attribute order, so the
//! same tree always gives the same bytes. `class` is `className`, an array;
//! `data-org-*` attributes are camel-cased (`dataOrgRel`), as
//! `property-information` names them; every other value is a string.

use super::tree::{Attr, Element, Node};
use super::{Rendered, TreeFormat};

/// `nodes` as a hast root.
pub fn to_hast(nodes: &[Node]) -> String {
    let mut out = String::new();
    root(nodes, &mut out);
    out
}

fn root(nodes: &[Node], out: &mut String) {
    out.push_str("{\"type\":\"root\",\"children\":");
    children(nodes, out);
    out.push('}');
}

fn children(nodes: &[Node], out: &mut String) {
    out.push('[');
    for (i, n) in nodes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        node(n, out);
    }
    out.push(']');
}

fn node(n: &Node, out: &mut String) {
    match n {
        Node::Text(t) => {
            out.push_str("{\"type\":\"text\",\"value\":");
            json_str(t, out);
            out.push('}');
        }
        Node::Element(e) => element(e, out),
    }
}

fn element(e: &Element, out: &mut String) {
    out.push_str("{\"type\":\"element\",\"tagName\":");
    json_str(e.tag.as_str(), out);
    out.push_str(",\"properties\":{");
    for (i, (a, v)) in e.attrs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_str(a.hast_name(), out);
        out.push(':');
        if *a == Attr::Class {
            out.push('[');
            for (j, c) in v.split(' ').filter(|c| !c.is_empty()).enumerate() {
                if j > 0 {
                    out.push(',');
                }
                json_str(c, out);
            }
            out.push(']');
        } else {
            json_str(v, out);
        }
    }
    out.push_str("},\"children\":");
    children(&e.children, out);
    out.push('}');
}

/// A JSON string: `"` and `\` escaped, control characters, U+2028 and
/// U+2029 as `\u` escapes (so the text is also a valid JavaScript literal).
pub(crate) fn json_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn tree(nodes: &[Node], format: TreeFormat, out: &mut String) {
    match format {
        TreeFormat::Hast => root(nodes, out),
        TreeFormat::Html => json_str(&super::to_html(nodes), out),
    }
}

/// [`Rendered::to_json`].
pub(crate) fn rendered_json(r: &Rendered, format: TreeFormat) -> String {
    let mut out = String::from("{\"body\":");
    tree(&r.body, format, &mut out);
    out.push_str(",\"nodes\":[");
    for (i, n) in r.nodes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"id\":");
        json_str(&n.id, &mut out);
        out.push_str(",\"anchor\":");
        match &n.anchor {
            Some(a) => json_str(a, &mut out),
            None => out.push_str("null"),
        }
        out.push_str(",\"title\":");
        tree(&n.title, format, &mut out);
        out.push_str(",\"title_unlinked\":");
        tree(&n.title_unlinked, format, &mut out);
        out.push_str(",\"aliases\":[");
        for (j, a) in n.aliases.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            tree(a, format, &mut out);
        }
        out.push_str("],\"properties\":[");
        for (j, (k, v)) in n.properties.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out.push_str("{\"key\":");
            json_str(k, &mut out);
            out.push_str(",\"value\":");
            tree(v, format, &mut out);
            out.push('}');
        }
        out.push_str("]}");
    }
    out.push_str("],\"diagnostics\":[");
    for (i, d) in r.diagnostics.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let s = d.span();
        out.push_str(&format!(
            "{{\"kind\":\"{}\",\"start\":{},\"end\":{}}}",
            d.kind_name(),
            s.start,
            s.end
        ));
    }
    out.push_str("]}");
    out
}
