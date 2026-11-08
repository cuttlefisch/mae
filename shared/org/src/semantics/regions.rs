//! Which parts of a document are rendered (ADR-112 D2), and what an
//! unrendered region would have contributed.
//!
//! Unrendered, as Org's exporter prunes them (`org-export--skip-p`,
//! `org-export--prune-tree`, Org 9.7.11): comment lines, comment and export
//! blocks, a `COMMENT` subtree, a subtree whose own tags or the file's tags
//! carry an exclude tag (select and exclude tags expanded through the
//! file's `#+TAGS:` groups, `org-tags-expand`), the contents of an archived
//! headline, and, when some headline carries a select tag, every headline
//! outside the selected trees and their ancestors, plus the zeroth
//! section's content (not the file's property drawer, which defines the
//! file node).
//!
//! Drawers are NOT Org's rule: Org's default (`org-export-with-drawers`,
//! `(not "LOGBOOK")`) exports every drawer but LOGBOOK. ADR-112 D2 chose to
//! leave every drawer but a property drawer (and MAE's list-item property
//! drawer) unrendered, so a drawer's links and ids are never edges.
//!
//! A footnote definition is rendered when rendered text references its
//! label and it is the ONE definition Org's export uses for that label
//! (`org-export-get-footnote-definition`): the last of the label's
//! definitions, standalone or inline (`[fn:n:text]`), left in the pruned
//! tree; failing that, the first in a region Org parses before pruning it
//! (`org-export--missing-definitions` re-adds it). A region Org never
//! parses (a `COMMENT` subtree, which the exporter deletes before parsing,
//! or a block or comment) holds no definition at all.
//!
//! Every headline's region is decided once, in [`Rules::new`], so the cost of
//! reading tags is paid once per headline, whatever asks.

use super::{items, Region, SemanticsOptions};
use crate::kind::SyntaxKind::*;
use crate::kind::SyntaxNode;
use crate::seeded::{HashMap, HashSet};
use crate::span::Span;
use crate::views::Headline;
use rowan::WalkEvent;
use std::collections::VecDeque;

/// What decides whether a node is rendered.
pub(crate) struct Rules {
    /// The region of each headline that is not rendered, by start offset.
    headlines: HashMap<u32, Region>,
    /// Start offsets of archived headlines whose contents are not rendered.
    archived: HashSet<u32>,
    /// The zeroth section, when a select tag hides its content.
    zeroth: Option<SyntaxNode>,
    /// Referenced footnote labels, each with the start of the reference that
    /// first made it referenced; `None` while they are being computed.
    pub referenced: Option<HashMap<String, u32>>,
    /// For each footnote label, the start of the definition Org uses (see
    /// the module doc): a standalone definition or an inline reference.
    pub chosen: HashMap<String, u32>,
    /// Keyword keys (upper case) whose values are not read for links.
    pub link_exclude_keywords: HashSet<String>,
    /// Property keys (canonical) whose values are not read for links.
    pub link_exclude_properties: HashSet<String>,
}

impl Region {
    /// Whether Org parses this region's contents before pruning it, so a
    /// footnote definition inside can still be the one Org uses: re-added
    /// by a reference from a pruned region, or an inline definition inside
    /// a definition Org does not render.
    pub(crate) fn parses_contents(self) -> bool {
        matches!(
            self,
            Region::ExcludedSubtree
                | Region::Drawer
                | Region::Unselected
                | Region::Archived
                | Region::UnreferencedFootnote
                | Region::OverriddenFootnote
        )
    }
}

