//! Highlighting source blocks (ADR-119 D12; P-105 to P-110).
//!
//! The core has no grammar: it is free of I/O and WASM-capable, so the
//! caller hands it a [`Highlighter`] through [`crate::SiteOptions`]. The
//! derivation asks it about each source block's text, as mae-org's render
//! tree holds it, and puts the answer back into that tree as
//! `<span class="org-code-<role>">` elements, before the tree is serialised
//! (mae-org escapes their text) and sanitised like any content.
//!
//! A block stays plain (exactly the text mae-org rendered) when it names no
//! language, the highlighter does not know the language, it is larger than
//! [`MAX_HIGHLIGHT_BYTES`], or the highlighted text would not be
//! byte-identical to it. Each is recorded as a [`PlainBlock`] for the caller
//! to log (P-108); none is an error.

use std::collections::BTreeSet;

use mae_org::{Attr, Element, Node, Tag};

/// A palette colour role for code (`--code-<role>`; palette/src/tokens.ts,
/// group `code`). Text with no role takes the block's text colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CodeRole {
    Comment,
    String,
    Number,
    Constant,
    Keyword,
    Operator,
    Function,
    Type,
    Variable,
}

impl CodeRole {
    /// Every role, in the palette's order.
    pub const ALL: [CodeRole; 9] = [
        CodeRole::Comment,
        CodeRole::String,
        CodeRole::Number,
        CodeRole::Constant,
        CodeRole::Keyword,
        CodeRole::Operator,
        CodeRole::Function,
        CodeRole::Type,
        CodeRole::Variable,
    ];

    /// The class a token of this role wears, which base.css colours with
    /// `--code-<role>`.
    pub fn class(self) -> &'static str {
        match self {
            CodeRole::Comment => "org-code-comment",
            CodeRole::String => "org-code-string",
            CodeRole::Number => "org-code-number",
            CodeRole::Constant => "org-code-constant",
            CodeRole::Keyword => "org-code-keyword",
            CodeRole::Operator => "org-code-operator",
            CodeRole::Function => "org-code-function",
            CodeRole::Type => "org-code-type",
            CodeRole::Variable => "org-code-variable",
        }
    }
}

/// One highlighted token: a byte range of the block's text and its role.
/// Text between tokens is the block's text colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeSpan {
    pub start: usize,
    pub end: usize,
    pub role: CodeRole,
}

/// What the caller highlights with: the daemon's is tree-sitter over the
/// editor's grammars (`mae-syntax`).
pub trait Highlighter: Send + Sync {
    /// The tokens of `text` in `language` (the block's own name for it, as
    /// written after `#+begin_src`), in order and not overlapping; `None`
    /// when the language is not one it highlights. An answer that is not
    /// a partition of `text` in order makes the block plain, never wrong.
    fn highlight(&self, language: &str, text: &str) -> Option<Vec<CodeSpan>>;
}

/// The largest block, in bytes, that is highlighted; a larger one is plain
/// (ADR-119 D21: one block must not hold up the derivation of every KB the
/// daemon hosts).
///
/// 32 KiB, from a measurement of the daemon's grammars (2026-11-24, release
/// build, one core, on a loaded machine whose timings varied twofold):
/// highlighting is linear in the block, well-formed code takes 10 to 70 ms per
/// 64 KiB (PowerShell slowest), and text a grammar cannot parse, which keeps
/// it in error recovery throughout, about 100 ms. So one block at the cap
/// costs at most about 50 ms, a quarter of D20's 200 ms re-derivation target.
/// Source blocks written by hand are far smaller (the spike's largest was a
/// few KiB); a pasted log or data dump meets the cap, and reads fine plain.
/// Re-measure before changing it: a test pins the value.
pub const MAX_HIGHLIGHT_BYTES: usize = 32 * 1024;

/// Why a source block was left plain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Plain {
    /// `#+begin_src` with no language.
    Undeclared,
    /// A language the highlighter does not know (or a typo of one).
    Unknown,
    /// More than [`MAX_HIGHLIGHT_BYTES`].
    TooLarge,
    /// The highlighted text was not byte-identical to the block's.
    NotExact,
}

impl Plain {
    /// A word for logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Plain::Undeclared => "undeclared language",
            Plain::Unknown => "unknown language",
            Plain::TooLarge => "too large",
            Plain::NotExact => "not byte-exact",
        }
    }
}

