//! SVG sanitising (ADR-115 D3a): an element and attribute allowlist, applied
//! before hashing, so what is stored has no script, no event handler and no
//! reference outside the document. It is still never served inline from the
//! API origin (D3): this is defence for the clients that display it, which
//! sanitise again themselves.
//!
//! The output is re-serialised from the parsed tree in one canonical form, so
//! sanitising a sanitised file returns the same bytes (D3a's idempotence). A
//! DTD is refused outright (no entity expansion, so no entity bomb), which
//! makes such a file not an SVG here: it is stored as a download.

use roxmltree::{Document, Node, ParsingOptions};

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// Bound on parsed nodes: a well-formed but enormous tree is refused rather
/// than walked. Far above any diagram a page carries.
const MAX_NODES: u32 = 200_000;

/// Elements kept, in the SVG namespace only. Absent on purpose: `script`,
/// `style` (CSS can fetch), `foreignObject` (HTML), `image` and `feImage`
/// (external or embedded bitmaps with their own metadata), `a` (navigation),
/// `animate*`/`set` (they can rewrite `href`), `use` of anything not local.
const ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "title",
    "desc",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "textPath",
    "linearGradient",
    "radialGradient",
    "stop",
    "pattern",
    "clipPath",
    "mask",
    "marker",
];

/// Attributes kept, with no namespace. Every `on*` handler is absent, and so
/// is `style` (CSS `url()` and `@import` can fetch).
const ATTRIBUTES: &[&str] = &[
    "id",
    "class",
    "x",
    "y",
    "width",
    "height",
    "viewBox",
    "preserveAspectRatio",
    "version",
    "d",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "x1",
    "y1",
    "x2",
    "y2",
    "fx",
    "fy",
    "fr",
    "points",
    "transform",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-opacity",
    "opacity",
    "clip-path",
    "clip-rule",
    "mask",
    "marker-start",
    "marker-mid",
    "marker-end",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "text-anchor",
    "dominant-baseline",
    "letter-spacing",
    "word-spacing",
    "dx",
    "dy",
    "rotate",
    "lengthAdjust",
    "textLength",
    "startOffset",
    "offset",
    "stop-color",
    "stop-opacity",
    "gradientUnits",
    "gradientTransform",
    "spreadMethod",
    "patternUnits",
    "patternContentUnits",
    "patternTransform",
    "clipPathUnits",
    "maskUnits",
    "maskContentUnits",
    "markerWidth",
    "markerHeight",
    "markerUnits",
    "refX",
    "refY",
    "orient",
    "visibility",
    "display",
    "color",
    "vector-effect",
    "shape-rendering",
    "text-rendering",
    "paint-order",
    "href",
];

fn options<'a>() -> ParsingOptions<'a> {
    ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_NODES,
        entity_resolver: None,
    }
}

/// Whether `bytes` are an SVG document: UTF-8 XML, no DTD, whose root element
/// is `svg` in the SVG namespace.
pub fn is_svg(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let head = text.trim_start_matches('\u{FEFF}').trim_start();
    if !head.starts_with('<') {
        return false;
    }
    Document::parse_with_options(text, options()).is_ok_and(|d| is_svg_root(d.root_element()))
}

fn is_svg_root(n: Node) -> bool {
    n.tag_name().name() == "svg" && n.tag_name().namespace() == Some(SVG_NS)
}

/// The sanitised document, or `None` if `bytes` are not an SVG ([`is_svg`]).
pub fn sanitize(bytes: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let doc = Document::parse_with_options(text, options()).ok()?;
    let root = doc.root_element();
    if !is_svg_root(root) {
        return None;
    }
    let mut body = String::new();
    write_element(root, &mut body, true);
    Some(body.into_bytes())
}

fn kept_element(n: Node) -> bool {
    n.is_element()
        && n.tag_name().namespace() == Some(SVG_NS)
        && ELEMENTS.contains(&n.tag_name().name())
}

fn write_element(n: Node, out: &mut String, root: bool) {
    let name = n.tag_name().name();
    out.push('<');
    out.push_str(name);
    if root {
        out.push_str(" xmlns=\"");
        out.push_str(SVG_NS);
        out.push('"');
        if uses_xlink(n) {
            out.push_str(" xmlns:xlink=\"");
            out.push_str(XLINK_NS);
            out.push('"');
        }
    }
    for a in n.attributes() {
        let Some(qualified) = kept_attribute(a.namespace(), a.name(), a.value()) else {
            continue;
        };
        out.push(' ');
        out.push_str(qualified);
        out.push_str("=\"");
        escape(a.value(), true, out);
        out.push('"');
    }
    let children: Vec<Node> = n
        .children()
        .filter(|c| c.is_text() || kept_element(*c))
        .collect();
    if children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for c in children {
        if c.is_text() {
            escape(c.text().unwrap_or(""), false, out);
        } else {
            write_element(c, out, false);
        }
    }
    out.push_str("</");
    out.push_str(name);
    out.push('>');
}

/// The name to write an attribute under, or `None` to drop it. `href` (plain
/// or `xlink:`) only to a fragment of this document; any `url(...)` only to a
/// fragment too.
fn kept_attribute(ns: Option<&str>, name: &str, value: &str) -> Option<&'static str> {
    let qualified = match ns {
        None => ATTRIBUTES.iter().find(|a| **a == name).copied()?,
        Some(XLINK_NS) if name == "href" => "xlink:href",
        Some(XML_NS) if name == "space" => "xml:space",
        _ => return None,
    };
    if qualified.ends_with("href") && !value.trim_start().starts_with('#') {
        return None;
    }
    if !urls_are_local(value) {
        return None;
    }
    Some(qualified)
}

fn uses_xlink(root: Node) -> bool {
    root.descendants().filter(|d| kept_element(*d)).any(|d| {
        d.attributes().any(|a| {
            a.namespace() == Some(XLINK_NS)
                && kept_attribute(a.namespace(), a.name(), a.value()).is_some()
        })
    })
}

/// Every `url(` in `value` is followed (after optional spaces and a quote) by
/// `#`. Compared case-insensitively, since CSS functions are.
fn urls_are_local(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.match_indices("url(").all(|(i, m)| {
        lower[i + m.len()..]
            .trim_start()
            .trim_start_matches(['"', '\''])
            .starts_with('#')
    })
}

fn escape(s: &str, attribute: bool, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}
