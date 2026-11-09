//! Split and Regen for one file: the `org-literal-v1` contract (see `mod.rs`).
//!
//! The parse is mae-org's (ADR-112): which lines are the file drawer, which are
//! keyword elements, and where the anchors are, is what Org says they are, not a
//! line heuristic. Lines are then removed whole, so the body is a byte-exact
//! subsequence of the file.

use std::ops::Range;

use mae_org::{parse, semantics, NodeOrigin, ParseOptions, PropertyDrawer, SemanticDiagnostic};

use super::{Problem, ProblemKind};

/// Org's in-file declarations: they change how every node of the KB parses, so
/// they belong to a per-KB declarations store, which does not exist yet.
const DECLARATION_KEYWORDS: &[&str] = &[
    "TODO",
    "SEQ_TODO",
    "TYP_TODO",
    "MACRO",
    "LINK",
    "PROPERTY",
    "SETUPFILE",
];

/// MAE's own header keywords (`shared/kb/src/org.rs`). Each sets a typed field,
/// so leaving one in the body would store its meaning twice (one source per fact).
const EXTENSION_KEYWORDS: &[&str] = &["KIND", "ALIASES", "TODO_STATE", "PRIORITY"];

/// A file split into the typed fields and the literal body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitFile {
    /// The file drawer's `:ID:`, exactly as Org reads it.
    pub id: String,
    /// Every file drawer property, in file order: the key as written, the value
    /// as Org reads it (trimmed). `ID` is included.
    pub properties: Vec<(String, String)>,
    /// `#+title:`'s value, if the file has the keyword.
    pub title: Option<String>,
    /// `#+filetags:`'s tags in order, if the file has the keyword.
    pub filetags: Option<Vec<String>>,
    /// The file with the lifted lines removed, byte for byte.
    pub body: String,
    /// The `:ID:`s of headlines and list items inside the body, plus any `:ID:`
    /// line in an unrendered region (conservative: it is text a link could
    /// still target, so it counts for duplicate detection).
    pub anchors: Vec<String>,
}

impl SplitFile {
    /// The node kind: the `:KIND:` property's value exactly as written, case
    /// included. Never mapped onto MAE's own kinds; a per-KB mapping is a
    /// separate design. `None` when the drawer has no `:KIND:`.
    pub fn kind(&self) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("KIND"))
            .map(|(_, v)| v.as_str())
    }
}

/// Split `text` into fields and body, or list every reason it cannot be.
pub fn split(text: &str) -> Result<SplitFile, Vec<Problem>> {
    let encoding = encoding_problems(text);
    if !encoding.is_empty() {
        return Err(encoding);
    }
    let doc = parse(text, &ParseOptions::default());
    if doc.syntax().to_string() != text {
        return Err(vec![Problem::new(
            ProblemKind::NotLossless,
            "mae-org does not print this file back byte for byte",
        )]);
    }
    let drawer = file_drawer(&doc, text).map_err(|p| vec![p])?;
    let mut problems = Vec::new();
    let properties = drawer_properties(&drawer.view, &mut problems);
    let id = file_id(&properties, &mut problems);
    let header = header_keywords(&doc, text, drawer.end, &mut problems);
    let anchors = anchors(&doc, &mut problems);
    if !problems.is_empty() {
        return Err(problems);
    }
    let lifted: Vec<Range<usize>> = std::iter::once(0..drawer.end)
        .chain(header.title.as_ref().map(|(_, r)| r.clone()))
        .chain(header.filetags.as_ref().map(|(_, r)| r.clone()))
        .collect();
    Ok(SplitFile {
        id: id.unwrap_or_default(),
        properties,
        title: header.title.map(|(t, _)| t),
        filetags: header.filetags.map(|(t, _)| t),
        body: remove_ranges(text, lifted),
        anchors,
    })
}

/// Write the fields back in canonical form, followed by the body.
pub fn regen(f: &SplitFile) -> String {
    let mut out = String::with_capacity(f.body.len() + 64 * (f.properties.len() + 3));
    out.push_str(":PROPERTIES:\n");
    for (key, value) in &f.properties {
        out.push(':');
        out.push_str(key);
        out.push(':');
        if !value.is_empty() {
            out.push(' ');
            out.push_str(value);
        }
        out.push('\n');
    }
    out.push_str(":END:\n");
    if let Some(title) = &f.title {
        keyword_line(&mut out, "title", title);
    }
    if let Some(tags) = &f.filetags {
        let value = if tags.is_empty() {
            String::new()
        } else {
            format!(":{}:", tags.join(":"))
        };
        keyword_line(&mut out, "filetags", &value);
    }
    out.push_str(&f.body);
    out
}

/// `N(F)`: the file as the exporter will write it back.
pub fn normalise(text: &str) -> Result<String, Vec<Problem>> {
    split(text).map(|f| regen(&f))
}

