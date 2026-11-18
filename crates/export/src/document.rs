//! Org → rendered tree, through `mae-org` (ADR-112 O4, issue #942).
//!
//! What an export shows is `mae-org`'s decision, exactly as Org's own export
//! makes it: COMMENT and `:noexport:` subtrees, `#+SELECT_TAGS:` /
//! `#+EXCLUDE_TAGS:`, comment lines and blocks, drawers, babel's `:exports`
//! and export blocks are all resolved by [`mae_org::render`]. This module only
//! reads the page metadata the HTML shell and the Markdown front matter need
//! (title, author, date, language, `#+OPTIONS:`), and picks one subtree out of
//! the rendered page for a subtree export. The HTML ([`crate::html`]) and
//! Markdown ([`crate::markdown`]) writers serialise the same tree, so the two
//! formats cannot disagree about what is published.
//!
//! Code blocks are never evaluated here: `mae-org` renders the `#+RESULTS:`
//! already in the file, per `:exports`.

use std::collections::HashSet;

use mae_org::{
    heading_anchor, parse, render, render_inline, semantics_with, text_of, Attr, Document, Element,
    Headline, Keyword, Node, ParseOptions, RenderDiagnostic, RenderOptions, Resolver,
    SemanticsOptions, SyntaxKind, Tag, ORG_EXPORT_SELECT_TAGS,
};

/// The page settings `#+OPTIONS:` carries that the shell reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSettings {
    /// `toc:` — whether the HTML page carries a table of contents.
    pub toc: bool,
    /// `toc:N` — the deepest headline level (stars) the contents list.
    pub toc_depth: usize,
    /// `author:` — whether the author line is shown.
    pub with_author: bool,
    /// `date:` — whether the date line is shown.
    pub with_date: bool,
}

impl Default for PageSettings {
    /// Org's defaults (`org-export-with-toc` t, headline levels 3 deep,
    /// `org-export-with-author` t, `org-export-with-date` t).
    fn default() -> Self {
        PageSettings {
            toc: true,
            toc_depth: 3,
            with_author: true,
            with_date: true,
        }
    }
}

/// A document (or one subtree of it) ready to be written as HTML or Markdown.
#[derive(Debug, Clone)]
pub struct Exported {
    /// `#+TITLE:`, rendered inline-only. Empty when there is none.
    pub title: Vec<Node>,
    pub author: Vec<Node>,
    pub date: Vec<Node>,
    /// `#+LANGUAGE:` as written (escaped by each writer).
    pub language: Option<String>,
    pub settings: PageSettings,
    /// What the export shows: `mae-org`'s rendered body.
    pub body: Vec<Node>,
    /// The renderer's notes (content left out rather than guessed at).
    pub diagnostics: Vec<RenderDiagnostic>,
    /// HTML export blocks and `#+HTML:` lines in the document. No export
    /// emits them (`mae-org` renders no raw HTML in any form, ADR-112 D3);
    /// counted so a caller can say so instead of dropping them silently.
    pub raw_html: usize,
}

impl Exported {
    /// The macros could not be expanded as Org's export expands them, so
    /// the body is empty (as Org's export produces no output then).
    pub fn macro_failed(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| matches!(d, RenderDiagnostic::MacroFailed(..)))
    }
}

/// Why a subtree export produced nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtreeError {
    /// The position is not under any headline.
    NoHeading,
    /// The headline there is not exported: it is in a COMMENT subtree, an
    /// excluded (`:noexport:`) one, or outside the selected ones.
    NotExported,
    /// The rendered page's headings do not line up with the source's (a
    /// macro made or removed a headline), so the subtree cannot be placed.
    /// Refused rather than guessed: a wrong guess publishes another subtree.
    Unplaced,
}

impl std::fmt::Display for SubtreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SubtreeError::NoHeading => "No heading at cursor for subtree export",
            SubtreeError::NotExported => {
                "The heading at cursor is not exported (COMMENT, an excluded tag such as \
                 :noexport:, or outside the selected tags)"
            }
            SubtreeError::Unplaced => {
                "Cannot place the heading at cursor in the rendered page (a macro changes the \
                 headings); export the whole document instead"
            }
        })
    }
}

/// One parse, its semantics and its render, with the page metadata.
struct Pipeline {
    doc: Document,
    sem: mae_org::Semantics,
    rendered: mae_org::Rendered,
    page: Exported,
}

