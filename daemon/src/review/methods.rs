//! The writer methods PR-4 builds: `kb/review.request`, `.withdraw` (by the
//! requester), `.set_policy` (switch, mode, `required_acts`,
//! `approval_record`, code prefixes) and `.set_code`. Each change and its
//! event commit in one transaction (D7), or neither does.

use std::collections::BTreeSet;

use mae_mcp::protocol::McpError;
use mae_sync::kb::Role as SyncRole;
use serde_json::{json, Map, Value};

use super::store::{self, NewEvent, Performer, Policy, Request, Revision};
use super::{
    actor, gate, optional_bool, optional_str, over_limit, require_role, required_str,
    required_text, signer, storage_fault, time, within_bytes, MAX_CODE_PREFIXES, MAX_REASON_BYTES,
    MAX_REQUEST_NODES, MAX_SET_NAME_BYTES,
};
use crate::doc_store::{DocStore, FreezeError, Frozen};
use crate::kb_query::KbQueryLimits;
use crate::oauth_self_issue::ValidatedPrincipal;
use crate::storage::StorageError;

/// A revision is open while its request can still move: in review, or
/// approved and not yet in force.
pub(super) const OPEN_STATES: &[&str] = &["in_review", "approved"];
/// States a withdrawal can no longer end (E1): only a successor or a
/// derogation ends a revision in force.
pub(super) const ENDED_BY_FORCE: &[&str] = &["in_force", "superseded", "derogated"];

/// An opaque identifier (E9): `prefix-` and 16 random hex digits.
fn new_id(prefix: &str) -> String {
    let mut b = [0u8; 8];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut b);
    format!("{prefix}-{}", hex::encode(b))
}

fn node_not_in_kb() -> McpError {
    McpError::node_not_in_kb("no such node in this KB".into())
}

/// A refusal decided inside a transaction, or the transaction's own failure.
pub(super) type TxnResult<T> = Result<Result<T, McpError>, StorageError>;

pub(super) fn flatten<T>(r: TxnResult<T>) -> Result<T, McpError> {
    r.map_err(storage_fault)?
}

/// `nodes: [{node_id, version}]`: at least one, at most `max` (refused with
/// the cap in `data.limit`), no node twice.
fn parse_nodes(params: &Value, max: usize) -> Result<Vec<(String, String)>, McpError> {
    let bad = |m: &str| McpError::invalid_params("nodes", m.to_string());
    let list = params
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("nodes must be an array of {node_id, version}"))?;
    if list.is_empty() {
        return Err(bad("nodes must name at least one node"));
    }
    if list.len() > max {
        return Err(over_limit(
            "nodes",
            format!("a request names at most {max} nodes"),
            max,
        ));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(list.len());
    for n in list {
        let (Some(node_id), Some(version)) = (
            n.get("node_id").and_then(Value::as_str),
            n.get("version").and_then(Value::as_str),
        ) else {
            return Err(bad("each node needs a string node_id and version"));
        };
        if !seen.insert(node_id.to_string()) {
            return Err(bad("a node is named twice"));
        }
        out.push((node_id.to_string(), version.to_string()));
    }
    Ok(out)
}

/// The first open revision among `nodes`, as `(node_id, state)`.
fn first_open(
    c: &sqlite::Connection,
    kb_id: &str,
    nodes: &[String],
) -> Result<Option<(String, String)>, StorageError> {
    for n in nodes {
        if let Some(r) = store::node_revisions_in(c, kb_id, n, OPEN_STATES)?.first() {
            return Ok(Some((n.clone(), r.state.clone())));
        }
    }
    Ok(None)
}

fn already_open(node_id: &str, state: &str) -> McpError {
    McpError::invalid_transition(
        format!("node {node_id} already has a revision {state}; withdraw it first"),
        state,
        "request",
        None,
    )
}

