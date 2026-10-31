//! Which parts of a document are rendered (ADR-112 D2), and what an
//! unrendered region would have contributed.
//!
//! Unrendered: comment lines, comment and export blocks, every drawer except
//! a property drawer (and MAE's list-item property drawer), a `COMMENT`
//! subtree, a subtree carrying an exclude tag (`noexport` unless declared
//! otherwise), and a footnote definition no rendered text references.

use super::{items, Region};
use crate::kind::SyntaxKind::*;
use crate::kind::SyntaxNode;
use crate::span::Span;
use crate::views::Headline;
use rowan::WalkEvent;
use std::collections::HashSet;

/// What decides whether a node is rendered.
pub(crate) struct Rules {
    pub exclude_tags: Vec<String>,
    /// Labels of referenced footnote definitions; `None` while they are being
    /// computed, when every definition counts as unrendered.
    pub referenced: Option<HashSet<String>>,
}

impl Rules {
    /// The region `node` opens, if it is not rendered.
    pub fn region(&self, node: &SyntaxNode) -> Option<Region> {
        match node.kind() {
            COMMENT => Some(Region::CommentLine),
            COMMENT_BLOCK => Some(Region::CommentBlock),
            EXPORT_BLOCK => Some(Region::ExportBlock),
            DRAWER if items::item_property_drawer(node).is_none() => Some(Region::Drawer),
            HEADLINE => self.headline_region(node),
            FOOTNOTE_DEFINITION => {
                let label = definition_label(node);
                let referenced = match (&self.referenced, label) {
                    (Some(set), Some(l)) => set.contains(&l),
                    _ => false,
                };
                (!referenced).then_some(Region::UnreferencedFootnote)
            }
            _ => None,
        }
    }

    fn headline_region(&self, node: &SyntaxNode) -> Option<Region> {
        let h = Headline::cast(node.clone())?;
        if h.is_commented() {
            return Some(Region::CommentSubtree);
        }
        let excluded = h.tags().iter().any(|t| self.exclude_tags.contains(t));
        excluded.then_some(Region::ExcludedSubtree)
    }
}

/// `LABEL` of a definition's `[fn:LABEL]`.
fn definition_label(def: &SyntaxNode) -> Option<String> {
    let tok = def
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == FOOTNOTE_LABEL)?;
    let label = tok.text().strip_prefix("[fn:")?.strip_suffix(']')?;
    Some(label.to_string()).filter(|l| !l.is_empty())
}

/// `LABEL` of a reference `[fn:LABEL]` or `[fn:LABEL:inline definition]`;
/// `None` for an anonymous `[fn::…]`.
fn reference_label(reference: &SyntaxNode) -> Option<String> {
    let text = reference.to_string();
    let rest = text.strip_prefix("[fn:")?;
    let end = rest.find([':', ']'])?;
    Some(rest[..end].to_string()).filter(|l| !l.is_empty())
}

/// Labels of the footnote definitions rendered text references: references
/// outside every unrendered region, then, to a fixed point, references
/// inside definitions already found referenced.
pub(crate) fn referenced_footnotes(root: &SyntaxNode, exclude_tags: &[String]) -> HashSet<String> {
    let rules = Rules {
        exclude_tags: exclude_tags.to_vec(),
        referenced: None,
    };
    let mut refs = HashSet::new();
    collect_references(root, &rules, &mut refs);
    let defs: Vec<(String, SyntaxNode)> = root
        .descendants()
        .filter(|n| n.kind() == FOOTNOTE_DEFINITION)
        .filter(|n| !n.ancestors().skip(1).any(|a| rules.region(&a).is_some()))
        .filter_map(|n| Some((definition_label(&n)?, n)))
        .collect();
    let mut done = HashSet::new();
    loop {
        let ready: Vec<&(String, SyntaxNode)> = defs
            .iter()
            .filter(|(l, _)| refs.contains(l) && !done.contains(l))
            .collect();
        if ready.is_empty() {
            return refs;
        }
        let mut found = HashSet::new();
        for (label, def) in ready {
            done.insert(label.clone());
            for c in def.children() {
                collect_references(&c, &rules, &mut found);
            }
        }
        refs.extend(found);
    }
}

/// References under `start` that are not inside an unrendered region.
fn collect_references(start: &SyntaxNode, rules: &Rules, out: &mut HashSet<String>) {
    let mut walk = start.preorder();
    while let Some(ev) = walk.next() {
        let WalkEvent::Enter(n) = ev else { continue };
        if rules.region(&n).is_some() {
            walk.skip_subtree();
        } else if n.kind() == FOOTNOTE_REFERENCE {
            out.extend(reference_label(&n));
        }
    }
}

/// Every `:ID: value` line of `text` (`base` is its offset in the document),
/// including one commented out with `#` prefixes, with the line's span.
pub(crate) fn id_lines(text: &str, base: usize) -> Vec<(String, Span)> {
    let blank = [' ', '\t'];
    let mut out = Vec::new();
    let mut pos = base;
    for seg in text.split_inclusive(['\n', '\r']) {
        let line = seg.trim_end_matches(['\n', '\r']);
        let start = pos;
        pos += seg.len();
        let mut s = line.trim_start_matches(blank);
        while s.starts_with('#') && !s.starts_with("#+") {
            s = s[1..].trim_start_matches(blank);
        }
        let is_id = s.len() > 4 && s.as_bytes()[..4].eq_ignore_ascii_case(b":ID:");
        if !is_id || !s[4..].starts_with(blank) {
            continue;
        }
        let value = s[4..].trim_matches(blank);
        if !value.is_empty() {
            out.push((value.to_string(), Span::new(start, start + line.len())));
        }
    }
    out
}

/// Every link in an unrendered region: its LINK objects, and the links of
/// the raw text of comment/export blocks and comment lines inside it, read
/// by parsing that text (one level: a raw block inside a raw block is not
/// read again). Returns `(path as written, span)`.
pub(crate) fn region_links(region: &SyntaxNode) -> Vec<(String, Span)> {
    let mut out = Vec::new();
    for n in region.descendants() {
        match n.kind() {
            LINK => out.extend(super::plain::link_path(&n).map(|p| (p, Span::of(&n)))),
            COMMENT_BLOCK | EXPORT_BLOCK => {
                let content = n
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == BLOCK_CONTENT);
                if let Some(t) = content {
                    let base = usize::from(t.text_range().start());
                    out.extend(links_in_text(t.text(), base));
                }
            }
            COMMENT => {
                let base = usize::from(n.text_range().start());
                out.extend(links_in_text(&uncomment(&n.to_string()), base));
            }
            _ => {}
        }
    }
    out
}

/// The text of comment lines with each line's `#` and the space after it
/// replaced by spaces, so offsets are unchanged.
fn uncomment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for seg in text.split_inclusive(['\n', '\r']) {
        let indent = seg.len() - seg.trim_start_matches([' ', '\t']).len();
        let rest = &seg[indent..];
        let marker = match rest.strip_prefix('#') {
            Some(r) if r.starts_with(' ') => 2,
            Some(_) => 1,
            None => 0,
        };
        out.push_str(&" ".repeat(indent + marker));
        out.push_str(&rest[marker..]);
    }
    out
}

fn links_in_text(text: &str, base: usize) -> Vec<(String, Span)> {
    let doc = crate::parse(text, &crate::ParseOptions::default());
    doc.syntax()
        .descendants()
        .filter(|n| n.kind() == LINK)
        .filter_map(|n| {
            let s = Span::of(&n);
            let span = Span::new(base + s.start as usize, base + s.end as usize);
            Some((super::plain::link_path(&n)?, span))
        })
        .collect()
}