fn run(source: &str) -> Pipeline {
    let doc = parse(source, &ParseOptions::default());
    let settings = settings_keywords(&doc);
    let sem = semantics_with(&doc, &semantics_options(&settings));
    let options = RenderOptions::default();
    let resolver = Resolver::default();
    let rendered = render(&doc, &sem, &options, &resolver);

    let inline = |key: &str| {
        let joined = joined(&settings, key);
        if joined.is_empty() {
            Vec::new()
        } else {
            render_inline(&joined, &options, &resolver)
        }
    };
    let mut page_settings = PageSettings::default();
    for (_, value) in settings.iter().filter(|(k, _)| k == "OPTIONS") {
        apply_options(value, &mut page_settings);
    }
    let page = Exported {
        title: inline("TITLE"),
        author: inline("AUTHOR"),
        date: inline("DATE"),
        language: settings
            .iter()
            .find(|(k, v)| k == "LANGUAGE" && !v.is_empty())
            .map(|(_, v)| v.clone()),
        settings: page_settings,
        body: rendered.body.clone(),
        diagnostics: rendered.diagnostics.clone(),
        raw_html: raw_html_count(&doc),
    };
    Pipeline {
        doc,
        sem,
        rendered,
        page,
    }
}

/// The whole document, as Org's export shows it.
pub fn export_document(source: &str) -> Exported {
    run(source).page
}

/// The subtree of the innermost headline holding byte `offset` of `source`,
/// as the whole document's export shows it: every in-buffer setting, macro
/// and inherited header argument applies, and a subtree the whole export
/// would hide (or one inside such a subtree) is refused, never exported.
/// The footnotes its text references come with it.
pub fn export_subtree(source: &str, offset: usize) -> Result<Exported, SubtreeError> {
    let p = run(source);
    let len = source.len();
    let contains = |h: &Headline| {
        let r = h.syntax().text_range();
        let (start, end) = (usize::from(r.start()), usize::from(r.end()));
        start <= offset && (offset < end || (end == len && offset == len))
    };
    // Document order puts a headline before its descendants: the last one
    // holding the offset is the innermost.
    let target = p
        .doc
        .headlines()
        .filter(contains)
        .last()
        .ok_or(SubtreeError::NoHeading)?;

    // The source headlines the page renders, in order: the renderer skips
    // exactly `Semantics::unrendered`, so a headline starting inside one of
    // those regions has no section.
    let unrendered: Vec<(u32, u32)> = p
        .sem
        .unrendered
        .iter()
        .map(|u| (u.span.start, u.span.end))
        .collect();
    let shown: Vec<Headline> = p
        .doc
        .headlines()
        .filter(|h| {
            let start = u32::from(h.syntax().text_range().start());
            !unrendered.iter().any(|&(s, e)| s <= start && start < e)
        })
        .collect();
    let index = shown
        .iter()
        .position(|h| h.syntax() == target.syntax())
        .ok_or(SubtreeError::NotExported)?;

    let mut sections = Vec::new();
    headline_sections(&p.rendered.body, &mut sections);
    if sections.len() != shown.len() {
        return Err(SubtreeError::Unplaced);
    }
    let section = sections[index];
    if !anchor_matches(section, &target) {
        return Err(SubtreeError::Unplaced);
    }

    let mut body = vec![Node::Element(section.clone())];
    let mut refs = HashSet::new();
    footnote_refs(&section.children, &mut refs);
    if let Some(notes) = footnotes_section(&p.rendered.body, &refs) {
        body.push(Node::Element(notes));
    }
    let mut page = p.page;
    page.body = body;
    Ok(page)
}

/// Keyword elements outside COMMENT subtrees (Org's export reads its
/// settings from there only), as `(KEY upper-cased, value)`, in order.
fn settings_keywords(doc: &Document) -> Vec<(String, String)> {
    doc.keywords()
        .filter(|k| !in_comment_subtree(k))
        .map(|k| (k.key().to_ascii_uppercase(), k.value()))
        .collect()
}

fn in_comment_subtree(k: &Keyword) -> bool {
    k.syntax()
        .ancestors()
        .filter_map(Headline::cast)
        .any(|h| h.is_commented())
}

