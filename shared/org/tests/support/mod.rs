//! Shared helpers for mae-org's integration tests: corpus discovery, the
//! element extraction the oracle diff compares, and variant generation.
#![allow(dead_code)]

use mae_org::{parse, Document, ParseOptions, SyntaxKind, SyntaxKind::*};
use std::path::{Path, PathBuf};

/// The repository root (two levels above this crate).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root exists")
}

/// Repo-relative path with forward slashes.
pub fn rel(path: &Path) -> String {
    let root = repo_root();
    path.strip_prefix(&root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every `.org` file under `dir`, sorted, walked at test time.
pub fn org_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "org") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// The bundled corpus: every `.org` under the repo's `assets/` and `tests/`
/// that git TRACKS (`git ls-files`, as `gen-semantics-titles.sh` selects
/// it). An untracked file in a checkout (`:kb-register` writes
/// `tests/fixtures/kb/collabtest/*-instance.org`, a contributor's scratch
/// note) must change neither the corpus nor any count.
pub fn bundled_corpus() -> Vec<PathBuf> {
    let root = repo_root();
    tracked_org_files(&root, &["assets/*.org", "tests/*.org"]).unwrap_or_else(|| {
        // No git (a source tarball): every file there is the release's.
        let mut v = org_files(&root.join("assets"));
        v.extend(org_files(&root.join("tests")));
        v
    })
}

/// The `.org` files git tracks under `root` matching `pathspecs`, sorted;
/// `None` when git cannot list them (not installed, not a repository).
pub fn tracked_org_files(root: &Path, pathspecs: &[&str]) -> Option<Vec<PathBuf>> {
    let out = std::process::Command::new("git")
        .args(["ls-files", "-z", "--"])
        .args(pathspecs)
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let mut v: Vec<PathBuf> = out
        .stdout
        .split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| root.join(String::from_utf8_lossy(p).as_ref()))
        .collect();
    v.sort();
    Some(v)
}

/// The synthetic adversarial fixtures of this crate.
pub fn fixtures() -> Vec<PathBuf> {
    org_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"))
}

pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `text` with every LF that is not already part of a CRLF turned into CRLF.
pub fn to_crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 16);
    let mut prev = '\0';
    for c in text.chars() {
        if c == '\n' && prev != '\r' {
            out.push('\r');
        }
        out.push(c);
        prev = c;
    }
    out
}

/// Maps an element from an LF text onto the same text with CRLF line
/// endings: each LF before an offset adds one byte. An offset pointing AT an
/// LF (a table row's contents end) stays before the CR, as in Emacs.
pub fn lf_to_crlf(lf_text: &str, el: &El) -> El {
    let shift = |o: usize| {
        o + lf_text.as_bytes()[..o.min(lf_text.len())]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
    };
    shift_el(el, shift)
}

/// `el` with every offset passed through `f`.
pub fn shift_el(el: &El, f: impl Fn(usize) -> usize) -> El {
    El {
        kind: el.kind.clone(),
        begin: f(el.begin),
        end: f(el.end),
        contents: el.contents.map(|(a, b)| (f(a), f(b))),
        parent: (
            el.parent.0.clone(),
            // The document and a secondary string (a CAPTION value) have no
            // position: their "begin" is always 0.
            if el.parent.0 == "org-data" || el.parent.0 == "secondary" {
                0
            } else {
                f(el.parent.1)
            },
        ),
    }
}

