//! ADR-113 PR-5: the acts. `kb/review.act` (`reviewed`), `.approve`,
//! `.reject`, `.enter_into_force` and `.derogate`, and the listed approver's
//! side of `.withdraw`.
//!
//! **Who may act** (D3, A1, A3, C3), checked in this order for every act:
//! a person, never an agent (`agent_forbidden`); on the KB's list for the
//! act (`not_reviewer`, `not_approver`); and not an author of the revision
//! (`author_cannot_approve`), where the authors must be known
//! (`authors_unknown`). The author check reads the editor record before the
//! write (it is async); every state check is repeated inside the write.
//!
//! @ai-caution: [security] The author window is everything after the node's
//! last revision that reached approval (D3), read from the editor record up to
//! now: an approver who edited the node after the freeze also counts as an
//! author. That over-counts (it can only refuse), never under-counts.

use std::collections::BTreeSet;

use mae_mcp::protocol::McpError;
use serde_json::{json, Value};

use super::methods::{flatten, TxnResult, ENDED_BY_FORCE, OPEN_STATES};
use super::store::{self, NewEvent, Performer, Revision};
use super::{
    actor, gate, required_str, required_text, signer, storage_fault, time, within_bytes,
    MAX_REASON_BYTES,
};
use crate::doc_editors::{EditorKind, EditorsSince, InternalWriter};
use crate::doc_store::DocStore;
use crate::oauth_self_issue::{PrincipalKind, ValidatedPrincipal};

/// States a revision has once it reached approval: the start of the next
/// revision's author window (D3).
const REACHED_APPROVAL: &[&str] = &["approved", "in_force", "superseded", "derogated"];

/// Whether `who` is on `list`: a named member by principal, or a group the
/// caller's IdP sign-in carries.
pub(super) fn listed(list: &[Performer], who: &ValidatedPrincipal) -> bool {
    let groups: &[String] = who.sign_in.as_ref().map_or(&[], |s| &s.groups);
    list.iter().any(|p| match p {
        Performer::Member(m) => *m == who.principal,
        Performer::Group(g) => groups.iter().any(|x| x == g),
    })
}

/// D3, C3: an agent never acts.
pub(super) fn person(who: &ValidatedPrincipal) -> Result<(), McpError> {
    if who.kind == PrincipalKind::Person {
        Ok(())
    } else {
        Err(McpError::agent_forbidden(
            "only a person may act on a revision; an agent never does".into(),
        ))
    }
}

/// A revision of this KB by id, or `unknown_revision`.
async fn revision_of(store: &DocStore, kb_id: &str, id: &str) -> Result<Revision, McpError> {
    let (kb, id) = (kb_id.to_string(), id.to_string());
    store
        .review_read(move |c| store::revision(c, &kb, &id))
        .await
        .map_err(storage_fault)?
        .ok_or_else(|| McpError::unknown_revision("no such revision in this KB".into()))
}

/// Where `who` stands against a revision's authors (D3, A1, A3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    /// Not an author, and the authors are known.
    Clear,
    /// An author: the requester, or named by the editor record.
    Author,
    /// The authors cannot be established.
    Unknown,
}

/// `who`'s verdict on `rev`. The requester is an author; so is everyone the
/// editor record names since the node's last revision that reached approval.
pub(super) async fn author_verdict(
    store: &DocStore,
    rev: &Revision,
    who: &ValidatedPrincipal,
) -> Result<Verdict, McpError> {
    if rev.created_by == who.principal {
        return Ok(Verdict::Author);
    }
    let (kb, node, ord_of) = (
        rev.kb_id.clone(),
        rev.node_id.clone(),
        rev.revision_id.clone(),
    );
    let since = store
        .review_read(move |c| {
            let all = store::node_revisions(c, &kb, &node)?;
            let mine = all.iter().position(|r| r.revision_id == ord_of);
            Ok(all
                .iter()
                .take(mine.unwrap_or(all.len()))
                .filter(|r| REACHED_APPROVAL.contains(&r.state.as_str()))
                .map(|r| r.cutoff_wal)
                .next_back()
                .unwrap_or(0))
        })
        .await
        .map_err(storage_fault)?;
    let Some(authors) = authors_since(store, rev, since).await? else {
        return Ok(Verdict::Unknown);
    };
    Ok(if authors.contains(&who.principal) {
        Verdict::Author
    } else {
        Verdict::Clear
    })
}

