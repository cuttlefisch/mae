//! Exporting a seeded KB: each node written back to its source path as
//! `regen(fields) + body`, which for a node unedited since the seed is N(F).
//!
//! The header is rebuilt from the node: its properties in the recorded file
//! order (`org_header.property_order`), a key the order does not name placed
//! after the ordered ones in sorted order, a key the order names that the node
//! no longer has skipped, each of those reported; `#+title:` and `#+filetags:`
//! written when the node has a value or when the file had the line. The node's
//! source path comes from the daemon's seed record.
//!
//! Every file is checked before it is written: `split` of the text must give
//! back exactly the node's fields and body. A node that would not (someone put
//! a `#+title:` or a drawer into its body, or changed its `ID` property) is
//! refused by name and not written, because its file would seed differently.
//!
//! @ai-caution: [security] The target directory is the operator's, and a seed
//! path is input. Paths are relative and normal; every directory on the way
//! must be a real directory inside the target (a symlink is refused, never
//! followed), and a file is created with `create_new`, which does not follow a
//! symlink at its own name.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, BODY_FORMAT_ORG_LITERAL_V1};
use mae_sync::DocAddress;

use super::{regen, split, SeedError, SplitFile};
use crate::doc_store::DocStore;
use crate::storage::SeedNodeRow;

/// Why a node was not exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportRefusalKind {
    /// The node has no source path in the seed record (not made by the seed).
    NoSourcePath,
    /// The path is not a normal relative path.
    BadPath,
    /// The written text would not split back to the node's fields and body.
    NotReproducible,
    /// The node document could not be read.
    Unreadable,
    /// Writing the file failed (or would have followed a symlink).
    WriteFailed,
}

impl ExportRefusalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoSourcePath => "no_source_path",
            Self::BadPath => "bad_path",
            Self::NotReproducible => "not_reproducible",
            Self::Unreadable => "unreadable",
            Self::WriteFailed => "write_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRefusal {
    pub node_id: String,
    pub kind: ExportRefusalKind,
    pub detail: String,
}

/// What an export did. Sorted by node id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportReport {
    /// Paths written.
    pub written: Vec<String>,
    pub refused: Vec<ExportRefusal>,
    /// Header adjustments (keys outside the recorded order, or missing from
    /// the node), one line each, `node_id: …`.
    pub notes: Vec<String>,
}

/// The file text of one node, with its notes; or why it has none.
pub struct Rendered {
    pub text: String,
    pub notes: Vec<String>,
}

/// Export KB `kb_id` into `dir`. Refuses a non-empty `dir` unless `force`.
pub async fn export(
    store: &DocStore,
    kb_id: &str,
    dir: &Path,
    force: bool,
) -> Result<ExportReport, SeedError> {
    let coll = literal_collection(store, kb_id).await?;
    let rows: HashMap<String, SeedNodeRow> = match store.seed_record(kb_id).await {
        Ok(found) => found.map(|(_, r)| r).unwrap_or_default(),
        Err(e) => return Err(SeedError(format!("seed record of '{kb_id}': {e}"))),
    }
    .into_iter()
    .map(|r| (r.node_id.clone(), r))
    .collect();
    prepare_dir(dir, force)?;
    let mut ids: Vec<String> = coll.list_nodes().into_iter().map(|(id, _)| id).collect();
    ids.sort();
    let mut report = ExportReport::default();
    for id in ids {
        export_node(store, kb_id, &id, rows.get(&id), dir, &mut report).await;
    }
    Ok(report)
}

async fn literal_collection(store: &DocStore, kb_id: &str) -> Result<KbCollectionDoc, SeedError> {
    let name = DocAddress::KbCollection {
        kb_id: kb_id.to_string(),
    }
    .to_doc_name();
    let bytes = store
        .encode_state(&name)
        .await
        .map_err(|e| SeedError(format!("KB '{kb_id}': {e}")))?;
    let coll = KbCollectionDoc::from_bytes(&bytes)
        .map_err(|e| SeedError(format!("KB '{kb_id}': bad collection: {e}")))?;
    match coll.body_format() {
        Some(f) if f == BODY_FORMAT_ORG_LITERAL_V1 => Ok(coll),
        other => Err(SeedError(format!(
            "KB '{kb_id}' has body format {other:?}, not {BODY_FORMAT_ORG_LITERAL_V1}: its \
             bodies are not literal org text, so there is no file to write back"
        ))),
    }
}

/// The target must be a real directory (created if missing) and empty, unless
/// `force`.
fn prepare_dir(dir: &Path, force: bool) -> Result<(), SeedError> {
    let err = |e: std::io::Error| SeedError(format!("{}: {e}", dir.display()));
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir).map_err(err)
        }
        Err(e) => Err(err(e)),
        Ok(m) if !m.is_dir() => Err(SeedError(format!(
            "{} is not a directory (a symlink is not followed)",
            dir.display()
        ))),
        Ok(_) if !force && std::fs::read_dir(dir).map_err(err)?.next().is_some() => {
            Err(SeedError(format!(
                "{} is not empty; pass --force to write into it anyway",
                dir.display()
            )))
        }
        Ok(_) => Ok(()),
    }
}

