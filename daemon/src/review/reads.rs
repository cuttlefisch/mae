//! The reads PR-4 builds: `kb/review.status`, `.pending` and `.events`, and
//! `kb/query.revisions` and `kb/query.get` with `revision`. Each answers from
//! one read snapshot of the review tables.
//!
//! No principal of anyone but the caller is ever returned by `status` or
//! `pending`: they are readable by every reader of the KB, and who is a member
//! is not (ADR-117 D1). `requested_by` carries a label, which ADR-117 D8 will
//! fill; the chain itself, which names principals, is `events`, owner only.

use mae_mcp::protocol::McpError;
use mae_sync::kb::history::decode_content;
use mae_sync::kb::{KbNodeDoc, Role as SyncRole};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::chain::{self, ZERO_HASH};
use super::store::{self, Policy, Request, Revision};
use super::{cursor_position, gate, optional_str, page_limit, required_str, storage_fault, time};
use crate::doc_store::DocStore;
use crate::kb_query::KbQueryLimits;
use crate::oauth_self_issue::ValidatedPrincipal;

fn node_not_in_kb() -> McpError {
    McpError::node_not_in_kb("no such node in this KB".into())
}

fn t(ms: Option<i64>) -> Value {
    ms.map_or(Value::Null, |m| json!(time(m)))
}

/// What a revision still waits for (D8): the capacities its KB's
/// `required_acts` name that no one has performed yet (`done`), each with the
/// list that may perform it. `performer` is the list's first entry (what a
/// one-entry list, the common case, shows) and `performers` the whole list.
fn waiting_for(p: &Policy, revision_id: Option<&str>, done: &[String]) -> Vec<Value> {
    p.required_acts
        .iter()
        .filter(|a| !done.contains(a))
        .map(|a| {
            let (capacity, list) = if a == "reviewed" {
                ("review", &p.reviewers)
            } else {
                ("approve", &p.approvers)
            };
            let all: Vec<Value> = list.iter().map(store::Performer::to_json).collect();
            let mut w = json!({
                "capacity": capacity,
                "performer": all.first().cloned(),
                "performers": all,
            });
            if let Some(id) = revision_id {
                w["revision_id"] = json!(id);
            }
            w
        })
        .collect()
}

use store::EndNotes;

/// One revision as `status` and `kb/query.revisions` list it (E9).
fn revision_entry(
    r: &Revision,
    all: &[Revision],
    req: Option<&Request>,
    notes: &EndNotes,
) -> Value {
    let (reason, with) = notes.get(&r.revision_id).cloned().unwrap_or_default();
    let superseded_by = all
        .iter()
        .find(|o| o.replaces.as_deref() == Some(&r.revision_id) && o.state != "in_review")
        .filter(|_| r.state == "superseded")
        .map(|o| o.revision_id.clone());
    json!({
        "revision_id": r.revision_id,
        "rev": r.rev_no,
        "state": r.state,
        "request_id": r.request_id,
        "set_name": req.and_then(|q| q.set_name.clone()),
        "source_sha256": r.source_sha256,
        "version": r.version,
        "created_at": time(r.created_at),
        "approved_at": t(r.approved_at),
        "in_force_since": t(r.in_force_since),
        "ended_at": t(r.ended_at),
        "reason": reason,
        "rejected_with": with,
        "replaces": r.replaces,
        "superseded_by": superseded_by,
        "imported": null,
        "signed_pdf_sha256": null,
        "approval_record": r.approval_record,
    })
}

/// A node's review record, from one snapshot.
struct NodeRecord {
    revs: Vec<Revision>,
    reqs: Vec<Request>,
    code: Option<String>,
    notes: EndNotes,
    /// The acts already done on each revision still in review.
    done: std::collections::HashMap<String, Vec<String>>,
    /// Each request's members, for the approved revision's `waiting_for`.
    members: std::collections::HashMap<String, Vec<Revision>>,
}