/// `kb/review.request` (D2, D8, E9): freeze each node at the version the
/// requester names and open one request over them.
pub(super) async fn request(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
    limits: KbQueryLimits,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), true).await?;
    require_role(&g, SyncRole::Editor, "a member with Edit")?;
    let nodes = parse_nodes(params, MAX_REQUEST_NODES.min(limits.max_scan_nodes).max(1))?;
    let reason = required_text(params, "reason")?.to_string();
    within_bytes("reason", &reason, MAX_REASON_BYTES)?;
    let set_name = optional_str(params, "set_name")?.map(str::to_string);
    let name = set_name.as_deref().unwrap_or_default();
    within_bytes("set_name", name, MAX_SET_NAME_BYTES)?;
    let asked = optional_bool(params, "in_force_on_approval")?;
    // D3: a requester may only tighten; loosening is the owner's.
    if asked == Some(true) && !g.policy.in_force_on_approval && g.role != SyncRole::Owner {
        return Err(McpError::role_forbidden(
            "only the owner may loosen in_force_on_approval".into(),
            None,
        ));
    }
    let in_force_on_approval = asked.unwrap_or(g.policy.in_force_on_approval);
    let signer = signer(store)?;
    if nodes.iter().any(|(n, _)| !g.coll.has_node(n)) {
        return Err(node_not_in_kb());
    }
    let ids: Vec<String> = nodes.iter().map(|(n, _)| n.clone()).collect();
    let (kb, check) = (kb_id.to_string(), ids.clone());
    if let Some((n, s)) = store
        .review_read(move |c| first_open(c, &kb, &check))
        .await
        .map_err(storage_fault)?
    {
        return Err(already_open(&n, &s));
    }
    // Held to the commit, so a switch to E2E serializes with this request
    // (`doc_store_e2e_switch`): under it, the KB must still be plaintext.
    let _switch = store.e2e_switch_gate(kb_id).await;
    still_plaintext(store, kb_id).await?;
    let mut frozen = Vec::with_capacity(nodes.len());
    for (node_id, version) in &nodes {
        frozen.push(match store.review_freeze(kb_id, node_id, version).await {
            Ok(f) => f,
            Err(FreezeError::Stale { current }) => {
                return Err(McpError::stale_version(
                    "node has changed since that version".into(),
                    node_id,
                    &current,
                ))
            }
            Err(FreezeError::Unreadable(m)) => return Err(McpError::internal_fault(m)),
            Err(FreezeError::Storage(e)) => return Err(storage_fault(e)),
        });
    }
    let req = Request {
        ord: 0,
        request_id: new_id("rq"),
        kb_id: kb_id.to_string(),
        set_name,
        in_force_on_approval,
        reason,
        requester: who.principal.clone(),
        requester_kind: who.kind.as_str().to_string(),
        created_at: store.now_ms() as i64,
        ended_at: None,
        end_reason: None,
    };
    let actor = actor(who);
    let out = store
        .review_write(move |c| record_request(c, &signer, req, frozen, ids, actor))
        .await;
    flatten(out)
}

/// Refuse `e2e_unsupported` when `kb_id` has become E2E since the gate read
/// it. Called under the KB's switch gate.
async fn still_plaintext(store: &DocStore, kb_id: &str) -> Result<(), McpError> {
    let coll = crate::collab_handler::load_collection(store, kb_id)
        .await
        .map_err(|r| {
            tracing::warn!(kb_id, error = %r.message, "publication review: collection unreadable");
            McpError::internal_fault("the KB's collection could not be read".into())
        })?;
    if store.collection_is_e2e(kb_id, &coll).await {
        return Err(McpError::e2e_unsupported(
            "publication review serves plaintext KBs only".into(),
        ));
    }
    Ok(())
}

