//! Org source: the normal form of a document, and the form a KB node is
//! edited in (ADR-092 D3). The lossless print is `doc.syntax().to_string()`.
//!
//! Normalisation changes only what Emacs itself does not distinguish, so a
//! normalised text has the semantics of the original (pinned by
//! `tests/render_source.rs`):
//! - **Line endings become `\n`.** Emacs decodes a DOS or mac file to `\n`
//!   and keeps a CR that is not a line ending as content; so does this. One
//!   exception: a DOS file in which every line ending follows a content CR
//!   (`a\r\r\n`) has no LF spelling that reads back the same (`a\r\n` is
//!   read as DOS again, losing the CR), so it keeps its CRLF endings.
//! - **A leading BOM is dropped** (Emacs strips it on decode), unless a
//!   second follows it: that one is content, and dropping the first would
//!   make it trivia.
//! - **A final line ending is added** to a non-empty text without one.
//! - Nothing else. Property keys keep their spelling (Org reads them
//!   case-insensitively, and rewriting `:id:` to `:ID:` would be a diff on
//!   every save of a file written in lower case); trailing blanks, blank
//!   lines and link text are kept: ADR-092 asks for identity on the link
//!   grammar, never a normaliser.
//!
//! A node's edit form ([`node_source`]) is its `body_span` text with line
//! endings as `\n`; [`with_line_ending`] is its exact inverse, for writing
//! an edited body back into its file without changing the file's
//! convention (one lone LF in a DOS file makes Emacs read the WHOLE file as
//! unix, turning every CR into content).

use std::collections::HashSet;

use crate::{
    Document, LineEnding, ParseOptions, SemanticDiagnostic, SemanticNode, Semantics,
    SemanticsOptions, Span,
};

/// `text` in its document's convention, with line endings as `\n`.
fn to_lf(text: &str, eol: LineEnding) -> String {
    match eol {
        LineEnding::Lf => text.to_string(),
        LineEnding::CrLf => text.replace("\r\n", "\n"),
        LineEnding::Cr => text.replace('\r', "\n"),
    }
}

/// The normal form of an org document (see the module doc). Idempotent.
pub fn normalise(text: &str) -> String {
    // One BOM is trivia; a second is content, and would become trivia if
    // the first were dropped, so then both stay.
    let text = match text.strip_prefix('\u{feff}') {
        Some(rest) if !rest.starts_with('\u{feff}') => rest,
        _ => text,
    };
    let eol = LineEnding::detect(text);
    let end = match eol {
        LineEnding::Lf => "\n",
        LineEnding::CrLf => "\r\n",
        LineEnding::Cr => "\r",
    };
    let mut text = text.to_string();
    if !text.is_empty() && !text.ends_with(end) {
        text.push_str(end);
    }
    let lf = to_lf(&text, eol);
    if LineEnding::detect(&lf) == LineEnding::Lf {
        lf
    } else {
        text
    }
}

/// The text a KB node is edited in: its body (`body_span`), line endings
/// as `\n`. Nothing else changes.
pub fn node_source(doc: &Document, node: &SemanticNode) -> String {
    let text = doc.syntax().to_string();
    let body = text.get(node.body_span.range()).unwrap_or("");
    to_lf(body, doc.line_ending())
}

/// `text` (line endings `\n`, as [`node_source`] gives) in the convention
/// `eol`: the inverse of [`node_source`] on a body of a file with `eol`.
pub fn with_line_ending(text: &str, eol: LineEnding) -> String {
    match eol {
        LineEnding::Lf => text.to_string(),
        LineEnding::CrLf => text.replace('\n', "\r\n"),
        LineEnding::Cr => text.replace('\n', "\r"),
    }
}