impl Rules {
    /// The rules of `root` under `options`: `exclude` and `select` are the
    /// tags in force (in-file declarations already applied), `file_tags`
    /// the file's tags, which every headline inherits for exclusion and
    /// selection (`org-export-get-tags` with inheritance).
    pub fn new(
        root: &SyntaxNode,
        options: &SemanticsOptions,
        exclude: &[String],
        select: &[String],
        file_tags: &[String],
    ) -> Rules {
        let exclude: HashSet<&str> = exclude.iter().map(String::as_str).collect();
        let select: HashSet<&str> = select.iter().map(String::as_str).collect();
        let file_excluded = file_tags.iter().any(|t| exclude.contains(t.as_str()));
        let file_selected = file_tags.iter().any(|t| select.contains(t.as_str()));
        let mut own: Vec<(SyntaxNode, Vec<String>, bool)> = Vec::new();
        for n in crate::views::headline_nodes(root) {
            let Some(h) = Headline::cast(n.clone()) else {
                continue;
            };
            own.push((n, h.tags(), h.is_commented()));
        }
        // Org deletes the COMMENT subtrees before it selects: a select tag
        // under a COMMENT headline selects nothing.
        let mut commented: HashSet<u32> = HashSet::default();
        for (n, _, c) in own.iter().filter(|_| !options.comments_deleted) {
            let parent = n.parent().filter(|p| p.kind() == HEADLINE);
            if *c || parent.is_some_and(|p| commented.contains(&start(&p))) {
                commented.insert(start(n));
            }
        }
        let live: Vec<_> = own
            .iter()
            .filter(|h| !commented.contains(&start(&h.0)))
            .cloned()
            .collect();
        let selected = selected_headlines(&live, &select, file_selected);
        let mut headlines = HashMap::default();
        let mut archived = HashSet::default();
        for (n, tags, commented) in &own {
            let at = u32::from(n.text_range().start());
            let excluded = file_excluded || tags.iter().any(|t| exclude.contains(t.as_str()));
            let is_archived = tags.contains(&options.archive_tag);
            let region = if *commented && options.comments_deleted {
                // Made by an expansion: parsed, then pruned (`org-export--skip-p`).
                Some(Region::ExcludedSubtree)
            } else if *commented {
                Some(Region::CommentSubtree)
            } else if excluded {
                Some(Region::ExcludedSubtree)
            } else if selected.as_ref().is_some_and(|s| !s.contains(&at)) {
                Some(Region::Unselected)
            } else if is_archived && options.archived_trees == super::ArchivedTrees::None {
                Some(Region::Archived)
            } else {
                None
            };
            match region {
                Some(r) => {
                    headlines.insert(at, r);
                }
                None if is_archived && options.archived_trees == super::ArchivedTrees::Headline => {
                    archived.insert(at);
                }
                None => {}
            }
        }
        let zeroth = selected
            .and_then(|_| root.children().next())
            .filter(|s| s.kind() == SECTION);
        let upper = |v: &[String]| v.iter().map(|k| k.to_ascii_uppercase()).collect();
        Rules {
            headlines,
            archived,
            zeroth,
            referenced: None,
            chosen: HashMap::default(),
            link_exclude_keywords: upper(&options.link_exclude_keywords),
            link_exclude_properties: options
                .link_exclude_properties
                .iter()
                .map(|k| crate::views::canonical_key(k))
                .collect(),
        }
    }

    /// The region `node` opens, if it is not rendered.
    pub fn region(&self, node: &SyntaxNode) -> Option<Region> {
        let kind = node.kind();
        if kind == FOOTNOTE_DEFINITION {
            let label = definition_label(node);
            let referenced = match (&self.referenced, &label) {
                (Some(map), Some(l)) => map.contains_key(l),
                _ => false,
            };
            if !referenced {
                return Some(Region::UnreferencedFootnote);
            }
            return (!self.is_chosen(label.as_deref(), node)).then_some(Region::OverriddenFootnote);
        }
        if kind == FOOTNOTE_REFERENCE {
            // An inline definition Org does not use renders nothing (the
            // reference itself still references its label).
            let label = inline_label(node)?;
            return (!self.is_chosen(Some(&label), node)).then_some(Region::OverriddenFootnote);
        }
        let parent = node.parent();
        if let Some(z) = &self.zeroth {
            if parent.as_ref() == Some(z) && kind != PROPERTY_DRAWER {
                return Some(Region::Unselected);
            }
        }
        match kind {
            COMMENT => Some(Region::CommentLine),
            COMMENT_BLOCK => Some(Region::CommentBlock),
            EXPORT_BLOCK => Some(Region::ExportBlock),
            DRAWER if !items::is_item_property_drawer(node) => Some(Region::Drawer),
            // A COMMENT subtree is deleted before anything is parsed, even
            // inside an archived headline whose contents are parsed.
            HEADLINE if self.headlines.get(&start(node)) == Some(&Region::CommentSubtree) => {
                Some(Region::CommentSubtree)
            }
            SECTION | HEADLINE if parent.as_ref().is_some_and(|p| self.contents_hidden(p)) => {
                Some(Region::Archived)
            }
            HEADLINE => self.headlines.get(&start(node)).copied(),
            _ => None,
        }
    }

