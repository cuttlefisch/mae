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
//!
//! Three shapes are refused whole, as undecodable, rather than trimmed: a
//! tree nested deeper than [`MAX_DEPTH`] (the walk below recurses, and a
//! worker's stack is small), a `<use>` chain deeper than [`MAX_USE_DEPTH`],
//! and a document whose `<use>`s expand to more than [`MAX_EXPANDED`]
//! elements (the `<use>` form of a billion-laughs bomb: each level of a chain
//! multiplies the next, so no cap on the number of `<use>` elements alone
//! bounds what a viewer has to build). Each is decided on the tree as it will
//! be written, so the check is idempotent too.

use std::collections::{HashMap, HashSet};

use roxmltree::{Document, Node, NodeId, ParsingOptions};

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// Bound on parsed nodes: a well-formed but enormous tree is refused rather
/// than walked. Far above any diagram a page carries.
const MAX_NODES: u32 = 200_000;

/// The deepest element nesting kept: libxml2's default (`xmlParserMaxDepth`),
/// which every browser-era SVG toolchain already lives within. The sanitiser
/// recurses once per level on a blocking worker's 2 MiB stack; an unbounded
/// depth (a 100,000-level `<g>` nest is a few hundred kilobytes) would
/// overflow it and abort the daemon.
pub const MAX_DEPTH: usize = 256;

/// The longest chain of `<use>` references: generators reference at depth one
/// or two (matplotlib's markers and glyphs, MathJax's glyphs, an Inkscape
/// clone of a clone); eight leaves room for hand-made chains while bounding
/// the recursion that follows them.
pub const MAX_USE_DEPTH: u32 = 8;

/// The most elements the document may expand to once every `<use>` is
/// instantiated (each `<use>` counting itself and a copy of its target): the
/// same bound [`MAX_NODES`] puts on a document written out literally, so a
/// `<use>` never makes a viewer build more than the parser would accept. A
/// 10,000-point matplotlib scatter, one `<use>` per marker, is ~20,000.
pub const MAX_EXPANDED: u64 = MAX_NODES as u64;

/// CSS functions, besides `url()`, that take a URL (as a string): any of them
/// in an attribute value drops the attribute.
const URL_FUNCTIONS: &[&str] = &["image-set(", "image(", "src("];

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

/// The sanitised document, or why it is refused: not an SVG ([`is_svg`]), or
/// one of the shapes the module docs name.
pub fn sanitize(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let not_svg = || "not a well-formed SVG".to_string();
    let text = std::str::from_utf8(bytes).map_err(|_| not_svg())?;
    let doc = Document::parse_with_options(text, options()).map_err(|_| not_svg())?;
    let root = doc.root_element();
    if !is_svg_root(root) {
        return Err(not_svg());
    }
    check_depth(root)?;
    check_use_expansion(root)?;
    let mut body = String::new();
    write_element(root, &mut body, true);
    Ok(body.into_bytes())
}

/// Refuse a kept tree nested deeper than [`MAX_DEPTH`]. Iterative, so it is
/// safe on any depth; everything after it may recurse.
fn check_depth(root: Node) -> Result<(), String> {
    let mut stack = vec![(root, 1usize)];
    while let Some((n, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(format!("elements nest deeper than {MAX_DEPTH}"));
        }
        stack.extend(
            n.children()
                .filter(|c| kept_element(*c))
                .map(|c| (c, depth + 1)),
        );
    }
    Ok(())
}

/// The kept elements by `id`, the first in document order winning, as a
/// viewer resolves a duplicate.
fn kept_ids<'a, 'i>(root: Node<'a, 'i>) -> HashMap<&'a str, Node<'a, 'i>> {
    let mut ids = HashMap::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        // Only an `id` that is written can be referenced in the output.
        if let Some(id) = n
            .attribute("id")
            .filter(|v| kept_attribute(None, "id", v).is_some())
        {
            ids.entry(id).or_insert(n);
        }
        // Reversed, so the stack pops children in document order.
        let mut kids: Vec<Node> = n.children().filter(|c| kept_element(*c)).collect();
        kids.reverse();
        stack.extend(kids);
    }
    ids
}

/// What a kept `<use>` instantiates: the target of each `href` it keeps.
fn use_targets<'a, 'i>(n: Node<'a, 'i>, ids: &HashMap<&'a str, Node<'a, 'i>>) -> Vec<Node<'a, 'i>> {
    n.attributes()
        .filter(|a| {
            kept_attribute(a.namespace(), a.name(), a.value()).is_some_and(|q| q.ends_with("href"))
        })
        .filter_map(|a| ids.get(a.value().trim().strip_prefix('#')?).copied())
        .collect()
}

/// Refuse a `<use>` chain deeper than [`MAX_USE_DEPTH`], a `<use>` that
/// instantiates its own ancestor, and an expansion past [`MAX_EXPANDED`].
fn check_use_expansion(root: Node) -> Result<(), String> {
    let ids = kept_ids(root);
    let mut walk = Expansion {
        ids: &ids,
        memo: HashMap::new(),
        open: HashSet::new(),
    };
    walk.of(root).map(|_| ())
}

struct Expansion<'m, 'a, 'i> {
    ids: &'m HashMap<&'a str, Node<'a, 'i>>,
    /// Per element: what its subtree expands to, and its deepest `<use>` chain.
    memo: HashMap<NodeId, (u64, u32)>,
    /// The elements being expanded: one reached again is a cycle.
    open: HashSet<NodeId>,
}

impl<'a, 'i> Expansion<'_, 'a, 'i> {
    fn of(&mut self, n: Node<'a, 'i>) -> Result<(u64, u32), String> {
        if let Some(&done) = self.memo.get(&n.id()) {
            return Ok(done);
        }
        if !self.open.insert(n.id()) {
            return Err("a <use> instantiates its own ancestor".to_string());
        }
        let (mut count, mut depth) = (1u64, 0u32);
        let refs = if n.tag_name().name() == "use" {
            use_targets(n, self.ids)
        } else {
            Vec::new()
        };
        let kids = n.children().filter(|c| kept_element(*c));
        for (child, hop) in kids.map(|c| (c, 0)).chain(refs.into_iter().map(|t| (t, 1))) {
            let (k, d) = self.of(child)?;
            count = count.saturating_add(k);
            depth = depth.max(d + hop);
            if depth > MAX_USE_DEPTH {
                return Err(format!(
                    "<use> references chain deeper than {MAX_USE_DEPTH}"
                ));
            }
            if count > MAX_EXPANDED {
                return Err(format!(
                    "its <use> references expand to over {MAX_EXPANDED} elements"
                ));
            }
        }
        self.open.remove(&n.id());
        self.memo.insert(n.id(), (count, depth));
        Ok((count, depth))
    }
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
    // A CSS escape (`\75rl(` is `url(`) hides a function from the checks
    // below, and every kept attribute but the identifiers is a CSS value in
    // the presentation-attribute sense. Nothing a generator writes needs one.
    if value.contains('\\') {
        return None;
    }
    if !urls_are_local(value) || has_url_function(value) {
        return None;
    }
    Some(qualified)
}

/// Whether `value` calls a CSS function that fetches a URL given as a string.
fn has_url_function(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    URL_FUNCTIONS.iter().any(|f| lower.contains(f))
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
