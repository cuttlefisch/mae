//! ADR-113 E4 and B6: `mae-daemon kb import`, past and current revisions of
//! documents that were approved and signed before MAE, with their signed
//! PDFs. An operator act through the local CLI only (principle #16): no
//! network method, MCP tool or Scheme call reaches this module.
//!
//! **All or nothing.** [`import_check::check`] verifies every file against the
//! manifest before anything else; [`plan`] then checks the KB (it exists, is
//! plaintext, holds each node) and the overlap rule. An apply stores the PDFs
//! in the KB's attachment store first, then writes the request, the revisions
//! with their frozen sources and attachments, the attested approver records
//! and one `import` event in one review transaction, re-checking the overlap
//! inside it. A failure between the two leaves only unreferenced blobs, which
//! ADR-115's sweep collects after its grace period.
//!
//! **What is recorded.** The approver is recorded by name, as an attested
//! fact: no principal approved anything in MAE. Each revision's frozen source
//! is the org file as seeding it would build the node (`seed::split`,
//! `seed::node_from_file`), and its `source_sha256` is computed as a freeze
//! computes it.

use std::collections::BTreeMap;
use std::path::Path;

use mae_sync::kb::history::encode_content;
use mae_sync::kb::KbNodeDoc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::attach_store::{self, Attachment};
pub use super::import_check::{check, Checked, ImportError, FORMAT};
use super::import_check::{PageInChecked, RevisionIn};
use super::store::{self, NewEvent, Request, Revision};
use crate::doc_store::DocStore;
use crate::storage::StorageError;

/// The principal an import is recorded under: the local operator.
const OPERATOR: &str = "operator:local";

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS kb_review_imports (
     revision_id TEXT PRIMARY KEY,
     kb_id TEXT NOT NULL,
     approved_by_name TEXT NOT NULL,
     approved_on TEXT,
     import_hash TEXT NOT NULL,
     event_seq INTEGER NOT NULL
 );
 CREATE TRIGGER IF NOT EXISTS kb_review_imports_no_update BEFORE UPDATE ON kb_review_imports
 BEGIN SELECT RAISE(ABORT, 'an import record is immutable'); END;
 CREATE TRIGGER IF NOT EXISTS kb_review_imports_no_delete BEFORE DELETE ON kb_review_imports
 BEGIN SELECT RAISE(ABORT, 'an import record is immutable'); END;";

/// An imported revision's attested approval (E4), for the read path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportRecord {
    pub approved_by_name: String,
    pub approved_on: Option<String>,
    pub event_seq: u64,
}

/// The import record of `revision_id`, if it was imported.
pub(crate) fn record(
    conn: &sqlite::Connection,
    revision_id: &str,
) -> Result<Option<ImportRecord>, StorageError> {
    let mut st = conn.prepare(
        "SELECT approved_by_name, approved_on, event_seq FROM kb_review_imports
         WHERE revision_id = ?1",
    )?;
    st.bind((1, revision_id))?;
    if st.next()? != sqlite::State::Row {
        return Ok(None);
    }
    Ok(Some(ImportRecord {
        approved_by_name: st.read::<String, _>("approved_by_name")?,
        approved_on: st.read::<Option<String>, _>("approved_on")?,
        event_seq: u64::try_from(st.read::<i64, _>("event_seq")?)
            .map_err(|_| StorageError::Sqlite("import row: bad event_seq".into()))?,
    }))
}

/// One revision as it will be written: its frozen source built and hashed.
#[derive(Debug, Clone)]
struct Built {
    node_id: String,
    input: RevisionIn,
    content: Vec<u8>,
    source_sha256: String,
    version: String,
}

/// One page as it will be written.
#[derive(Debug, Clone)]
struct PageBuilt {
    node_id: String,
    code: Option<(String, i64)>,
    cutoff_wal: u64,
    revisions: Vec<Built>,
}

