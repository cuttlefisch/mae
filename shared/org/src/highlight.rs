//! Highlighting spans from the lossless tree, for an editor's decorations.
//!
//! [`highlights`] lists every node and every structural token of a parse as
//! a kind and a UTF-8 byte range, in document order. Plain text, spaces,
//! line endings and blank lines are left out: they are what an editor draws
//! by default, and leaving them out keeps the list proportional to markup,
//! not to text.
//!
//! [`sections`] cuts a parse into regions an editor can re-highlight on its
//! own: the text before the first headline, and each headline's own section
//! (after its line, up to its first child headline or its end). Each region
//! carries the [`FragmentContext`] its file gives it, so
//! [`section_highlights`] on that region's text, alone, gives exactly the
//! spans the whole parse gives there. When an edit changes what the rest of
//! the file depends on (a headline line, a radio target, a `#+TODO:` line,
//! or the line ending before the next headline), it says so
//! ([`NeedsFull`]) instead of answering: the caller then parses the whole
//! text again.

use crate::kind::{SyntaxKind, SyntaxKind::*, SyntaxNode};
use crate::views::{Headline, Keyword};
use crate::{parse_fragment, DiagnosticKind, Document, FragmentContext, ParseOptions};

/// One highlighted range: a node or a structural token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Highlight {
    /// What the range is.
    pub kind: SyntaxKind,
    /// UTF-8 byte offset where it starts, in the text the caller passed.
    pub start: u32,
    /// UTF-8 byte offset where it ends. As in `org-element`, this includes
    /// the node's post-blank: the spaces after an object (`*bold*  ` ends
    /// after the spaces) and the blank lines after an element.
    pub end: u32,
    /// Where the node's own text ends: [`end`](Self::end) less its
    /// post-blank, so `*bold*  ` ends after the closing `*`. An editor that
    /// styles a node, or decides whether the cursor is inside it, wants
    /// this end; the post-blank belongs to neither. Equal to `end` for a
    /// token and for a node with no post-blank.
    pub own_end: u32,
}

/// A region [`section_highlights`] can re-highlight alone.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Section {
    /// Where the region starts (after its headline's line ending; 0 for the
    /// text before the first headline).
    pub start: u32,
    /// Where it ends: its first child headline, the next headline, or the
    /// end of the text.
    pub end: u32,
    /// What the rest of the file tells a parse of the region.
    pub context: FragmentContext,
    /// A fingerprint of what the region declares that the REST of the file
    /// reads: its radio targets and its TODO keyword lines.
    pub declarations: u32,
    /// Whether a headline follows the region (then its text must keep
    /// ending with a line ending).
    pub followed_by_headline: bool,
}

impl Section {
    /// A region as [`sections`] described it, rebuilt by a caller that kept
    /// its fields (e.g. across a serialisation boundary).
    pub fn new(
        start: u32,
        end: u32,
        context: FragmentContext,
        declarations: u32,
        followed_by_headline: bool,
    ) -> Section {
        Section {
            start,
            end,
            context,
            declarations,
            followed_by_headline,
        }
    }
}

/// Why [`section_highlights`] cannot answer from the region alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NeedsFull {
    /// The region's text now holds a headline line.
    Headline,
    /// Its radio targets or TODO keyword lines changed.
    Declarations,
    /// It no longer ends with a line ending, so it would join the next
    /// headline's line.
    Boundary,
}

/// The answer of [`section_highlights`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionHighlights {
    /// The region's spans, offset by its start: as in a whole parse.
    Spans(Vec<Highlight>),
    /// The whole text must be parsed again.
    NeedsFull(NeedsFull),
}

fn is_plain(kind: SyntaxKind) -> bool {
    matches!(kind, BOM | NEWLINE | WHITESPACE | BLANK_LINE | TEXT)
}

/// `e`'s range as a highlight, moved by `shift` (added), unless it is plain:
/// the definition [`spans_of`] computes for a whole tree at once.
#[cfg(test)]
fn highlight(e: &crate::kind::SyntaxElement, shift: u32) -> Option<Highlight> {
    if is_plain(e.kind()) {
        return None;
    }
    let r = e.text_range();
    let end = u32::from(r.end()) + shift;
    let blank = match e {
        rowan::NodeOrToken::Node(n) => post_blank_len(e.kind(), n.green().children()),
        rowan::NodeOrToken::Token(_) => 0,
    };
    Some(Highlight {
        kind: e.kind(),
        start: u32::from(r.start()) + shift,
        end,
        own_end: end - blank,
    })
}

/// Whether a token of `token` kind at the end of a `node` kind node is
/// that node's post-blank: an object's trailing spaces, an element's
/// trailing blank lines.
fn is_post_blank(node: SyntaxKind, token: SyntaxKind) -> bool {
    if node.is_object() {
        token == WHITESPACE
    } else {
        token == BLANK_LINE
    }
}

