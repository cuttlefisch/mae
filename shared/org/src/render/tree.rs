//! The host-neutral element tree every output is serialised from (ADR-112
//! D3): HTML, hast JSON and plain text are three readings of ONE tree, built
//! by one builder, so they cannot disagree about what is shown.
//!
//! Tags and attribute names are enums, not strings: a renderer can emit only
//! what [`Tag`] and [`Attr`] list. That is the allowlist, by construction
//! (`tests/render_safety.rs` checks every output against it as well).

/// An element's tag. The complete set the renderer can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Tag {
    A,
    Blockquote,
    Br,
    Caption,
    Code,
    Dd,
    Del,
    Div,
    Dl,
    Dt,
    Em,
    Figcaption,
    Figure,
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Hr,
    Img,
    Li,
    Ol,
    P,
    Pre,
    Section,
    Span,
    Strong,
    Sub,
    Sup,
    Table,
    Tbody,
    Td,
    Th,
    Thead,
    Time,
    Tr,
    Ul,
}

/// Every tag, in `Ord` order.
pub const TAGS: &[Tag] = &[
    Tag::A,
    Tag::Blockquote,
    Tag::Br,
    Tag::Caption,
    Tag::Code,
    Tag::Dd,
    Tag::Del,
    Tag::Div,
    Tag::Dl,
    Tag::Dt,
    Tag::Em,
    Tag::Figcaption,
    Tag::Figure,
    Tag::H1,
    Tag::H2,
    Tag::H3,
    Tag::H4,
    Tag::H5,
    Tag::H6,
    Tag::Hr,
    Tag::Img,
    Tag::Li,
    Tag::Ol,
    Tag::P,
    Tag::Pre,
    Tag::Section,
    Tag::Span,
    Tag::Strong,
    Tag::Sub,
    Tag::Sup,
    Tag::Table,
    Tag::Tbody,
    Tag::Td,
    Tag::Th,
    Tag::Thead,
    Tag::Time,
    Tag::Tr,
    Tag::Ul,
];

impl Tag {
    /// The HTML tag name (lower case).
    pub fn as_str(self) -> &'static str {
        use Tag::*;
        match self {
            A => "a",
            Blockquote => "blockquote",
            Br => "br",
            Caption => "caption",
            Code => "code",
            Dd => "dd",
            Del => "del",
            Div => "div",
            Dl => "dl",
            Dt => "dt",
            Em => "em",
            Figcaption => "figcaption",
            Figure => "figure",
            H1 => "h1",
            H2 => "h2",
            H3 => "h3",
            H4 => "h4",
            H5 => "h5",
            H6 => "h6",
            Hr => "hr",
            Img => "img",
            Li => "li",
            Ol => "ol",
            P => "p",
            Pre => "pre",
            Section => "section",
            Span => "span",
            Strong => "strong",
            Sub => "sub",
            Sup => "sup",
            Table => "table",
            Tbody => "tbody",
            Td => "td",
            Th => "th",
            Thead => "thead",
            Time => "time",
            Tr => "tr",
            Ul => "ul",
        }
    }

    /// Void elements: no children, no end tag.
    pub fn is_void(self) -> bool {
        matches!(self, Tag::Br | Tag::Hr | Tag::Img)
    }

    /// Elements that start a block in [`super::text`]'s reading.
    pub(crate) fn is_block(self) -> bool {
        use Tag::*;
        matches!(
            self,
            Blockquote
                | Caption
                | Dd
                | Div
                | Dl
                | Dt
                | Figcaption
                | Figure
                | H1
                | H2
                | H3
                | H4
                | H5
                | H6
                | Hr
                | Li
                | Ol
                | P
                | Pre
                | Section
                | Table
                | Tbody
                | Thead
                | Tr
                | Ul
        )
    }

    /// `<h1>`…`<h6>` for a level, deeper levels as `<h6>`.
    pub(crate) fn heading(level: usize) -> Tag {
        match level {
            0 | 1 => Tag::H1,
            2 => Tag::H2,
            3 => Tag::H3,
            4 => Tag::H4,
            5 => Tag::H5,
            _ => Tag::H6,
        }
    }
}

/// An attribute name. The complete set the renderer can produce; which tags
/// carry which is [`ATTRIBUTES`], and what values each may hold is
/// documented on its variant (and checked by `tests/render_safety.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Attr {
    /// Space-separated classes from the fixed set [`CLASSES`], plus
    /// `org-level-N` (N decimal) and `language-L` (L a validated language
    /// name, see [`super::RenderOptions`]).
    Class,
    /// An anchor id: `[A-Za-z][A-Za-z0-9_.-]*`, built by the anchor encoder.
    Id,
    /// A link target: a resolver-supplied href, an `http:`/`https:`/
    /// `mailto:` URL, or `#ID` of an id in the same output.
    Href,
    /// An `http:`/`https:` image URL (only with images enabled).
    Src,
    /// An image's alternative text (any text).
    Alt,
    /// A list item's counter: decimal digits.
    Value,
    /// A header cell's scope: `col`.
    Scope,
    /// A KB link's `rel=`: `[A-Za-z0-9][A-Za-z0-9_.:-]*`, at most 64 bytes.
    DataOrgRel,
    /// A KB link's weight `w=`: a decimal number in 0–1.
    DataOrgW,
    /// A KB link's confidence `c=`: a decimal number in 0–1.
    DataOrgC,
    /// A special block's name: `[A-Za-z0-9][A-Za-z0-9_-]*`, at most 64 bytes.
    DataOrgBlock,
}