/// A checked import, with everything the store had to say about it.
#[derive(Debug, Clone)]
pub struct Plan {
    pub checked: Checked,
    pages: Vec<PageBuilt>,
}

fn refuse(kind: &'static str, message: impl Into<String>, data: Value) -> ImportError {
    ImportError::new(kind, message, data)
}

fn storage(e: impl std::fmt::Display) -> ImportError {
    refuse("internal", format!("the store failed: {e}"), json!({}))
}

/// E4's overlap rule, and B6's: an import fills a node's past but never
/// lands at or before a revision it already holds, nor beside one still in
/// review, approved or in force.
fn overlap(node: &str, first: i64, existing: &[Revision]) -> Option<ImportError> {
    // The end of a revision's window counts too: an import never lands inside
    // a recorded in-force period.
    let latest = existing
        .iter()
        .map(|r| {
            let start = r.in_force_since.or(r.approved_at).unwrap_or(r.created_at);
            r.ended_at.map_or(start, |end| end.max(start))
        })
        .max()?;
    let live = existing
        .iter()
        .any(|r| matches!(r.state.as_str(), "in_review" | "approved" | "in_force"));
    (first <= latest || live).then(|| {
        refuse(
            "import_overlap",
            format!("node {node} already holds a revision at or after the imported one"),
            json!({ "node_id": node, "latest_existing": crate::review::time(latest) }),
        )
    })
}

/// The document code a page names, checked against the node's and the KB's.
fn code_of(
    c: &sqlite::Connection,
    kb: &str,
    page: &PageInChecked,
) -> Result<Result<Option<(String, i64)>, ImportError>, StorageError> {
    let Some(code) = &page.doc_code else {
        return Ok(Ok(None));
    };
    let Some((prefix, n)) = super::methods::parse_code(code) else {
        return Ok(Err(ImportError::new(
            "invalid_params",
            format!("{code}: not a document code (PREFIX-N)"),
            json!({ "param": "doc_code", "reason": "PREFIX-N" }),
        )));
    };
    if let Some(holder) = store::code_holder(c, kb, prefix, n)? {
        if holder != page.node_id {
            return Ok(Err(refuse(
                "doc_code_taken",
                format!("{code} is another node's"),
                json!({ "holder": holder }),
            )));
        }
    }
    if let Some(current) = store::node_code(c, kb, &page.node_id)? {
        if current != *code {
            return Ok(Err(ImportError::new(
                "invalid_params",
                format!("node {} already has code {current}", page.node_id),
                json!({ "param": "doc_code", "reason": "the node has another code" }),
            )));
        }
    }
    Ok(Ok(Some((prefix.to_string(), n))))
}

/// The store-side checks, inside one read snapshot or the write.
fn check_in_store(
    c: &sqlite::Connection,
    kb: &str,
    pages: &[PageBuilt],
    checked: &Checked,
) -> Result<Result<(), ImportError>, StorageError> {
    for (p, input) in pages.iter().zip(&checked.pages) {
        let existing = store::node_revisions(c, kb, &p.node_id)?;
        let first = input.revisions[0].effective_at;
        if let Some(e) = overlap(&p.node_id, first, &existing) {
            return Ok(Err(e));
        }
        let highest = existing.iter().filter_map(|r| r.rev_no).max();
        if highest.is_some_and(|h| input.revisions[0].rev <= h) {
            return Ok(Err(ImportError::new(
                "invalid_params",
                format!(
                    "node {}: rev {} is not after its rev {}",
                    p.node_id,
                    input.revisions[0].rev,
                    highest.unwrap_or_default()
                ),
                json!({ "param": "rev", "reason": "not after the node's highest rev" }),
            )));
        }
        if let Err(e) = code_of(c, kb, input)? {
            return Ok(Err(e));
        }
    }
    Ok(Ok(()))
}