/// The byte length of the post-blank a `kind` node ends with: the run of
/// [`is_post_blank`] tokens that are its last direct children.
fn post_blank_len(kind: SyntaxKind, children: rowan::Children<'_>) -> u32 {
    let raw = |r: rowan::SyntaxKind| <crate::OrgLanguage as rowan::Language>::kind_from_raw(r);
    children
        .rev()
        .map_while(|c| match c {
            rowan::NodeOrToken::Token(t) if is_post_blank(kind, raw(t.kind())) => {
                Some(u32::from(t.text_len()))
            }
            _ => None,
        })
        .sum()
}

/// Every node and structural token of `doc`, its root excepted, in
/// document order.
pub fn highlights(doc: &Document) -> Vec<Highlight> {
    spans_of(&doc.syntax(), 0)
}

/// Every element under `root` (itself excepted) as a highlight, unless it
/// is plain, moved by `shift`, in document order: what
/// `descendants_with_tokens().skip(1)` with [`highlight`] gives, read from
/// the green tree with a running offset rather than through a cursor
/// (an allocation) per node and token.
fn spans_of(root: &SyntaxNode, shift: u32) -> Vec<Highlight> {
    let kind = |raw: rowan::SyntaxKind| <crate::OrgLanguage as rowan::Language>::kind_from_raw(raw);
    let mut out = Vec::new();
    let mut at = u32::from(root.text_range().start()) + shift;
    let mut stack = vec![root.green().children()];
    while let Some(top) = stack.last_mut() {
        match top.next() {
            Some(rowan::NodeOrToken::Node(g)) => {
                let len = u32::from(g.text_len());
                let k = kind(g.kind());
                if !is_plain(k) {
                    out.push(Highlight {
                        kind: k,
                        start: at,
                        end: at + len,
                        own_end: at + len - post_blank_len(k, g.children()),
                    });
                }
                stack.push(g.children());
            }
            Some(rowan::NodeOrToken::Token(t)) => {
                let len = u32::from(t.text_len());
                let k = kind(t.kind());
                if !is_plain(k) {
                    out.push(Highlight {
                        kind: k,
                        start: at,
                        end: at + len,
                        own_end: at + len,
                    });
                }
                at += len;
            }
            None => {
                stack.pop();
            }
        }
    }
    out
}

/// Where `h`'s own section starts: after the line ending of its line.
/// `None` when its line ends the text.
fn body_start(h: &SyntaxNode) -> Option<u32> {
    h.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == NEWLINE)
        .map(|t| u32::from(t.text_range().end()))
}

/// Where `h`'s own section ends: its first child headline, else its end.
fn own_end(h: &SyntaxNode) -> u32 {
    h.children()
        .find(|c| c.kind() == HEADLINE)
        .map_or(h.text_range().end(), |c| c.text_range().start())
        .into()
}

/// The regions of `doc` [`section_highlights`] can re-highlight alone, in
/// document order. A headline whose line ends the text has none.
pub fn sections(doc: &Document) -> Vec<Section> {
    let root = doc.syntax();
    let len = u32::from(root.text_range().end());
    let first = root
        .children()
        .find(|c| c.kind() == HEADLINE)
        .map_or(len, |h| h.text_range().start().into());
    // The few nodes that declare something, found once: a walk per
    // section would be quadratic in the headlines.
    let decls: Vec<SyntaxNode> = declaration_nodes(doc);
    let mut out = vec![section(doc, &decls, None, (0, first), len)];
    for h in crate::views::headline_nodes(&root) {
        if let Some(start) = body_start(&h) {
            let headline = Headline::cast(h.clone());
            out.push(section(
                doc,
                &decls,
                headline.as_ref(),
                (start, own_end(&h)),
                len,
            ));
        }
    }
    out
}

fn section(
    doc: &Document,
    decls: &[SyntaxNode],
    headline: Option<&Headline>,
    (start, end): (u32, u32),
    len: u32,
) -> Section {
    let inside = decls.iter().filter(|n| within(n, start, end)).cloned();
    Section {
        start,
        end,
        context: FragmentContext::from_document(doc, headline),
        declarations: fingerprint(inside),
        followed_by_headline: end < len,
    }
}

fn within(n: &SyntaxNode, start: u32, end: u32) -> bool {
    let r = n.text_range();
    u32::from(r.start()) >= start && u32::from(r.end()) <= end
}

/// FNV-1a over the radio targets and TODO keyword lines among `nodes`.
fn fingerprint(nodes: impl Iterator<Item = SyntaxNode>) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for n in nodes.filter(declares) {
        for b in n.to_string().bytes().chain([0]) {
            h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
        }
    }
    h
}

/// The nodes of `doc` that [`declares`] holds, in document order. A TODO
/// keyword is an element, so objects are walked only when the document
/// has a radio target among them.
fn declaration_nodes(doc: &Document) -> Vec<SyntaxNode> {
    let root = doc.syntax();
    if doc.has_kind(RADIO_TARGET) {
        return root.descendants().filter(declares).collect();
    }
    let mut out = Vec::new();
    let mut walk = root.preorder();
    while let Some(ev) = walk.next() {
        match ev {
            rowan::WalkEvent::Enter(n) if n.kind().is_object() => walk.skip_subtree(),
            rowan::WalkEvent::Enter(n) if declares(&n) => out.push(n),
            _ => {}
        }
    }
    out
}