fn keyword_line(out: &mut String, key: &str, value: &str) {
    out.push_str("#+");
    out.push_str(key);
    out.push(':');
    if !value.is_empty() {
        out.push(' ');
        out.push_str(value);
    }
    out.push('\n');
}

/// LF-only UTF-8 without a BOM. A CR anywhere would make "byte for byte"
/// depend on a line-ending convention the contract does not carry.
fn encoding_problems(text: &str) -> Vec<Problem> {
    let mut out = Vec::new();
    if text.starts_with('\u{feff}') {
        out.push(Problem::new(
            ProblemKind::ByteOrderMark,
            "the file starts with a byte order mark",
        ));
    }
    if let Some(at) = text.find('\r') {
        let line = text[..at].matches('\n').count() + 1;
        out.push(Problem::new(
            ProblemKind::CarriageReturn,
            format!("a carriage return on line {line} (the contract is LF-only)"),
        ));
    }
    out
}

/// The file drawer and the byte just past its `:END:` line. Trailing blank
/// lines that Org counts inside the element stay in the body.
struct FileDrawer {
    view: PropertyDrawer,
    end: usize,
}

fn file_drawer(doc: &mae_org::Document, text: &str) -> Result<FileDrawer, Problem> {
    let no_drawer = || {
        Problem::new(
            ProblemKind::NoFileDrawer,
            "the file does not start with a :PROPERTIES: drawer on its first line",
        )
    };
    let view = doc.property_drawer().ok_or_else(no_drawer)?;
    let range = view.syntax().text_range();
    if usize::from(range.start()) != 0 {
        return Err(no_drawer());
    }
    let region = &text[..usize::from(range.end())];
    let lines: Vec<(usize, &str)> = lines_with_offsets(region)
        .filter(|(_, l)| !l.trim().is_empty())
        .collect();
    let unrepresentable = |why: &str| Problem::new(ProblemKind::UnrepresentableDrawer, why);
    let Some(&(end_at, end_line)) = lines.last() else {
        return Err(no_drawer());
    };
    if !end_line.trim().eq_ignore_ascii_case(":END:") {
        return Err(unrepresentable("the drawer's last line is not :END:"));
    }
    let end = end_at + end_line.len();
    let line_count = lines_with_offsets(&text[..end]).count();
    if line_count != view.properties().len() + 2 {
        return Err(unrepresentable(
            "the drawer holds a line that is not a property (a blank line or a comment), \
             which the typed fields cannot carry",
        ));
    }
    Ok(FileDrawer { view, end })
}

fn drawer_properties(view: &PropertyDrawer, problems: &mut Vec<Problem>) -> Vec<(String, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for p in view.properties() {
        let written = p.key_as_written();
        if p.is_append() {
            problems.push(Problem::new(
                ProblemKind::AppendProperty,
                format!(":{written}: appends to a property; its meaning depends on order"),
            ));
        } else if !seen.insert(p.key.clone()) {
            problems.push(Problem::new(
                ProblemKind::DuplicateProperty,
                format!(":{written}: appears more than once in the file drawer"),
            ));
        }
        out.push((written, p.value));
    }
    out
}

fn file_id(properties: &[(String, String)], problems: &mut Vec<Problem>) -> Option<String> {
    let ids: Vec<&String> = properties
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("ID"))
        .map(|(_, v)| v)
        .collect();
    let id = match ids.as_slice() {
        [] => {
            problems.push(Problem::new(
                ProblemKind::MissingId,
                "the file drawer has no :ID:",
            ));
            return None;
        }
        [one] => (*one).clone(),
        _ => {
            problems.push(Problem::new(
                ProblemKind::MultipleIds,
                "the file drawer has more than one :ID:",
            ));
            return None;
        }
    };
    if id.is_empty() {
        problems.push(Problem::new(ProblemKind::EmptyId, "the :ID: is empty"));
    } else if id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        problems.push(Problem::new(
            ProblemKind::InvalidId,
            format!("the :ID: {id:?} contains whitespace or a control character"),
        ));
    }
    Some(id)
}

/// The lifted keyword lines: each value with the whole line's byte range.
#[derive(Default)]
struct Header {
    title: Option<(String, Range<usize>)>,
    filetags: Option<(Vec<String>, Range<usize>)>,
}

fn header_keywords(
    doc: &mae_org::Document,
    text: &str,
    drawer_end: usize,
    problems: &mut Vec<Problem>,
) -> Header {
    let keywords: Vec<(String, String, Range<usize>)> = doc
        .keywords()
        .map(|k| {
            let start = usize::from(k.syntax().text_range().start());
            (
                k.key().to_ascii_uppercase(),
                k.value(),
                line_at(text, start),
            )
        })
        .collect();
    let zone_end = header_zone_end(text, drawer_end, &keywords);
    let mut header = Header::default();
    for (key, value, line) in keywords {
        classify_keyword(
            &key,
            value,
            line,
            drawer_end..zone_end,
            &mut header,
            problems,
        );
    }
    header
}

