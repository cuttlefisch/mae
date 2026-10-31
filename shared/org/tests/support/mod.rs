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

/// The bundled corpus: every `.org` under the repo's `assets/` and `tests/`.
pub fn bundled_corpus() -> Vec<PathBuf> {
    let root = repo_root();
    let mut v = org_files(&root.join("assets"));
    v.extend(org_files(&root.join("tests")));
    v
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
            if el.parent.0 == "org-data" {
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