/// A page with a source block left plain, for the caller to log (P-108).
/// One per page, language and reason, however many blocks share them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlainBlock {
    /// The page's file-level ID.
    pub page: String,
    /// The block's language, as written; `None` when it names none.
    pub language: Option<String>,
    pub reason: Plain,
}

/// Highlight every source block in `nodes` (a page's rendered body) with
/// `highlighter`, recording each block left plain under `page`.
pub(crate) fn highlight_blocks(
    nodes: &mut [Node],
    page: &str,
    highlighter: &dyn Highlighter,
    plain: &mut BTreeSet<PlainBlock>,
) {
    for n in nodes {
        let Node::Element(e) = n else { continue };
        match source_code(e) {
            Some(code) => {
                if let Err(reason) = highlight_code(code, highlighter) {
                    plain.insert(PlainBlock {
                        page: page.to_string(),
                        language: language_of(code).map(str::to_string),
                        reason,
                    });
                }
            }
            None => highlight_blocks(&mut e.children, page, highlighter, plain),
        }
    }
}

/// `e`'s `code` when `e` is a source block as mae-org renders one:
/// `<pre class="org-src"><code [class="language-L"]>TEXT</code></pre>`.
fn source_code(e: &mut Element) -> Option<&mut Element> {
    let is_src = e.tag == Tag::Pre
        && e.get(Attr::Class)
            .is_some_and(|c| c.split(' ').any(|c| c == "org-src"));
    match e.children.as_mut_slice() {
        [Node::Element(code)] if is_src && code.tag == Tag::Code => Some(code),
        _ => None,
    }
}

/// The language a block's `code` names (`language-L`).
fn language_of(code: &Element) -> Option<&str> {
    code.get(Attr::Class)?
        .split(' ')
        .find_map(|c| c.strip_prefix("language-"))
        .filter(|l| !l.is_empty())
}

/// Replace `code`'s text with its highlighted tokens; on `Err`, `code` is
/// unchanged.
fn highlight_code(code: &mut Element, highlighter: &dyn Highlighter) -> Result<(), Plain> {
    let text = match code.children.as_slice() {
        [] => return Ok(()),
        [Node::Text(t)] => t.clone(),
        // Not a shape mae-org writes for a source block: leave it alone.
        _ => return Ok(()),
    };
    let language = language_of(code).ok_or(Plain::Undeclared)?;
    if text.len() > MAX_HIGHLIGHT_BYTES {
        return Err(Plain::TooLarge);
    }
    let spans = highlighter
        .highlight(language, &text)
        .ok_or(Plain::Unknown)?;
    let mut template = code.clone();
    template.tag = Tag::Span;
    template.attrs.clear();
    template.children.clear();
    code.children = tokens(&text, &spans, &template).ok_or(Plain::NotExact)?;
    Ok(())
}

/// `text` as text and token spans; `None` unless their text, read in order,
/// is exactly `text` (P-107).
///
/// mae-org builds its tree elements itself (its constructor is crate-private:
/// its tags and attributes are its allow-list), so a token is `template`, an
/// empty `span` taken from the block's own `code`, given its class and text.
/// Its class is then admitted by [`crate::sanitize`] on `span` only.
pub(crate) fn tokens(text: &str, spans: &[CodeSpan], template: &Element) -> Option<Vec<Node>> {
    let mut out = Vec::new();
    let mut at = 0;
    for s in spans {
        let gap = text.get(at..s.start).unwrap_or_default();
        if !gap.is_empty() {
            out.push(Node::Text(gap.to_string()));
        }
        let token = text.get(s.start..s.end).unwrap_or_default();
        if !token.is_empty() {
            let mut span = template.clone();
            span.attrs.push((Attr::Class, s.role.class().to_string()));
            span.children.push(Node::Text(token.to_string()));
            out.push(Node::Element(span));
        }
        at = at.max(s.end);
    }
    let rest = text.get(at..).unwrap_or_default();
    if !rest.is_empty() {
        out.push(Node::Text(rest.to_string()));
    }
    // The byte-exact check: whatever the spans said, what a reader would
    // see and copy must be the author's text, or the block is plain.
    if plain_text(&out) != text {
        return None;
    }
    Some(out)
}

/// The text of a block's highlighted children, read in order.
fn plain_text(nodes: &[Node]) -> String {
    let mut out = String::new();
    for n in nodes {
        match n {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) => out.push_str(&plain_text(&e.children)),
            _ => {}
        }
    }
    out
}