fn declares(n: &SyntaxNode) -> bool {
    match n.kind() {
        RADIO_TARGET => true,
        KEYWORD => Keyword::cast(n.clone()).is_some_and(|k| {
            let key = k.key();
            ["TODO", "SEQ_TODO", "TYP_TODO"]
                .iter()
                .any(|t| key.eq_ignore_ascii_case(t))
        }),
        _ => false,
    }
}

/// Whether `text`, starting at offset `start` of its file, can be followed
/// by a headline line: it ends with a line ending, or is empty, or (at the
/// start of the file only, where a byte-order mark is trivia) holds nothing
/// but one.
fn ends_line(text: &str, start: u32) -> bool {
    let rest = if start == 0 {
        text.trim_start_matches('\u{feff}')
    } else {
        text
    };
    rest.is_empty() || text.ends_with('\n') || text.ends_with('\r')
}

/// The spans of `section`'s region whose text is now `text`, as a whole
/// parse would give them, or why the whole text must be parsed again.
/// `options` must be the ones the file was parsed with.
pub fn section_highlights(
    text: &str,
    options: &ParseOptions,
    section: &Section,
) -> SectionHighlights {
    if section.followed_by_headline && !ends_line(text, section.start) {
        return SectionHighlights::NeedsFull(NeedsFull::Boundary);
    }
    let frag = parse_fragment(text, options, &section.context);
    let root = frag.syntax();
    let headline = frag
        .diagnostics()
        .iter()
        .any(|d| d.kind == DiagnosticKind::HeadlineOutsideFragment)
        || root.descendants().any(|n| n.kind() == HEADLINE);
    if headline {
        return SectionHighlights::NeedsFull(NeedsFull::Headline);
    }
    if fingerprint(root.descendants()) != section.declarations {
        return SectionHighlights::NeedsFull(NeedsFull::Declarations);
    }
    SectionHighlights::Spans(spans_of(&root, section.start))
}

#[cfg(test)]
mod span_walk_tests {
    use super::*;
    use proptest::prelude::*;

    fn by_cursor(root: &SyntaxNode, shift: u32) -> Vec<Highlight> {
        root.descendants_with_tokens()
            .skip(1)
            .filter_map(|e| highlight(&e, shift))
            .collect()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]

        /// The green walk gives every element the cursor walk does, with
        /// the same kind and range, in the same order, at any shift.
        #[test]
        fn the_span_walk_is_the_cursor_walk(
            text in "(\u{feff}|\\* |\\*\\* T |x|é|[ \t]|\n|\r\n|\\[\\[a\\]\\[\\*b\\*\\]\\]|=v= |\\| c \\| d \\|\n|:PROPERTIES:\n:ID: a\n:END:\n|#\\+begin_src\n|#\\+end_src\n|- i\n|<<t>>|\\[fn:1\\]){0,30}",
            shift in 0u32..1000,
        ) {
            let doc = crate::parse(&text, &Default::default());
            let root = doc.syntax();
            prop_assert_eq!(spans_of(&root, shift), by_cursor(&root, shift));
        }
    }

    /// The pruned walks find what full walks do, on every corpus file:
    /// every headline, and every declaring node.
    #[test]
    fn pruned_walks_find_what_full_walks_do_on_the_corpus() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let list = std::fs::read_to_string(dir.join("conformance/corpus.txt")).unwrap();
        let (mut heads, mut decls) = (0, 0);
        for rel in list
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let text = std::fs::read_to_string(dir.join("../..").join(rel)).unwrap();
            for text in [text.clone(), format!("#+TODO: A | B\n{text}")] {
                let doc = crate::parse(&text, &Default::default());
                let root = doc.syntax();
                let all: Vec<_> = root
                    .descendants()
                    .filter(|n| n.kind() == HEADLINE)
                    .collect();
                assert_eq!(crate::views::headline_nodes(&root), all, "{rel}");
                let d: Vec<_> = root.descendants().filter(declares).collect();
                assert_eq!(declaration_nodes(&doc), d, "{rel}");
                heads += all.len();
                decls += d.len();
            }
        }
        assert!(heads > 500 && decls > 100, "{heads} {decls}");
    }

    /// And on every file of the conformance corpus.
    #[test]
    fn the_span_walk_is_the_cursor_walk_on_the_corpus() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let list = std::fs::read_to_string(dir.join("conformance/corpus.txt")).unwrap();
        let mut n = 0;
        for rel in list
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let text = std::fs::read_to_string(dir.join("../..").join(rel)).unwrap();
            let doc = crate::parse(&text, &Default::default());
            assert_eq!(highlights(&doc), by_cursor(&doc.syntax(), 0), "{rel}");
            n += 1;
        }
        assert!(n > 100, "{n}");
    }
}
