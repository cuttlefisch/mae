//! Syntax kinds: every token and node the tree can contain.
//!
//! Node kinds mirror `org-element`'s element types one to one (ADR-112 D7), so
//! the Emacs differential compares kinds through a flat name table rather than
//! a mapping layer. Token kinds are deliberately coarse in O1: inline content
//! is a single `TEXT` token per line until O2 builds the inline tree.

/// Every kind of token or node in a `mae-org` tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
#[allow(non_camel_case_types)]
pub enum SyntaxKind {
    // ---- tokens -------------------------------------------------------
    /// A leading U+FEFF byte-order mark. Trivia: it never shifts column 0.
    BOM = 0,
    /// A line ending: `\n` or `\r\n`. A lone `\r` is content, not a line end.
    NEWLINE,
    /// Spaces and tabs inside a structural line.
    WHITESPACE,
    /// A whole blank line (only spaces/tabs), including its line ending.
    BLANK_LINE,
    /// Unstructured text: paragraph lines, titles until O2, values.
    TEXT,
    /// Text the parser could not place; never produced on well-formed input
    /// and asserted absent by the corpus tests.
    UNPARSED,
    /// Contents left unparsed because the nesting cap was reached.
    OPAQUE,
    /// The raw contents of a lesser block (src, example, export, comment, verse).
    BLOCK_CONTENT,
    /// The `*`s opening a headline.
    STARS,
    /// A TODO keyword from the active sequences.
    TODO_KEYWORD,
    /// A `[#X]` priority cookie.
    PRIORITY,
    /// The `COMMENT` marker of a commented headline.
    COMMENT_MARKER,
    /// A headline's title text.
    TITLE,
    /// A headline's trailing `:tag:list:`.
    TAGS,
    /// `#+` opening a keyword, block line or affiliated keyword.
    HASH_PLUS,
    /// A keyword or affiliated keyword name, without `#+` and `:`.
    KEY,
    /// A single `:`.
    COLON,
    /// A keyword's or property's value.
    VALUE,
    /// `begin_` / `end_` (any case) in a block delimiter line.
    BLOCK_MARKER,
    /// The `X` of `#+begin_X`, or a dynamic block's name.
    BLOCK_NAME,
    /// Everything after the name on a block's opening line.
    PARAMETERS,
    /// A drawer's or property's name between colons.
    NAME,
    /// A plain-list bullet.
    BULLET,
    /// An item's `[@N]` counter.
    COUNTER,
    /// An item's `[ ]` / `[X]` / `[-]` checkbox.
    CHECKBOX,
    /// A descriptive item's tag, including its `::`.
    ITEM_TAG,
    /// A footnote definition's `[fn:LABEL]`.
    FOOTNOTE_LABEL,
    /// `SCHEDULED:` / `DEADLINE:` / `CLOSED:` on a planning line.
    PLANNING_KEYWORD,

    // ---- nodes --------------------------------------------------------
    DOCUMENT,
    SECTION,
    HEADLINE,
    PLANNING,
    PROPERTY_DRAWER,
    NODE_PROPERTY,
    DRAWER,
    CENTER_BLOCK,
    QUOTE_BLOCK,
    SPECIAL_BLOCK,
    SRC_BLOCK,
    EXAMPLE_BLOCK,
    EXPORT_BLOCK,
    COMMENT_BLOCK,
    VERSE_BLOCK,
    DYNAMIC_BLOCK,
    /// The opening line of a block or drawer.
    BLOCK_BEGIN,
    /// The closing line of a block or drawer.
    BLOCK_END,
    KEYWORD,
    /// An affiliated keyword (`#+NAME:`, `#+CAPTION:`, `#+ATTR_*:` …), placed
    /// inside the element it belongs to, as `org-element` represents it.
    AFFILIATED_KEYWORD,
    BABEL_CALL,
    COMMENT,
    FIXED_WIDTH,
    HORIZONTAL_RULE,
    FOOTNOTE_DEFINITION,
    PARAGRAPH,
    PLAIN_LIST,
    ITEM,
    TABLE,
    TABLE_ROW,
    CLOCK,
    DIARY_SEXP,
    LATEX_ENVIRONMENT,
}

use SyntaxKind::*;

const LAST: u16 = LATEX_ENVIRONMENT as u16;

impl SyntaxKind {
    /// Inverse of `as u16`. `None` for a value no kind has.
    pub fn from_raw(raw: u16) -> Option<SyntaxKind> {
        ALL.get(raw as usize).copied().filter(|_| raw <= LAST)
    }

    /// True for node kinds, false for token kinds.
    pub fn is_node(self) -> bool {
        self as u16 >= DOCUMENT as u16
    }