/// The explicit mapping from mae-org node kinds to `org-element` types.
/// `DOCUMENT` (org-data) is not emitted by the oracle; line nodes and
/// affiliated keywords are parts of elements, not elements.
pub const KIND_MAP: &[(SyntaxKind, &str)] = &[
    (SECTION, "section"),
    (HEADLINE, "headline"),
    (PLANNING, "planning"),
    (PROPERTY_DRAWER, "property-drawer"),
    (NODE_PROPERTY, "node-property"),
    (DRAWER, "drawer"),
    (CENTER_BLOCK, "center-block"),
    (QUOTE_BLOCK, "quote-block"),
    (SPECIAL_BLOCK, "special-block"),
    (SRC_BLOCK, "src-block"),
    (EXAMPLE_BLOCK, "example-block"),
    (EXPORT_BLOCK, "export-block"),
    (COMMENT_BLOCK, "comment-block"),
    (VERSE_BLOCK, "verse-block"),
    (DYNAMIC_BLOCK, "dynamic-block"),
    (KEYWORD, "keyword"),
    (BABEL_CALL, "babel-call"),
    (COMMENT, "comment"),
    (FIXED_WIDTH, "fixed-width"),
    (HORIZONTAL_RULE, "horizontal-rule"),
    (FOOTNOTE_DEFINITION, "footnote-definition"),
    (PARAGRAPH, "paragraph"),
    (PLAIN_LIST, "plain-list"),
    (ITEM, "item"),
    (TABLE, "table"),
    (TABLE_ROW, "table-row"),
    (CLOCK, "clock"),
    (DIARY_SEXP, "diary-sexp"),
    (LATEX_ENVIRONMENT, "latex-environment"),
    (BOLD, "bold"),
    (CITATION, "citation"),
    (CITATION_REFERENCE, "citation-reference"),
    (CODE, "code"),
    (ENTITY, "entity"),
    (EXPORT_SNIPPET, "export-snippet"),
    (FOOTNOTE_REFERENCE, "footnote-reference"),
    (INLINE_BABEL_CALL, "inline-babel-call"),
    (INLINE_SRC_BLOCK, "inline-src-block"),
    (ITALIC, "italic"),
    (LINE_BREAK, "line-break"),
    (LATEX_FRAGMENT, "latex-fragment"),
    (LINK, "link"),
    (MACRO, "macro"),
    (RADIO_TARGET, "radio-target"),
    (STATISTICS_COOKIE, "statistics-cookie"),
    (STRIKE_THROUGH, "strike-through"),
    (SUBSCRIPT, "subscript"),
    (SUPERSCRIPT, "superscript"),
    (TABLE_CELL, "table-cell"),
    (TARGET, "target"),
    (TIMESTAMP, "timestamp"),
    (UNDERLINE, "underline"),
    (VERBATIM, "verbatim"),
];

/// One element as the differential compares it: its org-element type, byte
/// span, contents range, and its parent element's type and begin. Contents
/// and parent matter as much as the span: O4's span-based write path splices
/// contents, and a correct span under the wrong parent is a wrong tree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct El {
    pub kind: String,
    pub begin: usize,
    pub end: usize,
    pub contents: Option<(usize, usize)>,
    pub parent: (String, usize),
}

fn kind_name(kind: SyntaxKind) -> Option<&'static str> {
    KIND_MAP.iter().find(|(k, _)| *k == kind).map(|(_, s)| *s)
}

/// mae-org's elements, in document order, mapped through [`KIND_MAP`].
pub fn mae_elements(doc: &Document) -> Vec<El> {
    doc.syntax()
        .descendants()
        .filter_map(|n| {
            let name = kind_name(n.kind())?;
            let r = n.text_range();
            let parent = n
                .ancestors()
                .skip(1)
                .find_map(|p| {
                    // A CAPTION value is a secondary string with no owner in Org.
                    if p.kind() == AFFILIATED_VALUE {
                        return Some(("secondary".to_string(), 0));
                    }
                    kind_name(p.kind())
                        .map(|k| (k.to_string(), usize::from(p.text_range().start())))
                })
                .unwrap_or_else(|| ("org-data".to_string(), 0));
            Some(El {
                kind: name.to_string(),
                begin: r.start().into(),
                end: r.end().into(),
                contents: doc.contents(&n).map(|c| (c.start as usize, c.end as usize)),
                parent,
            })
        })
        .collect()
}

pub fn parse_default(text: &str) -> Document {
    parse(text, &ParseOptions::default())
}