impl Attr {
    /// The HTML attribute name.
    pub fn as_str(self) -> &'static str {
        match self {
            Attr::Class => "class",
            Attr::Id => "id",
            Attr::Href => "href",
            Attr::Src => "src",
            Attr::Alt => "alt",
            Attr::Value => "value",
            Attr::Scope => "scope",
            Attr::DataOrgRel => "data-org-rel",
            Attr::DataOrgW => "data-org-w",
            Attr::DataOrgC => "data-org-c",
            Attr::DataOrgBlock => "data-org-block",
        }
    }

    /// The hast property name (`property-information`'s camel case).
    pub fn hast_name(self) -> &'static str {
        match self {
            Attr::Class => "className",
            Attr::DataOrgRel => "dataOrgRel",
            Attr::DataOrgW => "dataOrgW",
            Attr::DataOrgC => "dataOrgC",
            Attr::DataOrgBlock => "dataOrgBlock",
            other => other.as_str(),
        }
    }
}

/// Which attributes each tag may carry (`*` lines apply to every tag).
pub const ATTRIBUTES: &[(Option<Tag>, Attr)] = &[
    (None, Attr::Class),
    (None, Attr::Id),
    (Some(Tag::A), Attr::Href),
    (Some(Tag::A), Attr::DataOrgRel),
    (Some(Tag::A), Attr::DataOrgW),
    (Some(Tag::A), Attr::DataOrgC),
    (Some(Tag::Img), Attr::Src),
    (Some(Tag::Img), Attr::Alt),
    (Some(Tag::Li), Attr::Value),
    (Some(Tag::Th), Attr::Scope),
    (Some(Tag::Div), Attr::DataOrgBlock),
];

/// The fixed classes (besides `org-level-N` and `language-L`).
pub const CLASSES: &[&str] = &[
    "org-archived",
    "org-center",
    "org-checkbox",
    "org-citation",
    "org-cookie",
    "org-done",
    "org-dynamic-block",
    "org-example",
    "org-figure",
    "org-fixed-width",
    "org-footnote",
    "org-footnote-back",
    "org-footnote-ref",
    "org-footnotes",
    "org-inline-src",
    "org-latex",
    "org-latex-environment",
    "org-link",
    "org-opaque",
    "org-priority",
    "org-properties",
    "org-radio",
    "org-radio-target",
    "org-section",
    "org-special-block",
    "org-src",
    "org-table-el",
    "org-tag",
    "org-tags",
    "org-target",
    "org-timestamp",
    "org-todo",
    "org-transclusion",
    "org-underline",
    "org-unresolved",
    "org-verbatim",
    "org-verse",
];

/// A node of the element tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Node {
    Element(Element),
    /// Text, as it is shown: entities decoded, line endings `\n`, no C0
    /// control character but tab and newline.
    Text(String),
}

/// An element: a tag, its attributes in a fixed order, and its children.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Element {
    pub tag: Tag,
    pub attrs: Vec<(Attr, String)>,
    pub children: Vec<Node>,
}

impl Element {
    pub(crate) fn new(tag: Tag) -> Element {
        Element {
            tag,
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }

    pub(crate) fn class(self, class: &str) -> Element {
        self.attr(Attr::Class, class)
    }

    pub(crate) fn attr(mut self, attr: Attr, value: impl Into<String>) -> Element {
        let value = value.into();
        match self.attrs.iter_mut().find(|(a, _)| *a == attr) {
            Some((_, v)) if attr == Attr::Class => {
                v.push(' ');
                v.push_str(&value);
            }
            Some((_, v)) => *v = value,
            None => self.attrs.push((attr, value)),
        }
        self
    }

    pub(crate) fn with(mut self, children: Vec<Node>) -> Element {
        for c in children {
            push(&mut self.children, c);
        }
        self
    }

    /// The value of `attr`, if set.
    pub fn get(&self, attr: Attr) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(a, _)| *a == attr)
            .map(|(_, v)| v.as_str())
    }
}

impl From<Element> for Node {
    fn from(e: Element) -> Node {
        Node::Element(e)
    }
}

/// Appends `node`, merging adjacent text (an HTML parser merges it too, so
/// the tree and its HTML parse back alike) and dropping empty text.
pub(crate) fn push(out: &mut Vec<Node>, node: Node) {
    match node {
        Node::Text(t) if t.is_empty() => {}
        Node::Text(t) => match out.last_mut() {
            Some(Node::Text(prev)) => prev.push_str(&t),
            _ => out.push(Node::Text(t)),
        },
        e => out.push(e),
    }
}

/// Text as the tree holds it: CRLF and CR read as `\n` (an HTML parser
/// normalises them, so the tree must already), and every other C0 control
/// character but tab, and DEL, as U+FFFD (an HTML parser drops or replaces
/// NUL, and none of them is printable).
pub(crate) fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\t' | '\n' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
    out
}

/// A text node of `s`, cleaned.
pub(crate) fn text(s: &str) -> Node {
    Node::Text(clean_text(s))
}
