//! Edits outside the seed, and the token that binds a discard to them.
//!
//! A node is edited outside the seed when its document is not what the seed
//! last left it at. Three baselines, one per state a node can be in:
//! - a node of the applied plan: its seed row (`seed_version`, or for a row
//!   from before that column, the node rebuilt from its recorded blob);
//! - a node an earlier update removed that this plan brings back: the version
//!   its document had when it was removed (`kb_seed_removed`). A document the
//!   seed never wrote is an edit when it is not the plan's content;
//! - while an update is in progress: its own row, the version the node had
//!   when the update began, or for a node already done, the version the
//!   update left it at.
//!
//! `--discard-edits` takes the dry run's `edits_hash`, the sha256 over every
//! node the apply would discard an edit of, with its document's version. The
//! apply recomputes it and refuses on any difference, so an edit made after
//! the dry run is never discarded unreviewed. It is deliberately not part of
//! `plan_hash`, which a resumed update compares with its recorded intent.
//!
//! @ai-caution: [kb-truth] Every node an update overwrites, removes or
//! returns must be judged here, and every edit it would discard must be in
//! the token. A node left out is overwritten without review.

use std::collections::HashMap;

use mae_sync::kb::history::diff as field_diff;
use mae_sync::kb::KbNodeDoc;
use sha2::{Digest, Sha256};

use super::super::apply::{
    build_node, refuse, row_from_plan, storage, ApplyError, ApplyRefusalKind,
};
use super::super::{node_from_file, split, GitRepo, SeedPlan};
use super::{read_node, same_content, OutsideEdit};
use crate::doc_editors::{EditorKind, EditorsSince, InternalWriter};
use crate::doc_store::DocStore;
use crate::storage::{SeedNodeRow, SeedUpdateNode};

/// Every edit a fresh update would discard: the applied plan's nodes, then
/// the nodes this plan returns.
pub(super) async fn fresh(
    store: &DocStore,
    kb_id: &str,
    plan: &SeedPlan,
    rows: &HashMap<String, SeedNodeRow>,
) -> Result<Vec<OutsideEdit>, ApplyError> {
    let mut out = recorded(store, kb_id, &plan.source.repo, rows).await?;
    out.extend(returning(store, kb_id, plan, rows).await?);
    Ok(out)
}