/// Why [`replace_body`] refused an edit. Nothing is written when it does.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BodyEditError {
    /// `text` is not the text `doc` was parsed from.
    StaleDocument,
    /// The node's `body_span` is not a range of `text`.
    NoSuchBody,
    /// Line `line` (0-based) of the new body would be read as a headline,
    /// which would end the body and move every later node. Refused rather
    /// than escaped: write it comma-escaped (`,* text`) inside a block, as
    /// Org does, or as text that does not start with stars.
    HeadlineLine { line: usize },
    /// The edit would change how the whole file's line endings are read
    /// (e.g. remove the one bare LF of an LF file whose other LFs follow a
    /// CR, making Emacs read it as DOS). A guard: no node can be in such a
    /// file today, as every drawer line that makes one ends in a bare LF.
    LineEnding,
    /// The edit would change node `id`: another node, or this node's own
    /// title, properties, tags or place (a list item's body can end its
    /// list, for one).
    ChangesNode { id: String },
    /// The body would add, remove or change a name that the rest of the
    /// file refers to, or that something outside the body also names
    /// (review round 2: another node's `[fn:x]`, `[[tgt]]` or `#+CALL: fn()`
    /// would then render something else). A name defined and referred to
    /// only inside the body is the body's own. `kind` is `footnote` (a
    /// definition, or an inline one with a label), `target` (`<<x>>`),
    /// `radio target`, `name` (`#+NAME:` and its aliases on any element),
    /// `results` (a named `#+RESULTS:`), `CUSTOM_ID` or `ID`.
    ChangesName { kind: String, name: String },
    /// The edit would change the links or transclusions of another node
    /// (`id`, or `None` for text outside every node): every field the
    /// semantics give a link, those no rendering shows included (a
    /// footnote definition's links belong to the node that references it).
    ChangesLinks { id: Option<String> },
    /// The document would render differently outside the edited body
    /// (`what`: `the page`, `the footnotes`), or any node's metadata (the
    /// edited node's included) would: the last line of defence, behind the
    /// precise checks. Rendered whole with a resolver that gives every KB
    /// target its own href and title, so a changed target shows.
    ChangesRendering { what: String },
    /// The edit would change how text outside the body parses: an element
    /// or object there (at `offset` in the new text) differs, as when a
    /// radio target in the body makes a radio link elsewhere.
    ChangesOutside { offset: u32 },
    /// The edit would add, remove or change a file-level declaration that
    /// the parse or the semantics read (`#+TODO:`, `#+LINK:`, `#+FILETAGS:`
    /// …: [`DECLARATION_KEYWORDS`]), which can change every node.
    ChangesDeclaration { key: String },
    /// The edit would give an `:ID:` that another element of the file
    /// holds: which of them is the node would depend on their order.
    DuplicateId { id: String },
}

/// The file-level keywords that change how the whole file parses or reads
/// (TODO keywords, link abbreviations, tags, inherited properties, a setup
/// file): [`replace_body`] refuses an edit that changes any of them.
pub const DECLARATION_KEYWORDS: &[&str] = &[
    "TODO",
    "SEQ_TODO",
    "TYP_TODO",
    "LINK",
    "TAGS",
    "SELECT_TAGS",
    "EXCLUDE_TAGS",
    "FILETAGS",
    "PROPERTY",
    "SETUPFILE",
];

impl std::fmt::Display for BodyEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BodyEditError::StaleDocument => write!(f, "the text is not the parsed document's"),
            BodyEditError::NoSuchBody => write!(f, "the node's body is not in the text"),
            BodyEditError::HeadlineLine { line } => {
                write!(f, "line {line} of the new body would be a headline")
            }
            BodyEditError::LineEnding => {
                write!(
                    f,
                    "the edit would change how the file's line endings are read"
                )
            }
            BodyEditError::ChangesNode { id } => write!(f, "the edit would change node {id:?}"),
            BodyEditError::ChangesName { kind, name } => {
                write!(
                    f,
                    "the edit would add, remove or change the {kind} {name:?}"
                )
            }
            BodyEditError::ChangesLinks { id: Some(id) } => {
                write!(f, "the edit would change the links of node {id:?}")
            }
            BodyEditError::ChangesLinks { id: None } => {
                write!(f, "the edit would change links outside every node")
            }
            BodyEditError::ChangesRendering { what } => {
                write!(f, "the edit would change how {what} renders")
            }
            BodyEditError::ChangesOutside { offset } => {
                write!(f, "the edit would change the text's reading at {offset}")
            }
            BodyEditError::ChangesDeclaration { key } => {
                write!(f, "the edit would change the file's #+{key} declarations")
            }
            BodyEditError::DuplicateId { id } => {
                write!(f, "the edit would repeat the id {id:?}")
            }
        }
    }
}

impl std::error::Error for BodyEditError {}