async fn node_record(store: &DocStore, kb_id: &str, node_id: &str) -> Result<NodeRecord, McpError> {
    let (kb, n) = (kb_id.to_string(), node_id.to_string());
    store
        .review_read(move |c| {
            let revs = store::node_revisions(c, &kb, &n)?;
            let mut reqs = Vec::new();
            let (mut done, mut members) = (Default::default(), Default::default());
            for r in &revs {
                if !reqs.iter().any(|q: &Request| q.request_id == r.request_id) {
                    reqs.extend(store::request(c, &kb, &r.request_id)?);
                }
                if r.state == "in_review" {
                    let acts = store::acts_of(c, &r.revision_id)?
                        .into_iter()
                        .map(|(a, _)| a)
                        .collect();
                    let d: &mut std::collections::HashMap<String, Vec<String>> = &mut done;
                    d.insert(r.revision_id.clone(), acts);
                }
                if r.state == "approved" {
                    let m: &mut std::collections::HashMap<String, Vec<Revision>> = &mut members;
                    m.insert(
                        r.request_id.clone(),
                        store::request_revisions(c, &r.request_id)?,
                    );
                }
            }
            Ok(NodeRecord {
                code: store::node_code(c, &kb, &n)?,
                notes: store::end_notes(c, &kb, &n)?,
                revs,
                reqs,
                done,
                members,
            })
        })
        .await
        .map_err(storage_fault)
}

/// What an approved revision waits for (E9): the rest of its set, its signed
/// PDF (E3, O19), or an entry into force.
fn approved_waits_for(r: &Revision, members: &[Revision]) -> &'static str {
    if members.iter().any(|m| m.state != "approved") {
        "set"
    } else if r.approval_record == "signed_pdf" {
        "signed_pdf"
    } else {
        "enter_into_force"
    }
}

fn request_of<'a>(reqs: &'a [Request], r: &Revision) -> Option<&'a Request> {
    reqs.iter().find(|q| q.request_id == r.request_id)
}

/// The working copy's `version`, `None` when the node has no readable content.
async fn working_version(store: &DocStore, kb_id: &str, node_id: &str) -> Option<String> {
    let name = mae_sync::kb_node_doc_name(kb_id, node_id);
    if !store.has_durable_doc(&name).await {
        return None;
    }
    let state = store.encode_state(&name).await.ok()?;
    KbNodeDoc::from_bytes(&state).ok().map(|d| d.version())
}

/// `kb/review.status {kb_id, node_id}` (D8, E9): where one document is in
/// the flow. `draft` is exactly "the working copy's version differs from the
/// latest revision's frozen version"; a node never requested is a draft.
pub(super) async fn status(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), true).await?;
    let node_id = required_str(params, "node_id")?.to_string();
    let NodeRecord {
        revs,
        reqs,
        code,
        notes,
        done,
        members,
    } = node_record(store, kb_id, &node_id).await?;
    if !g.coll.has_node(&node_id) && revs.is_empty() {
        return Err(node_not_in_kb());
    }
    let working = working_version(store, kb_id, &node_id).await;
    let draft = revs
        .last()
        .is_none_or(|r| Some(&r.version) != working.as_ref());
    let latest_in = |state: &str| revs.iter().rev().find(|r| r.state == state);
    let in_review = latest_in("in_review").map(|r| {
        json!({
            "request_id": r.request_id,
            "revision_id": r.revision_id,
            "set_name": request_of(&reqs, r).and_then(|q| q.set_name.clone()),
            "waiting_for": waiting_for(&g.policy, None, done.get(&r.revision_id).map_or(&[], |v| v)),
        })
    });
    let approved = latest_in("approved").map(|r| {
        json!({
            "request_id": r.request_id,
            "revision_id": r.revision_id,
            "rev": r.rev_no,
            "approved_at": t(r.approved_at),
            "waiting_for": approved_waits_for(r, members.get(&r.request_id).map_or(&[], |v| v)),
        })
    });
    let in_force = latest_in("in_force").map(|r| {
        json!({
            "revision_id": r.revision_id,
            "rev": r.rev_no,
            "since": t(r.in_force_since),
            "approval_record": r.approval_record,
            "signed_pdf_sha256": null,
        })
    });
    let entries: Vec<Value> = revs
        .iter()
        .map(|r| revision_entry(r, &revs, request_of(&reqs, r), &notes))
        .collect();
    Ok(json!({
        "kb_id": kb_id,
        "node_id": node_id,
        "doc_code": code,
        "approval_record": g.policy.approval_record,
        "draft": draft,
        "working_version": working,
        "in_review": in_review,
        "approved": approved,
        "in_force": in_force,
        "revisions": entries,
    }))
}