    /// The `org-element` type name this node corresponds to, if any.
    ///
    /// Line nodes (`BLOCK_BEGIN`, `BLOCK_END`) and `AFFILIATED_KEYWORD` are
    /// parts of an element in `org-element`, not elements, so they have none.
    pub fn org_element_name(self) -> Option<&'static str> {
        Some(match self {
            DOCUMENT => "org-data",
            SECTION => "section",
            HEADLINE => "headline",
            PLANNING => "planning",
            PROPERTY_DRAWER => "property-drawer",
            NODE_PROPERTY => "node-property",
            DRAWER => "drawer",
            CENTER_BLOCK => "center-block",
            QUOTE_BLOCK => "quote-block",
            SPECIAL_BLOCK => "special-block",
            SRC_BLOCK => "src-block",
            EXAMPLE_BLOCK => "example-block",
            EXPORT_BLOCK => "export-block",
            COMMENT_BLOCK => "comment-block",
            VERSE_BLOCK => "verse-block",
            DYNAMIC_BLOCK => "dynamic-block",
            KEYWORD => "keyword",
            BABEL_CALL => "babel-call",
            COMMENT => "comment",
            FIXED_WIDTH => "fixed-width",
            HORIZONTAL_RULE => "horizontal-rule",
            FOOTNOTE_DEFINITION => "footnote-definition",
            PARAGRAPH => "paragraph",
            PLAIN_LIST => "plain-list",
            ITEM => "item",
            TABLE => "table",
            TABLE_ROW => "table-row",
            CLOCK => "clock",
            DIARY_SEXP => "diary-sexp",
            LATEX_ENVIRONMENT => "latex-environment",
            _ => return None,
        })
    }
}

/// Every kind, indexed by its `u16` value.
const ALL: &[SyntaxKind] = &[
    BOM,
    NEWLINE,
    WHITESPACE,
    BLANK_LINE,
    TEXT,
    UNPARSED,
    OPAQUE,
    BLOCK_CONTENT,
    STARS,
    TODO_KEYWORD,
    PRIORITY,
    COMMENT_MARKER,
    TITLE,
    TAGS,
    HASH_PLUS,
    KEY,
    COLON,
    VALUE,
    BLOCK_MARKER,
    BLOCK_NAME,
    PARAMETERS,
    NAME,
    BULLET,
    COUNTER,
    CHECKBOX,
    ITEM_TAG,
    FOOTNOTE_LABEL,
    PLANNING_KEYWORD,
    DOCUMENT,
    SECTION,
    HEADLINE,
    PLANNING,
    PROPERTY_DRAWER,
    NODE_PROPERTY,
    DRAWER,
    CENTER_BLOCK,
    QUOTE_BLOCK,
    SPECIAL_BLOCK,
    SRC_BLOCK,
    EXAMPLE_BLOCK,
    EXPORT_BLOCK,
    COMMENT_BLOCK,
    VERSE_BLOCK,
    DYNAMIC_BLOCK,
    BLOCK_BEGIN,
    BLOCK_END,
    KEYWORD,
    AFFILIATED_KEYWORD,
    BABEL_CALL,
    COMMENT,
    FIXED_WIDTH,
    HORIZONTAL_RULE,
    FOOTNOTE_DEFINITION,
    PARAGRAPH,
    PLAIN_LIST,
    ITEM,
    TABLE,
    TABLE_ROW,
    CLOCK,
    DIARY_SEXP,
    LATEX_ENVIRONMENT,
];

/// The rowan language tag for org trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OrgLanguage {}

impl rowan::Language for OrgLanguage {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        // Only this crate builds trees, and it only writes valid kinds; an
        // unknown value can only come from a foreign green tree. Map it to
        // UNPARSED rather than panic (totality, ADR-112 D1).
        SyntaxKind::from_raw(raw.0).unwrap_or(UNPARSED)
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

/// A node of an org syntax tree.
pub type SyntaxNode = rowan::SyntaxNode<OrgLanguage>;
/// A token of an org syntax tree.
pub type SyntaxToken = rowan::SyntaxToken<OrgLanguage>;
/// A node or a token.
pub type SyntaxElement = rowan::SyntaxElement<OrgLanguage>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_values_round_trip_for_every_kind() {
        // A kind added to the enum but not to ALL would map to the wrong
        // variant; this walks every raw value up to the last one.
        for raw in 0..=LAST {
            let kind = SyntaxKind::from_raw(raw).expect("every raw value up to LAST is a kind");
            assert_eq!(kind as u16, raw);
        }
        assert_eq!(SyntaxKind::from_raw(LAST + 1), None);
        assert_eq!(ALL.len(), LAST as usize + 1);
    }
}