/// Checks the oracle's self-check line against the text it claims to have
/// parsed: its byte count, and that `org-mode` left the buffer unmodified.
pub fn check_golden_header(jsonl: &str, text: &str) -> Result<(), String> {
    let first = jsonl.lines().next().ok_or("empty golden")?;
    let v: serde_json::Value = serde_json::from_str(first).map_err(|e| e.to_string())?;
    if is_nonterminating(jsonl) {
        // Emacs hit the runner's memory or time cap: there is no oracle
        // parse, so every mae-org element differs and must be registered.
        return Ok(());
    }
    if v["type"] != "#file" {
        return Err("golden has no #file self-check line".into());
    }
    if v["bytes"].as_u64() != Some(text.len() as u64) {
        return Err(format!(
            "oracle saw {} bytes, file has {}",
            v["bytes"],
            text.len()
        ));
    }
    if v["unmodified"] != true {
        return Err("org-mode modified the buffer before parsing".into());
    }
    // Emacs's own EOL detection must agree with mae-org's: the oracle's
    // offsets are mapped through it, and so is every line mae-org sees.
    let mine = match mae_org::LineEnding::detect(text) {
        mae_org::LineEnding::Lf => "unix",
        mae_org::LineEnding::CrLf => "dos",
        mae_org::LineEnding::Cr => "mac",
    };
    if v["eol"] != mine {
        return Err(format!(
            "Emacs decoded the file as {}, mae-org as {mine}",
            v["eol"]
        ));
    }
    Ok(())
}

/// The golden of a file on which Emacs did not terminate under the runner's
/// caps (`conformance/regen.sh`, `tests/oracle_external.rs`).
pub fn is_nonterminating(jsonl: &str) -> bool {
    jsonl.starts_with("{\"type\":\"#nonterminating\"")
}

/// Why a golden holds no oracle parse, if it holds none: Emacs did not
/// terminate under the caps, or signalled an error on the file (e.g. "Not
/// an Org time string"). Such a file's differences are every mae-org
/// element, and the conformance gate requires the file to be registered.
pub fn oracle_failure(jsonl: &str) -> Option<&'static str> {
    if is_nonterminating(jsonl) {
        Some("Emacs did not terminate")
    } else if jsonl.starts_with("{\"type\":\"#error\"") {
        Some("Emacs signalled an error")
    } else {
        None
    }
}

/// What one bounded Emacs run ended with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmacsExit {
    Ok,
    /// Emacs reported memory exhausted (it then exits 3 from
    /// `org-element-dump.el`, or 1 or 255 when the exhaustion strikes outside
    /// it: measured, the status is not reliable, the message is).
    OutOfMemory,
    /// Killed at the wall-clock limit.
    TimedOut,
    /// Killed by a signal, or another nonzero status.
    Failed(Option<i32>),
}

/// Runs `emacs ARGS` under a virtual-memory cap (`ulimit -v`, through `sh`,
/// so no `unsafe` pre-exec hook) and a wall-clock limit, with stdout to
/// `stdout`. On some inputs org-element never terminates, allocating without
/// bound (`<<<x>>>\n\nx[[x]]`), so the oracle must never run unbounded.
pub fn run_emacs_bounded(
    args: &[std::ffi::OsString],
    vmem_kb: u64,
    limit: std::time::Duration,
    stdout: std::process::Stdio,
) -> EmacsExit {
    let err = std::env::temp_dir().join(format!(
        "mae-org-emacs-{}-{:?}.err",
        std::process::id(),
        std::thread::current().id()
    ));
    let status = run_bounded_raw(args, vmem_kb, limit, stdout, &err);
    let log = std::fs::read_to_string(&err).unwrap_or_default();
    let _ = std::fs::remove_file(&err);
    match status {
        EmacsExit::Failed(_) if log.contains("Memory exhausted") => EmacsExit::OutOfMemory,
        s => s,
    }
}

