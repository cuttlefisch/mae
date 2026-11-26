//! ADR-113 E4, the part of `kb import` that reads only the import directory:
//! `import.json` (`mae-import/1`), the exact `SHA256SUMS` manifest, every
//! file it names, and the `import_hash` over all of it. Nothing here touches
//! a store, so a dry run and an apply check the same bytes the same way.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The import file's format tag.
pub const FORMAT: &str = "mae-import/1";
/// The states an imported revision may have (E4).
const STATES: &[&str] = &["in_force", "superseded", "derogated"];

/// A refusal of the whole import: its kind and the fields the operator needs.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportError {
    pub kind: &'static str,
    pub data: Map<String, Value>,
    pub message: String,
}

impl ImportError {
    pub(crate) fn new(kind: &'static str, message: impl Into<String>, data: Value) -> Self {
        ImportError {
            kind,
            data: data.as_object().cloned().unwrap_or_default(),
            message: message.into(),
        }
    }

    /// `{"ok": false, "error": {"kind", ...fields}}`, as `--json` prints it.
    pub fn to_json(&self) -> Value {
        let mut e = self.data.clone();
        e.insert("kind".into(), json!(self.kind));
        json!({ "ok": false, "error": e })
    }

    fn manifest(path: &str, reason: &str, extra: Value) -> Self {
        let mut data = json!({ "path": path, "reason": reason });
        if let (Some(d), Some(x)) = (data.as_object_mut(), extra.as_object()) {
            d.extend(x.clone());
        }
        ImportError::new(
            "manifest_mismatch",
            format!("SHA256SUMS: {path}: {reason}"),
            data,
        )
    }

    fn invalid(field: &str, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        ImportError::new(
            "invalid_params",
            format!("import.json: {field}: {reason}"),
            json!({ "param": field, "reason": reason }),
        )
    }
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportFile {
    format: String,
    kb_id: String,
    pages: Vec<PageIn>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageIn {
    node_id: String,
    #[serde(default)]
    doc_code: Option<String>,
    revisions: Vec<RevIn>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RevIn {
    rev: i64,
    state: String,
    source: String,
    signed_pdf: String,
    #[serde(default)]
    rendered_pdf: Option<String>,
    approved_by_name: String,
    #[serde(default)]
    approved_on: Option<String>,
    effective_at: String,
    #[serde(default)]
    ended_at: Option<String>,
}

/// One file the import names, verified against its manifest line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
}

/// One imported revision, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionIn {
    pub rev: i64,
    pub state: String,
    /// The org source's text.
    pub source: File,
    pub signed_pdf: File,
    pub rendered_pdf: Option<File>,
    pub approved_by_name: String,
    pub approved_on: Option<String>,
    pub effective_at: i64,
    pub ended_at: Option<i64>,
}

/// One page and its revisions in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageInChecked {
    pub node_id: String,
    pub doc_code: Option<String>,
    pub revisions: Vec<RevisionIn>,
}

/// Everything the directory holds, verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub kb_id: String,
    pub pages: Vec<PageInChecked>,
    pub import_hash: String,
    pub manifest_sha256: String,
}

impl Checked {
    pub fn revisions(&self) -> usize {
        self.pages.iter().map(|p| p.revisions.len()).sum()
    }

    pub fn pdfs(&self) -> usize {
        self.pages
            .iter()
            .flat_map(|p| &p.revisions)
            .map(|r| 1 + usize::from(r.rendered_pdf.is_some()))
            .sum()
    }
}

fn hex_sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A path as the manifest and `import.json` may name it: relative, plain
/// components only (no `..`, `.`, root or empty part), UTF-8.
fn safe_path(p: &str) -> bool {
    !p.is_empty()
        && !p.contains('\\')
        && Path::new(p)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && !p.split('/').any(str::is_empty)
}