/// The header block after the drawer: the run of blank lines and keyword lines.
/// `#+title:` and `#+filetags:` must sit in it.
fn header_zone_end(
    text: &str,
    drawer_end: usize,
    keywords: &[(String, String, Range<usize>)],
) -> usize {
    let starts: std::collections::HashSet<usize> =
        keywords.iter().map(|(_, _, r)| r.start).collect();
    lines_with_offsets(&text[drawer_end..])
        .map(|(at, line)| (drawer_end + at, line))
        .find(|(at, line)| !line.trim().is_empty() && !starts.contains(at))
        .map_or(text.len(), |(at, _)| at)
}

fn classify_keyword(
    key: &str,
    value: String,
    line: Range<usize>,
    zone: Range<usize>,
    header: &mut Header,
    problems: &mut Vec<Problem>,
) {
    if DECLARATION_KEYWORDS.contains(&key) {
        problems.push(Problem::new(
            ProblemKind::Declaration,
            format!("#+{key}: is a declaration; per-KB declarations are not supported yet"),
        ));
        return;
    }
    if EXTENSION_KEYWORDS.contains(&key) {
        problems.push(Problem::new(
            ProblemKind::ExtensionKeyword,
            format!("#+{key}: is a MAE header keyword the contract does not lift"),
        ));
        return;
    }
    if key != "TITLE" && key != "FILETAGS" {
        return;
    }
    if !zone.contains(&line.start) {
        problems.push(Problem::new(
            ProblemKind::MisplacedHeaderKeyword,
            format!("#+{key}: is not in the header block right after the file drawer"),
        ));
    } else if key == "TITLE" {
        set_once(
            &mut header.title,
            (value, line),
            ProblemKind::MultipleTitle,
            problems,
        );
    } else {
        match parse_filetags(&value) {
            Ok(tags) => set_once(
                &mut header.filetags,
                (tags, line),
                ProblemKind::MultipleFiletags,
                problems,
            ),
            Err(p) => problems.push(p),
        }
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, kind: ProblemKind, problems: &mut Vec<Problem>) {
    if slot.is_some() {
        problems.push(Problem::new(kind, "the keyword appears more than once"));
    } else {
        *slot = Some(value);
    }
}

/// `:a:b:` → `["a", "b"]`; empty → `[]`. Anything else (space-separated tags,
/// an empty tag, a tag with a blank) would not regenerate as written: refused.
fn parse_filetags(value: &str) -> Result<Vec<String>, Problem> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let bad = || {
        Problem::new(
            ProblemKind::BadFiletags,
            format!("#+filetags: {value:?} is not of the form :tag1:tag2:"),
        )
    };
    let inner = value
        .strip_prefix(':')
        .and_then(|v| v.strip_suffix(':'))
        .filter(|v| !v.is_empty())
        .ok_or_else(bad)?;
    let tags: Vec<String> = inner.split(':').map(str::to_string).collect();
    if tags
        .iter()
        .any(|t| t.is_empty() || t.chars().any(char::is_whitespace))
    {
        return Err(bad());
    }
    Ok(tags)
}

/// Every id inside the body: headline and item nodes, ids in unrendered
/// regions; a second element with an id already used in the file is refused.
fn anchors(doc: &mae_org::Document, problems: &mut Vec<Problem>) -> Vec<String> {
    let sem = semantics(doc);
    let mut out: Vec<String> = sem
        .nodes
        .iter()
        .filter(|n| n.origin != NodeOrigin::File)
        .map(|n| n.id.clone())
        .collect();
    for d in &sem.diagnostics {
        match d {
            SemanticDiagnostic::IdInUnrenderedRegion { id, .. } => out.push(id.clone()),
            SemanticDiagnostic::DuplicateId { id, .. } => problems.push(Problem::new(
                ProblemKind::DuplicateId,
                format!("the id {id:?} is used twice in this file"),
            )),
            _ => {}
        }
    }
    out
}

/// The whole line (with its `\n`) containing byte `at`.
fn line_at(text: &str, at: usize) -> Range<usize> {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i + 1);
    start..end
}

fn lines_with_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.split_inclusive('\n').scan(0, |at, line| {
        let start = *at;
        *at += line.len();
        Some((start, line))
    })
}

/// `text` without `ranges` (non-overlapping), concatenated in order.
fn remove_ranges(text: &str, mut ranges: Vec<Range<usize>>) -> String {
    ranges.sort_by_key(|r| r.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for r in ranges {
        out.push_str(&text[at..r.start]);
        at = r.end;
    }
    out.push_str(&text[at..]);
    out
}