fn run_bounded_raw(
    args: &[std::ffi::OsString],
    vmem_kb: u64,
    limit: std::time::Duration,
    stdout: std::process::Stdio,
    err: &Path,
) -> EmacsExit {
    use std::process::{Command, Stdio};
    let stderr = std::fs::File::create(err).expect("stderr file");
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(
            "kb=$1; shift; ulimit -v \"$kb\" 2>/dev/null \
             || echo 'oracle: no ulimit -v here; Emacs has no memory cap' >&2; \
             exec emacs \"$@\"",
        )
        .arg("sh")
        .arg(vmem_kb.to_string())
        .args(args)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("sh runs");
    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait().expect("wait for emacs") {
            Some(s) if s.success() => return EmacsExit::Ok,
            Some(s) => return EmacsExit::Failed(s.code()),
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return EmacsExit::TimedOut;
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

/// The oracle's elements from a golden (JSON lines), without the self-check.
pub fn golden_elements(jsonl: &str) -> Vec<(El, serde_json::Value)> {
    jsonl
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with("{\"type\":\"#"))
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("golden line is JSON");
            let num = |k: &str| v[k].as_u64().map(|n| n as usize);
            let contents = num("contents_begin").zip(num("contents_end"));
            let el = El {
                kind: v["type"].as_str().expect("type").to_string(),
                begin: num("begin").expect("begin"),
                end: num("end").expect("end"),
                contents,
                parent: (
                    v["parent_type"].as_str().expect("parent_type").to_string(),
                    num("parent_begin").expect("parent_begin"),
                ),
            };
            (el, v)
        })
        .collect()
}

/// Multiset difference: elements only in `a`, and only in `b`.
pub fn diff(a: &[El], b: &[El]) -> (Vec<El>, Vec<El>) {
    let mut a_sorted = a.to_vec();
    let mut b_sorted = b.to_vec();
    a_sorted.sort();
    b_sorted.sort();
    let (mut only_a, mut only_b) = (Vec::new(), Vec::new());
    let (mut i, mut j) = (0, 0);
    while i < a_sorted.len() || j < b_sorted.len() {
        match (a_sorted.get(i), b_sorted.get(j)) {
            (Some(x), Some(y)) if x == y => {
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) if x < y => {
                only_a.push(x.clone());
                i += 1;
            }
            (Some(_), Some(y)) => {
                only_b.push(y.clone());
                j += 1;
            }
            (Some(x), None) => {
                only_a.push(x.clone());
                i += 1;
            }
            (None, Some(y)) => {
                only_b.push(y.clone());
                j += 1;
            }
            (None, None) => break,
        }
    }
    (only_a, only_b)
}

/// Structural invariants every tree must satisfy, whatever the input:
/// lossless print; no empty tokens or nodes; children tile their parent in
/// order; element kinds only where Org allows them; nested headlines deeper
/// than their parent; depth bounded by the nesting cap. Returns the first
/// violation.
pub fn check_tree(doc: &Document, text: &str, max_depth: usize) -> Result<(), String> {
    let root = doc.syntax();
    if root.to_string() != text {
        return Err("tree does not print back to the input".into());
    }
    if usize::from(root.text_range().end()) != text.len() {
        return Err("root span does not cover the input".into());
    }
    for node in root.descendants() {
        check_node(&node)?;
        let depth = node.ancestors().count();
        // Each container frame adds at most one level, plus the BLOCK_BEGIN /
        // AFFILIATED_KEYWORD line nodes inside the deepest element.
        if depth > max_depth + 2 {
            return Err(format!(
                "depth {depth} exceeds the cap {max_depth} at {:?}",
                node.text_range()
            ));
        }
    }
    Ok(())
}

fn check_node(node: &mae_org::SyntaxNode) -> Result<(), String> {
    let r = node.text_range();
    if r.is_empty() && node.kind() != DOCUMENT {
        return Err(format!("empty {:?} node at {r:?}", node.kind()));
    }
    let mut at = r.start();
    for child in node.children_with_tokens() {
        let cr = child.text_range();
        if cr.start() != at || cr.is_empty() {
            return Err(format!(
                "child {:?} at {cr:?} does not tile {:?} at {r:?}",
                child.kind(),
                node.kind()
            ));
        }
        at = cr.end();
    }
    if at != r.end() {
        return Err(format!(
            "children of {:?} at {r:?} stop at {at:?}",
            node.kind()
        ));
    }
    check_parent(node)
}

