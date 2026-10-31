//! Syntax kinds: every token and node the tree can contain.
//!
//! Node kinds mirror `org-element`'s element types one to one (ADR-112 D7), so
//! the Emacs differential compares kinds through a flat name table rather than
//! a mapping layer. Token kinds are deliberately coarse in O1: inline content
//! is a single `TEXT` token per line until O2 builds the inline tree.

/// Every kind of token or node in a `mae-org` tree.
///
/// The numeric values are PINNED (see `numeric_values_are_pinned`): a JSON
/// or WASM consumer may persist them. Tokens are numbered from 0 and nodes
/// from 1000, so a later phase appends new kinds to either range without
/// renumbering. `#[non_exhaustive]`: matches outside this crate need a
/// wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
#[allow(non_camel_case_types)]
#[non_exhaustive]
pub enum SyntaxKind {
    // ---- tokens -------------------------------------------------------
    /// A leading U+FEFF byte-order mark. Trivia: it never shifts column 0.
    BOM = 0,
    /// A line ending, per the document's convention (`LineEnding`): `\n`,
    /// `\r\n` or `\r`. A CR that is not part of one is content.
    NEWLINE = 1,
    /// Spaces and tabs inside a structural line.
    WHITESPACE = 2,
    /// A whole blank line (only spaces/tabs), including its line ending.
    BLANK_LINE = 3,
    /// Unstructured text: paragraph lines, titles until O2, values.
    TEXT = 4,
    /// Text the parser could not place; never produced on well-formed input
    /// and asserted absent by the corpus tests.
    UNPARSED = 5,
    /// Contents left unparsed because the nesting cap was reached.
    OPAQUE = 6,
    /// The raw contents of a lesser block (src, example, export, comment, verse).
    BLOCK_CONTENT = 7,
    /// The `*`s opening a headline.
    STARS = 8,
    /// A TODO keyword from the active sequences.
    TODO_KEYWORD = 9,
    /// A `[#X]` priority cookie.
    PRIORITY = 10,
    /// The `COMMENT` marker of a commented headline.
    COMMENT_MARKER = 11,
    /// A headline's title text.
    TITLE = 12,
    /// A headline's trailing `:tag:list:`.
    TAGS = 13,
    /// `#+` opening a keyword, block line or affiliated keyword.
    HASH_PLUS = 14,
    /// A keyword or affiliated keyword name, without `#+` and `:`.
    KEY = 15,
    /// A single `:`.
    COLON = 16,
    /// A keyword's or property's value.
    VALUE = 17,
    /// `begin_` / `end_` (any case) in a block delimiter line.
    BLOCK_MARKER = 18,
    /// The `X` of `#+begin_X`, or a dynamic block's name.
    BLOCK_NAME = 19,
    /// Everything after the name on a block's opening line.
    PARAMETERS = 20,
    /// A drawer's or property's name between colons.
    NAME = 21,
    /// A plain-list bullet.
    BULLET = 22,
    /// An item's `[@N]` counter.
    COUNTER = 23,
    /// An item's `[ ]` / `[X]` / `[-]` checkbox.
    CHECKBOX = 24,
    /// A descriptive item's tag, including its `::`.
    ITEM_TAG = 25,
    /// A footnote definition's `[fn:LABEL]`.
    FOOTNOTE_LABEL = 26,
    /// `SCHEDULED:` / `DEADLINE:` / `CLOSED:` on a planning line.
    PLANNING_KEYWORD = 27,