/// The authors of a node since WAL id `since`, or `None` when they cannot be
/// known (A3). The editor record decides, with one addition: the seed's own
/// writes, which name no author, count as authored by the member the operator
/// attested (`kb seed --author`), when the seed write log accounts for every
/// seed write and each one in the window was attested.
///
/// A seed's first write to a node replaces the document (`restore_doc`), which
/// resets the editor record's origin to its own WAL id, so a window opening
/// before it is `Incomplete`. Only such a replacement sets the origin to its
/// own WAL id, so when the origin is a logged, attested seed write, that write
/// replaced everything before it: the window starts there, with its author.
///
/// @ai-caution: [security] The log is written after the document, so a crash
/// between them leaves a seed write unlogged, and a seed's re-run skips the
/// node it already wrote. The seed row's `updates` count must therefore equal
/// the number of logged writes: a seed write the log missed, or one from
/// before the log, leaves the authors unknown, never attested by a later write.
async fn authors_since(
    store: &DocStore,
    rev: &Revision,
    since: u64,
) -> Result<Option<BTreeSet<String>>, McpError> {
    let doc = format!("kbn:{}:{}", rev.kb_id, rev.node_id);
    let (kb, node) = (rev.kb_id.clone(), rev.node_id.clone());
    let log = store
        .review_read(move |c| store::seed_writes(c, &kb, &node))
        .await
        .map_err(storage_fault)?;
    let mut authors = BTreeSet::new();
    let (start, rows) = match store
        .editors_since_wal(&doc, since)
        .await
        .map_err(storage_fault)?
    {
        EditorsSince::Known(rows) => (since, rows),
        EditorsSince::CutoffAhead { .. } => return Ok(None),
        EditorsSince::Incomplete {
            recorded_after_wal_id: origin,
        } => {
            let Some(author) = log
                .iter()
                .find(|w| w.wal_id == origin)
                .and_then(|w| w.author.clone())
            else {
                return Ok(None);
            };
            authors.insert(author);
            match store
                .editors_since_wal(&doc, origin)
                .await
                .map_err(storage_fault)?
            {
                EditorsSince::Known(rows) => (origin, rows),
                _ => return Ok(None),
            }
        }
    };
    let seed = InternalWriter::Seed.as_str();
    authors.extend(
        rows.iter()
            .filter(|r| r.kind == EditorKind::Principal)
            .map(|r| r.editor.clone()),
    );
    let hidden: Vec<_> = rows.iter().filter(|r| r.kind.hides_an_author()).collect();
    let seed_row = match hidden.as_slice() {
        [] => return Ok(Some(authors)),
        [r] if r.kind == EditorKind::Internal && r.editor == seed => r,
        _ => return Ok(None),
    };
    if usize::try_from(seed_row.updates).ok() != Some(log.len()) {
        return Ok(None);
    }
    for w in log.iter().filter(|w| w.wal_id > start) {
        match &w.author {
            Some(a) => authors.insert(a.clone()),
            None => return Ok(None),
        };
    }
    Ok(Some(authors))
}

/// Whether `params` sets the boolean `name` to true.
fn acknowledged(params: &Value, name: &str) -> bool {
    params.get(name).and_then(Value::as_bool) == Some(true)
}

