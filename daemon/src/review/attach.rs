//! ADR-113 PR-6: a revision's attachments (D6, B1–B4, B7). Every file reaches
//! the daemon through ADR-115's blob route first (O3); these methods bind an
//! uploaded blob to a revision by hash, where ADR-113's rules apply:
//!
//! - `kb/review.attach_rendered_pdf {kb_id, revision_id, blob_sha256,
//!   source_sha256}`: by the requester only (O13), in any state but rejected
//!   and withdrawn, declaring the revision's own frozen source (D6).
//! - `kb/review.attach_signed_pdf {kb_id, revision_id, blob_sha256}`: by a
//!   listed approver who is a person (B3), once the revision is approved. Under
//!   `signed_pdf` and `in_force_on_approval`, the attach that gives a set's
//!   last member its signed PDF puts the set in force (E3, O19).
//! - `kb/review.annul_attachment {kb_id, revision_id, kind, reason}`: by the
//!   owner or the uploader (B1). Never the rendered PDF an approval bound (B7).
//!
//! Write-once (B1): one current attachment per kind; the same bytes again
//! answer the first attach; other bytes are `attachment_exists`. Both kinds are
//! PDFs (B2): stored as `application/pdf` and beginning `%PDF-`.
//!
//! @ai-caution: [data-loss] Each attach holds the KB's blob binding lock
//! (ADR-115 D5) from the check that the KB holds the blob until the binding
//! is committed, so a sweep, which marks under the same lock, can never
//! collect a blob between the two. The sweep's live set reads the binding
//! (`attach_store::hashes`). A request dropped mid-attach releases the lock
//! safely only because `SqliteBackend::review_txn` runs the op and its
//! `COMMIT` with no await between them: the drop lands before the op (nothing
//! written) or after the commit. Keep it so, or move the guard into the job.
//!
//! Who attaches is B3 and O13 as written: a signed PDF is not author-checked
//! (the approval it records was, and whoever holds the signed original must
//! be able to file it, owner decision 2026-11-25), except when it completes a
//! set, which is an entry into force.

use mae_mcp::protocol::McpError;
use serde_json::{json, Value};

use super::acts::{author_gate, enter_into_force_now, listed, person, revision_of, wrong_state};
use super::attach_store::{self, Annulment, Attachment, KINDS};
use super::methods::{flatten, TxnResult};
use super::store::{self, NewEvent, Revision};
use super::{
    actor, gate, required_str, required_text, signer, storage_fault, time, within_bytes,
    MAX_REASON_BYTES,
};
use crate::blob_store::{sniff::MediaType, BlobMeta, KbGuard};
use crate::doc_store::DocStore;
use crate::oauth_self_issue::ValidatedPrincipal;
use crate::storage::StorageError;

/// B3: the states a signed PDF attaches in. Never a pending one, so no
/// revision rejected later carries a "signed original" of what was never
/// approved.
const SIGNED_STATES: &[&str] = &["approved", "in_force", "superseded", "derogated"];
/// B3: the states a rendered PDF is refused in.
const RENDERED_REFUSED: &[&str] = &["rejected", "withdrawn"];

/// The members of `members` whose record of approval is the signed PDF and
/// that hold none (O19: `signed_pdf_required`'s `missing`).
pub(super) fn without_signed_pdf(
    c: &sqlite::Connection,
    members: &[Revision],
) -> Result<Vec<String>, StorageError> {
    let mut out = Vec::new();
    for m in members.iter().filter(|m| m.approval_record == "signed_pdf") {
        if attach_store::current(c, &m.revision_id, "signed_pdf")?.is_none() {
            out.push(m.revision_id.clone());
        }
    }
    Ok(out)
}