    // ---- nodes --------------------------------------------------------
    DOCUMENT = 1000,
    SECTION = 1001,
    HEADLINE = 1002,
    PLANNING = 1003,
    PROPERTY_DRAWER = 1004,
    NODE_PROPERTY = 1005,
    DRAWER = 1006,
    CENTER_BLOCK = 1007,
    QUOTE_BLOCK = 1008,
    SPECIAL_BLOCK = 1009,
    SRC_BLOCK = 1010,
    EXAMPLE_BLOCK = 1011,
    EXPORT_BLOCK = 1012,
    COMMENT_BLOCK = 1013,
    VERSE_BLOCK = 1014,
    DYNAMIC_BLOCK = 1015,
    /// The opening line of a block or drawer.
    BLOCK_BEGIN = 1016,
    /// The closing line of a block or drawer.
    BLOCK_END = 1017,
    KEYWORD = 1018,
    /// An affiliated keyword (`#+NAME:`, `#+CAPTION:`, `#+ATTR_*:` …), placed
    /// inside the element it belongs to, as `org-element` represents it.
    AFFILIATED_KEYWORD = 1019,
    BABEL_CALL = 1020,
    COMMENT = 1021,
    FIXED_WIDTH = 1022,
    HORIZONTAL_RULE = 1023,
    FOOTNOTE_DEFINITION = 1024,
    PARAGRAPH = 1025,
    PLAIN_LIST = 1026,
    ITEM = 1027,
    TABLE = 1028,
    TABLE_ROW = 1029,
    CLOCK = 1030,
    DIARY_SEXP = 1031,
    LATEX_ENVIRONMENT = 1032,
}

use SyntaxKind::*;

impl SyntaxKind {
    /// Inverse of `as u16`. `None` for a value no kind has.
    pub fn from_raw(raw: u16) -> Option<SyntaxKind> {
        ALL.binary_search_by_key(&raw, |k| *k as u16)
            .ok()
            .map(|i| ALL[i])
    }

    /// True for node kinds, false for token kinds.
    pub fn is_node(self) -> bool {
        self as u16 >= FIRST_NODE
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

/// The first node kind's value; tokens are below it.
const FIRST_NODE: u16 = 1000;

/// Every kind, in ascending value order.
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
        for &k in ALL {
            assert_eq!(SyntaxKind::from_raw(k as u16), Some(k));
        }
        assert!(
            ALL.windows(2).all(|w| (w[0] as u16) < (w[1] as u16)),
            "ALL must be sorted"
        );
        assert_eq!(SyntaxKind::from_raw(999), None);
        assert_eq!(SyntaxKind::from_raw(u16::MAX), None);
    }

    /// Changing any of these numbers breaks persisted data; add new kinds
    /// with new numbers instead.
    #[test]
    fn numeric_values_are_pinned() {
        let got: Vec<String> = ALL.iter().map(|k| format!("{k:?}={}", *k as u16)).collect();
        let want = "BOM=0 NEWLINE=1 WHITESPACE=2 BLANK_LINE=3 TEXT=4 UNPARSED=5 OPAQUE=6 BLOCK_CONTENT=7 \
            STARS=8 TODO_KEYWORD=9 PRIORITY=10 COMMENT_MARKER=11 TITLE=12 TAGS=13 HASH_PLUS=14 KEY=15 \
            COLON=16 VALUE=17 BLOCK_MARKER=18 BLOCK_NAME=19 PARAMETERS=20 NAME=21 BULLET=22 COUNTER=23 \
            CHECKBOX=24 ITEM_TAG=25 FOOTNOTE_LABEL=26 PLANNING_KEYWORD=27 DOCUMENT=1000 SECTION=1001 \
            HEADLINE=1002 PLANNING=1003 PROPERTY_DRAWER=1004 NODE_PROPERTY=1005 DRAWER=1006 \
            CENTER_BLOCK=1007 QUOTE_BLOCK=1008 SPECIAL_BLOCK=1009 SRC_BLOCK=1010 EXAMPLE_BLOCK=1011 \
            EXPORT_BLOCK=1012 COMMENT_BLOCK=1013 VERSE_BLOCK=1014 DYNAMIC_BLOCK=1015 BLOCK_BEGIN=1016 \
            BLOCK_END=1017 KEYWORD=1018 AFFILIATED_KEYWORD=1019 BABEL_CALL=1020 COMMENT=1021 \
            FIXED_WIDTH=1022 HORIZONTAL_RULE=1023 FOOTNOTE_DEFINITION=1024 PARAGRAPH=1025 \
            PLAIN_LIST=1026 ITEM=1027 TABLE=1028 TABLE_ROW=1029 CLOCK=1030 DIARY_SEXP=1031 \
            LATEX_ENVIRONMENT=1032";
        assert_eq!(
            got.join(" "),
            want.split_whitespace().collect::<Vec<_>>().join(" ")
        );
    }
}