/// D3 and A3 as a soft block (owner decision, 2026-11-25). An author, or a
/// revision whose authors are unknown, is refused (`author_cannot_approve`,
/// `authors_unknown`) unless the call acknowledges it
/// (`acknowledge_self_approval`, `acknowledge_unknown_authors`) and gives an
/// `override_reason`. Then the act goes ahead and its event and act row
/// record the override: what was acknowledged, for which revisions, and why.
/// Returns that record, or `null` when nothing needed acknowledging.
///
/// @ai-caution: [security] The soft block keeps "approver is not the author"
/// as a recorded exception, never a silent one: every override must reach the
/// signed event chain. Agents and the performer lists stay hard.
pub(super) async fn author_gate(
    store: &DocStore,
    revs: &[Revision],
    who: &ValidatedPrincipal,
    params: &Value,
) -> Result<Value, McpError> {
    let (mut authored, mut unknown) = (Vec::new(), Vec::new());
    for r in revs {
        match author_verdict(store, r, who).await? {
            Verdict::Author => authored.push(r.revision_id.clone()),
            Verdict::Unknown => unknown.push(r.revision_id.clone()),
            Verdict::Clear => {}
        }
    }
    if !authored.is_empty() && !acknowledged(params, "acknowledge_self_approval") {
        return Err(McpError::author_cannot_approve(
            "an author acting on their own revision must acknowledge it \
             (acknowledge_self_approval, with an override_reason)"
                .into(),
        ));
    }
    if !unknown.is_empty() && !acknowledged(params, "acknowledge_unknown_authors") {
        return Err(McpError::authors_unknown(
            "the revision's authors cannot be established; acting anyway must be acknowledged \
             (acknowledge_unknown_authors, with an override_reason)"
                .into(),
        ));
    }
    if authored.is_empty() && unknown.is_empty() {
        return Ok(Value::Null);
    }
    let reason = required_text(params, "override_reason")?.to_string();
    within_bytes("override_reason", &reason, MAX_REASON_BYTES)?;
    Ok(json!({
        "self_approval": authored,
        "unknown_authors": unknown,
        "reason": reason,
    }))
}

/// An effective time from the wire (E9): RFC 3339 UTC with milliseconds, not
/// in the future (O20: a scheduled entry into force is not designed).
fn effective_at(params: &Value, now: i64) -> Result<i64, McpError> {
    let s = required_str(params, "effective_at")?;
    let at = crate::rfc3339::parse_utc_millis(s).ok_or_else(|| {
        McpError::invalid_params(
            "effective_at",
            "effective_at must be RFC 3339 UTC with milliseconds, such as 2027-01-22T09:00:00.000Z"
                .into(),
        )
    })?;
    // One minute of clock leeway, as the token validator allows.
    if at > now + 60_000 {
        return Err(McpError::invalid_params(
            "effective_at",
            "effective_at may not be in the future".into(),
        ));
    }
    Ok(at)
}

/// The refusal for a revision not in a state the act accepts.
fn wrong_state(state: &str, act: &str) -> McpError {
    McpError::invalid_transition(
        format!("a revision that is {state} cannot be the subject of {act}"),
        state,
        act,
        None,
    )
}

/// `kb/review.act {kb_id, revision_id, act, comment?}`: a required act other
/// than `approved` (D3), by a listed reviewer who is not an author.
pub(super) async fn act(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), true).await?;
    person(who)?;
    let rev = revision_of(store, kb_id, required_str(params, "revision_id")?).await?;
    let kind = required_str(params, "act")?.to_string();
    if kind == "approved" || !g.policy.required_acts.contains(&kind) {
        return Err(McpError::invalid_params(
            "act",
            "act must be one of this KB's required acts other than approved".into(),
        ));
    }
    if !listed(&g.policy.reviewers, who) {
        return Err(McpError::not_reviewer(
            "not on this KB's reviewer list".into(),
        ));
    }
    let ovr = author_gate(store, std::slice::from_ref(&rev), who, params).await?;
    let comment = super::optional_str(params, "comment")?.map(str::to_string);
    if let Some(c) = &comment {
        within_bytes("comment", c, MAX_REASON_BYTES)?;
    }
    let signer = signer(store)?;
    let (kb, principal, actor) = (kb_id.to_string(), who.principal.clone(), actor(who));
    let at = store.now_ms() as i64;
    let id = rev.revision_id;
    flatten(
        store
            .review_write(move |c| -> TxnResult<Value> {
                let Some(r) = store::revision(c, &kb, &id)? else {
                    return Ok(Err(McpError::unknown_revision("no such revision".into())));
                };
                if r.state != "in_review" {
                    return Ok(Err(wrong_state(&r.state, "act")));
                }
                if store::acts_of(c, &id)?.iter().any(|(_, p)| *p == principal) {
                    return Ok(Err(McpError::invalid_transition(
                        "one person performs one act on a revision (C3)".into(),
                        &r.state,
                        "act",
                        None,
                    )));
                }
                let payload =
                    json!({ "revision_id": id, "act": kind, "comment": comment, "override": ovr });
                let seq = store::append_event(
                    c,
                    &signer,
                    NewEvent {
                        kb_id: &kb,
                        kind: "act",
                        at: time(at),
                        actor,
                        payload,
                    },
                )?;
                store::insert_act(c, &kb, &id, &kind, &principal, at, seq, &ovr)?;
                Ok(Ok(
                    json!({ "event_id": store::event_id(seq), "at": time(at) }),
                ))
            })
            .await,
    )
}