/// The edits an update in progress meets. A node not yet done must still be
/// at its version when the update began, or already at the plan (`plan`:
/// the in-progress plan, `None` when it is not this one); the resume would
/// discard either. A node done must be at the version the update left it
/// at; the resume keeps such an edit (`discarded: false`), and the next
/// update finds it.
pub(super) async fn in_progress(
    store: &DocStore,
    kb_id: &str,
    plan: Option<&SeedPlan>,
    nodes: &[SeedUpdateNode],
) -> Result<Vec<OutsideEdit>, ApplyError> {
    let planned: HashMap<&str, _> = plan
        .map(|p| p.nodes.iter().map(|n| (n.file.id.as_str(), n)).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for n in nodes {
        let doc = read_node(store, kb_id, &n.row.node_id).await?;
        let now = doc.as_ref().map(KbNodeDoc::version);
        let (moved, note) = if n.done {
            (
                now != n.row.seed_version,
                "written since this update wrote it",
            )
        } else {
            let at_plan = match (planned.get(n.row.node_id.as_str()), &doc) {
                (Some(p), Some(d)) if !n.remove => same_content(d, &build_node(p, &n.row)),
                _ => false,
            };
            (
                now != n.base_version && !at_plan,
                "written since this update began",
            )
        };
        if moved {
            let mut e = edit(&n.row.node_id, &n.row.path, now, note);
            e.discarded = !n.done;
            out.push(e);
        }
    }
    Ok(out)
}

fn edit(node_id: &str, path: &str, version: Option<String>, note: &str) -> OutsideEdit {
    OutsideEdit {
        node_id: node_id.to_string(),
        path: path.to_string(),
        fields: None,
        editors: Vec::new(),
        note: Some(note.to_string()),
        version,
        discarded: true,
    }
}

/// The token for `edits`: the sha256 over every edit the apply would
/// discard, sorted, as `(node_id, version)`. `None` when there is none.
pub fn edits_hash(edits: &[OutsideEdit]) -> Option<String> {
    let mut v: Vec<(&str, Option<&str>)> = edits
        .iter()
        .filter(|e| e.discarded)
        .map(|e| (e.node_id.as_str(), e.version.as_deref()))
        .collect();
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let mut h = Sha256::new();
    let field = |h: &mut Sha256, b: &[u8]| {
        h.update((b.len() as u64).to_le_bytes());
        h.update(b);
    };
    field(&mut h, b"mae-seed-edits/1");
    h.update((v.len() as u64).to_le_bytes());
    for (id, version) in v {
        field(&mut h, id.as_bytes());
        match version {
            Some(ver) => {
                h.update([1u8]);
                field(&mut h, ver.as_bytes());
            }
            None => h.update([0u8]),
        }
    }
    Some(hex::encode(h.finalize()))
}

/// Whether the apply may discard `edits`: there are none, or `token` is
/// their [`edits_hash`].
pub(super) fn check_discard(edits: &[OutsideEdit], token: Option<&str>) -> Result<(), ApplyError> {
    let Some(hash) = edits_hash(edits) else {
        return Ok(());
    };
    match token {
        None => Err(refusal(edits)),
        Some(t) if t == hash => Ok(()),
        Some(_) => Err(refuse(
            ApplyRefusalKind::EditsChanged,
            "the edits changed since the dry run: run it again, review them, and give its \
             edits_hash to --discard-edits",
        )),
    }
}

fn refusal(edits: &[OutsideEdit]) -> ApplyError {
    let names: Vec<String> = edits
        .iter()
        .filter(|e| e.discarded)
        .map(|e| format!("{} ({})", e.node_id, e.path))
        .collect();
    refuse(
        ApplyRefusalKind::EditedOutsideSeed,
        format!(
            "{} node(s) were edited outside the seed since its last apply: {}; run the dry \
             run to see how, and give its edits_hash to --discard-edits to overwrite them \
             (the edits stay in each node's history)",
            names.len(),
            names.join(", ")
        ),
    )
}

/// Each planned node with a document but no row: one an earlier update
/// removed, or one the seed never wrote. Judged against its version at the
/// removal; one the seed never wrote, against the plan.
async fn returning(
    store: &DocStore,
    kb_id: &str,
    plan: &SeedPlan,
    rows: &HashMap<String, SeedNodeRow>,
) -> Result<Vec<OutsideEdit>, ApplyError> {
    let removed: HashMap<String, _> = store
        .seed_removed(kb_id)
        .await
        .map_err(storage)?
        .into_iter()
        .map(|r| (r.node_id.clone(), r))
        .collect();
    let mut out = Vec::new();
    for n in plan.nodes.iter().filter(|n| !rows.contains_key(&n.file.id)) {
        let Some(doc) = read_node(store, kb_id, &n.file.id).await? else {
            continue;
        };
        let now = doc.version();
        let (edited, wal, note) = match removed.get(&n.file.id) {
            Some(r) => (
                r.seed_version.as_deref() != Some(now.as_str()),
                r.seed_wal_id.unwrap_or(0),
                "returns: written since an earlier update removed it",
            ),
            None => (
                !same_content(&doc, &build_node(n, &row_from_plan(n))),
                0,
                "returns: the seed never wrote this document, and it is not the plan's",
            ),
        };
        if edited {
            let e = edit(&n.file.id, &n.path, Some(now), note);
            out.push(with_editors(store, kb_id, &n.file.id, wal, e).await);
        }
    }
    Ok(out)
}

/// Every recorded node whose document is not what the seed last wrote.
async fn recorded(
    store: &DocStore,
    kb_id: &str,
    repo: &str,
    rows: &HashMap<String, SeedNodeRow>,
) -> Result<Vec<OutsideEdit>, ApplyError> {
    let mut rows: Vec<&SeedNodeRow> = rows.values().collect();
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    let mut docs = HashMap::new();
    for r in &rows {
        docs.insert(
            r.node_id.clone(),
            read_node(store, kb_id, &r.node_id).await?,
        );
    }
    let suspects: Vec<&SeedNodeRow> = rows
        .iter()
        .copied()
        .filter(|r| {
            let now = docs[&r.node_id].as_ref().map(KbNodeDoc::version);
            r.seed_version.is_none() || now != r.seed_version
        })
        .collect();
    let written = seed_written(repo, &suspects);
    let mut out = Vec::new();
    for r in suspects {
        let doc = docs[&r.node_id].as_ref();
        if let Some(e) = judge(r, doc, written.get(&r.node_id)) {
            let wal = r.seed_wal_id.unwrap_or(0);
            out.push(with_editors(store, kb_id, &r.node_id, wal, e).await);
        }
    }
    Ok(out)
}

/// Is `r`'s node edited? `written`: the node the seed wrote, rebuilt from the
/// recorded blob (or why it could not be).
fn judge(
    r: &SeedNodeRow,
    doc: Option<&KbNodeDoc>,
    written: Option<&Result<KbNodeDoc, String>>,
) -> Option<OutsideEdit> {
    let mut e = OutsideEdit {
        node_id: r.node_id.clone(),
        path: r.path.clone(),
        fields: None,
        editors: Vec::new(),
        note: None,
        version: doc.map(KbNodeDoc::version),
        discarded: true,
    };
    let Some(doc) = doc else {
        e.note = Some("the node's document is missing".into());
        return Some(e);
    };
    let w = match written {
        Some(Ok(w)) => w,
        Some(Err(why)) => {
            e.note = Some(match r.seed_version {
                None => format!(
                    "cannot tell: the seed record predates its version digest and the \
                     seed's write cannot be rebuilt ({why})"
                ),
                Some(_) => format!("differs from the seed's write; fields unknown ({why})"),
            });
            return Some(e);
        }
        None => {
            e.note = Some("differs from the seed's write".into());
            return Some(e);
        }
    };
    if r.seed_version.as_ref().is_some_and(|v| *v != w.version()) {
        e.note = Some(
            "differs from the seed's write; fields unknown (the recorded blob does not \
             rebuild it)"
                .into(),
        );
        return Some(e);
    }
    let fields = field_diff(&w.materialize(), &doc.materialize());
    if fields.is_empty() {
        return None;
    }
    e.fields = Some(fields.into_iter().map(|c| c.field).collect());
    Some(e)
}

/// Who wrote the node after WAL id `wal` (the seed's last write), from the
/// editor record.
async fn with_editors(
    store: &DocStore,
    kb_id: &str,
    node_id: &str,
    wal: u64,
    mut e: OutsideEdit,
) -> OutsideEdit {
    let name = mae_sync::kb_node_doc_name(kb_id, node_id);
    let seed = InternalWriter::Seed.as_str();
    match store.editors_since_wal(&name, wal).await {
        Ok(EditorsSince::Known(rows)) => {
            e.editors = rows
                .into_iter()
                .filter(|w| !(w.kind == EditorKind::Internal && w.editor == seed))
                .map(|w| format!("{}:{}", w.kind.as_str(), w.editor))
                .collect();
        }
        Ok(_) => e.editors = vec!["(the edit record does not cover it)".into()],
        Err(err) => e.editors = vec![format!("(edit record unreadable: {err})")],
    }
    e
}

/// The node the seed wrote for each row, rebuilt from its blob: the seed's one
/// mapping (`node_from_file`) of the file, with the recorded creation time.
/// Every blob is hash-checked; one git process reads them all.
fn seed_written(repo: &str, rows: &[&SeedNodeRow]) -> HashMap<String, Result<KbNodeDoc, String>> {
    if rows.is_empty() {
        return HashMap::new();
    }
    let git = GitRepo::new(repo);
    let shas: Vec<String> = rows.iter().map(|r| r.blob.clone()).collect();
    let read = git
        .object_format()
        .and_then(|f| git.read_blobs(&shas).map(|b| (f, b)));
    let (format, blobs) = match read {
        Ok(v) => v,
        Err(e) => {
            return rows
                .iter()
                .map(|r| (r.node_id.clone(), Err(e.to_string())))
                .collect()
        }
    };
    rows.iter()
        .zip(blobs)
        .map(|(r, bytes)| (r.node_id.clone(), rebuild(r, &format, &bytes)))
        .collect()
}

fn rebuild(r: &SeedNodeRow, format: &str, bytes: &[u8]) -> Result<KbNodeDoc, String> {
    if super::super::git::blob_name(format, bytes) != r.blob {
        return Err(format!("blob {} does not hash to its name", r.blob));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "the blob is not UTF-8".to_string())?;
    let file = split(text).map_err(|_| "the blob no longer splits".to_string())?;
    Ok(node_from_file(&file, r.created_at))
}