/// B7 inside `approve`'s transaction: the rendered PDF the approval binds.
/// `named`: what the approver passed; `required`: the KB's switch. With the
/// switch off and nothing named, nothing is bound.
pub(super) fn approval_binds(
    c: &sqlite::Connection,
    revision_id: &str,
    named: &Option<String>,
    required: bool,
) -> Result<Result<Option<String>, McpError>, StorageError> {
    if named.is_none() && !required {
        return Ok(Ok(None));
    }
    let Some(named) = named else {
        return Ok(Err(McpError::invalid_params(
            "rendered_pdf_sha256",
            "this KB requires an approval to name the revision's rendered PDF".into(),
        )));
    };
    match attach_store::current(c, revision_id, "rendered_pdf")? {
        None => Ok(Err(McpError::rendered_pdf_required(
            "this revision has no rendered PDF to approve".into(),
        ))),
        Some(cur) if cur.sha256 != *named => Ok(Err(McpError::rendered_pdf_mismatch(
            "that is not this revision's current rendered PDF".into(),
            &cur.sha256,
        ))),
        Some(cur) => Ok(Ok(Some(cur.sha256))),
    }
}

/// A blob of `kb_id` checked to be a PDF, with the KB's binding lock held.
/// `blob_missing` when the KB holds no such blob (or has no attachment
/// store); `attachment_type` when it is not a PDF.
async fn pdf_blob(
    store: &DocStore,
    kb_id: &str,
    sha: &str,
) -> Result<(BlobMeta, KbGuard), McpError> {
    if !crate::blob_store::valid_sha256(sha) {
        return Err(McpError::invalid_params(
            "blob_sha256",
            "blob_sha256 is 64 lowercase hex characters".into(),
        ));
    }
    let missing = || McpError::blob_missing("this KB holds no blob with that hash".into());
    let Some(blobs) = store.blob_store().cloned() else {
        return Err(missing());
    };
    let (kb, sha) = (kb_id.to_string(), sha.to_string());
    let checked = tokio::task::spawn_blocking(move || {
        let guard = blobs.binding_lock(&kb);
        let meta = blobs.get(&kb, &sha)?;
        let head = match &meta {
            Some(_) => starts_pdf(&blobs.blob_path(&kb, &sha))?,
            None => false,
        };
        Ok::<_, crate::blob_store::BlobError>((guard, meta, head))
    })
    .await
    .map_err(|e| McpError::internal_fault(format!("attachment check: {e}")))?
    .map_err(|e| {
        tracing::warn!(error = %e, "publication review: attachment store unreadable");
        McpError::internal_fault("the attachment store could not be read".into())
    })?;
    let (guard, meta, head) = checked;
    let meta = meta.ok_or_else(missing)?;
    if meta.media_type != MediaType::Pdf || !head {
        return Err(McpError::attachment_type(
            "a revision attachment must be a PDF (application/pdf)".into(),
        ));
    }
    Ok((meta, guard))
}