/// The members of a request still waiting for something, as ids.
fn ids(rs: &[Revision], pred: impl Fn(&Revision) -> bool) -> Vec<String> {
    rs.iter()
        .filter(|r| pred(r))
        .map(|r| r.revision_id.clone())
        .collect()
}

/// Put every member of a request in force from `since`, in this one
/// transaction (D4), superseding each node's previous in-force revision.
/// Returns the event's sequence number.
#[allow(clippy::too_many_arguments)]
fn enter_into_force_now(
    c: &sqlite::Connection,
    signer: &mae_mcp::identity::Identity,
    kb_id: &str,
    request_id: &str,
    members: &[Revision],
    since: i64,
    at: i64,
    actor: Value,
    on_approval: Option<u64>,
    ovr: Value,
) -> Result<u64, crate::storage::StorageError> {
    let mut superseded = Vec::new();
    for m in members {
        for prev in store::node_revisions(c, kb_id, &m.node_id)? {
            if prev.state == "in_force" && prev.revision_id != m.revision_id {
                store::end_revision(c, &prev.revision_id, "superseded", since, None, None)?;
                superseded.push(prev.revision_id);
            }
        }
        store::put_in_force(c, &m.revision_id, since)?;
    }
    let payload = json!({
        "request_id": request_id,
        "revisions": members.iter().map(|m| m.revision_id.clone()).collect::<Vec<_>>(),
        "effective_at": time(since),
        "superseded": superseded,
        "on_approval": on_approval.map(store::event_id),
        "override": ovr,
    });
    store::append_event(
        c,
        signer,
        NewEvent {
            kb_id,
            kind: "enter_into_force",
            at: time(at),
            actor,
            payload,
        },
    )
}

/// What `approve` needs before its write, gathered and checked.
struct ApproveInput {
    kb: String,
    id: String,
    principal: String,
    actor: Value,
    required: Vec<String>,
    /// The acknowledged override, `null` when none (`author_gate`).
    ovr: Value,
    /// The author check covered every member of the request, so this
    /// approval may put the set in force (`in_force_on_approval`).
    gated_set: bool,
}

/// The required acts other than `approved` that `acts` lacks, in order, each
/// to be done by someone other than `principal` (C3).
fn awaiting(required: &[String], acts: &[(String, String)], principal: &str) -> Option<String> {
    required
        .iter()
        .filter(|a| a.as_str() != "approved")
        .find(|a| !acts.iter().any(|(k, p)| k == *a && p != principal))
        .cloned()
}

