//! ADR-113 PR-8: a recent second factor at the moment of acting (B7, C4, E8),
//! through the methods a client calls. Each test names the guard it kills.

use serde_json::{json, Value};

use super::act_tests::{as_, policy, requested, signed_in, NOW};
use super::attach_tests::{pdf, put, with_blobs};
use super::*;
use crate::oauth_self_issue::PrincipalKind;
use crate::sign_in::{SecondFactorRule, StepUpRule};

/// The operator accepts `mfa`, with the default 300 s window.
fn configure(env: &Env) {
    env.store.set_step_up_rule(StepUpRule {
        rule: SecondFactorRule {
            amr: vec!["mfa".into()],
            acr: vec!["strong".into()],
        },
        max_age_secs: 300,
    });
}

fn now_secs(env: &Env) -> u64 {
    env.store.now_ms() / 1000
}

/// `p` in `groups`, signed in `age` seconds ago with `amr`, as a person.
fn signed(env: &Env, p: &str, groups: &[&str], amr: &[&str], age: u64) -> ValidatedPrincipal {
    let mut v = signed_in(p, groups, PrincipalKind::Person);
    let s = v.sign_in.as_mut().unwrap();
    s.amr = amr.iter().map(|a| a.to_string()).collect();
    s.auth_time = Some(now_secs(env) - age);
    s.acr = Some("strong".into());
    v
}

fn strong(env: &Env, p: &str, groups: &[&str]) -> ValidatedPrincipal {
    signed(env, p, groups, &["pwd", "mfa"], 5)
}

/// Review on with the requirement, set by an owner who meets it.
async fn require_mfa(env: &Env, extra: Value) {
    policy(env, json!({})).await;
    let mut p = json!({"kb_id": KB, "approval_requires_mfa": true});
    p.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    as_(
        env,
        &strong(env, &env.owner, &[]),
        "kb/review.set_policy",
        p,
    )
    .await
    .unwrap();
}

/// The last event of `kind`, its actor's recorded sign-in.
async fn recorded_sign_in(env: &Env, kind: &str) -> Value {
    event_objects(env)
        .await
        .into_iter()
        .rev()
        .find(|e| e["kind"] == json!(kind))
        .unwrap_or_else(|| panic!("no {kind} event"))["actor"]["sign_in"]
        .clone()
}

/// The three controlled acts each refuse a sign-in without the factor, a
/// stale one, an acr alone, and a token claiming it in parameters; each
/// accepts a recent one and records it. Kills each act's check removed.
#[tokio::test]
async fn every_controlled_act_needs_a_recent_second_factor() {
    let env = env().await;
    let _dir = with_blobs(&env);
    configure(&env);
    require_mfa(&env, json!({})).await;
    let (rq, revs) = requested(&env, &["n1"]).await;
    let approvers = ["approvers"];
    let weak = signed(&env, &env.viewer, &approvers, &["pwd"], 5);
    let stale = signed(&env, &env.viewer, &approvers, &["pwd", "mfa"], 301);
    let ok = strong(&env, &env.viewer, &approvers);

    let approve = json!({"kb_id": KB, "revision_id": revs[0]});
    let relayed = json!({"kb_id": KB, "revision_id": revs[0], "amr": ["mfa"],
        "auth_time": now_secs(&env), "sign_in": {"amr": ["mfa"], "auth_time": now_secs(&env)}});
    let m = "kb/review.approve";
    assert_eq!(
        err_kind(as_(&env, &weak, m, approve.clone()).await),
        "mfa_required",
        "acr alone"
    );
    assert_eq!(
        err_kind(as_(&env, &weak, m, relayed).await),
        "mfa_required",
        "a relayed claim"
    );
    assert_eq!(
        err_kind(as_(&env, &stale, m, approve.clone()).await),
        "reauthentication_required"
    );
    as_(&env, &ok, m, approve).await.unwrap();
    let s = recorded_sign_in(&env, "approve").await;
    assert_eq!(
        s["second_factor"],
        json!({"claim": "amr", "value": "mfa"}),
        "{s}"
    );
    assert_eq!(s["auth_time"], json!(now_secs(&env) - 5), "{s}");

    let blob = put(&env, &pdf("signed")).await;
    let attach = json!({"kb_id": KB, "revision_id": revs[0], "blob_sha256": blob});
    let m = "kb/review.attach_signed_pdf";
    assert_eq!(
        err_kind(as_(&env, &weak, m, attach.clone()).await),
        "mfa_required"
    );
    assert_eq!(
        err_kind(as_(&env, &stale, m, attach.clone()).await),
        "reauthentication_required"
    );
    as_(&env, &ok, m, attach).await.unwrap();
    assert_eq!(
        recorded_sign_in(&env, "attach").await["second_factor"]["value"],
        json!("mfa")
    );

    let force = json!({"kb_id": KB, "request_id": rq, "effective_at": NOW});
    let m = "kb/review.enter_into_force";
    assert_eq!(
        err_kind(as_(&env, &weak, m, force.clone()).await),
        "mfa_required"
    );
    assert_eq!(
        err_kind(as_(&env, &stale, m, force.clone()).await),
        "reauthentication_required"
    );
    as_(&env, &ok, m, force).await.unwrap();
    assert_eq!(
        recorded_sign_in(&env, "enter_into_force").await["second_factor"]["value"],
        json!("mfa")
    );
}