/// `text` with KB node `node`'s body (its `body_span`) replaced by
/// `new_body`, the O4 write path's splice (ADR-092 D3).
///
/// `new_body` is in the edit form [`node_source`] gives (line endings
/// `\n`). It is written in the file's own convention ([`with_line_ending`])
/// and, when not empty, always ends with a line ending, so it can never run
/// into the next headline or node; a body after a headline line that ended
/// the file without one gets that line ending first. Every byte outside
/// the body is kept.
///
/// Refused ([`BodyEditError`]) when a line of the new body would be a
/// headline, when the file's line endings would be read differently, and
/// when the result, parsed FRESH with the caller's `parse` and `semantics`
/// options (so every in-file declaration is read from the new text), would
/// differ in anything but this node's body: any other node (id, title,
/// TODO keyword, priority, tags, properties, aliases, kind, level; spans
/// moved by the edit), this node's own metadata, any link or transclusion
/// whose source is not this node, any file-level declaration
/// ([`DECLARATION_KEYWORDS`]), an `:ID:` the new body would repeat (in
/// either order), any name the body adds, removes or changes that the
/// rest of the file refers to or names itself (a footnote label, a target,
/// a radio target, a `#+NAME:`, a named `#+RESULTS:`, a `CUSTOM_ID` or
/// `ID`; one the body alone defines and uses is its own), any element,
/// object or token outside the body, or, last, how anything renders
/// outside the body: the whole page with the body left out, every node's
/// metadata (this node's included) and which footnote definitions outside
/// the body are drawn, under a resolver giving every KB target its own
/// href and title. One render per side, whatever the nodes. A new node
/// in the body is refused too: a body runs to the first node nested in it,
/// so the edited node's body would change.
///
/// `parse` and `semantics` must be the options `doc` and `node` came from:
/// the check compares the file before and after under them.
pub fn replace_body(
    text: &str,
    doc: &Document,
    node: &SemanticNode,
    new_body: &str,
    parse: &ParseOptions,
    semantics: &SemanticsOptions,
) -> Result<String, BodyEditError> {
    if doc.syntax().to_string() != text {
        return Err(BodyEditError::StaleDocument);
    }
    let (s, e) = (node.body_span.start as usize, node.body_span.end as usize);
    if s > e || e > text.len() || !text.is_char_boundary(s) || !text.is_char_boundary(e) {
        return Err(BodyEditError::NoSuchBody);
    }
    let eol = doc.line_ending();
    let end = match eol {
        LineEnding::Lf => "\n",
        LineEnding::CrLf => "\r\n",
        LineEnding::Cr => "\r",
    };
    let mut body = with_line_ending(new_body, eol);
    if !body.is_empty() && !body.ends_with(end) {
        body.push_str(end);
    }
    for (i, line) in super::build::lines(&body, eol).into_iter().enumerate() {
        let stars = line.len() - line.trim_start_matches('*').len();
        let rest = &line[stars..];
        if stars > 0 && (rest.is_empty() || rest.starts_with([' ', '\t', '\n', '\r'])) {
            return Err(BodyEditError::HeadlineLine { line: i });
        }
    }
    let before = &text[..s];
    let prefix = if !body.is_empty() && !before.is_empty() && !before.ends_with(end) {
        end
    } else {
        ""
    };
    let out = format!("{before}{prefix}{body}{}", &text[e..]);
    if LineEnding::detect(&out) != eol {
        return Err(BodyEditError::LineEnding);
    }
    let start = s + prefix.len();
    let edit = Edit {
        old: text,
        new: &out,
        edited: node,
        range: (s, e),
        new_range: (start, start + body.len()),
    };
    edit.check(parse, semantics)?;
    Ok(out)
}

/// One splice: `old[s..e]` became `new[ns..ne]`.
struct Edit<'a> {
    old: &'a str,
    new: &'a str,
    edited: &'a SemanticNode,
    range: (usize, usize),
    new_range: (usize, usize),
}

/// A document parsed fresh, and its semantics.
fn read(text: &str, parse: &ParseOptions, sem: &SemanticsOptions) -> (Document, Semantics) {
    let doc = crate::parse(text, parse);
    let s = crate::semantics_with(&doc, sem);
    (doc, s)
}

