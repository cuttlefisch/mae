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
    /// The region's own line terminators, which the FILE's convention
    /// ([`crate::LineEnding::detect`]) is computed from together with every other
    /// region's: an edit that changes them may change the convention.
    pub eol: EolProfile,
    /// The line terminator just before the region (its headline line's), or
    /// `None` at the start of the file. It belongs to no region, so it only
    /// changes with a full parse, and it pins part of the file's convention.
    pub eol_anchor: Option<crate::LineEnding>,
}

/// What [`crate::LineEnding::detect`] reads of a stretch of text: whether it holds
/// an LF, an LF without a CR before it, and a CR. A region starts just after a
/// line ending (or at the start of the file) and is followed by a headline or
/// the end, so an LF at its start is bare and no CRLF spans its edges: the
/// file's convention is a function of these facts, OR-ed over every region and
/// the headline lines between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct EolProfile {
    pub has_lf: bool,
    pub bare_lf: bool,
    pub has_cr: bool,
}

impl EolProfile {
    /// The profile of `region` (as the text of a [`Section`]).
    pub fn of(region: &str) -> EolProfile {
        let b = region.as_bytes();
        let mut p = EolProfile::default();
        for (i, &c) in b.iter().enumerate() {
            match c {
                b'\n' => {
                    p.has_lf = true;
                    if i == 0 || b[i - 1] != b'\r' {
                        p.bare_lf = true;
                    }
                }
                b'\r' => p.has_cr = true,
                _ => {}
            }
        }
        p
    }

    /// As three bits (LF 1, bare LF 2, CR 4), for a serialisation boundary.
    pub fn bits(self) -> u8 {
        u8::from(self.has_lf) | u8::from(self.bare_lf) << 1 | u8::from(self.has_cr) << 2
    }

    /// From [`Self::bits`]; `None` for a value no profile has.
    pub fn from_bits(b: u8) -> Option<EolProfile> {
        (b < 8).then_some(EolProfile {
            has_lf: b & 1 != 0,
            bare_lf: b & 2 != 0,
            has_cr: b & 4 != 0,
        })
    }
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
        eol: EolProfile,
        eol_anchor: Option<crate::LineEnding>,
    ) -> Section {
        Section {
            start,
            end,
            context,
            declarations,
            followed_by_headline,
            eol,
            eol_anchor,
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
    /// Its line terminators changed in a way the file's line-ending
    /// convention is computed from (an LF, a bare LF or a CR appeared or went),
    /// so the convention, a fact about the whole file, may have changed.
    LineEnding,
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
        eol: EolProfile::of(&slice(doc, start, end)),
        eol_anchor: anchor_before(doc, start),
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

fn slice(doc: &Document, start: u32, end: u32) -> String {
    doc.syntax()
        .text()
        .slice(rowan::TextRange::new(start.into(), end.into()))
        .to_string()
}

/// The terminator ending just before offset `start`: `None` at the start of
/// the file. Read byte by byte from the tokens, not by slicing the text: the
/// byte two before `start` may be inside a multi-byte character (`"* ñ\n"`),
/// where a string slice panics. Only an ASCII CR or LF matters, and no byte
/// of a multi-byte UTF-8 character is one.
fn anchor_before(doc: &Document, start: u32) -> Option<crate::LineEnding> {
    use crate::LineEnding::*;
    let byte = |back: u32| start.checked_sub(back).and_then(|at| byte_at(doc, at));
    match (byte(2), byte(1)) {
        (Some(b'\r'), Some(b'\n')) => Some(CrLf),
        (_, Some(b'\n')) => Some(Lf),
        (_, Some(b'\r')) => Some(Cr),
        _ => None,
    }
}

/// The byte at offset `at` of `doc`'s text, from the token that holds it.
fn byte_at(doc: &Document, at: u32) -> Option<u8> {
    let off = rowan::TextSize::from(at);
    let token = doc.syntax().token_at_offset(off).right_biased()?;
    let i = usize::from(off.checked_sub(token.text_range().start())?);
    token.text().as_bytes().get(i).copied()
}

/// Whether the region's new terminators can change the file's convention,
/// given its old ones and the terminator before it (see [`Section::eol_anchor`]):
/// - a bare LF before it: the file is unix whatever the region holds;
/// - a CRLF: the file has an LF and a CR, so only a bare LF appearing or
///   going in the region can change it (DOS vs unix);
/// - a CR: the file is mac (no LF anywhere), so only an LF appearing can;
/// - the start of the file: any change in the region's facts might.
fn line_ending_may_change(
    old: EolProfile,
    new: EolProfile,
    anchor: Option<crate::LineEnding>,
) -> bool {
    use crate::LineEnding::*;
    match anchor {
        Some(Lf) => false,
        Some(CrLf) => old.bare_lf != new.bare_lf,
        Some(Cr) => old.has_lf != new.has_lf,
        None => old != new,
    }
}

/// Whether `text`, starting at offset `start` of its file, can be followed
/// by a headline line: it ends with a line ending, or is empty, or (at the
/// start of the file only, where a byte-order mark is trivia) holds nothing
/// but one.
/// The terminator is the file's: a lone CR ends a line only in a mac file;
/// in a unix or DOS file it is content, and the next line would join it.
fn ends_line(text: &str, start: u32, eol: crate::LineEnding) -> bool {
    let rest = if start == 0 {
        text.trim_start_matches('\u{feff}')
    } else {
        text
    };
    rest.is_empty()
        || match eol {
            crate::LineEnding::Cr => text.ends_with('\r'),
            _ => text.ends_with('\n'),
        }
}

/// The spans of `section`'s region whose text is now `text`, as a whole
/// parse would give them, or why the whole text must be parsed again.
/// `options` must be the ones the file was parsed with.
pub fn section_highlights(
    text: &str,
    options: &ParseOptions,
    section: &Section,
) -> SectionHighlights {
    if section.followed_by_headline && !ends_line(text, section.start, section.context.line_ending)
    {
        return SectionHighlights::NeedsFull(NeedsFull::Boundary);
    }
    // If no region's profile changes without a full parse, the facts the
    // convention is computed from cannot change, so neither can it.
    if line_ending_may_change(section.eol, EolProfile::of(text), section.eol_anchor) {
        return SectionHighlights::NeedsFull(NeedsFull::LineEnding);
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