/// The request's rows and its `request` event, in the caller's transaction.
/// Re-checks that no node is already open: two requests racing past the
/// first check are decided here, under the writer, and the loser writes
/// nothing (its freeze versions stay, as plain history).
fn record_request(
    c: &sqlite::Connection,
    signer: &mae_mcp::identity::Identity,
    req: Request,
    frozen: Vec<Frozen>,
    ids: Vec<String>,
    actor: Value,
) -> TxnResult<Value> {
    if let Some((n, s)) = first_open(c, &req.kb_id, &ids)? {
        return Ok(Err(already_open(&n, &s)));
    }
    store::insert_request(c, &req)?;
    let approval_record = store::policy(c, &req.kb_id)?.approval_record; // E3: when requested
    let mut answer = Vec::with_capacity(frozen.len());
    let mut logged = Vec::with_capacity(frozen.len());
    for (i, f) in frozen.into_iter().enumerate() {
        let node_id = f.node.id.clone();
        let replaces = store::node_revisions_in(c, &req.kb_id, &node_id, &["in_force"])?
            .last()
            .map(|r| r.revision_id.clone());
        let rev = Revision {
            revision_id: format!("{}-{}", req.request_id.replacen("rq-", "rv-", 1), i + 1),
            kb_id: req.kb_id.clone(),
            node_id: node_id.clone(),
            request_id: req.request_id.clone(),
            doc_code: store::node_code(c, &req.kb_id, &node_id)?,
            rev_no: None,
            source_sha256: f.source_sha256.clone(),
            version: f.version.clone(),
            cutoff_wal: f.cutoff_wal,
            history_seq: f.history_seq,
            state: "in_review".into(),
            doc_type: None,
            template_id: None,
            template_sha256: None,
            approval_record: approval_record.clone(),
            replaces,
            created_by: req.requester.clone(),
            created_by_kind: req.requester_kind.clone(),
            created_at: req.created_at,
            approved_at: None,
            in_force_since: None,
            ended_at: None,
        };
        store::put_source(c, &req.kb_id, &f.source_sha256, &f.content)?;
        store::insert_revision(c, &rev)?;
        answer.push(json!({
            "revision_id": rev.revision_id,
            "node_id": node_id,
            "rev": null,
            "source_sha256": rev.source_sha256,
        }));
        logged.push(json!({
            "revision_id": rev.revision_id,
            "node_id": node_id,
            "doc_code": rev.doc_code,
            "version": rev.version,
            "source_sha256": rev.source_sha256,
            "cutoff_wal": rev.cutoff_wal,
            "history_seq": rev.history_seq,
            "replaces": rev.replaces,
        }));
    }
    let seq = store::append_event(
        c,
        signer,
        NewEvent {
            kb_id: &req.kb_id,
            kind: "request",
            at: time(req.created_at),
            actor,
            payload: json!({
                "request_id": req.request_id,
                "set_name": req.set_name,
                "in_force_on_approval": req.in_force_on_approval,
                "reason": req.reason,
                "revisions": logged,
            }),
        },
    )?;
    Ok(Ok(json!({
        "request_id": req.request_id,
        "event_id": store::event_id(seq),
        "state": "in_review",
        "revisions": answer,
    })))
}

/// `kb/review.withdraw` (E2), PR-4's part: by the requester, before entry
/// into force; it ends the whole request, every member `withdrawn`. A listed
/// approver's withdrawal comes with the lists (PR-5): until then, anyone
/// else is `not_approver`.
pub(super) async fn withdraw(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), true).await?;
    let request_id = required_str(params, "request_id")?.to_string();
    let reason = required_text(params, "reason")?.to_string();
    within_bytes("reason", &reason, MAX_REASON_BYTES)?;
    // E2: a listed approver who is a person and an author of no member may
    // withdraw too; anyone else gets the requester's refusal.
    let approver =
        super::acts::approver_may_withdraw(store, kb_id, &request_id, who, &g.policy.approvers)
            .await?;
    let signer = signer(store)?;
    let (kb, principal, actor) = (kb_id.to_string(), who.principal.clone(), actor(who));
    let at = store.now_ms() as i64;
    flatten(
        store
            .review_write(move |c| {
                let who = Withdrawer {
                    principal: &principal,
                    approver,
                };
                record_withdrawal(c, &signer, &kb, &request_id, &reason, who, actor, at)
            })
            .await,
    )
}

/// Who is withdrawing: their principal, and whether they qualified as a
/// listed approver (checked before the write, E2).
struct Withdrawer<'a> {
    principal: &'a str,
    approver: bool,
}

#[allow(clippy::too_many_arguments)]
fn record_withdrawal(
    c: &sqlite::Connection,
    signer: &mae_mcp::identity::Identity,
    kb_id: &str,
    request_id: &str,
    reason: &str,
    who: Withdrawer<'_>,
    actor: Value,
    at: i64,
) -> TxnResult<Value> {
    let Some(req) = store::request(c, kb_id, request_id)? else {
        return Ok(Err(McpError::unknown_request(
            "no such request in this KB".into(),
        )));
    };
    // Before anything about its state: a non-requester learns nothing of where
    // the request stands (open, ended or in force) from the refusal.
    if req.requester != who.principal && !who.approver {
        return Ok(Err(McpError::not_approver(
            "only the requester or a listed approver may withdraw this request".into(),
        )));
    }
    let members = store::request_revisions(c, request_id)?;
    if let Some(r) = members
        .iter()
        .find(|r| !OPEN_STATES.contains(&r.state.as_str()))
    {
        let message = if ENDED_BY_FORCE.contains(&r.state.as_str()) {
            "a revision in force is ended only by a successor or a derogation"
        } else {
            "the request has already ended"
        };
        return Ok(Err(McpError::invalid_transition(
            message.into(),
            &r.state,
            "withdraw",
            None,
        )));
    }
    store::end_request_revisions(c, request_id, "withdrawn", at)?;
    store::end_request(c, request_id, at, "withdrawn")?;
    let seq = store::append_event(
        c,
        signer,
        NewEvent {
            kb_id,
            kind: "withdraw",
            at: time(at),
            actor,
            payload: json!({
                "request_id": request_id,
                "reason": reason,
                "revisions": members.iter().map(|r| r.revision_id.clone()).collect::<Vec<_>>(),
            }),
        },
    )?;
    Ok(Ok(json!({ "event_id": store::event_id(seq) })))
}