/// The document's declaration keywords, in order: `(KEY, value)`.
fn declarations(doc: &Document) -> Vec<(String, String)> {
    doc.keywords()
        .map(|k| (k.key().to_ascii_uppercase(), k.value()))
        .filter(|(k, _)| DECLARATION_KEYWORDS.contains(&k.as_str()))
        .collect()
}

impl Edit<'_> {
    /// Where position `p` of the old text is in the new; `None` inside the
    /// replaced range.
    fn moved(&self, p: u32) -> Option<u32> {
        let ((s, e), (_, ne)) = (self.range, self.new_range);
        let p = p as usize;
        if p >= e {
            u32::try_from(p as i64 + (ne as i64 - e as i64)).ok()
        } else if p <= s {
            Some(p as u32)
        } else {
            None
        }
    }

    fn moved_span(&self, sp: Span) -> Option<Span> {
        Some(Span {
            start: self.moved(sp.start)?,
            end: self.moved(sp.end)?,
        })
    }

    fn inside_new(&self, sp: Span) -> bool {
        let (ns, ne) = self.new_range;
        sp.start as usize >= ns && sp.end as usize <= ne
    }

    fn check(&self, parse: &ParseOptions, sem: &SemanticsOptions) -> Result<(), BodyEditError> {
        let (old_doc, before) = read(self.old, parse, sem);
        let (new_doc, after) = read(self.new, parse, sem);
        let (was, now) = (declarations(&old_doc), declarations(&new_doc));
        if was != now {
            let key = was
                .iter()
                .zip(&now)
                .find(|(a, b)| a != b)
                .map(|(a, b)| (&a.0).max(&b.0).clone())
                .or_else(|| {
                    was.get(now.len())
                        .or(now.get(was.len()))
                        .map(|d| d.0.clone())
                })
                .unwrap_or_default();
            return Err(BodyEditError::ChangesDeclaration { key });
        }
        for d in &after.diagnostics {
            if let SemanticDiagnostic::DuplicateId { id, span, first } = d {
                if self.inside_new(*span) || self.inside_new(*first) {
                    return Err(BodyEditError::DuplicateId { id: id.clone() });
                }
            }
        }
        self.check_names(&old_doc, &new_doc)?;
        self.check_nodes(&before, &after)?;
        self.check_links(&before, &after)?;
        self.check_outside(&old_doc, &new_doc)?;
        self.check_renderings((&old_doc, &before), (&new_doc, &after))
    }

    /// The names the rest of the file can resolve are the same in the old
    /// body and the new one ([`BodyEditError::ChangesName`]).
    fn check_names(&self, old: &Document, new: &Document) -> Result<(), BodyEditError> {
        let mut was = names_in(old, self.range, true);
        let mut now = names_in(new, self.new_range, true);
        was.sort();
        now.sort();
        // What the rest of the file refers to, or names itself, on either
        // side: a name the body adds, removes or changes must be none of it.
        let mut taken = refs_outside(old, self.range);
        taken.extend(refs_outside(new, self.new_range));
        let outside = names_in(old, self.range, false).into_iter().chain(names_in(
            new,
            self.new_range,
            false,
        ));
        taken.extend(outside.map(|(_, n)| crate::chars::fold(&n)));
        let changed = |n: &(String, String)| taken.contains(&crate::chars::fold(&n.1));
        let gone = was.iter().filter(|n| !now.contains(n));
        let new = now.iter().filter(|n| !was.contains(n));
        match gone.chain(new).find(|n| changed(n)) {
            Some((kind, name)) => Err(BodyEditError::ChangesName {
                kind: kind.clone(),
                name: name.clone(),
            }),
            None => Ok(()),
        }
    }

    /// The document renders the same outside the body, every node's
    /// metadata included, under a resolver that gives every KB target on
    /// either side its own href and title; and the footnote definitions
    /// rendered from outside the body are the same ones (a reference the
    /// body adds may publish a definition elsewhere). One render per side.
    fn check_renderings(
        &self,
        (old, before): (&Document, &Semantics),
        (new, after): (&Document, &Semantics),
    ) -> Result<(), BodyEditError> {
        let resolver = probing_resolver([before, after]);
        let (s, e) = self.range;
        let (ns, ne) = self.new_range;
        let was = super::build::rendering_without(old, before, &resolver, Span::new(s, e));
        let now = super::build::rendering_without(new, after, &resolver, Span::new(ns, ne));
        if was != now {
            return Err(BodyEditError::ChangesRendering {
                what: "the page".into(),
            });
        }
        let in_old = |sp: Span| sp.start as usize >= s && sp.end as usize <= e;
        let notes_was: Vec<_> = before
            .footnotes
            .iter()
            .filter(|f| !in_old(f.span))
            .map(|f| (f.label.clone(), self.moved_span(f.span)))
            .collect();
        let notes_now: Vec<_> = after
            .footnotes
            .iter()
            .filter(|f| !self.inside_new(f.span))
            .map(|f| (f.label.clone(), Some(f.span)))
            .collect();
        if notes_was != notes_now {
            return Err(BodyEditError::ChangesRendering {
                what: "the footnotes".into(),
            });
        }
        Ok(())
    }

    /// Every link and transclusion whose source is not the edited node (nor
    /// a node new in its body) is the same before and after, spans moved.
    fn check_links(&self, before: &Semantics, after: &Semantics) -> Result<(), BodyEditError> {
        let new_ids: std::collections::HashSet<&str> = after
            .nodes
            .iter()
            .filter(|m| !before.nodes.iter().any(|n| n.id == m.id))
            .map(|m| m.id.as_str())
            .collect();
        let others = |from: &Option<String>| {
            from.as_deref()
                .is_none_or(|f| f != self.edited.id && !new_ids.contains(f))
        };
        let changed = |from: &Option<String>| BodyEditError::ChangesLinks { id: from.clone() };
        // A link inside the body that another node owns (in a footnote
        // definition it references) has no span to compare: the body's
        // text moved it. Everything else about it must be the same.
        let (s, e) = self.range;
        let in_old = |sp: Span| sp.start as usize >= s && sp.end as usize <= e;
        let was: Vec<_> = before
            .links
            .iter()
            .filter(|l| others(&l.from))
            .map(|l| {
                let mut l = l.clone();
                l.span = if in_old(l.span) {
                    Span::default()
                } else {
                    self.moved_span(l.span).unwrap_or_default()
                };
                l
            })
            .collect();
        let now: Vec<_> = after
            .links
            .iter()
            .filter(|l| others(&l.from))
            .map(|l| {
                let mut l = l.clone();
                if self.inside_new(l.span) {
                    l.span = Span::default();
                }
                l
            })
            .collect();
        let now: Vec<_> = now.iter().collect();
        if was.len() != now.len() || was.iter().zip(&now).any(|(a, b)| a != *b) {
            let at = was.iter().zip(&now).find(|(a, b)| a != *b);
            let from = at.map(|(a, _)| a.from.clone());
            let from = from.or_else(|| was.get(now.len()).map(|l| l.from.clone()));
            let from = from.or_else(|| now.get(was.len()).map(|l| l.from.clone()));
            return Err(changed(&from.flatten()));
        }
        let mut was = Vec::new();
        for t in before.transclusions.iter().filter(|t| others(&t.from)) {
            let mut t = t.clone();
            t.span = self.moved_span(t.span).ok_or_else(|| changed(&t.from))?;
            was.push(t);
        }
        let now: Vec<_> = after
            .transclusions
            .iter()
            .filter(|t| others(&t.from))
            .collect();
        if was.len() != now.len() || was.iter().zip(&now).any(|(a, b)| a != *b) {
            let from = was
                .first()
                .or(now.first().copied())
                .and_then(|t| t.from.clone());
            return Err(changed(&from));
        }
        Ok(())
    }

    /// Every element, object and token wholly outside the body reads the
    /// same: same kind, same (moved) span, in the same order. "Outside" is
    /// before the splice point (not touching it: a line ending the splice
    /// adds there extends what ends at it) or after the replaced text.
    fn check_outside(&self, old: &Document, new: &Document) -> Result<(), BodyEditError> {
        let ((s, e), (_, ne)) = (self.range, self.new_range);
        let outside = |d: &Document, b: usize| {
            d.syntax()
                .descendants_with_tokens()
                .filter_map(move |el| {
                    let r = el.text_range();
                    let (x, y) = (usize::from(r.start()), usize::from(r.end()));
                    (y < s || x >= b).then_some((el.kind(), x, y))
                })
                .collect::<Vec<_>>()
        };
        let was = outside(old, e);
        let now = outside(new, ne);
        let shift = |p: usize| {
            if p >= e {
                (p as i64 + ne as i64 - e as i64) as usize
            } else {
                p
            }
        };
        let differs = was
            .iter()
            .map(|&(k, x, y)| (k, shift(x), shift(y)))
            .zip(now.iter().copied())
            .position(|(a, b)| a != b);
        match differs {
            None if was.len() == now.len() => Ok(()),
            at => {
                let i = at.unwrap_or(was.len().min(now.len()));
                let offset = now.get(i).map_or(self.new.len(), |n| n.1) as u32;
                Err(BodyEditError::ChangesOutside { offset })
            }
        }
    }

    /// Every node of `before` but the edited one is in `after` unchanged
    /// but for its spans, moved by the edit; the edited one keeps its
    /// metadata and its body is exactly the new range; a node that is new
    /// lies in the new body (and then, as it ends the edited node's body,
    /// the check above has already refused it).
    fn check_nodes(&self, before: &Semantics, after: &Semantics) -> Result<(), BodyEditError> {
        let (ns, ne) = self.new_range;
        let changed = |id: &str| BodyEditError::ChangesNode { id: id.to_string() };
        for n in &before.nodes {
            let Some(m) = after.nodes.iter().find(|m| m.id == n.id) else {
                return Err(changed(&n.id));
            };
            let mut want = n.clone();
            want.span = self.moved_span(n.span).ok_or_else(|| changed(&n.id))?;
            want.body_span = if n.id == self.edited.id {
                // Within the new range: blank lines at its start are the
                // drawer's post-blank to Org, so they may fall outside it.
                let b = m.body_span;
                let blank = |r: std::ops::Range<usize>| {
                    self.new
                        .get(r)
                        .is_some_and(|t| t.chars().all(char::is_whitespace))
                };
                let inside = b.start as usize >= ns && b.end as usize <= ne && b.start <= b.end;
                if !(inside && blank(ns..b.start as usize) && blank(b.end as usize..ne)) {
                    return Err(changed(&n.id));
                }
                b
            } else {
                self.moved_span(n.body_span).ok_or_else(|| changed(&n.id))?
            };
            if *m != want {
                return Err(changed(&n.id));
            }
        }
        for m in &after.nodes {
            let new_node = !before.nodes.iter().any(|n| n.id == m.id);
            if new_node && !self.inside_new(m.span) {
                return Err(changed(&m.id));
            }
        }
        Ok(())
    }
}