    /// Whether `node` is the definition Org uses for `label`.
    fn is_chosen(&self, label: Option<&str>, node: &SyntaxNode) -> bool {
        label.is_some_and(|l| self.chosen.get(l) == Some(&start(node)))
    }

    /// Whether `headline` is archived, so rendered without its contents.
    pub fn contents_hidden(&self, headline: &SyntaxNode) -> bool {
        headline.kind() == HEADLINE && self.archived.contains(&start(headline))
    }
}

fn start(n: &SyntaxNode) -> u32 {
    u32::from(n.text_range().start())
}

/// `org-export--selected-trees`: `None` when no select tag is in force (no
/// headline and no file tag carries one); otherwise the start offsets of
/// the headlines in a selected tree and of their ancestors. A file tag that
/// is a select tag selects every headline. `own` is in document order.
fn selected_headlines(
    own: &[(SyntaxNode, Vec<String>, bool)],
    select: &HashSet<&str>,
    file_selected: bool,
) -> Option<HashSet<u32>> {
    if file_selected {
        // Every headline is selected; with none, nothing is (nil), so the
        // zeroth section stays rendered, as in Org.
        return (!own.is_empty()).then(|| own.iter().map(|h| start(&h.0)).collect());
    }
    // In a selected tree, and ancestors of one: separate sets, or a
    // selected tree's ancestor would pass selection on to its other
    // children (Emacs renders the ancestor, not its siblings' trees).
    let mut in_tree: HashSet<u32> = HashSet::default();
    let mut ancestors: HashSet<u32> = HashSet::default();
    let mut any = false;
    for (n, tags, _) in own {
        let root = tags.iter().any(|t| select.contains(t.as_str()));
        let parent_in = n
            .parent()
            .filter(|p| p.kind() == HEADLINE)
            .is_some_and(|p| in_tree.contains(&start(&p)));
        if root || parent_in {
            in_tree.insert(start(n));
        }
        if root {
            any = true;
            // Stop at an ancestor already marked, so each is visited once.
            for a in n.ancestors().skip(1).filter(|a| a.kind() == HEADLINE) {
                if !ancestors.insert(start(&a)) {
                    break;
                }
            }
        }
    }
    let chosen: HashSet<u32> = in_tree.union(&ancestors).copied().collect();
    any.then_some(chosen)
}

/// `LABEL` of a definition's `[fn:LABEL]`.
pub(crate) fn definition_label(def: &SyntaxNode) -> Option<String> {
    let tok = def
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == FOOTNOTE_LABEL)?;
    let label = tok.text().strip_prefix("[fn:")?.strip_suffix(']')?;
    Some(label.to_string()).filter(|l| !l.is_empty())
}

/// `LABEL` of an inline definition `[fn:LABEL:text]`; `None` for a plain
/// reference `[fn:LABEL]` or an anonymous `[fn::text]`.
pub(crate) fn inline_label(reference: &SyntaxNode) -> Option<String> {
    let text = reference.to_string();
    let rest = text.strip_prefix("[fn:")?;
    let end = rest.find([':', ']'])?;
    (rest.as_bytes()[end] == b':' && end > 0).then(|| rest[..end].to_string())
}

/// For each label, the start of the definition Org's export uses (see the
/// module doc): the last of the label's definitions outside every pruned
/// region, else the first inside one. One walk; regions Org never parses
/// are skipped.
pub(crate) fn choose_definitions(root: &SyntaxNode, rules: &Rules) -> HashMap<String, u32> {
    // Per label: the last rendered candidate, and the first pruned one.
    let mut found: HashMap<String, (Option<u32>, Option<u32>)> = HashMap::default();
    let mut pruned: Vec<SyntaxNode> = Vec::new();
    let mut walk = root.preorder();
    while let Some(ev) = walk.next() {
        let n = match ev {
            WalkEvent::Enter(n) => n,
            WalkEvent::Leave(n) => {
                if pruned.last() == Some(&n) {
                    pruned.pop();
                }
                continue;
            }
        };
        let label = match n.kind() {
            FOOTNOTE_DEFINITION => definition_label(&n),
            FOOTNOTE_REFERENCE => inline_label(&n),
            _ => {
                match rules.region(&n) {
                    Some(r) if r.parses_contents() => pruned.push(n),
                    Some(_) => walk.skip_subtree(),
                    None => {}
                }
                continue;
            }
        };
        if let Some(label) = label {
            let at = start(&n);
            let e = found.entry(label).or_default();
            if pruned.is_empty() {
                e.0 = Some(at);
            } else if e.1.is_none() {
                e.1 = Some(at);
            }
        }
    }
    found
        .into_iter()
        .filter_map(|(l, (rendered, first_pruned))| rendered.or(first_pruned).map(|at| (l, at)))
        .collect()
}