/// `SHA256SUMS` as `sha256sum` writes it: `<64 lowercase hex>  <path>` or
/// `<hex> *<path>`, LF line endings. Every line well formed, no path twice.
fn parse_manifest(text: &[u8]) -> Result<BTreeMap<String, String>, ImportError> {
    let text = std::str::from_utf8(text)
        .map_err(|_| ImportError::manifest("SHA256SUMS", "malformed_line", json!({})))?;
    let mut out = BTreeMap::new();
    let lines: Vec<&str> = text.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() && i + 1 == lines.len() {
            break;
        }
        let malformed = || {
            ImportError::manifest(
                &format!("SHA256SUMS:{}", i + 1),
                "malformed_line",
                json!({ "line": i + 1 }),
            )
        };
        let (hash, rest) = line.split_at_checked(64).ok_or_else(malformed)?;
        let path = rest
            .strip_prefix("  ")
            .or_else(|| rest.strip_prefix(" *"))
            .ok_or_else(malformed)?;
        let hex_ok = hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex_ok || !safe_path(path) {
            return Err(malformed());
        }
        if out.insert(path.to_string(), hash.to_string()).is_some() {
            return Err(ImportError::manifest(path, "duplicate", json!({})));
        }
    }
    Ok(out)
}

/// Read one named file under `dir`, checked against its manifest line.
fn read_file(
    dir: &Path,
    path: &str,
    manifest: &BTreeMap<String, String>,
    max_bytes: u64,
) -> Result<File, ImportError> {
    if !safe_path(path) {
        return Err(ImportError::invalid(path, "not a plain relative path"));
    }
    let Some(expected) = manifest.get(path) else {
        return Err(ImportError::manifest(path, "missing_line", json!({})));
    };
    let full = dir.join(path);
    let size = std::fs::metadata(&full)
        .map_err(|_| ImportError::manifest(path, "missing_file", json!({})))?
        .len();
    if size > max_bytes {
        return Err(ImportError::new(
            "quota_exceeded",
            format!("{path}: {size} bytes, over the {max_bytes}-byte attachment cap"),
            json!({ "path": path, "size": size, "limit": max_bytes }),
        ));
    }
    let bytes =
        std::fs::read(&full).map_err(|_| ImportError::manifest(path, "missing_file", json!({})))?;
    let actual = hex_sha(&bytes);
    if &actual != expected {
        return Err(ImportError::manifest(
            path,
            "hash",
            json!({ "expected": expected, "actual": actual }),
        ));
    }
    Ok(File {
        path: path.to_string(),
        sha256: actual,
        bytes,
    })
}

/// B2: a PDF begins `%PDF-`.
fn pdf(f: File) -> Result<File, ImportError> {
    if !f.bytes.starts_with(b"%PDF-") {
        return Err(ImportError::new(
            "attachment_type",
            format!("{}: not a PDF", f.path),
            json!({ "path": f.path }),
        ));
    }
    Ok(f)
}

fn time(field: &str, s: &str, now_ms: i64) -> Result<i64, ImportError> {
    let at = crate::rfc3339::parse_utc_millis(s).ok_or_else(|| {
        ImportError::invalid(
            field,
            "RFC 3339 UTC with milliseconds, as 2000-01-01T00:00:00.000Z",
        )
    })?;
    if at > now_ms + 60_000 {
        return Err(ImportError::invalid(field, "in the future"));
    }
    Ok(at)
}

/// A calendar date `YYYY-MM-DD` (the attested approval date).
fn date(field: &str, s: &str) -> Result<(), ImportError> {
    let ok =
        s.len() == 10 && crate::rfc3339::parse_utc_millis(&format!("{s}T00:00:00.000Z")).is_some();
    if ok {
        Ok(())
    } else {
        Err(ImportError::invalid(field, "a date, YYYY-MM-DD"))
    }
}

/// E4's order rules for one page: revisions in increasing `rev` and time, at
/// most one in force and it the last, an ended revision with its end.
fn check_order(node: &str, revs: &[RevisionIn]) -> Result<(), ImportError> {
    let f = |i: usize, k: &str| format!("pages[{node}].revisions[{i}].{k}");
    if revs.is_empty() {
        return Err(ImportError::invalid(
            &format!("pages[{node}].revisions"),
            "empty",
        ));
    }
    for (i, r) in revs.iter().enumerate() {
        let last = i + 1 == revs.len();
        match (r.state.as_str(), r.ended_at) {
            ("in_force", None) if last => {}
            ("in_force", _) => {
                return Err(ImportError::invalid(
                    &f(i, "state"),
                    "only the last revision may be in force, and it has no ended_at",
                ))
            }
            (_, None) => return Err(ImportError::invalid(&f(i, "ended_at"), "required")),
            (_, Some(end)) if end < r.effective_at => {
                return Err(ImportError::invalid(
                    &f(i, "ended_at"),
                    "before effective_at",
                ))
            }
            _ => {}
        }
        if let Some(prev) = i.checked_sub(1).map(|j| &revs[j]) {
            if r.rev <= prev.rev {
                return Err(ImportError::invalid(&f(i, "rev"), "not after the previous"));
            }
            if r.effective_at < prev.ended_at.unwrap_or(prev.effective_at) {
                return Err(ImportError::invalid(
                    &f(i, "effective_at"),
                    "before the previous revision ended",
                ));
            }
        }
    }
    Ok(())
}