/// Check `dir` and the KB it imports into: everything a dry run reports and
/// an apply writes, short of storing anything.
pub async fn plan(
    store: &DocStore,
    dir: &Path,
    kb_id: &str,
    max_bytes: u64,
) -> Result<Plan, ImportError> {
    let now = store.now_ms() as i64;
    let checked = check(dir, kb_id, max_bytes, now)?;
    let coll = crate::collab_handler::load_collection(store, kb_id)
        .await
        .map_err(|r| refuse("kb_not_found", r.message, json!({ "kb_id": kb_id })))?;
    if crate::collab_handler::kb_is_e2e(store, kb_id, &coll).await {
        return Err(refuse(
            "e2e_unsupported",
            "an end-to-end encrypted KB holds no plaintext revisions (ADR-113 O17)",
            json!({}),
        ));
    }
    let mut pages = Vec::new();
    for p in &checked.pages {
        if !coll.has_node(&p.node_id) {
            return Err(refuse(
                "node_not_in_kb",
                format!("node {} is not in KB {kb_id}", p.node_id),
                json!({ "node_id": p.node_id }),
            ));
        }
        pages.push(build_page(store, kb_id, p).await?);
    }
    let (kb, probe, ch) = (kb_id.to_string(), pages.clone(), checked.clone());
    let fine = store
        .review_read(move |c| {
            check_in_store(c, &kb, &probe, &ch).map(|r| r.and_then(|()| codes(c, &kb, &ch)))
        })
        .await
        .map_err(storage)?;
    let codes = fine?;
    for (p, code) in pages.iter_mut().zip(codes) {
        p.code = code;
    }
    Ok(Plan { checked, pages })
}

fn codes(
    c: &sqlite::Connection,
    kb: &str,
    ch: &Checked,
) -> Result<Vec<Option<(String, i64)>>, ImportError> {
    ch.pages
        .iter()
        .map(|p| code_of(c, kb, p).map_err(storage)?)
        .collect()
}

/// A page's frozen sources, each built as seeding its file would build the
/// node, created at the live node's creation time.
async fn build_page(
    store: &DocStore,
    kb_id: &str,
    p: &PageInChecked,
) -> Result<PageBuilt, ImportError> {
    let name = mae_sync::kb_node_doc_name(kb_id, &p.node_id);
    let live = store
        .encode_state(&name)
        .await
        .ok()
        .and_then(|s| KbNodeDoc::from_bytes(&s).ok())
        .map(|d| d.materialize());
    // The imported text is the file's, not the live node's, and nothing of the
    // live node has been approved in MAE: the next review's author window
    // covers the node's whole record (A1, A3), not only what follows the
    // import. Unattributed content there reads as unknown authors.
    let cutoff_wal = 0;
    let mut revisions = Vec::new();
    for r in &p.revisions {
        let created = live
            .as_ref()
            .and_then(|n| n.created_at)
            .unwrap_or(r.effective_at / 1000);
        let text = std::str::from_utf8(&r.source.bytes).map_err(|_| {
            ImportError::new(
                "invalid_params",
                format!("{}: not UTF-8 text", r.source.path),
                json!({ "param": r.source.path, "reason": "not UTF-8" }),
            )
        })?;
        let split = crate::seed::split(text).map_err(|problems| {
            ImportError::new(
                "invalid_params",
                format!("{}: {problems:?}", r.source.path),
                json!({ "param": r.source.path, "reason": "not an org page seeding accepts" }),
            )
        })?;
        if split.id != p.node_id {
            return Err(ImportError::new(
                "invalid_params",
                format!(
                    "{}: its :ID: is {}, not {}",
                    r.source.path, split.id, p.node_id
                ),
                json!({ "param": r.source.path, "reason": "the :ID: is another node's" }),
            ));
        }
        let node = crate::seed::node_from_file(&split, created).materialize();
        let content = encode_content(&node);
        revisions.push(Built {
            node_id: p.node_id.clone(),
            input: r.clone(),
            source_sha256: hex::encode(Sha256::digest(&content)),
            version: hex::encode(&node.digest()[..16]),
            content,
        });
    }
    Ok(PageBuilt {
        node_id: p.node_id.clone(),
        code: None,
        cutoff_wal,
        revisions,
    })
}