/// Whether the stored file begins `%PDF-` (B2), read from the file itself.
fn starts_pdf(path: &std::path::Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut head = [0u8; 5];
    let mut f = std::fs::File::open(path)?;
    match f.read_exact(&mut head) {
        Ok(()) => Ok(&head == b"%PDF-"),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

/// What an attach writes, decided before its transaction.
struct AttachInput {
    kb: String,
    kind: &'static str,
    revision_id: String,
    meta: BlobMeta,
    declared_source: Option<String>,
    who: ValidatedPrincipal,
    /// Signed PDF only: the set may enter into force here (its author check
    /// covered every member), and the override it recorded.
    completes: Option<Value>,
    /// The sign-in E8's check accepted, when the policy required one: the
    /// event records it.
    satisfied: Option<super::strength::Satisfied>,
}

/// The attach transaction: write-once, the `attach` event, the row, and, for
/// a signed PDF that completes a set, its entry into force (E3).
fn record_attach(
    c: &sqlite::Connection,
    signer: &mae_mcp::identity::Identity,
    i: AttachInput,
    at: i64,
) -> TxnResult<Value> {
    let Some(r) = store::revision(c, &i.kb, &i.revision_id)? else {
        return Ok(Err(McpError::unknown_revision("no such revision".into())));
    };
    if let Some(e) = state_refusal(&r, i.kind) {
        return Ok(Err(e));
    }
    if let Some(cur) = attach_store::current(c, &r.revision_id, i.kind)? {
        if cur.sha256 == i.meta.sha256 {
            // A retry of the same bytes answers the first attach (B1),
            // including whether it put the set in force: that entry is
            // stamped with the attach's own time (`complete_signed_set`).
            let since =
                (i.kind == "signed_pdf" && r.in_force_since == Some(cur.at)).then(|| time(cur.at));
            return Ok(Ok(json!({ "event_id": store::event_id(cur.event_seq),
                "in_force_since": since })));
        }
        return Ok(Err(McpError::attachment_exists(
            format!("this revision already holds a {}", i.kind),
            &cur.sha256,
        )));
    }
    let principal = i.who.principal.clone();
    let nonempty = |s: &str| (!s.is_empty()).then(|| s.to_string());
    let payload = json!({ "revision_id": r.revision_id, "kind": i.kind,
        "sha256": i.meta.sha256, "size": i.meta.size, "content_type": "application/pdf",
        "source_sha256": i.declared_source });
    let seq = store::append_event(
        c,
        signer,
        NewEvent {
            kb_id: &i.kb,
            kind: "attach",
            at: time(at),
            actor: super::strength::actor(&i.who, &i.satisfied),
            payload,
        },
    )?;
    attach_store::insert(
        c,
        &Attachment {
            kb_id: i.kb.clone(),
            revision_id: r.revision_id.clone(),
            kind: i.kind.to_string(),
            seq: attach_store::next_seq(c, &r.revision_id, i.kind)?,
            sha256: i.meta.sha256.clone(),
            size: i.meta.size,
            content_type: "application/pdf".into(),
            declared_source: i.declared_source.clone(),
            uploader: principal,
            uploader_kind: i.who.kind.as_str().to_string(),
            iss: nonempty(&i.who.issuer),
            sub: nonempty(&i.who.subject),
            at,
            event_seq: seq,
            state: "current".into(),
            annulled: None,
        },
    )?;
    let in_force_since = match i.completes {
        Some(ovr) => {
            let actor = super::strength::actor(&i.who, &i.satisfied);
            complete_signed_set(c, signer, &i.kb, &r, at, actor, seq, ovr)?
        }
        None => None,
    };
    Ok(Ok(
        json!({ "event_id": store::event_id(seq), "in_force_since": in_force_since }),
    ))
}

/// B3's states for `kind`, re-read inside the transaction.
fn state_refusal(r: &Revision, kind: &str) -> Option<McpError> {
    let ok = match kind {
        "signed_pdf" => SIGNED_STATES.contains(&r.state.as_str()),
        _ => !RENDERED_REFUSED.contains(&r.state.as_str()),
    };
    (!ok).then(|| wrong_state(&r.state, &format!("attach_{kind}")))
}

/// E3, O19: under `signed_pdf` and `in_force_on_approval`, a set whose every
/// member is approved enters into force when its last signed PDF arrives.
#[allow(clippy::too_many_arguments)]
fn complete_signed_set(
    c: &sqlite::Connection,
    signer: &mae_mcp::identity::Identity,
    kb: &str,
    r: &Revision,
    at: i64,
    actor: Value,
    attach_seq: u64,
    ovr: Value,
) -> Result<Option<String>, StorageError> {
    let Some(req) = store::request(c, kb, &r.request_id)? else {
        return Ok(None);
    };
    let members = store::request_revisions(c, &r.request_id)?;
    let ready = req.in_force_on_approval
        && members.iter().all(|m| m.state == "approved")
        && without_signed_pdf(c, &members)?.is_empty();
    if !ready {
        return Ok(None);
    }
    enter_into_force_now(
        c,
        signer,
        kb,
        &r.request_id,
        &members,
        at,
        at,
        actor,
        Some(attach_seq),
        ovr,
    )?;
    Ok(Some(time(at)))
}

/// Whether this signed PDF would complete its set (every member approved and
/// every other one holding its signed PDF), read before the write: then the
/// author check covers every member, as an entry into force's does.
async fn completing_members(
    store: &DocStore,
    kb_id: &str,
    rev: &Revision,
) -> Result<Option<Vec<Revision>>, McpError> {
    let (kb, rev) = (kb_id.to_string(), rev.clone());
    store
        .review_read(move |c| {
            let Some(req) = store::request(c, &kb, &rev.request_id)? else {
                return Ok(None);
            };
            let members = store::request_revisions(c, &rev.request_id)?;
            let others: Vec<Revision> = members
                .iter()
                .filter(|m| m.revision_id != rev.revision_id)
                .cloned()
                .collect();
            let completes = req.in_force_on_approval
                && rev.approval_record == "signed_pdf"
                && members.iter().all(|m| m.state == "approved")
                && without_signed_pdf(c, &others)?.is_empty();
            Ok(completes.then_some(members))
        })
        .await
        .map_err(storage_fault)
}

/// `kb/review.attach_signed_pdf` (B3, E3).
pub(super) async fn attach_signed_pdf(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), true).await?;
    person(who)?;
    let rev = revision_of(store, kb_id, required_str(params, "revision_id")?).await?;
    if !listed(&g.policy.approvers, who) {
        return Err(McpError::not_approver(
            "not on this KB's approver list".into(),
        ));
    }
    if let Some(e) = state_refusal(&rev, "signed_pdf") {
        return Err(e);
    }
    let satisfied = super::strength::required(store, &g.policy, who)?;
    let sha = required_str(params, "blob_sha256")?;
    // The binding lock first: attaches to one KB then run one at a time from
    // this check to their commit, so of two approvers attaching a set's last
    // two signed PDFs at once, the second sees the first's and completes it.
    let (meta, guard) = pdf_blob(store, kb_id, sha).await?;
    let completes = match completing_members(store, kb_id, &rev).await? {
        Some(members) => Some(author_gate(store, &members, who, params).await?),
        None => None,
    };
    let input = AttachInput {
        kb: kb_id.to_string(),
        kind: "signed_pdf",
        revision_id: rev.revision_id,
        meta,
        declared_source: None,
        who: who.clone(),
        completes,
        satisfied,
    };
    write(store, input, guard).await
}