/// One revision of `import.json`, its fields checked and its files read.
fn revision_in(
    r: &RevIn,
    at: &dyn Fn(&str) -> String,
    read: &mut dyn FnMut(&str) -> Result<File, ImportError>,
    now_ms: i64,
) -> Result<RevisionIn, ImportError> {
    if !STATES.contains(&r.state.as_str()) {
        return Err(ImportError::invalid(
            &at("state"),
            "in_force, superseded or derogated",
        ));
    }
    if r.approved_by_name.trim().is_empty() {
        return Err(ImportError::invalid(&at("approved_by_name"), "required"));
    }
    if let Some(d) = &r.approved_on {
        date(&at("approved_on"), d)?;
    }
    let source = read(&r.source)?;
    let signed = pdf(read(&r.signed_pdf)?)?;
    let rendered = match &r.rendered_pdf {
        Some(path) => Some(pdf(read(path)?)?),
        None => None,
    };
    Ok(RevisionIn {
        rev: r.rev,
        state: r.state.clone(),
        source,
        signed_pdf: signed,
        rendered_pdf: rendered,
        approved_by_name: r.approved_by_name.trim().to_string(),
        approved_on: r.approved_on.clone(),
        effective_at: time(&at("effective_at"), &r.effective_at, now_ms)?,
        ended_at: match &r.ended_at {
            Some(e) => Some(time(&at("ended_at"), e, now_ms)?),
            None => None,
        },
    })
}

/// Read and verify the import directory for KB `kb_id`. Every file named is
/// in the manifest with its hash, and every manifest line is used.
pub fn check(dir: &Path, kb_id: &str, max_bytes: u64, now_ms: i64) -> Result<Checked, ImportError> {
    let raw = std::fs::read(dir.join("import.json"))
        .map_err(|e| ImportError::invalid("import.json", format!("unreadable: {e}")))?;
    let file: ImportFile = serde_json::from_slice(&raw)
        .map_err(|e| ImportError::invalid("import.json", format!("not mae-import/1: {e}")))?;
    if file.format != FORMAT {
        return Err(ImportError::invalid("format", format!("must be {FORMAT}")));
    }
    if file.kb_id != kb_id {
        return Err(ImportError::invalid("kb_id", "not the --kb-id given"));
    }
    let sums = std::fs::read(dir.join("SHA256SUMS"))
        .map_err(|_| ImportError::manifest("SHA256SUMS", "missing_file", json!({})))?;
    let manifest = parse_manifest(&sums)?;
    let mut used = BTreeSet::new();
    let mut read = |path: &str| -> Result<File, ImportError> {
        used.insert(path.to_string());
        read_file(dir, path, &manifest, max_bytes)
    };
    let mut pages = Vec::new();
    let mut nodes = BTreeSet::new();
    for p in &file.pages {
        if !nodes.insert(p.node_id.clone()) {
            return Err(ImportError::invalid(
                &format!("pages[{}]", p.node_id),
                "twice",
            ));
        }
        let mut revs = Vec::new();
        for (i, r) in p.revisions.iter().enumerate() {
            let at = |k: &str| format!("pages[{}].revisions[{i}].{k}", p.node_id);
            revs.push(revision_in(r, &at, &mut read, now_ms)?);
        }
        check_order(&p.node_id, &revs)?;
        pages.push(PageInChecked {
            node_id: p.node_id.clone(),
            doc_code: p.doc_code.clone(),
            revisions: revs,
        });
    }
    if let Some(unused) = manifest.keys().find(|k| !used.contains(*k)) {
        return Err(ImportError::manifest(unused, "unused_line", json!({})));
    }
    let manifest_sha256 = hex_sha(&sums);
    let import_hash = hex_sha(
        format!(
            "mae-import-hash/1\n{kb_id}\n{}\n{manifest_sha256}\n",
            hex_sha(&raw)
        )
        .as_bytes(),
    );
    Ok(Checked {
        kb_id: kb_id.to_string(),
        pages,
        import_hash,
        manifest_sha256,
    })
}