/// A resolver giving every KB target either semantics link to (or
/// transclude) its own href and title, derived from the target: under it,
/// a link whose target changes renders differently.
fn probing_resolver(sems: [&Semantics; 2]) -> super::Resolver {
    let mut r = super::Resolver::default();
    for sem in sems {
        let targets = sem.links.iter().map(|l| l.target.clone());
        let members = sem.transclusions.iter().map(|t| t.member.clone());
        for t in targets.chain(members) {
            let href = format!("/probe/{}", super::encode_id(&t));
            let resolved = super::Resolved::new(href).with_title(format!("probe {t}"));
            r.targets.entry(t).or_insert(resolved);
        }
    }
    r
}

/// What the text of `doc` outside `range` refers to by name, folded: the
/// labels its footnote references use, its links' targets (the path, and
/// what follows `*`, `#`, a `type:` or a `::`), the names its babel calls
/// call, the names its `#+RESULTS:` lines give.
fn refs_outside(doc: &Document, (a, b): (usize, usize)) -> HashSet<String> {
    use crate::kind::SyntaxKind::*;
    let mut out = HashSet::new();
    let mut add = |s: &str| {
        let s = s.trim();
        if !s.is_empty() {
            out.insert(crate::chars::fold(s));
        }
    };
    for n in doc.syntax().descendants() {
        let sp = Span::of(&n);
        if (sp.start as usize) >= a && sp.end as usize <= b {
            continue;
        }
        match n.kind() {
            FOOTNOTE_REFERENCE => {
                if let Some(l) = crate::semantics::regions::reference_label(&n) {
                    add(&l);
                }
            }
            LINK => {
                let path = crate::semantics::plain::link_path(&n).unwrap_or_default();
                add(&path);
                add(path.trim_start_matches(['*', '#']));
                if let Some((_, rest)) = path.split_once(':') {
                    add(rest.trim_start_matches(['*', '#']));
                }
                if let Some((_, rest)) = path.split_once("::") {
                    add(rest.trim_start_matches(['*', '#']));
                }
                if let Some((_, frag)) = path.split_once('#') {
                    add(frag);
                }
                // A radio link's text is the radio target it names.
                if let Some(l) = crate::views::Link::cast(n.clone()) {
                    if l.is_radio() {
                        add(&l.description_text().unwrap_or_default());
                    }
                }
            }
            BABEL_CALL | INLINE_BABEL_CALL => {
                if let Some(c) = super::babel::call_name(&n) {
                    add(&c);
                }
            }
            _ => {}
        }
        for (k, v) in super::babel::affiliated(&n) {
            if k == "RESULTS" {
                add(&v);
            }
        }
    }
    out
}