/// What the apply needs beyond the plan.
pub struct ApplyInput<'a> {
    pub import_hash: &'a str,
    /// The OS user who ran the import (recorded in the event).
    pub os_user: &'a str,
    pub limits: crate::seed::AttachmentLimits,
    pub quota: Option<crate::blob_store::StorageQuota>,
}

/// Apply a checked import: store its PDFs, then write everything in one
/// review transaction. Answers the `import` event's id.
pub async fn apply(store: &DocStore, plan: Plan, a: ApplyInput<'_>) -> Result<String, ImportError> {
    if a.import_hash != plan.checked.import_hash {
        return Err(refuse(
            "import_hash_mismatch",
            "the directory is not what the dry run checked: run it again",
            json!({ "expected": a.import_hash, "actual": plan.checked.import_hash }),
        ));
    }
    let signer = crate::review::signer(store)
        .map_err(|e| refuse("signer_unavailable", e.message, json!({})))?;
    store_pdfs(store, &plan, &a).await?;
    let now = store.now_ms() as i64;
    let os_user = a.os_user.to_string();
    let kb = plan.checked.kb_id.clone();
    let out = store
        .review_write(move |c| write(c, &signer, &kb, &plan, &os_user, now))
        .await
        .map_err(storage)?;
    out
}

/// Every PDF into the KB's attachment store (stored as sent, B2), checked to
/// come back at the address the manifest names.
async fn store_pdfs(store: &DocStore, plan: &Plan, a: &ApplyInput<'_>) -> Result<(), ImportError> {
    let Some(blobs) = store.blob_store().cloned() else {
        return Err(refuse(
            "attachments_disabled",
            "attachments are off (oauth.attachments_enabled): the signed PDFs have nowhere to go",
            json!({}),
        ));
    };
    let files: Vec<(String, String, Vec<u8>)> = plan
        .pages
        .iter()
        .flat_map(|p| &p.revisions)
        .flat_map(|r| std::iter::once(&r.input.signed_pdf).chain(&r.input.rendered_pdf))
        .map(|f| (f.path.clone(), f.sha256.clone(), f.bytes.clone()))
        .collect();
    let (kb, limits, quota) = (
        plan.checked.kb_id.clone(),
        a.limits.process(),
        a.quota.clone(),
    );
    let job = tokio::task::spawn_blocking(move || -> Result<(), ImportError> {
        let processor: &crate::blob_store::upload::Processor = &crate::blob_store::process::process;
        for (path, sha, bytes) in files {
            let meta = crate::blob_store::upload::store_bytes(
                &blobs,
                &kb,
                bytes,
                &limits,
                quota.as_ref(),
                processor,
                &crate::blob_store::upload::Job::new(),
            )
            .map_err(|e| {
                refuse(
                    "internal",
                    format!("{path}: not stored: {e:?}"),
                    json!({ "path": path }),
                )
            })?;
            if meta.sha256 != sha || meta.media_type != crate::blob_store::MediaType::Pdf {
                return Err(refuse(
                    "attachment_type",
                    format!(
                        "{path}: stored as {} ({})",
                        meta.sha256,
                        meta.media_type.as_str()
                    ),
                    json!({ "path": path }),
                ));
            }
        }
        Ok(())
    });
    job.await.map_err(storage)?
}