/// `approve`'s write: the revision approved with its number, its act and
/// event; and, when it completes a set that enters into force on approval
/// with the act as its record, the whole set in force in the same
/// transaction (D4, C2). Under `signed_pdf`, the set waits for its PDFs (O19).
fn record_approval(
    c: &sqlite::Connection,
    signer: &mae_mcp::identity::Identity,
    i: ApproveInput,
    at: i64,
) -> TxnResult<Value> {
    let Some(r) = store::revision(c, &i.kb, &i.id)? else {
        return Ok(Err(McpError::unknown_revision("no such revision".into())));
    };
    if r.state != "in_review" {
        return Ok(Err(wrong_state(&r.state, "approve")));
    }
    // C3 needs no check of its own here: every required act must have been
    // done by someone other than the approver (`awaiting`).
    let acts = store::acts_of(c, &i.id)?;
    if let Some(missing) = awaiting(&i.required, &acts, &i.principal) {
        return Ok(Err(McpError::invalid_transition(
            format!("this revision is waiting for its {missing} act first"),
            &r.state,
            "approve",
            None,
        )));
    }
    let rev_no = store::next_rev_no(c, &i.kb, &r.node_id)?;
    store::approve_revision(c, &i.id, rev_no, at)?;
    let payload = json!({ "revision_id": i.id, "rev": rev_no, "request_id": r.request_id,
        "override": i.ovr });
    let seq = store::append_event(
        c,
        signer,
        NewEvent {
            kb_id: &i.kb,
            kind: "approve",
            at: time(at),
            actor: i.actor.clone(),
            payload,
        },
    )?;
    store::insert_act(c, &i.kb, &i.id, "approved", &i.principal, at, seq, &i.ovr)?;
    let req = store::request(c, &i.kb, &r.request_id)?;
    let members = store::request_revisions(c, &r.request_id)?;
    let all_approved = members.iter().all(|m| m.state == "approved");
    let on_approval = req.is_some_and(|q| q.in_force_on_approval);
    let by_act = members.iter().all(|m| m.approval_record != "signed_pdf");
    // Entry into force is an act on every member, so its author check must
    // have covered them all (as `enter_into_force` does). A set that
    // completed after the check stays approved, for an explicit entry.
    let in_force_since = if on_approval && all_approved && by_act && i.gated_set {
        enter_into_force_now(
            c,
            signer,
            &i.kb,
            &r.request_id,
            &members,
            at,
            at,
            i.actor,
            Some(seq),
            i.ovr,
        )?;
        Some(time(at))
    } else {
        None
    };
    let state = if in_force_since.is_some() {
        "in_force"
    } else {
        "approved"
    };
    Ok(Ok(json!({
        "event_id": store::event_id(seq),
        "revision_state": state,
        "in_force_since": in_force_since,
    })))
}

/// `kb/review.approve {kb_id, revision_id}` (D3, D4): by a listed approver who
/// is a person and not an author, once the required acts are done.
pub(super) async fn approve(
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
    let set = completing_set(store, kb_id, &rev).await?;
    let gated_set = set.is_some();
    let checked = set.unwrap_or_else(|| vec![rev.clone()]);
    let ovr = author_gate(store, &checked, who, params).await?;
    let signer = signer(store)?;
    let input = ApproveInput {
        gated_set,
        ovr,
        kb: kb_id.to_string(),
        id: rev.revision_id,
        principal: who.principal.clone(),
        actor: actor(who),
        required: g.policy.required_acts.clone(),
    };
    let at = store.now_ms() as i64;
    flatten(
        store
            .review_write(move |c| record_approval(c, &signer, input, at))
            .await,
    )
}

/// When approving `rev` would complete its request and put the set in force
/// (`in_force_on_approval`, every other member approved, approval by act):
/// every member, for the author check. Otherwise `None`.
async fn completing_set(
    store: &DocStore,
    kb_id: &str,
    rev: &Revision,
) -> Result<Option<Vec<Revision>>, McpError> {
    let (kb, rid) = (kb_id.to_string(), rev.request_id.clone());
    let (req, members) = store
        .review_read(move |c| {
            Ok((
                store::request(c, &kb, &rid)?,
                store::request_revisions(c, &rid)?,
            ))
        })
        .await
        .map_err(storage_fault)?;
    let others_approved = members
        .iter()
        .all(|m| m.revision_id == rev.revision_id || m.state == "approved");
    let by_act = !members.iter().any(|m| m.approval_record == "signed_pdf");
    let completes = req.is_some_and(|q| q.in_force_on_approval) && others_approved && by_act;
    Ok(completes.then_some(members))
}