async fn export_node(
    store: &DocStore,
    kb_id: &str,
    id: &str,
    row: Option<&SeedNodeRow>,
    dir: &Path,
    report: &mut ExportReport,
) {
    let refuse = |kind, detail: String| ExportRefusal {
        node_id: id.to_string(),
        kind,
        detail,
    };
    let result = async {
        let row = row.ok_or_else(|| {
            refuse(
                ExportRefusalKind::NoSourcePath,
                "the seed record has no source path for this node".into(),
            )
        })?;
        let rel = checked_relative(&row.path).map_err(|d| refuse(ExportRefusalKind::BadPath, d))?;
        let name = mae_sync::kb_node_doc_name(kb_id, id);
        let doc = match store
            .encode_state(&name)
            .await
            .map(|b| KbNodeDoc::from_bytes(&b))
        {
            Ok(Ok(doc)) => doc,
            Ok(Err(e)) => return Err(refuse(ExportRefusalKind::Unreadable, e.to_string())),
            Err(e) => return Err(refuse(ExportRefusalKind::Unreadable, e.to_string())),
        };
        let r = render(id, &doc).map_err(|d| refuse(ExportRefusalKind::NotReproducible, d))?;
        write_inside(dir, &rel, &r.text).map_err(|d| refuse(ExportRefusalKind::WriteFailed, d))?;
        Ok((row.path.clone(), r.notes))
    }
    .await;
    match result {
        Ok((path, notes)) => {
            report.written.push(path);
            report
                .notes
                .extend(notes.into_iter().map(|n| format!("{id}: {n}")));
        }
        Err(r) => report.refused.push(r),
    }
}

/// The file a node exports to, checked to read back as the node.
pub fn render(id: &str, doc: &KbNodeDoc) -> Result<Rendered, String> {
    let (properties, notes) = ordered_properties(doc);
    let header = doc.org_header();
    let title = doc.title();
    let tags = doc.tags();
    let has_title_line = header.as_ref().is_some_and(|h| h.has_title_line);
    let has_filetags_line = header.as_ref().is_some_and(|h| h.has_filetags_line);
    let file = SplitFile {
        id: id.to_string(),
        properties,
        title: (has_title_line || !title.is_empty()).then_some(title),
        filetags: (has_filetags_line || !tags.is_empty()).then_some(tags),
        body: doc.body(),
        anchors: Vec::new(),
    };
    let text = regen(&file);
    reads_back(&file, &text)?;
    Ok(Rendered { text, notes })
}

/// The node's properties in the recorded order, then the rest sorted.
fn ordered_properties(doc: &KbNodeDoc) -> (Vec<(String, String)>, Vec<String>) {
    let mut props = doc.properties();
    let order = doc
        .org_header()
        .map(|h| h.property_order)
        .unwrap_or_default();
    let mut out = Vec::with_capacity(props.len());
    let mut notes = Vec::new();
    let mut seen = HashSet::new();
    for key in order {
        if !seen.insert(key.clone()) {
            continue;
        }
        match props.remove(&key) {
            Some(v) => out.push((key, v)),
            None => notes.push(format!(
                "property {key:?} is in the recorded order but not on the node; skipped"
            )),
        }
    }
    let mut rest: Vec<(String, String)> = props.into_iter().collect();
    rest.sort();
    for (k, _) in &rest {
        notes.push(format!(
            "property {k:?} has no recorded position; written after the ordered ones"
        ));
    }
    out.extend(rest);
    (out, notes)
}

/// `split(text)` must give back the node's id, fields and body exactly.
fn reads_back(file: &SplitFile, text: &str) -> Result<(), String> {
    let back = split(text).map_err(|ps| {
        let kinds: Vec<&str> = ps.iter().map(|p| p.kind.as_str()).collect();
        format!(
            "the exported text would be refused by the seed: {}",
            kinds.join(", ")
        )
    })?;
    let differs = [
        ("ID property", back.id != file.id),
        ("properties", back.properties != file.properties),
        ("title", back.title != file.title),
        ("filetags", back.filetags != file.filetags),
        ("body", back.body != file.body),
    ];
    let bad: Vec<&str> = differs
        .iter()
        .filter(|(_, d)| *d)
        .map(|(n, _)| *n)
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the exported text would seed back with a different {} (a header line or \
             drawer in the body, or an edited ID?)",
            bad.join(", ")
        ))
    }
}

/// A repo path as a relative path of normal components only.
fn checked_relative(path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    let normal = !path.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)));
    if normal {
        Ok(p.to_path_buf())
    } else {
        Err(format!("{path:?} is not a normal relative path"))
    }
}

/// Write `text` at `dir/rel`, creating real directories on the way and never
/// following a symlink.
fn write_inside(dir: &Path, rel: &Path, text: &str) -> Result<(), String> {
    let mut at = dir.to_path_buf();
    let parts: Vec<_> = rel.components().collect();
    let (file, parents) = parts.split_last().ok_or("empty path")?;
    for part in parents {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(format!("{} is not a real directory", at.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&at).map_err(|e| format!("{}: {e}", at.display()))?
            }
            Err(e) => return Err(format!("{}: {e}", at.display())),
        }
    }
    at.push(file);
    replace_file(&at, text)
}

fn replace_file(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() || !m.is_file() => {
            return Err(format!(
                "{} exists and is not a regular file",
                path.display()
            ))
        }
        Ok(_) => std::fs::remove_file(path).map_err(err)?,
        Err(_) => {}
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(err)?;
    f.write_all(text.as_bytes()).map_err(err)
}