/// `kb/review.attach_rendered_pdf` (D6, O13): the requester's rendering of
/// the frozen source. Answers `{event_id}`.
pub(super) async fn attach_rendered_pdf(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
) -> Result<Value, McpError> {
    gate(store, kb_id, Some(&who.principal), true).await?;
    let rev = revision_of(store, kb_id, required_str(params, "revision_id")?).await?;
    let (kb, rid) = (kb_id.to_string(), rev.request_id.clone());
    let req = store
        .review_read(move |c| store::request(c, &kb, &rid))
        .await
        .map_err(storage_fault)?;
    if req.is_none_or(|q| q.requester != who.principal) {
        return Err(McpError::role_forbidden(
            "only the requester uploads a revision's rendered PDF".into(),
            None,
        ));
    }
    if let Some(e) = state_refusal(&rev, "rendered_pdf") {
        return Err(e);
    }
    let declared = required_str(params, "source_sha256")?;
    if declared != rev.source_sha256 {
        return Err(McpError::source_mismatch(
            "that is not this revision's frozen source".into(),
            &rev.source_sha256,
        ));
    }
    let sha = required_str(params, "blob_sha256")?;
    let (meta, guard) = pdf_blob(store, kb_id, sha).await?;
    let input = AttachInput {
        kb: kb_id.to_string(),
        kind: "rendered_pdf",
        revision_id: rev.revision_id,
        meta,
        declared_source: Some(declared.to_string()),
        who: who.clone(),
        completes: None,
        // The requester's rendering is not a controlled act (E8).
        satisfied: None,
    };
    let mut out = write(store, input, guard).await?;
    if let Some(o) = out.as_object_mut() {
        o.remove("in_force_since");
    }
    Ok(out)
}