/// `LABEL` of a reference `[fn:LABEL]` or `[fn:LABEL:inline definition]`;
/// `None` for an anonymous `[fn::…]`.
pub(crate) fn reference_label(reference: &SyntaxNode) -> Option<String> {
    let text = reference.to_string();
    let rest = text.strip_prefix("[fn:")?;
    let end = rest.find([':', ']'])?;
    Some(rest[..end].to_string()).filter(|l| !l.is_empty())
}

/// The footnote labels rendered text references, each with the start of the
/// reference that first made it referenced, and the labels in that order.
///
/// References in rendered text (outside every unrendered region and every
/// definition) come first, in document order; then, breadth first, the
/// references inside the definition Org uses for each referenced label
/// ([`Rules::chosen`]: references inside a definition it does not use
/// reference nothing). A worklist: each label's definition is read once,
/// so a chain of N definitions costs O(N).
pub(crate) fn referenced_footnotes(
    root: &SyntaxNode,
    rules: &Rules,
) -> (HashMap<String, u32>, Vec<String>) {
    let mut outside: Vec<(String, u32)> = Vec::new();
    // The definition Org uses for each label, by its start.
    let mut defs: HashMap<u32, SyntaxNode> = HashMap::default();
    let mut hidden: Vec<SyntaxNode> = Vec::new();
    let mut walk = root.preorder();
    while let Some(ev) = walk.next() {
        let n = match ev {
            WalkEvent::Enter(n) => n,
            WalkEvent::Leave(n) => {
                if hidden.last() == Some(&n) {
                    hidden.pop();
                }
                continue;
            }
        };
        let kind = n.kind();
        if kind == FOOTNOTE_DEFINITION || kind == FOOTNOTE_REFERENCE {
            if kind == FOOTNOTE_REFERENCE && hidden.is_empty() {
                outside.extend(reference_label(&n).map(|l| (l, start(&n))));
            }
            let label = match kind {
                FOOTNOTE_DEFINITION => definition_label(&n),
                _ => inline_label(&n),
            };
            if label
                .as_ref()
                .is_some_and(|l| rules.chosen.get(l) == Some(&start(&n)))
            {
                defs.insert(start(&n), n.clone());
            }
            // A definition's text is read through the worklist below.
            if label.is_some() || kind == FOOTNOTE_DEFINITION {
                hidden.push(n);
            }
        } else if let Some(r) = rules.region(&n) {
            if r.parses_contents() {
                hidden.push(n);
            } else {
                walk.skip_subtree();
            }
        }
    }
    let mut first: HashMap<String, u32> = HashMap::default();
    let mut order: Vec<String> = Vec::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    let mut add = |l: String, at: u32, queue: &mut VecDeque<String>| {
        if let std::collections::hash_map::Entry::Vacant(e) = first.entry(l.clone()) {
            e.insert(at);
            order.push(l.clone());
            queue.push_back(l);
        }
    };
    for (l, at) in outside {
        add(l, at, &mut queue);
    }
    while let Some(label) = queue.pop_front() {
        let def = rules.chosen.get(&label).and_then(|at| defs.remove(at));
        for c in def.iter().flat_map(|d| d.children()) {
            for (l, at) in references_under(&c, rules) {
                add(l, at, &mut queue);
            }
        }
    }
    (first, order)
}

/// References under `start` outside every unrendered region, in order; an
/// inline definition's own text is not read (the worklist reads the one Org
/// uses).
fn references_under(start: &SyntaxNode, rules: &Rules) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut walk = start.preorder();
    while let Some(ev) = walk.next() {
        let WalkEvent::Enter(n) = ev else { continue };
        if n.kind() == FOOTNOTE_REFERENCE {
            out.extend(reference_label(&n).map(|l| (l, self::start(&n))));
            walk.skip_subtree();
        } else if n.kind() == FOOTNOTE_DEFINITION || rules.region(&n).is_some() {
            walk.skip_subtree();
        }
    }
    out
}