/// The revision row an imported revision becomes.
#[allow(clippy::too_many_arguments)]
fn revision_row(
    kb: &str,
    request_id: &str,
    id: &str,
    prev: Option<String>,
    code: Option<String>,
    cutoff_wal: u64,
    b: &Built,
    now: i64,
) -> Revision {
    let r = &b.input;
    let approved_at = r
        .approved_on
        .as_deref()
        .and_then(|d| crate::rfc3339::parse_utc_millis(&format!("{d}T00:00:00.000Z")))
        .unwrap_or(r.effective_at);
    Revision {
        revision_id: id.to_string(),
        kb_id: kb.to_string(),
        node_id: b.node_id.clone(),
        request_id: request_id.to_string(),
        doc_code: code,
        rev_no: Some(r.rev),
        source_sha256: b.source_sha256.clone(),
        version: b.version.clone(),
        cutoff_wal,
        history_seq: 0,
        state: r.state.clone(),
        doc_type: None,
        template_id: None,
        template_sha256: None,
        approval_record: "signed_pdf".into(),
        replaces: prev,
        created_by: OPERATOR.into(),
        created_by_kind: "operator".into(),
        created_at: now,
        approved_at: Some(approved_at),
        in_force_since: Some(r.effective_at),
        ended_at: r.ended_at,
    }
}

/// The `import` event's entry for one revision (E4's list).
fn event_entry(id: &str, b: &Built) -> Value {
    let r = &b.input;
    json!({
        "revision_id": id, "node_id": b.node_id, "rev": r.rev, "state": r.state,
        "source_sha256": b.source_sha256, "signed_pdf_sha256": r.signed_pdf.sha256,
        "rendered_pdf_sha256": r.rendered_pdf.as_ref().map(|f| f.sha256.clone()),
        "effective_at": crate::review::time(r.effective_at),
        "ended_at": r.ended_at.map(crate::review::time),
        "approved_by_name": r.approved_by_name, "approved_on": r.approved_on,
    })
}

type Rows<'a> = (Vec<(Revision, &'a Built)>, Vec<Value>);

/// Every revision of the plan as a row, chained by `replaces` within its page,
/// and its event entry.
fn rows<'a>(
    c: &sqlite::Connection,
    kb: &str,
    plan: &'a Plan,
    request_id: &str,
    now: i64,
) -> Result<Rows<'a>, StorageError> {
    let (mut rows, mut entries) = (Vec::new(), Vec::new());
    for p in &plan.pages {
        let mut prev: Option<String> = None;
        let code = p.code.as_ref().map(|(pre, n)| format!("{pre}-{n}"));
        let code = code.or(store::node_code(c, kb, &p.node_id)?);
        for b in &p.revisions {
            let id = format!(
                "{}-{}",
                request_id.replacen("rq-", "rv-", 1),
                rows.len() + 1
            );
            entries.push(event_entry(&id, b));
            let row = revision_row(
                kb,
                request_id,
                &id,
                prev,
                code.clone(),
                p.cutoff_wal,
                b,
                now,
            );
            rows.push((row, b));
            prev = Some(id);
        }
    }
    Ok((rows, entries))
}

/// The request record an import's revisions belong to: one per import, ended.
fn import_request(kb: &str, request_id: &str, import_hash: &str, now: i64) -> Request {
    Request {
        ord: 0,
        request_id: request_id.to_string(),
        kb_id: kb.to_string(),
        set_name: Some("import".into()),
        in_force_on_approval: false,
        reason: format!("Imported ({import_hash})"),
        requester: OPERATOR.into(),
        requester_kind: "operator".into(),
        created_at: now,
        ended_at: Some(now),
        end_reason: Some("imported".into()),
    }
}