/// The attach transaction, with the binding lock held until it commits.
async fn write(store: &DocStore, input: AttachInput, guard: KbGuard) -> Result<Value, McpError> {
    let signer = signer(store)?;
    let at = store.now_ms() as i64;
    let out = store
        .review_write(move |c| record_attach(c, &signer, input, at))
        .await;
    drop(guard);
    flatten(out)
}

/// `kb/review.annul_attachment {kb_id, revision_id, kind, reason}` (B1): the
/// owner or the uploader marks the current attachment of `kind` annulled; its
/// bytes stay readable and the kind takes one new attach. The rendered PDF
/// an approval bound is never annulled (B7).
pub(super) async fn annul_attachment(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), true).await?;
    let rev = revision_of(store, kb_id, required_str(params, "revision_id")?).await?;
    let kind = required_str(params, "kind")?;
    let Some(kind) = KINDS.iter().find(|k| **k == kind).copied() else {
        return Err(McpError::invalid_params(
            "kind",
            "kind is \"rendered_pdf\" or \"signed_pdf\"".into(),
        ));
    };
    let reason = required_text(params, "reason")?.to_string();
    within_bytes("reason", &reason, MAX_REASON_BYTES)?;
    let owner = g.role == mae_sync::kb::Role::Owner;
    let signer = signer(store)?;
    let (kb, who) = (kb_id.to_string(), who.clone());
    let at = store.now_ms() as i64;
    flatten(
        store
            .review_write(move |c| -> TxnResult<Value> {
                let Some(cur) = attach_store::current(c, &rev.revision_id, kind)? else {
                    return Ok(Err(McpError::invalid_transition(
                        format!("this revision holds no current {kind}"),
                        &rev.state,
                        "annul_attachment",
                        Some("no_attachment"),
                    )));
                };
                if !owner && cur.uploader != who.principal {
                    return Ok(Err(McpError::role_forbidden(
                        "only the KB's owner or the uploader annuls an attachment".into(),
                        None,
                    )));
                }
                if kind == "rendered_pdf"
                    && store::bound_rendered_pdf(c, &rev.revision_id)?.as_deref()
                        == Some(cur.sha256.as_str())
                {
                    let now = store::revision(c, &kb, &rev.revision_id)?
                        .map_or(rev.state.clone(), |r| r.state);
                    return Ok(Err(McpError::invalid_transition(
                        "an approval's rendered PDF cannot be annulled".into(),
                        &now,
                        "annul_attachment",
                        None,
                    )));
                }
                let payload = json!({ "revision_id": rev.revision_id, "kind": kind,
                    "sha256": cur.sha256, "reason": reason });
                let seq = store::append_event(
                    c,
                    &signer,
                    NewEvent {
                        kb_id: &kb,
                        kind: "annul_attachment",
                        at: time(at),
                        actor: actor(&who),
                        payload,
                    },
                )?;
                let by = Annulment {
                    at,
                    by: who.principal.clone(),
                    reason,
                };
                attach_store::annul(c, &rev.revision_id, kind, &by, seq)?;
                Ok(Ok(json!({ "event_id": store::event_id(seq) })))
            })
            .await,
    )
}

/// D5's `attachments` list for a revision read: every attachment in upload
/// order, annulled ones included.
pub(super) fn attachments_json(kb_id: &str, list: &[Attachment]) -> Value {
    Value::Array(
        list.iter()
            .map(|a| {
                json!({
                    "kind": a.kind,
                    "sha256": a.sha256,
                    "size": a.size,
                    "content_type": a.content_type,
                    "url": format!("/kb/{kb_id}/blobs/{}", a.sha256),
                    "annulled": a.annulled.as_ref().map(|n| json!({
                        "at": time(n.at), "by": n.by, "reason": n.reason })),
                })
            })
            .collect(),
    )
}