/// The owner's administration needs it too while it is on, and so does
/// switching it on or off; once off, the plain approval is back. Kills the
/// set_policy and set_code checks removed.
#[tokio::test]
async fn administration_and_the_switch_itself_need_it() {
    let env = env().await;
    configure(&env);
    policy(&env, json!({})).await;
    let weak_owner = signed(&env, &env.owner, &[], &["pwd"], 5);
    let on = json!({"kb_id": KB, "approval_requires_mfa": true});
    let m = "kb/review.set_policy";
    assert_eq!(
        err_kind(as_(&env, &weak_owner, m, on.clone()).await),
        "mfa_required",
        "on"
    );
    as_(&env, &strong(&env, &env.owner, &[]), m, on)
        .await
        .unwrap();

    let code = json!({"kb_id": KB, "node_id": "n1", "doc_code": "PRO-1"});
    let c = "kb/review.set_code";
    assert_eq!(
        err_kind(as_(&env, &weak_owner, c, code.clone()).await),
        "mfa_required"
    );
    as_(&env, &strong(&env, &env.owner, &[]), c, code)
        .await
        .unwrap();
    assert_eq!(
        recorded_sign_in(&env, "code_set").await["second_factor"]["value"],
        json!("mfa")
    );

    let off = json!({"kb_id": KB, "approval_requires_mfa": false});
    assert_eq!(
        err_kind(as_(&env, &weak_owner, m, off.clone()).await),
        "mfa_required",
        "off"
    );
    as_(&env, &strong(&env, &env.owner, &[]), m, off)
        .await
        .unwrap();
    let (_, revs) = requested(&env, &["n2"]).await;
    let weak = signed(&env, &env.viewer, &["approvers"], &["pwd"], 5);
    let p = json!({"kb_id": KB, "revision_id": revs[0]});
    as_(&env, &weak, "kb/review.approve", p).await.unwrap();
    assert_eq!(
        recorded_sign_in(&env, "approve").await,
        Value::Null,
        "the plain approval"
    );
}

/// A KB may only shorten the operator's window, never under 60 s, and the
/// shorter window is the one held. Kills the bounds and the window ignored.
#[tokio::test]
async fn a_kb_window_only_shortens_the_operators() {
    let env = env().await;
    configure(&env);
    require_mfa(&env, json!({})).await;
    let owner = strong(&env, &env.owner, &[]);
    let m = "kb/review.set_policy";
    for bad in [json!(59), json!(301), json!(-1), json!("120")] {
        let p = json!({"kb_id": KB, "reauth_max_age_secs": bad});
        assert_eq!(
            err_kind(as_(&env, &owner, m, p).await),
            "invalid_params",
            "{bad}"
        );
    }
    let p = json!({"kb_id": KB, "reauth_max_age_secs": 120});
    let r = as_(&env, &owner, m, p).await.unwrap();
    assert_eq!(r["policy"]["reauth_max_age_secs"], json!(120));
    let (_, revs) = requested(&env, &["n1"]).await;
    let p = json!({"kb_id": KB, "revision_id": revs[0]});
    let at_200 = signed(&env, &env.viewer, &["approvers"], &["mfa"], 200);
    let e = as_(&env, &at_200, "kb/review.approve", p.clone())
        .await
        .unwrap_err();
    assert_eq!(kind(&e), "reauthentication_required");
    assert_eq!(e.data.unwrap()["max_age"], json!(120));
    let at_100 = signed(&env, &env.viewer, &["approvers"], &["mfa"], 100);
    as_(&env, &at_100, "kb/review.approve", p).await.unwrap();
}

/// Nothing configured: the requirement cannot be switched on, and a KB that
/// has it refuses every act (fail closed). Kills the empty configuration
/// allowed.
#[tokio::test]
async fn with_nothing_configured_nothing_passes() {
    let env = env().await;
    policy(&env, json!({})).await;
    let owner = strong(&env, &env.owner, &[]);
    let on = json!({"kb_id": KB, "approval_requires_mfa": true});
    assert_eq!(
        err_kind(as_(&env, &owner, "kb/review.set_policy", on).await),
        "second_factor_unconfigured"
    );
    env.store
        .review_write(|c| {
            let mut p = crate::review::store::policy(c, KB)?;
            p.approval_requires_mfa = true;
            crate::review::store::put_policy(c, KB, &p)
        })
        .await
        .unwrap();
    let (_, revs) = requested(&env, &["n1"]).await;
    let p = json!({"kb_id": KB, "revision_id": revs[0]});
    let ok = strong(&env, &env.viewer, &["approvers"]);
    assert_eq!(
        err_kind(as_(&env, &ok, "kb/review.approve", p).await),
        "second_factor_unconfigured"
    );
}