/// The policy fields a later PR adds (refused until then, never silently
/// ignored).
const LATER_FIELDS: &[&str] = &[
    "approval_requires_mfa",
    "reauth_max_age_secs",
    "attachment_retention_days",
];
const ACTS: &[&str] = &["reviewed", "approved"];
const RECORDS: &[&str] = &["act", "signed_pdf"];

/// A document-code prefix: an uppercase letter, then up to fifteen uppercase
/// letters or digits (`PRO`, `IT`, `REG`).
fn valid_prefix(p: &str) -> bool {
    let mut b = p.bytes();
    b.next().is_some_and(|c| c.is_ascii_uppercase())
        && p.len() <= 16
        && b.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn string_list(params: &Value, name: &str) -> Result<Option<Vec<String>>, McpError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .map(Some)
            .ok_or_else(|| McpError::invalid_params(name, format!("{name} must be strings"))),
        Some(_) => Err(McpError::invalid_params(
            name,
            format!("{name} must be an array"),
        )),
    }
}

/// Most performers one list may name, and the longest group or member
/// string: a policy row is read on every act, so it stays small.
const MAX_PERFORMERS: usize = 64;
const MAX_PERFORMER_BYTES: usize = 256;

/// A performer list from `params`, if present: objects with exactly one of
/// `group` or `member`, distinct, a member a fingerprint (D3, Amendment 3).
fn performer_list(params: &Value, name: &str) -> Result<Option<Vec<Performer>>, McpError> {
    let Some(v) = params.get(name).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let bad = || {
        McpError::invalid_params(
            name,
            format!(
                "{name} must be an array of {{\"group\": name}} or {{\"member\": fingerprint}}"
            ),
        )
    };
    let items = v.as_array().ok_or_else(bad)?;
    if items.len() > MAX_PERFORMERS {
        return Err(over_limit(
            name,
            format!("{name} names at most {MAX_PERFORMERS} performers"),
            MAX_PERFORMERS,
        ));
    }
    let list: Vec<Performer> = items
        .iter()
        .map(Performer::from_json)
        .collect::<Option<_>>()
        .ok_or_else(bad)?;
    for p in &list {
        let (Performer::Group(x) | Performer::Member(x)) = p;
        within_bytes(name, x, MAX_PERFORMER_BYTES)?;
        if let Performer::Member(m) = p {
            if !crate::identity_binding::is_fingerprint(m) {
                return Err(bad());
            }
        }
    }
    let unique: BTreeSet<_> = list
        .iter()
        .map(Performer::to_json)
        .map(|j| j.to_string())
        .collect();
    if unique.len() != list.len() {
        return Err(bad());
    }
    Ok(Some(list))
}

