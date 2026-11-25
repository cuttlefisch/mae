//! ADR-113 PR-5: the acts, through the methods a client calls. Approvers and
//! reviewers are IdP groups the caller's sign-in carries; the viewer is the
//! approver (approving needs the list, not an edit role). Each test names
//! the guard it kills.

use serde_json::{json, Value};

use super::*;
use crate::doc_editors::{Attribution, InternalWriter};
use crate::oauth_self_issue::PrincipalKind;
use crate::sign_in::SignIn;

/// `p` as a person whose sign-in carries `groups`.
pub(super) fn signed_in(p: &str, groups: &[&str], kind: PrincipalKind) -> ValidatedPrincipal {
    let mut v = person(p);
    v.kind = kind;
    v.sign_in = Some(SignIn {
        groups: groups.iter().map(|g| g.to_string()).collect(),
        ..SignIn::default()
    });
    v
}

pub(super) async fn as_(
    env: &Env,
    who: &ValidatedPrincipal,
    method: &str,
    params: Value,
) -> Result<Value, McpError> {
    crate::review::dispatch(method, &params, &env.store, who, limits()).await
}

pub(super) fn approver(env: &Env) -> ValidatedPrincipal {
    signed_in(&env.viewer, &["approvers"], PrincipalKind::Person)
}

/// Review on, with group lists, and the given policy fields on top.
pub(super) async fn policy(env: &Env, extra: Value) {
    let mut p = json!({"kb_id": KB, "review_enabled": true, "code_prefixes": ["PRO"],
        "reviewers": [{"group": "reviewers"}], "approvers": [{"group": "approvers"}]});
    p.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    call(env, &env.owner, "kb/review.set_policy", p)
        .await
        .unwrap();
}

