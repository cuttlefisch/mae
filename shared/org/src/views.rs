//! Typed views over the tree. Each is a thin wrapper over a node; everything
//! it returns is read from the node's tokens, so a view can never disagree
//! with the text.

use crate::kind::SyntaxKind::{self, *};
use crate::kind::{SyntaxNode, SyntaxToken};

fn token(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == kind)
}

fn child(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
    node.children().find(|c| c.kind() == kind)
}

macro_rules! view {
    ($(#[$doc:meta])* $name:ident, $($kind:pat_param)|+) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(SyntaxNode);

        impl $name {
            /// The view of `node`, if it is of this kind.
            pub fn cast(node: SyntaxNode) -> Option<Self> {
                matches!(node.kind(), $($kind)|+).then(|| Self(node))
            }

            /// The underlying node.
            pub fn syntax(&self) -> &SyntaxNode {
                &self.0
            }
        }
    };
}

view!(
    /// A headline and its subtree.
    Headline,
    HEADLINE
);
view!(
    /// A `:PROPERTIES:` drawer in a position Org reads properties from.
    PropertyDrawer,
    PROPERTY_DRAWER
);
view!(
    /// Any `#+begin_X`…`#+end_X` block, or a dynamic block.
    Block,
    CENTER_BLOCK | QUOTE_BLOCK | SPECIAL_BLOCK | SRC_BLOCK | EXAMPLE_BLOCK | EXPORT_BLOCK | COMMENT_BLOCK | VERSE_BLOCK | DYNAMIC_BLOCK
);
view!(
    /// A `#+KEY: value` keyword element.
    Keyword,
    KEYWORD
);

impl Headline {
    /// The number of stars.
    pub fn level(&self) -> usize {
        token(&self.0, STARS).map_or(0, |t| t.text().len())
    }

    /// The TODO keyword, if the headline has one from the active sequences.
    pub fn todo(&self) -> Option<String> {
        token(&self.0, TODO_KEYWORD).map(|t| t.text().to_string())
    }

    /// The priority character of a `[#X]` cookie.
    pub fn priority(&self) -> Option<char> {
        token(&self.0, PRIORITY).and_then(|t| t.text().chars().nth(2))
    }

    /// Whether the headline is marked `COMMENT`.
    pub fn is_commented(&self) -> bool {
        token(&self.0, COMMENT_MARKER).is_some()
    }

    /// The title as written (inline markup is not interpreted until O2).
    pub fn title(&self) -> String {
        token(&self.0, TITLE).map_or_else(String::new, |t| t.text().to_string())
    }

    /// The tags, in order, as written.
    pub fn tags(&self) -> Vec<String> {
        token(&self.0, TAGS).map_or_else(Vec::new, |t| {
            t.text()
                .split(':')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
    }

    /// The section directly under the headline (before any sub-headline).
    pub fn section(&self) -> Option<SyntaxNode> {
        child(&self.0, SECTION)
    }

    /// The property drawer Org reads this headline's properties from.
    pub fn property_drawer(&self) -> Option<PropertyDrawer> {
        self.section()
            .and_then(|s| child(&s, PROPERTY_DRAWER))
            .and_then(PropertyDrawer::cast)
    }

    /// Direct sub-headlines.
    pub fn children(&self) -> impl Iterator<Item = Headline> {
        self.0.children().filter_map(Headline::cast)
    }
}

/// One `:KEY: value` line of a property drawer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// The key in canonical case (upper): Org property keys are
    /// case-insensitive. An appending key keeps its `+` (`MULTI+`).
    pub key: String,
    /// The value, trimmed.
    pub value: String,
    /// The `NODE_PROPERTY` node; the key as written is its `NAME` token.
    pub node: SyntaxNode,
}

impl Property {
    /// The key exactly as written in the file.
    pub fn key_as_written(&self) -> String {
        token(&self.node, NAME).map_or_else(String::new, |t| t.text().to_string())
    }

    /// Whether the key ends in `+` (`:KEY+:`, which appends in Org).
    pub fn is_append(&self) -> bool {
        self.key.ends_with('+') && self.key.len() > 1
    }
}

impl PropertyDrawer {
    /// The properties, in file order.
    pub fn properties(&self) -> Vec<Property> {
        self.0
            .children()
            .filter(|n| n.kind() == NODE_PROPERTY)
            .filter_map(|node| {
                let key = token(&node, NAME)?.text().to_uppercase();
                let value = token(&node, VALUE).map_or_else(String::new, |t| t.text().to_string());
                Some(Property { key, value, node })
            })
            .collect()
    }

    /// The value of `key` (any case). The last occurrence wins, as in Org.
    pub fn get(&self, key: &str) -> Option<String> {
        let want = key.to_uppercase();
        self.properties()
            .into_iter()
            .rev()
            .find(|p| p.key == want)
            .map(|p| p.value)
    }
}

impl Block {
    /// The block's kind (`SRC_BLOCK`, `SPECIAL_BLOCK`, …).
    pub fn kind(&self) -> SyntaxKind {
        self.0.kind()
    }

    fn begin_line(&self) -> Option<SyntaxNode> {
        child(&self.0, BLOCK_BEGIN)
    }

    /// The name as written: `src` in `#+begin_src`, `foo` in `#+BEGIN: foo`.
    pub fn name(&self) -> String {
        self.begin_line()
            .and_then(|l| token(&l, BLOCK_NAME))
            .map_or_else(String::new, |t| t.text().to_string())
    }

    /// Everything after the name on the opening line, trimmed; `None` if empty.
    pub fn parameters(&self) -> Option<String> {
        self.begin_line()
            .and_then(|l| token(&l, PARAMETERS))
            .map(|t| t.text().to_string())
    }

    /// The raw contents of a lesser block (src, example, export, comment,
    /// verse). `None` for blocks whose contents are elements, or empty ones.
    pub fn raw_contents(&self) -> Option<String> {
        token(&self.0, BLOCK_CONTENT).map(|t| t.text().to_string())
    }
}

impl Keyword {
    /// The key as written, without `#+` and `:`.
    pub fn key(&self) -> String {
        token(&self.0, KEY).map_or_else(String::new, |t| t.text().to_string())
    }

    /// The value, trimmed.
    pub fn value(&self) -> String {
        token(&self.0, VALUE).map_or_else(String::new, |t| t.text().to_string())
    }
}