/// `(kind, name)` of every name in `doc` within `range` (`inside`), or
/// outside it, that the rest of the file can resolve (see
/// [`BodyEditError::ChangesName`]).
fn names_in(doc: &Document, (a, b): (usize, usize), inside: bool) -> Vec<(String, String)> {
    use crate::kind::SyntaxKind::*;
    let mut out = Vec::new();
    let token = |n: &crate::SyntaxNode, kind: crate::SyntaxKind| {
        n.children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == kind)
            .map(|t| t.text().to_string())
    };
    for n in doc.syntax().descendants() {
        let sp = Span::of(&n);
        let within = (sp.start as usize) >= a && sp.end as usize <= b;
        if within != inside {
            continue;
        }
        let mut add = |kind: &str, name: String| out.push((kind.to_string(), name));
        match n.kind() {
            FOOTNOTE_DEFINITION => {
                if let Some(l) = crate::semantics::regions::definition_label(&n) {
                    add("footnote", l);
                }
            }
            FOOTNOTE_REFERENCE => {
                if let Some(l) = crate::semantics::regions::inline_label(&n) {
                    add("footnote", l);
                }
            }
            TARGET => add("target", super::objects::inner_text(&n)),
            RADIO_TARGET => add("radio target", super::objects::inner_text(&n)),
            NODE_PROPERTY => {
                let key = token(&n, NAME).map(|k| crate::views::canonical_key(&k));
                if let Some(k @ ("CUSTOM_ID" | "ID")) = key.as_deref() {
                    let v = token(&n, VALUE).unwrap_or_default();
                    add(k, v.trim().to_string());
                }
            }
            // A list item's property drawer is a drawer to the tree.
            DRAWER => {
                for p in crate::semantics::items::item_property_drawer(&n).unwrap_or_default() {
                    let key = crate::views::canonical_key(&p.key);
                    if matches!(key.as_str(), "CUSTOM_ID" | "ID") {
                        add(&key, p.value.trim().to_string());
                    }
                }
            }
            _ => {}
        }
        for (k, v) in super::babel::affiliated(&n) {
            match k.as_str() {
                "NAME" => add("name", v),
                "RESULTS" if !v.is_empty() => add("results", v),
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review round 3: the rendering check rendered every node's page, so
    /// an edit of a file of 8,000 nodes took 12.8 s. Counted, not timed:
    /// one edit walks the document a fixed number of times (two passes
    /// for each side's page, one for each side's node metadata), whatever
    /// its number of nodes.
    #[test]
    fn an_edit_renders_each_side_once_whatever_the_nodes() {
        let walks = |nodes: usize| {
            let text: String = (0..nodes)
                .map(|i| format!("* N{i}\n:PROPERTIES:\n:ID: n{i}\n:END:\nbody {i} [[id:n0]]\n"))
                .collect();
            let doc = crate::parse(&text, &ParseOptions::default());
            let sem = crate::semantics(&doc);
            super::super::build::RUNS.with(|r| r.set(0));
            let r = replace_body(
                &text,
                &doc,
                &sem.nodes[0],
                "a new body\n",
                &ParseOptions::default(),
                &SemanticsOptions::default(),
            );
            assert!(r.is_ok(), "{r:?}");
            super::super::build::RUNS.with(std::cell::Cell::get)
        };
        let (few, many) = (walks(10), walks(400));
        assert_eq!(few, many, "walks grow with the nodes: {few} -> {many}");
        assert!(many <= 4, "{many} walks");
    }
}