/// The footnote definitions under an unrendered `region` that Org still
/// renders (only when the region's contents are parsed): each referenced
/// label's chosen definition, standalone or inline, in document order.
pub(crate) fn rendered_definitions(region: &SyntaxNode, rules: &Rules) -> Vec<SyntaxNode> {
    let referenced = |label: Option<String>| {
        label.is_some_and(|l| {
            rules
                .referenced
                .as_ref()
                .is_some_and(|m| m.contains_key(&l))
        })
    };
    let mut out = Vec::new();
    let mut walk = region.preorder();
    walk.next(); // the region itself
    while let Some(ev) = walk.next() {
        let WalkEvent::Enter(n) = ev else { continue };
        let rendered = match n.kind() {
            FOOTNOTE_DEFINITION => rules.region(&n).is_none(),
            FOOTNOTE_REFERENCE => {
                let label = inline_label(&n);
                label.is_some() && rules.region(&n).is_none() && referenced(label)
            }
            _ => false,
        };
        if rendered {
            out.push(n);
            walk.skip_subtree();
        } else if rules.region(&n).is_some_and(|r| !r.parses_contents()) {
            walk.skip_subtree();
        }
    }
    out
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

/// What an unrendered region would have contributed: its links and its
/// `#+TRANSCLUDE` members, each `(text, span)`.
#[derive(Default)]
pub(crate) struct Hidden {
    pub links: Vec<(String, Span)>,
    pub transclusions: Vec<(String, Span)>,
}

/// Everything under an unrendered region that would be a link or a
/// transclusion if rendered: its LINK objects, the links in keyword and
/// property values the semantics read, its `#+TRANSCLUDE` keywords, and the
/// same in the raw text of comment/export blocks and comment lines inside
/// it, read by parsing that text (one level: a raw block inside a raw block
/// is not read again).
pub(crate) fn region_contents(region: &SyntaxNode, rules: &Rules) -> Hidden {
    let mut out = Hidden::default();
    scan_tree(region, rules, &mut out, 0);
    for n in region.descendants() {
        match n.kind() {
            COMMENT_BLOCK | EXPORT_BLOCK => {
                let content = n
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == BLOCK_CONTENT);
                if let Some(t) = content {
                    let base = usize::from(t.text_range().start());
                    scan_text(t.text(), base, rules, &mut out);
                }
            }
            COMMENT => {
                let base = usize::from(n.text_range().start());
                scan_text(&uncomment(&n.to_string()), base, rules, &mut out);
            }
            _ => {}
        }
    }
    out
}

/// The links and transclusions of a parsed tree (`base` shifts its spans).
fn scan_tree(root: &SyntaxNode, rules: &Rules, out: &mut Hidden, base: usize) {
    let shift = |s: Span| Span::new(base + s.start as usize, base + s.end as usize);
    for n in root.descendants() {
        match n.kind() {
            LINK => out
                .links
                .extend(super::plain::link_path(&n).map(|p| (p, shift(Span::of(&n))))),
            KEYWORD => {
                if let Some((member, span)) = super::transclude_member(&n) {
                    out.transclusions.push((member, shift(span)));
                } else if let Some((text, at)) = super::keyword_link_value(&n, rules) {
                    out.links.extend(value_links(&text, base + at));
                }
            }
            NODE_PROPERTY => {
                if let Some((text, at)) = super::property_link_value(&n, rules) {
                    out.links.extend(value_links(&text, base + at));
                }
            }
            _ => {}
        }
    }
}

/// The paths and spans of the links in a keyword or property value.
fn value_links(text: &str, base: usize) -> Vec<(String, Span)> {
    let tree = crate::green::objects(text, PARAGRAPH);
    tree.descendants()
        .filter(|n| n.kind() == LINK)
        .filter_map(|n| {
            let s = Span::of(&n);
            let span = Span::new(base + s.start as usize, base + s.end as usize);
            Some((super::plain::link_path(&n)?, span))
        })
        .collect()
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

fn scan_text(text: &str, base: usize, rules: &Rules, out: &mut Hidden) {
    let doc = crate::parse(text, &crate::ParseOptions::default());
    scan_tree(&doc.syntax(), rules, out, base);
}