/// The editor edits then requests `nodes`; the revision ids in order, and the request id.
async fn requested(env: &Env, nodes: &[&str]) -> (String, Vec<String>) {
    for n in nodes {
        retitle(env, n, &format!("Edited {n}")).await;
    }
    let r = request(env, &env.editor, nodes).await.unwrap();
    let ids = r["revisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["revision_id"].as_str().unwrap().to_string())
        .collect();
    (r["request_id"].as_str().unwrap().to_string(), ids)
}

pub(super) async fn status(env: &Env, node: &str) -> Value {
    call(
        env,
        &env.owner,
        "kb/review.status",
        json!({"kb_id": KB, "node_id": node}),
    )
    .await
    .unwrap()
}

pub(super) const NOW: &str = "2000-01-01T00:00:00.000Z";

/// Approve then enter into force; a second revision supersedes the first.
/// Kills: the pointer not moving, the previous revision left in force, the
/// revision number not assigned.
#[tokio::test]
async fn approve_then_enter_into_force_then_supersede() {
    let env = env().await;
    policy(&env, json!({})).await;
    let (rq, revs) = requested(&env, &["n1"]).await;
    let a = as_(
        &env,
        &approver(&env),
        "kb/review.approve",
        json!({"kb_id": KB, "revision_id": revs[0]}),
    )
    .await
    .unwrap();
    assert_eq!(a["revision_state"], json!("approved"));
    assert_eq!(
        status(&env, "n1").await["approved"]["waiting_for"],
        json!("enter_into_force")
    );
    as_(
        &env,
        &approver(&env),
        "kb/review.enter_into_force",
        json!({"kb_id": KB, "request_id": rq, "effective_at": NOW}),
    )
    .await
    .unwrap();
    let s = status(&env, "n1").await;
    assert_eq!(s["in_force"]["revision_id"], json!(revs[0]));
    assert_eq!(s["in_force"]["rev"], json!(0));
    let (rq2, revs2) = requested(&env, &["n1"]).await;
    as_(
        &env,
        &approver(&env),
        "kb/review.approve",
        json!({"kb_id": KB, "revision_id": revs2[0]}),
    )
    .await
    .unwrap();
    as_(
        &env,
        &approver(&env),
        "kb/review.enter_into_force",
        json!({"kb_id": KB, "request_id": rq2, "effective_at": NOW}),
    )
    .await
    .unwrap();
    let s = status(&env, "n1").await;
    assert_eq!(s["in_force"]["revision_id"], json!(revs2[0]));
    assert_eq!(s["in_force"]["rev"], json!(1));
    let first = s["revisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["revision_id"] == json!(revs[0]))
        .unwrap()
        .clone();
    assert_eq!(first["state"], json!("superseded"));
}

/// D3: an author, an agent and an unlisted person are each refused, before
/// anything is recorded. Kills the author check, the requester-is-author
/// rule, the agent check and the list check.
#[tokio::test]
async fn authors_agents_and_unlisted_people_are_refused() {
    let env = env().await;
    policy(&env, json!({})).await;
    let (_, revs) = requested(&env, &["n1"]).await;
    let p = json!({"kb_id": KB, "revision_id": revs[0]});
    let editor = signed_in(&env.editor, &["approvers"], PrincipalKind::Person);
    assert_eq!(
        err_kind(as_(&env, &editor, "kb/review.approve", p.clone()).await),
        "author_cannot_approve"
    );
    let agent = signed_in(&env.viewer, &["approvers"], PrincipalKind::Agent);
    assert_eq!(
        err_kind(as_(&env, &agent, "kb/review.approve", p.clone()).await),
        "agent_forbidden"
    );
    let unlisted = signed_in(&env.viewer, &["someone-else"], PrincipalKind::Person);
    assert_eq!(
        err_kind(as_(&env, &unlisted, "kb/review.approve", p.clone()).await),
        "not_approver"
    );
    // The owner edited nothing here, but is not on the list either.
    assert_eq!(
        err_kind(as_(&env, &person(&env.owner), "kb/review.approve", p).await),
        "not_approver"
    );
    assert_eq!(status(&env, "n1").await["approved"], Value::Null);
    // The requester is an author even with no edit of their own: the owner
    // requests n3, which only the seed wrote, and may not approve it.
    let r = request(&env, &env.owner, &["n3"]).await.unwrap();
    let id = r["revisions"][0]["revision_id"].clone();
    let owner = signed_in(&env.owner, &["approvers"], PrincipalKind::Person);
    let p = json!({"kb_id": KB, "revision_id": id});
    assert_eq!(
        err_kind(as_(&env, &owner, "kb/review.approve", p).await),
        "author_cannot_approve"
    );
}

/// A3: an edit by the daemon itself since the last approval leaves the
/// authors unknown, and approval is refused. Kills treating unknown as empty.
#[tokio::test]
async fn unknown_authors_refuse_approval() {
    let env = env().await;
    policy(&env, json!({})).await;
    let name = mae_sync::kb_node_doc_name(KB, "n1");
    let state = env.store.encode_state(&name).await.unwrap();
    let mut doc = KbNodeDoc::from_bytes_with_client_id(&state, 778).unwrap();
    let up = doc.set_title("Migrated");
    env.store
        .apply_update(
            &name,
            &up,
            None,
            Attribution::Internal(InternalWriter::Seed),
        )
        .await
        .unwrap();
    let (_, revs) = requested(&env, &["n1"]).await;
    let r = as_(
        &env,
        &approver(&env),
        "kb/review.approve",
        json!({"kb_id": KB, "revision_id": revs[0]}),
    )
    .await;
    assert_eq!(err_kind(r), "authors_unknown");
}

/// D3, C3: with `reviewed` required, approval waits for it; the reviewer may
/// not also approve; another approver then may. Kills skipping the required
/// act and the one-act-per-person rule.
#[tokio::test]
async fn a_required_review_comes_first_and_by_someone_else() {
    let env = env().await;
    policy(&env, json!({"required_acts": ["reviewed", "approved"]})).await;
    let (_, revs) = requested(&env, &["n1"]).await;
    let p = json!({"kb_id": KB, "revision_id": revs[0]});
    assert_eq!(
        err_kind(as_(&env, &approver(&env), "kb/review.approve", p.clone()).await),
        "invalid_transition"
    );
    let both = signed_in(
        &env.viewer,
        &["reviewers", "approvers"],
        PrincipalKind::Person,
    );
    let mut act = p.clone();
    act["act"] = json!("reviewed");
    as_(&env, &both, "kb/review.act", act.clone())
        .await
        .unwrap();
    assert_eq!(
        err_kind(as_(&env, &both, "kb/review.approve", p.clone()).await),
        "invalid_transition"
    );
    // The owner is a member; listed as an approver by group, they may approve.
    let owner = signed_in(&env.owner, &["approvers"], PrincipalKind::Person);
    assert_eq!(
        as_(&env, &owner, "kb/review.approve", p).await.unwrap()["revision_state"],
        json!("approved")
    );
}

/// D4: a set under in_force_on_approval enters into force only when its last
/// member is approved, all at once; `enter_into_force` on an incomplete set
/// names what is missing. Kills moving the pointer per member.
#[tokio::test]
async fn a_set_enters_into_force_whole_or_not_at_all() {
    let env = env().await;
    policy(&env, json!({"in_force_on_approval": true})).await;
    let (_, revs) = requested(&env, &["n1", "n2"]).await;
    let a1 = as_(
        &env,
        &approver(&env),
        "kb/review.approve",
        json!({"kb_id": KB, "revision_id": revs[0]}),
    )
    .await
    .unwrap();
    assert_eq!(a1["in_force_since"], Value::Null);
    assert_eq!(
        status(&env, "n1").await["approved"]["waiting_for"],
        json!("set")
    );
    assert_eq!(status(&env, "n1").await["in_force"], Value::Null);
    let a2 = as_(
        &env,
        &approver(&env),
        "kb/review.approve",
        json!({"kb_id": KB, "revision_id": revs[1]}),
    )
    .await
    .unwrap();
    assert_eq!(a2["revision_state"], json!("in_force"));
    for n in ["n1", "n2"] {
        assert!(status(&env, n).await["in_force"].is_object(), "{n}");
    }
    // A separate set, half approved: entry into force names the missing one.
    policy(&env, json!({"in_force_on_approval": false})).await;
    let (rq, revs) = requested(&env, &["n2", "n3"]).await;
    as_(
        &env,
        &approver(&env),
        "kb/review.approve",
        json!({"kb_id": KB, "revision_id": revs[0]}),
    )
    .await
    .unwrap();
    let e = as_(
        &env,
        &approver(&env),
        "kb/review.enter_into_force",
        json!({"kb_id": KB, "request_id": rq, "effective_at": NOW}),
    )
    .await
    .unwrap_err();
    assert_eq!(kind(&e), "set_incomplete");
    assert_eq!(e.data.as_deref().unwrap()["missing"], json!([revs[1]]));
}

/// E3, O19: under signed_pdf nothing enters into force without its PDF, on
/// approval or by the act. Kills the signed-PDF wait.
#[tokio::test]
async fn under_signed_pdf_entry_into_force_waits_for_the_pdf() {
    let env = env().await;
    policy(&env, json!({"in_force_on_approval": true, "approval_record": "signed_pdf", "reason": "Interim record."})).await;
    let (rq, revs) = requested(&env, &["n1"]).await;
    let a = as_(
        &env,
        &approver(&env),
        "kb/review.approve",
        json!({"kb_id": KB, "revision_id": revs[0]}),
    )
    .await
    .unwrap();
    assert_eq!(a["revision_state"], json!("approved"));
    assert_eq!(
        status(&env, "n1").await["approved"]["waiting_for"],
        json!("signed_pdf")
    );
    let e = as_(
        &env,
        &approver(&env),
        "kb/review.enter_into_force",
        json!({"kb_id": KB, "request_id": rq, "effective_at": NOW}),
    )
    .await
    .unwrap_err();
    assert_eq!(kind(&e), "signed_pdf_required");
    assert_eq!(status(&env, "n1").await["in_force"], Value::Null);
}

/// E2: rejecting one member ends the whole request; the named one carries
/// the reason, the other names it. Kills ending only the named member.
#[tokio::test]
async fn rejecting_one_member_ends_the_set() {
    let env = env().await;
    policy(&env, json!({})).await;
    let (rq, revs) = requested(&env, &["n1", "n2"]).await;
    as_(
        &env,
        &approver(&env),
        "kb/review.reject",
        json!({"kb_id": KB, "request_id": rq, "revision_id": revs[0], "reason": "Wrong template."}),
    )
    .await
    .unwrap();
    let entry = |s: &Value, id: &str| {
        s["revisions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["revision_id"] == json!(id))
            .unwrap()
            .clone()
    };
    let one = entry(&status(&env, "n1").await, &revs[0]);
    assert_eq!(
        (one["state"].clone(), one["reason"].clone()),
        (json!("rejected"), json!("Wrong template."))
    );
    let two = entry(&status(&env, "n2").await, &revs[1]);
    assert_eq!(
        (two["state"].clone(), two["rejected_with"].clone()),
        (json!("rejected"), json!(revs[0]))
    );
    let again = as_(
        &env,
        &approver(&env),
        "kb/review.approve",
        json!({"kb_id": KB, "revision_id": revs[1]}),
    )
    .await;
    assert_eq!(err_kind(again), "invalid_transition");
}

/// E2: a listed approver who is not an author may withdraw; one who is an
/// author gets the same refusal as a stranger. Kills the approver path and
/// its author check.
#[tokio::test]
async fn a_listed_approver_may_withdraw_unless_an_author() {
    let env = env().await;
    policy(&env, json!({})).await;
    // The viewer (an approver) edits n2 before the editor requests it: on n2
    // the viewer is an author.
    let viewer = env.viewer.clone();
    edit_with(&env.store, KB, "n2", &viewer, |d| {
        d.set_title("Viewer's words")
    })
    .await;
    let (rq2, _) = requested(&env, &["n2"]).await;
    let w2 = json!({"kb_id": KB, "request_id": rq2, "reason": "Withdrawn."});
    assert_eq!(
        err_kind(as_(&env, &approver(&env), "kb/review.withdraw", w2.clone()).await),
        "not_approver"
    );
    let unlisted = signed_in(&env.owner, &["nobody"], PrincipalKind::Person);
    assert_eq!(
        err_kind(as_(&env, &unlisted, "kb/review.withdraw", w2).await),
        "not_approver"
    );
    // On n1 the viewer authored nothing: as a listed approver they may withdraw.
    let (rq1, _) = requested(&env, &["n1"]).await;
    let w1 = json!({"kb_id": KB, "request_id": rq1, "reason": "Superseded by a new draft."});
    as_(&env, &approver(&env), "kb/review.withdraw", w1)
        .await
        .unwrap();
    assert_eq!(
        status(&env, "n1").await["revisions"][0]["state"],
        json!("withdrawn")
    );
}

/// D4: derogation ends force with no successor. Kills leaving it in force.
#[tokio::test]
async fn derogation_ends_a_document_in_force() {
    let env = env().await;
    policy(&env, json!({"in_force_on_approval": true})).await;
    let (_, revs) = requested(&env, &["n1"]).await;
    as_(
        &env,
        &approver(&env),
        "kb/review.approve",
        json!({"kb_id": KB, "revision_id": revs[0]}),
    )
    .await
    .unwrap();
    as_(
        &env,
        &approver(&env),
        "kb/review.derogate",
        json!({"kb_id": KB, "node_id": "n1", "effective_at": NOW, "reason": "Procedure retired."}),
    )
    .await
    .unwrap();
    let s = status(&env, "n1").await;
    assert_eq!(s["in_force"], Value::Null);
    assert_eq!(s["revisions"][0]["state"], json!("derogated"));
    let again = as_(
        &env,
        &approver(&env),
        "kb/review.derogate",
        json!({"kb_id": KB, "node_id": "n1", "effective_at": NOW, "reason": "Again."}),
    )
    .await;
    assert_eq!(err_kind(again), "invalid_transition");
}

/// The soft block (owner decision, 2026-11-25): an author may approve their
/// own revision only by acknowledging it with a reason, and the override is
/// recorded in the signed event and on the revision's acts. Kills a silent
/// override, an override without a reason, and recording one not needed.
#[tokio::test]
async fn an_acknowledged_self_approval_goes_through_and_is_recorded() {
    let env = env().await;
    policy(&env, json!({})).await;
    let (_, revs) = requested(&env, &["n1"]).await;
    let editor = signed_in(&env.editor, &["approvers"], PrincipalKind::Person);
    let base = json!({"kb_id": KB, "revision_id": revs[0], "acknowledge_self_approval": true});
    let e = as_(&env, &editor, "kb/review.approve", base.clone())
        .await
        .unwrap_err();
    assert_eq!(
        (kind(&e), e.data.as_deref().unwrap()["param"].clone()),
        ("invalid_params".into(), json!("override_reason"))
    );
    let mut ok = base;
    ok["override_reason"] = json!("The only approver is away; signed copy follows.");
    as_(&env, &editor, "kb/review.approve", ok).await.unwrap();
    let ev = event_objects(&env).await;
    let approve = ev.iter().find(|e| e["kind"] == json!("approve")).unwrap();
    assert_eq!(
        approve["payload"]["override"]["self_approval"],
        json!([revs[0]])
    );
    assert_eq!(
        approve["payload"]["override"]["reason"],
        json!("The only approver is away; signed copy follows.")
    );
    let read = call(
        &env,
        &env.owner,
        "kb/query.get",
        json!({"kb_id": KB, "node_id": "n1", "revision": revs[0]}),
    )
    .await
    .unwrap();
    assert_eq!(
        read["revision"]["acts"][0]["override"]["self_approval"],
        json!([revs[0]])
    );
    // An acknowledgement nobody needed records no override.
    let (_, revs2) = requested(&env, &["n2"]).await;
    let p = json!({"kb_id": KB, "revision_id": revs2[0], "acknowledge_self_approval": true, "override_reason": "Not needed."});
    as_(&env, &approver(&env), "kb/review.approve", p)
        .await
        .unwrap();
    let ev = event_objects(&env).await;
    let last = ev
        .iter()
        .rev()
        .find(|e| e["kind"] == json!("approve"))
        .unwrap();
    assert_eq!(last["payload"]["override"], Value::Null);
}

/// The soft block covers unknown authors, acknowledged; agents stay hard.
/// Kills acknowledging unknown authors silently, and an agent passing by
/// acknowledging.
#[tokio::test]
async fn unknown_authors_can_be_acknowledged_but_an_agent_never_acts() {
    let env = env().await;
    policy(&env, json!({})).await;
    let name = mae_sync::kb_node_doc_name(KB, "n1");
    let state = env.store.encode_state(&name).await.unwrap();
    let mut doc = KbNodeDoc::from_bytes_with_client_id(&state, 779).unwrap();
    let up = doc.set_title("Seeded");
    env.store
        .apply_update(
            &name,
            &up,
            None,
            Attribution::Internal(InternalWriter::Seed),
        )
        .await
        .unwrap();
    let (_, revs) = requested(&env, &["n1"]).await;
    let p = json!({"kb_id": KB, "revision_id": revs[0], "acknowledge_unknown_authors": true,
        "acknowledge_self_approval": true, "override_reason": "Seeded from the approved source."});
    let agent = signed_in(&env.viewer, &["approvers"], PrincipalKind::Agent);
    assert_eq!(
        err_kind(as_(&env, &agent, "kb/review.approve", p.clone()).await),
        "agent_forbidden"
    );
    as_(&env, &approver(&env), "kb/review.approve", p)
        .await
        .unwrap();
    let ev = event_objects(&env).await;
    let approve = ev.iter().find(|e| e["kind"] == json!("approve")).unwrap();
    assert_eq!(
        approve["payload"]["override"]["unknown_authors"],
        json!([revs[0]])
    );
    assert_eq!(approve["payload"]["override"]["self_approval"], json!([]));
}

/// C3 on `act`: the same reviewer does not review twice. Kills its check.
#[tokio::test]
async fn one_reviewer_reviews_once() {
    let env = env().await;
    policy(&env, json!({"required_acts": ["reviewed", "approved"]})).await;
    let (_, revs) = requested(&env, &["n1"]).await;
    let reviewer = signed_in(&env.viewer, &["reviewers"], PrincipalKind::Person);
    let act = json!({"kb_id": KB, "revision_id": revs[0], "act": "reviewed"});
    as_(&env, &reviewer, "kb/review.act", act.clone())
        .await
        .unwrap();
    assert_eq!(
        err_kind(as_(&env, &reviewer, "kb/review.act", act).await),
        "invalid_transition"
    );
}