/// Objects (inline) are numbered from 2000.
pub fn is_object(kind: SyntaxKind) -> bool {
    kind as u16 >= 2000
}

/// Where each element kind may appear.
fn check_parent(node: &mae_org::SyntaxNode) -> Result<(), String> {
    let kind = node.kind();
    let parent = node.parent().map(|p| p.kind());
    let ok = match kind {
        DOCUMENT => parent.is_none(),
        HEADLINE => matches!(parent, Some(DOCUMENT | HEADLINE)),
        SECTION => matches!(parent, Some(DOCUMENT | HEADLINE)),
        ITEM => parent == Some(PLAIN_LIST),
        NODE_PROPERTY => parent == Some(PROPERTY_DRAWER),
        TABLE_ROW => parent == Some(TABLE),
        PLANNING => parent == Some(SECTION),
        BLOCK_BEGIN | BLOCK_END | AFFILIATED_KEYWORD => {
            parent.is_some_and(|p| p != DOCUMENT && p != SECTION)
        }
        HEADLINE_TITLE => parent == Some(HEADLINE),
        ITEM_TAG_CONTENT => parent == Some(ITEM),
        AFFILIATED_VALUE => parent == Some(AFFILIATED_KEYWORD),
        TABLE_CELL => parent == Some(TABLE_ROW),
        CITATION_REFERENCE => parent == Some(CITATION),
        // Any other object sits in an object container or another object.
        k if is_object(k) => parent.is_some_and(|p| {
            is_object(p)
                || matches!(
                    p,
                    PARAGRAPH | VERSE_BLOCK | HEADLINE_TITLE | ITEM_TAG_CONTENT | AFFILIATED_VALUE
                )
        }),
        _ => !matches!(
            parent,
            None | Some(HEADLINE | PARAGRAPH | PLAIN_LIST | TABLE | PROPERTY_DRAWER)
        ),
    };
    if !ok {
        return Err(format!(
            "{kind:?} inside {parent:?} at {:?}",
            node.text_range()
        ));
    }
    if kind == HEADLINE {
        let level =
            |n: &mae_org::SyntaxNode| mae_org::Headline::cast(n.clone()).map_or(0, |h| h.level());
        if let Some(p) = node.parent().filter(|p| p.kind() == HEADLINE) {
            if level(node) <= level(&p) {
                return Err(format!(
                    "headline at {:?} is not deeper than its parent",
                    node.text_range()
                ));
            }
        }
    }
    Ok(())
}

/// Tokens of kind `UNPARSED`: never expected (the builder emits them only
/// if a recogniser returns an element that starts past the cursor).
pub fn unparsed_count(doc: &Document) -> usize {
    doc.syntax()
        .descendants_with_tokens()
        .filter(|e| e.kind() == UNPARSED)
        .count()
}