/// `kb/review.pending {kb_id, cursor?, limit?}` (D8, E9): the KB's open
/// requests, oldest first, paged by an opaque cursor.
pub(super) async fn pending(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
    limits: KbQueryLimits,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), true).await?;
    let after = cursor_position(params)?;
    let limit = page_limit(params, limits)?;
    let kb = kb_id.to_string();
    let page = store
        .review_read(move |c| {
            if let Some(a) = after {
                if !store::request_ord_exists(c, &kb, a)? {
                    return Ok(None);
                }
            }
            let reqs = store::open_requests(c, &kb, after.unwrap_or(0), limit + 1)?;
            let mut out = Vec::with_capacity(reqs.len());
            for q in reqs {
                let members = store::request_revisions(c, &q.request_id)?;
                out.push((q, members));
            }
            Ok(Some(out))
        })
        .await
        .map_err(storage_fault)?
        .ok_or_else(|| {
            McpError::invalid_params("cursor", "cursor is not one this method issued".into())
        })?;
    let more = page.len() > limit;
    let page = &page[..page.len().min(limit)];
    let requests: Vec<Value> = page
        .iter()
        .map(|(q, members)| {
            let waiting: Vec<Value> = members
                .iter()
                .filter(|r| r.state == "in_review")
                .flat_map(|r| waiting_for(&g.policy, Some(&r.revision_id), &[]))
                .collect();
            let state = if members.iter().all(|r| r.state == "approved") {
                "approved"
            } else {
                "in_review"
            };
            json!({
                "request_id": q.request_id,
                "set_name": q.set_name,
                "state": state,
                "requested_by": { "label": null },
                "requested_at": time(q.created_at),
                "reason": q.reason,
                "in_force_on_approval": q.in_force_on_approval,
                "revisions": members.iter().map(|r| json!({
                    "revision_id": r.revision_id,
                    "node_id": r.node_id,
                    "doc_code": r.doc_code,
                    "state": r.state,
                })).collect::<Vec<_>>(),
                "waiting_for": waiting,
            })
        })
        .collect();
    let next = more
        .then(|| page.last().map(|(q, _)| q.ord.to_string()))
        .flatten();
    Ok(json!({ "kb_id": kb_id, "requests": requests, "next_cursor": next }))
}

/// The chain's header, every event from `after` (up to `limit`, 0 for all),
/// whether more follow, and the signed head over the whole chain as it stands
/// (E10). `None` for the header's key and the head when this daemon has no
/// signing identity.
pub(crate) async fn chain_page(
    store: &DocStore,
    kb_id: &str,
    after: u64,
    limit: usize,
) -> Result<(Value, Vec<Value>, bool, Value), McpError> {
    let kb = kb_id.to_string();
    let fetch = if limit == 0 { 0 } else { limit + 1 };
    let (rows, last) = store
        .review_read(move |c| {
            Ok((
                store::events(c, &kb, after, fetch)?,
                store::last_event(c, &kb)?,
            ))
        })
        .await
        .map_err(storage_fault)?;
    let more = limit != 0 && rows.len() > limit;
    let lines = rows
        .iter()
        .take(if limit == 0 { rows.len() } else { limit })
        .map(store::EventRow::export_line)
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_fault)?;
    let signer = store.signer().cloned();
    let header = match &signer {
        Some(s) => chain::header_line(kb_id, &s.public().to_bytes()),
        None => {
            json!({"type": "header", "format": chain::EXPORT_FORMAT, "kb_id": kb_id, "daemon_key": null})
        }
    };
    let head = match &signer {
        None => Value::Null,
        Some(s) => {
            let (seq, hash) = last.map_or((0, ZERO_HASH.to_string()), |l| (l.seq, l.entry_hash));
            chain::head_line(kb_id, seq, &hash, &time(store.now_ms() as i64), s).map_err(|e| {
                tracing::warn!(kb_id, error = %e, "publication review: head not signed");
                McpError::internal_fault("the event chain's head could not be signed".into())
            })?
        }
    };
    Ok((header, lines, more, head))
}

/// The whole chain as E10's JSON Lines export: header, events, signed head.
pub async fn export_lines(store: &DocStore, kb_id: &str) -> Result<Vec<Value>, McpError> {
    let (header, events, _, head) = chain_page(store, kb_id, 0, 0).await?;
    if head.is_null() {
        return Err(McpError::signer_unavailable(
            "no daemon identity to sign the export's head with".into(),
        ));
    }
    let mut out = vec![header];
    out.extend(events);
    out.push(head);
    Ok(out)
}