/// A request's members, read outside the write (for the author checks).
async fn members_of(
    store: &DocStore,
    kb_id: &str,
    request_id: &str,
) -> Result<Vec<Revision>, McpError> {
    let (kb, rid) = (kb_id.to_string(), request_id.to_string());
    let (req, members) = store
        .review_read(move |c| {
            Ok((
                store::request(c, &kb, &rid)?,
                store::request_revisions(c, &rid)?,
            ))
        })
        .await
        .map_err(storage_fault)?;
    if req.is_none() {
        return Err(McpError::unknown_request(
            "no such request in this KB".into(),
        ));
    }
    Ok(members)
}

/// `kb/review.enter_into_force {kb_id, request_id, effective_at}` (D4, C2):
/// every member approved (`set_incomplete`), and under `signed_pdf` every
/// member's signed PDF attached (`signed_pdf_required`, O19).
pub(super) async fn enter_into_force(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), true).await?;
    person(who)?;
    let request_id = required_str(params, "request_id")?.to_string();
    let members = members_of(store, kb_id, &request_id).await?;
    if !listed(&g.policy.approvers, who) {
        return Err(McpError::not_approver(
            "not on this KB's approver list".into(),
        ));
    }
    let now = store.now_ms() as i64;
    let since = effective_at(params, now)?;
    let ovr = author_gate(store, &members, who, params).await?;
    let signer = signer(store)?;
    let (kb, actor) = (kb_id.to_string(), actor(who));
    flatten(
        store
            .review_write(move |c| -> TxnResult<Value> {
                let members = store::request_revisions(c, &request_id)?;
                if let Some(r) = members
                    .iter()
                    .find(|r| !OPEN_STATES.contains(&r.state.as_str()))
                {
                    return Ok(Err(wrong_state(&r.state, "enter_into_force")));
                }
                let missing = ids(&members, |r| r.state != "approved");
                if !missing.is_empty() {
                    return Ok(Err(McpError::set_incomplete(
                        "every member must be approved first".into(),
                        missing,
                    )));
                }
                // PR-6 attaches signed PDFs; until it is built no member has one.
                let no_pdf = ids(&members, |r| r.approval_record == "signed_pdf");
                if !no_pdf.is_empty() {
                    return Ok(Err(McpError::signed_pdf_required(
                        "every member needs its signed PDF before entry into force".into(),
                        no_pdf,
                    )));
                }
                let seq = enter_into_force_now(
                    c,
                    &signer,
                    &kb,
                    &request_id,
                    &members,
                    since,
                    now,
                    actor,
                    None,
                    ovr,
                )?;
                Ok(Ok(json!({ "event_id": store::event_id(seq) })))
            })
            .await,
    )
}

/// `kb/review.reject {kb_id, request_id, revision_id, reason}` (E2): by a
/// listed reviewer or approver who is a person and not an author; it ends the
/// whole request, every member `rejected`, the named one with the reason.
pub(super) async fn reject(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), true).await?;
    person(who)?;
    let request_id = required_str(params, "request_id")?.to_string();
    let revision_id = required_str(params, "revision_id")?.to_string();
    let reason = required_text(params, "reason")?.to_string();
    within_bytes("reason", &reason, MAX_REASON_BYTES)?;
    let members = members_of(store, kb_id, &request_id).await?;
    let Some(named) = members.iter().find(|r| r.revision_id == revision_id) else {
        return Err(McpError::unknown_revision(
            "no such revision in this request".into(),
        ));
    };
    if !listed(&g.policy.reviewers, who) && !listed(&g.policy.approvers, who) {
        return Err(McpError::not_reviewer(
            "not on this KB's reviewer or approver list".into(),
        ));
    }
    let ovr = author_gate(store, std::slice::from_ref(named), who, params).await?;
    let signer = signer(store)?;
    let (kb, actor) = (kb_id.to_string(), actor(who));
    let at = store.now_ms() as i64;
    flatten(
        store
            .review_write(move |c| -> TxnResult<Value> {
                let members = store::request_revisions(c, &request_id)?;
                if let Some(r) = members
                    .iter()
                    .find(|r| !OPEN_STATES.contains(&r.state.as_str()))
                {
                    return Ok(Err(wrong_state(&r.state, "reject")));
                }
                for m in &members {
                    let named = m.revision_id == revision_id;
                    let note = named.then_some(reason.as_str());
                    let with = (!named).then_some(revision_id.as_str());
                    store::end_revision(c, &m.revision_id, "rejected", at, note, with)?;
                }
                store::end_request(c, &request_id, at, "rejected")?;
                let payload = json!({
                    "request_id": request_id,
                    "revision_id": revision_id,
                    "reason": reason,
                    "revisions": ids(&members, |_| true),
                    "override": ovr,
                });
                let seq = store::append_event(
                    c,
                    &signer,
                    NewEvent {
                        kb_id: &kb,
                        kind: "reject",
                        at: time(at),
                        actor,
                        payload,
                    },
                )?;
                Ok(Ok(json!({ "event_id": store::event_id(seq) })))
            })
            .await,
    )
}

