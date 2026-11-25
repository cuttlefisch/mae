//! E9's gate on every method PR-4 serves: a non-member and an unknown KB get
//! one identical refusal (no oracle), an E2E KB `e2e_unsupported`, and review
//! off `review_not_enabled` on every `kb/review.*` method but `set_policy`.
//! Kills a gate that tells an unknown KB from a foreign one, one that checks
//! E2E after anything else, and any method that skips the gate.

use mae_sync::kb::Encryption;
use serde_json::json;

use super::*;

/// Every method PR-4 serves, with parameters that would otherwise pass the
/// gate.
fn methods() -> Vec<(&'static str, Value)> {
    vec![
        (
            "kb/review.request",
            json!({"nodes": [{"node_id": "n1", "version": "0"}], "reason": "r"}),
        ),
        (
            "kb/review.withdraw",
            json!({"request_id": "rq-x", "reason": "r"}),
        ),
        ("kb/review.status", json!({"node_id": "n1"})),
        ("kb/review.pending", json!({})),
        ("kb/review.set_policy", json!({"review_enabled": true})),
        (
            "kb/review.set_code",
            json!({"node_id": "n1", "doc_code": "PRO-1"}),
        ),
        ("kb/review.events", json!({})),
        ("kb/query.revisions", json!({"node_id": "n1"})),
        ("kb/query.get", json!({"node_id": "n1", "revision": "rv-x"})),
    ]
}

fn with_kb(kb: &str, mut p: Value) -> Value {
    p["kb_id"] = json!(kb);
    p
}

fn whole(e: McpError) -> Value {
    serde_json::to_value(e).unwrap()
}

#[tokio::test]
async fn a_non_member_and_an_unknown_kb_get_one_identical_refusal_from_every_method() {
    let env = env().await;
    enable(&env).await;
    for (m, p) in methods() {
        let foreign = call(&env, &env.stranger, m, with_kb(KB, p.clone()))
            .await
            .unwrap_err();
        let unknown = call(&env, &env.owner, m, with_kb("kb-nobody-made", p.clone()))
            .await
            .unwrap_err();
        assert_eq!(kind(&foreign), "not_member", "{m}");
        assert_eq!(whole(foreign), whole(unknown), "{m}: an oracle");
    }
}

#[tokio::test]
async fn every_method_refuses_an_e2e_kb() {
    let env = env().await;
    let kb = "kb-sealed";
    let mut coll = KbCollectionDoc::new_owned(kb, &env.owner, "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    coll.upsert_member(&env.editor, "editor", Role::Editor);
    let _ = coll.add_node("n1", "n1");
    let _ = coll.set_encryption(Encryption::E2e);
    env.store
        .share_doc(
            &format!("kbc:{kb}"),
            &coll.encode_state(),
            Attribution::Principal("seed"),
        )
        .await
        .unwrap();
    for who in [&env.owner, &env.editor] {
        for (m, p) in methods() {
            let r = call(&env, who, m, with_kb(kb, p)).await;
            assert_eq!(err_kind(r), "e2e_unsupported", "{m}");
        }
    }
    // The gate's order: a stranger on the E2E KB learns nothing about it.
    for (m, p) in methods() {
        let r = call(&env, &env.stranger, m, with_kb(kb, p)).await;
        assert_eq!(err_kind(r), "not_member", "{m}");
    }
}

#[tokio::test]
async fn with_review_off_every_review_method_but_set_policy_is_refused() {
    let env = env().await;
    let gated: Vec<_> = methods()
        .into_iter()
        .filter(|(m, _)| m.starts_with("kb/review.") && *m != "kb/review.set_policy")
        .collect();
    assert_eq!(gated.len(), 6);
    for (m, p) in gated {
        let r = call(&env, &env.owner, m, with_kb(KB, p)).await;
        assert_eq!(err_kind(r), "review_not_enabled", "{m}");
    }
    call(
        &env,
        &env.owner,
        "kb/review.set_policy",
        json!({"kb_id": KB, "review_enabled": true}),
    )
    .await
    .unwrap();
}

/// A viewer is admitted to read but not to request (`role_forbidden`); the
/// reads answer it.
#[tokio::test]
async fn a_viewer_reads_but_does_not_request() {
    let env = env().await;
    enable(&env).await;
    assert_eq!(
        err_kind(request(&env, &env.viewer, &["n1"]).await),
        "role_forbidden"
    );
    for (m, p) in [
        ("kb/review.status", json!({"node_id": "n1"})),
        ("kb/review.pending", json!({})),
        ("kb/query.revisions", json!({"node_id": "n1"})),
    ] {
        call(&env, &env.viewer, m, with_kb(KB, p)).await.unwrap();
    }
    let r = call(&env, &env.editor, "kb/review.events", json!({"kb_id": KB})).await;
    assert_eq!(err_kind(r), "role_forbidden");
}