/// `kb/review.events {kb_id, cursor?, limit?}` (E9, E10): the chain, paged,
/// with the current signed head on every page. Owner only: it names
/// principals. (ADR-117 D9's auditor is added with it.)
pub(super) async fn events(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
    limits: KbQueryLimits,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), true).await?;
    if g.role != SyncRole::Owner {
        return Err(McpError::role_forbidden(
            "the event chain is readable by the owner and the auditor only".into(),
            None,
        ));
    }
    let limit = page_limit(params, limits)?;
    let after = match cursor_position(params)? {
        None => 0,
        Some(a) => {
            let kb = kb_id.to_string();
            let last = store
                .review_read(move |c| store::last_event(c, &kb))
                .await
                .map_err(storage_fault)?
                .map_or(0, |l| l.seq);
            if a as u64 > last {
                return Err(McpError::invalid_params(
                    "cursor",
                    "cursor is not one this method issued".into(),
                ));
            }
            a as u64
        }
    };
    let (header, lines, more, head) = chain_page(store, kb_id, after, limit).await?;
    let next = more
        .then(|| {
            lines
                .last()
                .and_then(|l| l["seq"].as_u64())
                .map(|s| s.to_string())
        })
        .flatten();
    Ok(json!({
        "kb_id": kb_id,
        "header": header,
        "events": lines,
        "head": head,
        "next_cursor": next,
    }))
}

/// `kb/query.revisions {kb_id, node_id}`.
pub(super) async fn revisions(
    store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    params: &Value,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, principal, false).await?;
    let node_id = required_str(params, "node_id")?.to_string();
    let NodeRecord {
        revs,
        reqs,
        code,
        notes,
        ..
    } = node_record(store, kb_id, &node_id).await?;
    if !g.coll.has_node(&node_id) && revs.is_empty() {
        return Err(node_not_in_kb());
    }
    let entries: Vec<Value> = revs
        .iter()
        .map(|r| revision_entry(r, &revs, request_of(&reqs, r), &notes))
        .collect();
    Ok(json!({ "kb_id": kb_id, "node_id": node_id, "doc_code": code, "revisions": entries }))
}

/// `kb/query.get {kb_id, revision, node_id?}` (D5): the frozen node as the
/// working view shows a node, its frozen `version`, and its revision record.
/// The source is checked against its sha256 and its version before it is
/// served: a frozen source that no longer matches its record is a fault, not
/// a document.
pub(super) async fn get_revision(
    store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    params: &Value,
    max_body_bytes: usize,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, principal, false).await?;
    let revision_id = required_str(params, "revision")?.to_string();
    let named_node = optional_str(params, "node_id")?.map(str::to_string);
    let (kb, id) = (kb_id.to_string(), revision_id.clone());
    let found = store
        .review_read(move |c| {
            let Some(r) = store::revision(c, &kb, &id)? else {
                return Ok(None);
            };
            let src = store::source(c, &kb, &r.source_sha256)?;
            let acts = store::act_records(c, &r.revision_id)?;
            Ok(Some((r, src, acts)))
        })
        .await
        .map_err(storage_fault)?;
    let unknown = || McpError::unknown_revision("no such revision in this KB".into());
    let (r, src, acts) = found.ok_or_else(unknown)?;
    if named_node.is_some_and(|n| n != r.node_id) {
        return Err(unknown());
    }
    let fault = |what: &str| McpError::internal_fault(format!("revision {revision_id}: {what}"));
    let src = src.ok_or_else(|| fault("its frozen source is missing"))?;
    if hex::encode(Sha256::digest(&src)) != r.source_sha256 {
        return Err(fault("its frozen source does not match its sha256"));
    }
    let node = decode_content(&src).map_err(|e| {
        tracing::warn!(kb_id, revision_id, error = %e, "publication review: source undecodable");
        fault("its frozen source does not decode")
    })?;
    if hex::encode(&node.digest()[..16]) != r.version {
        return Err(fault("its frozen source does not match its version"));
    }
    let truncated = node.body.len() > max_body_bytes;
    let mut out = crate::kb_query::content_fields(&node, g.coll.body_format().as_deref());
    let mut body = node.body.clone();
    if truncated {
        let mut end = max_body_bytes;
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
        body.truncate(end);
    }
    let ended = |state: &str| (r.state == state).then_some(r.ended_at).flatten();
    out.extend(
        json!({
            "kb_id": kb_id,
            "node_id": r.node_id,
            "encryption": "none",
            "version": r.version,
            "body": body,
            "body_truncated": truncated,
            "links": node.links,
            "revision": {
                "revision_id": r.revision_id,
                "doc_code": r.doc_code,
                "rev": r.rev_no,
                "state": r.state,
                "approved_at": t(r.approved_at),
                "in_force_at": t(r.in_force_since),
                "superseded_at": t(ended("superseded")),
                "acts": acts,
                "attachments": [],
            },
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
    );
    Ok(Value::Object(out))
}