/// `kb/review.derogate {kb_id, node_id, effective_at, reason}` (D4): ends the
/// node's in-force revision with no successor, by a listed approver who is a
/// person and not its author; the node leaves the published view.
pub(super) async fn derogate(
    store: &DocStore,
    kb_id: &str,
    who: &ValidatedPrincipal,
    params: &Value,
) -> Result<Value, McpError> {
    let g = gate(store, kb_id, Some(&who.principal), true).await?;
    person(who)?;
    let node_id = required_str(params, "node_id")?.to_string();
    let reason = required_text(params, "reason")?.to_string();
    within_bytes("reason", &reason, MAX_REASON_BYTES)?;
    let now = store.now_ms() as i64;
    let at = effective_at(params, now)?;
    if !listed(&g.policy.approvers, who) {
        return Err(McpError::not_approver(
            "not on this KB's approver list".into(),
        ));
    }
    let (kb, node) = (kb_id.to_string(), node_id.clone());
    let all = store
        .review_read(move |c| store::node_revisions(c, &kb, &node))
        .await
        .map_err(storage_fault)?;
    let Some(current) = all.iter().find(|r| r.state == "in_force").cloned() else {
        let last = all.last().map_or("none", |r| r.state.as_str()).to_string();
        return Err(wrong_state(&last, "derogate"));
    };
    let ovr = author_gate(store, std::slice::from_ref(&current), who, params).await?;
    let signer = signer(store)?;
    let (kb, actor) = (kb_id.to_string(), actor(who));
    flatten(
        store
            .review_write(move |c| -> TxnResult<Value> {
                let Some(r) = store::revision(c, &kb, &current.revision_id)? else {
                    return Ok(Err(McpError::unknown_revision("no such revision".into())));
                };
                if r.state != "in_force" {
                    return Ok(Err(wrong_state(&r.state, "derogate")));
                }
                store::end_revision(c, &r.revision_id, "derogated", at, Some(&reason), None)?;
                let payload = json!({
                    "node_id": node_id,
                    "revision_id": r.revision_id,
                    "effective_at": time(at),
                    "reason": reason,
                    "override": ovr,
                });
                let seq = store::append_event(
                    c,
                    &signer,
                    NewEvent {
                        kb_id: &kb,
                        kind: "derogate",
                        at: time(now),
                        actor,
                        payload,
                    },
                )?;
                Ok(Ok(json!({ "event_id": store::event_id(seq) })))
            })
            .await,
    )
}

/// The listed approver's side of `withdraw` (E2): a person on the approver
/// list who is an author of no member. `Ok(true)` when `who` qualifies.
pub(super) async fn approver_may_withdraw(
    store: &DocStore,
    kb_id: &str,
    request_id: &str,
    who: &ValidatedPrincipal,
    approvers: &[Performer],
) -> Result<bool, McpError> {
    if who.kind != PrincipalKind::Person || !listed(approvers, who) {
        return Ok(false);
    }
    let members = members_of(store, kb_id, request_id).await?;
    if members
        .iter()
        .any(|m| ENDED_BY_FORCE.contains(&m.state.as_str()))
    {
        // Refused by the state check inside the write, as for the requester.
        return Ok(true);
    }
    // Withdrawal keeps the hard rule: an author has the requester's path, and
    // a withdrawal never blocks an official document.
    for m in &members {
        if author_verdict(store, m, who).await? != Verdict::Clear {
            return Ok(false);
        }
    }
    Ok(true)
}