/// One revision's rows: its source, the revision, its PDFs, its import record.
fn write_row(
    c: &sqlite::Connection,
    rev: &Revision,
    b: &Built,
    (seq, now, import_hash): (u64, i64, &str),
) -> Result<(), StorageError> {
    let kb = rev.kb_id.as_str();
    store::put_source(c, kb, &b.source_sha256, &b.content)?;
    store::insert_revision(c, rev)?;
    let r = &b.input;
    let rendered = r
        .rendered_pdf
        .as_ref()
        .map(|f| ("rendered_pdf", f, Some(b.source_sha256.clone())));
    for (kind, f, declared) in std::iter::once(("signed_pdf", &r.signed_pdf, None)).chain(rendered)
    {
        attach_store::insert(
            c,
            &Attachment {
                kb_id: kb.to_string(),
                revision_id: rev.revision_id.clone(),
                kind: kind.to_string(),
                seq: 1,
                sha256: f.sha256.clone(),
                size: f.bytes.len() as u64,
                content_type: "application/pdf".into(),
                declared_source: declared,
                uploader: OPERATOR.into(),
                uploader_kind: "operator".into(),
                iss: None,
                sub: None,
                at: now,
                event_seq: seq,
                state: "current".into(),
                annulled: None,
            },
        )?;
    }
    let mut st = c.prepare(
        "INSERT INTO kb_review_imports
             (revision_id, kb_id, approved_by_name, approved_on, import_hash, event_seq)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    st.bind((1, rev.revision_id.as_str()))?;
    st.bind((2, kb))?;
    st.bind((3, r.approved_by_name.as_str()))?;
    let on = r.approved_on.as_deref();
    st.bind((4, on.map_or(sqlite::Value::Null, |s| s.into())))?;
    st.bind((5, import_hash))?;
    st.bind((6, i64::try_from(seq).unwrap_or(i64::MAX)))?;
    st.next()?;
    Ok(())
}

/// The import's transaction: re-check, then one event, the request, every
/// revision with its source, attachments and import record, and the codes.
fn write(
    c: &sqlite::Connection,
    signer: &mae_mcp::identity::Identity,
    kb: &str,
    plan: &Plan,
    os_user: &str,
    now: i64,
) -> Result<Result<String, ImportError>, StorageError> {
    if let Err(e) = check_in_store(c, kb, &plan.pages, &plan.checked)? {
        return Ok(Err(e));
    }
    let request_id = super::methods::new_id("rq");
    let (rows, entries) = rows(c, kb, plan, &request_id, now)?;
    let hash = plan.checked.import_hash.as_str();
    let actor = json!({ "principal": OPERATOR, "kind": "operator", "os_user": os_user,
        "iss": null, "sub": null, "sign_in": null });
    let payload = json!({ "import_hash": hash, "manifest_sha256": plan.checked.manifest_sha256,
        "request_id": request_id, "revisions": entries });
    let at = crate::review::time(now);
    let event = NewEvent {
        kb_id: kb,
        kind: "import",
        at,
        actor,
        payload,
    };
    let seq = store::append_event(c, signer, event)?;
    store::insert_request(c, &import_request(kb, &request_id, hash, now))?;
    for (rev, b) in &rows {
        write_row(c, rev, b, (seq, now, hash))?;
    }
    for p in &plan.pages {
        if let Some((prefix, n)) = &p.code {
            store::assign_code(c, kb, prefix, *n, &p.node_id, seq)?;
            if store::code_holder(c, kb, prefix, *n)?.as_deref() != Some(p.node_id.as_str()) {
                return Err(StorageError::Sqlite(format!(
                    "import: code {prefix}-{n} did not go to node {}",
                    p.node_id
                )));
            }
        }
    }
    Ok(Ok(store::event_id(seq)))
}

/// The dry run's report (`--json`).
pub fn dry_run_json(plan: &Plan) -> Value {
    json!({
        "ok": true,
        "import_hash": plan.checked.import_hash,
        "manifest_sha256": plan.checked.manifest_sha256,
        "pages": plan.checked.pages.len(),
        "revisions": plan.checked.revisions(),
        "pdfs": plan.checked.pdfs(),
    })
}

/// The pages' node ids and their imported revision counts, for a text report.
pub fn summary(plan: &Plan) -> BTreeMap<String, usize> {
    plan.pages
        .iter()
        .map(|p| (p.node_id.clone(), p.revisions.len()))
        .collect()
}