/// The policy as `params` would leave it, and the reason, if one is needed.
fn proposed_policy(current: &Policy, params: &Value) -> Result<(Policy, Option<String>), McpError> {
    if let Some(f) = LATER_FIELDS.iter().find(|f| params.get(**f).is_some()) {
        return Err(McpError::invalid_params(
            f,
            format!("{f} is not supported by this daemon yet"),
        ));
    }
    let mut p = current.clone();
    if let Some(b) = optional_bool(params, "review_enabled")? {
        p.review_enabled = b;
    }
    if let Some(b) = optional_bool(params, "approval_requires_rendered_pdf")? {
        p.approval_requires_rendered_pdf = b;
    }
    if let Some(b) = optional_bool(params, "in_force_on_approval")? {
        p.in_force_on_approval = b;
    }
    if let Some(acts) = string_list(params, "required_acts")? {
        let set: BTreeSet<&str> = acts.iter().map(String::as_str).collect();
        if set.len() != acts.len()
            || !set.contains("approved")
            || set.iter().any(|a| !ACTS.contains(a))
        {
            return Err(McpError::invalid_params(
                "required_acts",
                "required_acts must include \"approved\", may add \"reviewed\", and names each once"
                    .into(),
            ));
        }
        p.required_acts = acts;
    }
    if let Some(l) = performer_list(params, "reviewers")? {
        p.reviewers = l;
    }
    if let Some(l) = performer_list(params, "approvers")? {
        p.approvers = l;
    }
    if let Some(r) = optional_str(params, "approval_record")? {
        if !RECORDS.contains(&r) {
            return Err(McpError::invalid_params(
                "approval_record",
                "approval_record must be \"act\" or \"signed_pdf\"".into(),
            ));
        }
        p.approval_record = r.to_string();
    }
    if let Some(prefixes) = string_list(params, "code_prefixes")? {
        if prefixes.len() > MAX_CODE_PREFIXES {
            return Err(over_limit(
                "code_prefixes",
                format!("a KB declares at most {MAX_CODE_PREFIXES} code prefixes"),
                MAX_CODE_PREFIXES,
            ));
        }
        let unique: BTreeSet<&String> = prefixes.iter().collect();
        if unique.len() != prefixes.len() || !prefixes.iter().all(|x| valid_prefix(x)) {
            return Err(McpError::invalid_params(
                "code_prefixes",
                "code_prefixes must be distinct uppercase prefixes such as \"PRO\"".into(),
            ));
        }
        p.code_prefixes = prefixes;
    }
    let reason = optional_str(params, "reason")?
        .filter(|r| !r.trim().is_empty())
        .map(str::to_string);
    if let Some(r) = &reason {
        within_bytes("reason", r, MAX_REASON_BYTES)?;
    }
    // E3: changing what counts as the record of approval needs a reason.
    if p.approval_record != current.approval_record && reason.is_none() {
        return Err(McpError::invalid_params(
            "reason",
            "reason is required when approval_record changes".into(),
        ));
    }
    Ok((p, reason))
}

/// The policy as the wire shows it (E9): every field, the ones a later PR
/// builds at the values that hold until then.
fn policy_json(p: &Policy) -> Value {
    json!({
        "review_enabled": p.review_enabled,
        "in_force_on_approval": p.in_force_on_approval,
        "required_acts": p.required_acts,
        "reviewers": p.reviewers.iter().map(Performer::to_json).collect::<Vec<_>>(),
        "approvers": p.approvers.iter().map(Performer::to_json).collect::<Vec<_>>(),
        "approval_requires_mfa": false,
        "approval_requires_rendered_pdf": p.approval_requires_rendered_pdf,
        "approval_record": p.approval_record,
        "reauth_max_age_secs": null,
        "attachment_retention_days": null,
        "code_prefixes": p.code_prefixes,
    })
}