fn opt_str(v: &serde_json::Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// Compares the typed views with the oracle's properties for every element
/// of a golden whose span mae-org reproduces (span differences are the
/// differential's job). Returns how many of each view were compared, and a
/// description of every field that differs.
pub fn compare_views(
    doc: &Document,
    golden: &str,
) -> (std::collections::BTreeMap<String, usize>, Vec<String>) {
    let mut counts = std::collections::BTreeMap::new();
    let mut bad = Vec::new();
    // One pass over the tree, not one per golden element.
    let mut by_span = std::collections::HashMap::new();
    for n in doc.syntax().descendants() {
        if let Some(name) = kind_name(n.kind()) {
            let r = n.text_range();
            by_span
                .entry((usize::from(r.start()), usize::from(r.end()), name))
                .or_insert(n);
        }
    }
    let abbrevs = mae_org::LinkAbbrevs::of(&doc.syntax());
    for (el, v) in golden_elements(golden) {
        let Some(node) = by_span.get(&(el.begin, el.end, el.kind.as_str())).cloned() else {
            continue;
        };
        let Some(fields) = view_fields(&el.kind, node, &abbrevs) else {
            continue;
        };
        let group = match el.kind.as_str() {
            "headline" | "keyword" | "node-property" | "link" | "entity" | "macro"
            | "inline-src-block" | "export-snippet" | "timestamp" => el.kind.clone(),
            _ => "drawer/block".to_string(),
        };
        *counts.entry(group).or_default() += 1;
        for (field, mine) in fields {
            let want = &v[field];
            let want = if field == "commentedp" && want.is_null() {
                serde_json::json!(false)
            } else {
                want.clone()
            };
            if want != mine {
                bad.push(format!(
                    "{} @{} {field}: oracle {want} mae {mine}",
                    el.kind, el.begin
                ));
            }
        }
    }
    (counts, bad)
}

/// The text of a node's direct token of `kind` (several are joined, as a
/// value split over lines is), line endings read as `\n`.
fn token_text(node: &mae_org::SyntaxNode, kind: SyntaxKind) -> Option<String> {
    let toks: Vec<_> = node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .collect();
    let first = toks.iter().position(|t| t.kind() == kind)?;
    let last = toks.iter().rposition(|t| t.kind() == kind)?;
    Some(
        toks[first..=last]
            .iter()
            .map(|t| {
                if t.kind() == NEWLINE {
                    "\n".to_string()
                } else {
                    t.text().to_string()
                }
            })
            .collect(),
    )
}

/// The text after the first MARKUP token reading `open` and before the
/// last MARKUP token, line endings read as `\n`.
fn between(node: &mae_org::SyntaxNode, open: &str) -> String {
    let toks: Vec<_> = node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .collect();
    let markup = |t: &mae_org::SyntaxToken| t.kind() == MARKUP;
    let Some(a) = toks.iter().position(|t| markup(t) && t.text() == open) else {
        return String::new();
    };
    let b = toks.iter().rposition(markup).unwrap_or(toks.len());
    toks.get(a + 1..b)
        .unwrap_or_default()
        .iter()
        .map(|t| {
            if t.kind() == NEWLINE {
                "\n".to_string()
            } else {
                t.text().to_string()
            }
        })
        .collect()
}

/// `org-element-timestamp-parser`'s `:type` and `:range-type` from the raw
/// value: diary if it opens `<%%`; a range if a second stamp follows `--`
/// (daterange) or the first holds `H:MM-H:MM` (timerange); active if it
/// opens with `<`. Independent of mae-org's parser, so a wrong extent shows.
fn timestamp_type(raw: &str) -> (&'static str, Option<&'static str>) {
    let diary = raw.starts_with("<%%");
    let close = raw[1..].find([']', '>']).map_or(raw.len(), |k| k + 1);
    let date_end = raw[close..].starts_with("]--") || raw[close..].starts_with(">--");
    // A diary's time is read after its sexp: the text after the last `)`.
    let start = if diary {
        &raw[raw[..close].rfind(')').map_or(close, |k| k + 1)..close]
    } else {
        &raw[..close]
    };
    let time_range = has_time_range(start);
    let active = raw.starts_with('<');
    let ty = match (diary, active, date_end || time_range) {
        (true, _, _) => "diary",
        (_, true, true) => "active-range",
        (_, true, false) => "active",
        (_, false, true) => "inactive-range",
        (_, false, false) => "inactive",
    };
    let range = if date_end {
        Some("daterange")
    } else if time_range {
        Some("timerange")
    } else {
        None
    };
    (ty, range)
}

/// `[012]?[0-9]:[0-5][0-9]-[012]?[0-9]:[0-5][0-9]` somewhere in `s`.
fn has_time_range(s: &str) -> bool {
    let b = s.as_bytes();
    // `H:MM` or `HH:MM` ending at `k` (exclusive), `[012]?` optional.
    let clock_at = |i: usize| -> Option<usize> {
        let d = |k: usize| b.get(k).is_some_and(u8::is_ascii_digit);
        let start = if d(i) && d(i + 1) && b.get(i + 2) == Some(&b':') {
            (matches!(b[i], b'0'..=b'2')).then_some(i + 2)?
        } else if d(i) && b.get(i + 1) == Some(&b':') {
            i + 1
        } else {
            return None;
        };
        let m = start + 1;
        (b.get(m).is_some_and(|c| matches!(c, b'0'..=b'5')) && d(m + 1)).then_some(m + 2)
    };
    (0..b.len()).any(|i| {
        clock_at(i)
            .filter(|&e| b.get(e) == Some(&b'-'))
            .and_then(|e| clock_at(e + 1))
            .is_some()
    })
}

/// The oracle's field names and mae-org's values for them, per view.
fn view_fields(
    kind: &str,
    node: mae_org::SyntaxNode,
    abbrevs: &mae_org::LinkAbbrevs,
) -> Option<Vec<(&'static str, serde_json::Value)>> {
    use mae_org::{Block, Headline, Keyword, Link, ParseOptions, PropertyDrawer};
    use serde_json::json;
    Some(match kind {
        "headline" => {
            let h = Headline::cast(node)?;
            vec![
                // `level` is reduced under `#+STARTUP: odd`; mae-org's level is
                // the star count, Org's `:true-level`.
                ("true_level", json!(h.level())),
                ("raw_title", json!(h.title())),
                ("todo", json!(h.todo())),
                ("priority", json!(h.priority().map(String::from))),
                ("tags", json!(h.tags())),
                ("commentedp", json!(h.is_commented())),
            ]
        }
        "keyword" => {
            let k = Keyword::cast(node)?;
            vec![
                ("key", json!(k.key().to_uppercase())),
                ("value", json!(k.value())),
            ]
        }
        "node-property" => {
            let props = node.parent().and_then(PropertyDrawer::cast)?.properties();
            let p = props.into_iter().find(|p| p.node == node)?;
            vec![
                ("key", json!(p.key_as_written())),
                ("value", json!(p.value)),
            ]
        }
        "special-block" => vec![("block_type", json!(Block::cast(node)?.name()))],
        "link" => {
            // The default link types: every corpus file is parsed with them.
            let info = Link::cast(node)?.info_with(&ParseOptions::default().link_types, abbrevs);
            vec![
                ("link_type", json!(info.link_type)),
                ("path", json!(info.path)),
                ("raw_link", json!(info.raw_link)),
                ("format", json!(info.format.as_str())),
            ]
        }
        // Objects without a typed view yet: their fields read off the tree.
        "entity" => vec![("name", json!(token_text(&node, NAME)))],
        // `:key` is the name downcased.
        "macro" => vec![(
            "key",
            json!(token_text(&node, NAME).map(|k| k.to_lowercase())),
        )],
        // Between the `:` (or `{`) markup and the closing markup: a value
        // that is only a line ending holds no VALUE token (`@@a:\n@@`).
        "export-snippet" => vec![("value", json!(between(&node, ":")))],
        "inline-src-block" => vec![("value", json!(between(&node, "{")))],
        "timestamp" => {
            let raw = token_text(&node, VALUE).unwrap_or_default();
            let (ty, range) = timestamp_type(&raw);
            vec![
                ("timestamp_type", json!(ty)),
                ("range_type", json!(range)),
                ("raw_value", json!(raw)),
            ]
        }
        "drawer" => {
            // The opening line, not the first child: affiliated keywords come first.
            let name = node
                .children()
                .find(|c| c.kind() == BLOCK_BEGIN)
                .and_then(|l| {
                    l.children_with_tokens()
                        .filter_map(|t| t.into_token())
                        .find(|t| t.kind() == NAME)
                        .map(|t| t.text().to_string())
                });
            vec![("name", json!(opt_str(&serde_json::Value::from(name))))]
        }
        _ => return None,
    })
}
