//! The plain-text reading of objects: how a title becomes a node title.
//!
//! The rule, pinned by `tests/semantics.rs`:
//! - text is kept as written;
//! - bold, italic, underline, strike-through, verbatim and code lose their
//!   delimiters and keep their contents (`*a*` → `a`, `=x=` → `x`);
//! - a link reads as its description; failing that, the description of the
//!   store form `[[T|D]]`; failing that, its path as Org displays it (the
//!   normalised path, scheme included: `[[id:x]]` → `id:x`). This is
//!   `org-link-display-format`, which org-roam uses for titles;
//! - every other object (timestamp, entity, macro, footnote reference,
//!   statistics cookie, LaTeX, …) is kept exactly as written;
//! - finally every run of spaces, tabs and line endings becomes one space,
//!   and the result is trimmed.

use crate::kind::SyntaxKind::{self, *};
use crate::kind::SyntaxNode;
use crate::link_grammar::classify_link;
use rowan::{NodeOrToken, WalkEvent};

/// Objects whose delimiters are dropped.
const STRIPPED: [SyntaxKind; 6] = [BOLD, ITALIC, UNDERLINE, STRIKE_THROUGH, VERBATIM, CODE];

/// The plain text of `node`'s objects, whitespace collapsed and trimmed.
pub(crate) fn plain_text(node: &SyntaxNode) -> String {
    collapse(&raw_text(node))
}

/// As [`plain_text`], without collapsing whitespace.
fn raw_text(node: &SyntaxNode) -> String {
    let mut out = String::new();
    let mut walk = node.preorder_with_tokens();
    while let Some(ev) = walk.next() {
        let WalkEvent::Enter(el) = ev else { continue };
        match el {
            NodeOrToken::Node(n) if n.kind() == LINK => {
                out.push_str(&link_text(&n));
                // The blanks Org counts after the link are text after it.
                if let Some(NodeOrToken::Token(t)) = n.last_child_or_token() {
                    if t.kind() == WHITESPACE {
                        out.push_str(t.text());
                    }
                }
                walk.skip_subtree();
            }
            NodeOrToken::Node(_) => {}
            NodeOrToken::Token(t) => {
                let parent = t.parent().map(|p| p.kind());
                let dropped = t.kind() == MARKUP && parent.is_some_and(|k| STRIPPED.contains(&k));
                if !dropped {
                    out.push_str(t.text());
                }
            }
        }
    }
    out
}

/// How a link reads: its description, the store form's, or its path.
pub(crate) fn link_text(link: &SyntaxNode) -> String {
    if let Some(d) = description(link) {
        return d;
    }
    let path = link_path_buffer(link).unwrap_or_default();
    let class = classify_link(&path);
    match class.description.filter(|d| !d.is_empty()) {
        Some(d) => d,
        None => collapse(&crate::link_grammar::display_path(&path)),
    }
}

/// The plain text of a link's own description (`[[path][DESCRIPTION]]`),
/// `None` if it has none or it is blank.
pub(crate) fn description(link: &SyntaxNode) -> Option<String> {
    let mut parts: Vec<_> = link.children_with_tokens().collect();
    if parts.last().is_some_and(|p| p.kind() == WHITESPACE) {
        parts.pop();
    }
    let mut out = String::new();
    for p in parts {
        match p {
            NodeOrToken::Token(t) if matches!(t.kind(), MARKUP | LINK_PATH) => {}
            NodeOrToken::Token(t) => out.push_str(t.text()),
            NodeOrToken::Node(n) => out.push_str(&raw_text(&n)),
        }
    }
    Some(collapse(&out)).filter(|d| !d.is_empty())
}

/// A link's path exactly as written in the source, across line breaks (a
/// path split over lines is several `LINK_PATH` tokens).
pub(crate) fn link_path(link: &SyntaxNode) -> Option<String> {
    let toks: Vec<_> = link
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .collect();
    let first = toks.iter().position(|t| t.kind() == LINK_PATH)?;
    let last = toks.iter().rposition(|t| t.kind() == LINK_PATH)?;
    Some(toks[first..=last].iter().map(|t| t.text()).collect())
}

/// A link's path as Emacs's buffer holds it (line endings as `\n`): what
/// `classify_link` reads, so a DOS or mac file classifies like a unix one.
pub(crate) fn link_path_buffer(link: &SyntaxNode) -> Option<String> {
    let path = crate::views::Link::cast(link.clone())?.path_text();
    (!path.is_empty()).then_some(path)
}

/// Runs of ASCII blanks and line endings become one space; trimmed.
pub(crate) fn collapse(s: &str) -> String {
    let parts: Vec<&str> = s
        .split([' ', '\t', '\n', '\r', '\x0c'])
        .filter(|p| !p.is_empty())
        .collect();
    parts.join(" ")
}