/// The fields that differ between `a` and `b`, with `b`'s values.
fn changes(a: &Policy, b: &Policy) -> Map<String, Value> {
    let (ja, jb) = (policy_json(a), policy_json(b));
    jb.as_object()
        .expect("an object")
        .iter()
        .filter(|(k, v)| ja.get(k.as_str()) != Some(*v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// `kb/review.set_policy` (D3, E3, E9), owner only. The only method served
/// with review off: it is how review is turned on. Each change is a
/// `policy_changed` event carrying the changed fields and the reason; a call
/// that changes nothing records nothing and answers `event_id: null`.
pub(super) async fn set_policy(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), false).await?;
    if g.role != SyncRole::Owner {
        return Err(McpError::role_forbidden(
            "only the KB owner may change the review policy".into(),
            None,
        ));
    }
    // Validated against the gate's read, and again against the row under the
    // writer, so a concurrent change is never silently overwritten.
    let (proposed, _) = proposed_policy(&g.policy, params)?;
    named_members_are_persons(store, &proposed)?;
    let signer = signer(store)?;
    let (kb, params, actor) = (kb_id.to_string(), params.clone(), actor(who));
    let at = store.now_ms() as i64;
    flatten(
        store
            .review_write(move |c| {
                let current = store::policy(c, &kb)?;
                let (next, reason) = match proposed_policy(&current, &params) {
                    Ok(p) => p,
                    Err(e) => return Ok(Err(e)),
                };
                let mut changed = changes(&current, &next);
                if changed.is_empty() {
                    return Ok(Ok(
                        json!({ "event_id": null, "policy": policy_json(&next) }),
                    ));
                }
                store::put_policy(c, &kb, &next)?;
                changed.insert("reason".into(), json!(reason));
                let seq = store::append_event(
                    c,
                    &signer,
                    NewEvent {
                        kb_id: &kb,
                        kind: "policy_changed",
                        at: time(at),
                        actor,
                        payload: Value::Object(changed),
                    },
                )?;
                Ok(Ok(json!({
                    "event_id": store::event_id(seq),
                    "policy": policy_json(&next),
                })))
            })
            .await,
    )
}

/// ADR-117 D8: a member named as a performer must be bound as a person, or no
/// act of theirs could ever pass `agent_forbidden`. Without identity bindings
/// nobody is a person, so a named member is refused.
fn named_members_are_persons(store: &DocStore, p: &Policy) -> Result<(), McpError> {
    let members: Vec<&String> = p
        .reviewers
        .iter()
        .chain(&p.approvers)
        .filter_map(|x| match x {
            Performer::Member(m) => Some(m),
            Performer::Group(_) => None,
        })
        .collect();
    if members.is_empty() {
        return Ok(());
    }
    let persons: BTreeSet<String> = match store.bindings() {
        Some(b) => b
            .list()
            .map_err(|e| {
                tracing::warn!(error = %e, "publication review: binding table unreadable");
                McpError::internal_fault("the identity bindings could not be read".into())
            })?
            .into_iter()
            .filter(|b| b.kind == crate::oauth_self_issue::PrincipalKind::Person)
            .map(|b| b.fingerprint)
            .collect(),
        None => BTreeSet::new(),
    };
    match members.into_iter().find(|m| !persons.contains(*m)) {
        Some(m) => Err(McpError::performer_not_person(
            format!("{m} is not bound as a person"),
            m,
        )),
        None => Ok(()),
    }
}

/// `PREFIX-n`, `n` a positive integer without leading zeros.
fn parse_code(code: &str) -> Option<(&str, i64)> {
    let (prefix, n) = code.split_once('-')?;
    let ok = valid_prefix(prefix)
        && !n.is_empty()
        && n.len() <= 15
        && n.bytes().all(|b| b.is_ascii_digit())
        && !n.starts_with('0');
    ok.then(|| n.parse().ok().map(|n| (prefix, n))).flatten()
}

/// `kb/review.set_code` (D2), owner only: assign a node its document code,
/// unique per `(KB, prefix, number)` and from one of the KB's prefixes. A
/// number is never freed, so it names one document for good.
pub(super) async fn set_code(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), true).await?;
    if g.role != SyncRole::Owner {
        return Err(McpError::role_forbidden(
            "only the KB owner may assign document codes".into(),
            None,
        ));
    }
    let node_id = required_str(params, "node_id")?.to_string();
    let code = required_str(params, "doc_code")?.to_string();
    let bad_code = || {
        McpError::invalid_params(
            "doc_code",
            "doc_code must be one of this KB's prefixes, a dash and a number (PRO-1)".into(),
        )
    };
    let (prefix, number) = parse_code(&code).ok_or_else(bad_code)?;
    let prefix = prefix.to_string();
    if !g.coll.has_node(&node_id) {
        return Err(node_not_in_kb());
    }
    let signer = signer(store)?;
    let (kb, actor) = (kb_id.to_string(), actor(who));
    let at = store.now_ms() as i64;
    flatten(
        store
            .review_write(move |c| {
                if !store::policy(c, &kb)?.code_prefixes.contains(&prefix) {
                    return Ok(Err(bad_code()));
                }
                if let Some(holder) = store::code_holder(c, &kb, &prefix, number)? {
                    if holder != node_id {
                        return Ok(Err(McpError::doc_code_taken(
                            format!("{code} is already assigned"),
                            &holder,
                        )));
                    }
                }
                let previous = store::node_code(c, &kb, &node_id)?;
                let seq = store::append_event(
                    c,
                    &signer,
                    NewEvent {
                        kb_id: &kb,
                        kind: "code_set",
                        at: time(at),
                        actor,
                        payload: json!({
                            "node_id": node_id,
                            "doc_code": code,
                            "previous": previous,
                        }),
                    },
                )?;
                store::assign_code(c, &kb, &prefix, number, &node_id, seq)?;
                Ok(Ok(
                    json!({ "event_id": store::event_id(seq), "doc_code": code }),
                ))
            })
            .await,
    )
}