/// Every non-empty value of `key`, joined with one space (Org's `parse`
/// behaviour for TITLE, AUTHOR and DATE).
fn joined(settings: &[(String, String)], key: &str) -> String {
    settings
        .iter()
        .filter(|(k, v)| k == key && !v.is_empty())
        .map(|(_, v)| v.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Export selection as Org's exporter does it: `noexport` excluded and
/// Org's default select tag (`export`) in force, both replaced by in-file
/// `#+EXCLUDE_TAGS:` / `#+SELECT_TAGS:` lines (`mae-org` reads those).
///
/// The pre-Org-8 spellings `#+EXPORT_EXCLUDE_TAGS:` and
/// `#+EXPORT_SELECT_TAGS:`, which MAE's own exporter honoured before it
/// rendered through `mae-org`, are kept: their tags join the defaults, so a
/// file written for them hides at least what it hid before.
fn semantics_options(settings: &[(String, String)]) -> SemanticsOptions {
    let words = |key: &str| -> Vec<String> {
        settings
            .iter()
            .filter(|(k, _)| k == key)
            .flat_map(|(_, v)| v.split_whitespace().map(str::to_string))
            .collect()
    };
    let mut options = SemanticsOptions::default();
    options.exclude_tags.extend(words("EXPORT_EXCLUDE_TAGS"));
    options.select_tags = ORG_EXPORT_SELECT_TAGS
        .iter()
        .map(|s| s.to_string())
        .chain(words("EXPORT_SELECT_TAGS"))
        .collect();
    options
}

/// `#+OPTIONS:` items the shell reads; everything else in the line is
/// `mae-org`'s business or not modelled.
fn apply_options(value: &str, settings: &mut PageSettings) {
    let on = |v: &str| v != "nil" && v != "no";
    for item in value.split_whitespace() {
        let Some((key, val)) = item.split_once(':') else {
            continue;
        };
        match key {
            "toc" => match val.parse::<usize>() {
                Ok(n) => {
                    settings.toc = true;
                    settings.toc_depth = n;
                }
                Err(_) => settings.toc = on(val),
            },
            "author" => settings.with_author = on(val),
            "date" => settings.with_date = on(val),
            _ => {}
        }
    }
}

/// HTML export blocks and `#+HTML:` keywords outside COMMENT subtrees.
fn raw_html_count(doc: &Document) -> usize {
    let blocks = doc
        .blocks()
        .filter(|b| b.kind() == SyntaxKind::EXPORT_BLOCK)
        .filter(|b| {
            b.parameters()
                .and_then(|p| p.split_whitespace().next().map(str::to_string))
                .is_some_and(|backend| backend.eq_ignore_ascii_case("html"))
        })
        .filter(|b| {
            !b.syntax()
                .ancestors()
                .filter_map(Headline::cast)
                .any(|h| h.is_commented())
        })
        .count();
    let keywords = settings_keywords(doc)
        .iter()
        .filter(|(k, _)| k == "HTML")
        .count();
    blocks + keywords
}

/// The headline sections of `nodes`, in document (pre-)order.
fn headline_sections<'a>(nodes: &'a [Node], out: &mut Vec<&'a Element>) {
    for n in nodes {
        if let Node::Element(e) = n {
            if is_headline_section(e) {
                out.push(e);
            }
            headline_sections(&e.children, out);
        }
    }
}

pub(crate) fn has_class(e: &Element, class: &str) -> bool {
    e.get(Attr::Class)
        .is_some_and(|c| c.split_ascii_whitespace().any(|x| x == class))
}

/// A `<section class="org-section org-level-N">`: one rendered headline.
pub(crate) fn is_headline_section(e: &Element) -> bool {
    e.tag == Tag::Section && has_class(e, "org-section")
}

/// The headline level (stars) of a headline section, from `org-level-N`.
pub(crate) fn section_level(e: &Element) -> Option<usize> {
    e.get(Attr::Class)?
        .split_ascii_whitespace()
        .find_map(|c| c.strip_prefix("org-level-")?.parse().ok())
}

/// The `<hN>` of a headline section.
pub(crate) fn section_heading(e: &Element) -> Option<&Element> {
    e.children.iter().find_map(|c| match c {
        Node::Element(h) if is_heading_tag(h.tag) => Some(h),
        _ => None,
    })
}

pub(crate) fn is_heading_tag(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6
    )
}

/// The rendered heading's anchor is the one `target`'s title gives
/// (`h-SLUG`, or `h-SLUG--N` for a repeat): the second check that the
/// section found by position is the target's.
fn anchor_matches(section: &Element, target: &Headline) -> bool {
    let Some(id) = section_heading(section).and_then(|h| h.get(Attr::Id)) else {
        return false;
    };
    let title = text_of(&render_inline(
        &target.title(),
        &RenderOptions::default(),
        &Resolver::default(),
    ));
    let expected = heading_anchor(&title);
    match id.strip_prefix(expected.as_str()) {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix("--")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    }
}

/// Every `#fn-N` a footnote reference in `nodes` points at.
fn footnote_refs(nodes: &[Node], out: &mut HashSet<String>) {
    for n in nodes {
        if let Node::Element(e) = n {
            if let Some(target) = e.get(Attr::Href).and_then(|h| h.strip_prefix('#')) {
                if target.starts_with("fn-") {
                    out.insert(target.to_string());
                }
            }
            footnote_refs(&e.children, out);
        }
    }
}

/// The page's footnotes section, keeping only the definitions in `refs`.
fn footnotes_section(body: &[Node], refs: &HashSet<String>) -> Option<Element> {
    let notes = body.iter().find_map(|n| match n {
        Node::Element(e) if e.tag == Tag::Section && has_class(e, "org-footnotes") => Some(e),
        _ => None,
    })?;
    let mut kept = notes.clone();
    kept.children.retain(|c| match c {
        Node::Element(d) if has_class(d, "org-footnote") => {
            d.get(Attr::Id).is_some_and(|id| refs.contains(id))
        }
        _ => false,
    });
    (!kept.children.is_empty()).then_some(kept)
}
