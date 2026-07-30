//! ADR-067 Phase D3 adversarial coverage for the `kb/query.self_token` mTLS
//! RPC (`collab_handler/mod.rs`'s dispatch arm). The crypto itself
//! (mint/validate, wrong-audience/expired/forged-signature/wrong-issuer) is
//! already covered by `daemon/src/oauth_self_issue.rs`'s own `#[cfg(test)]
//! mod tests` and, over the real wire, by `daemon/tests/
//! self_issued_token_e2e.rs` -- this file is scoped to what's NEW at this
//! layer specifically: the RPC requires a real authenticated connection, is
//! feature-gated, and binds the minted `sub` to the CONNECTION's own
//! verified principal with no way for a caller to smuggle a different one.

use super::*;

fn self_token_msg() -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/query.self_token","params":{}})
}

fn enabled_self_issue() -> crate::oauth_self_issue::SelfIssueConfig {
    use mae_mcp::identity::Identity;
    crate::oauth_self_issue::SelfIssueConfig {
        identity: Arc::new(Identity::generate("daemon-under-test")),
        audience: "https://mae.example.com/mcp".to_string(),
        ttl_secs: 3600,
    }
}

#[tokio::test]
async fn authenticated_principal_receives_a_token_bound_to_their_own_fingerprint() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let self_issue = enabled_self_issue();
    let daemon_pubkey = self_issue.identity.public().to_bytes();
    let audience = self_issue.audience.clone();

    let member_fp = fp("real-member");
    let resp = dispatch_as_with_self_issue(
        &store,
        &bc,
        Some("real-member"),
        Some(&member_fp),
        self_token_msg(),
        &mut docs,
        Some(self_issue),
    )
    .await;

    assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
    let result = resp.result.unwrap();
    let token = result["token"].as_str().expect("token field");
    assert_eq!(result["token_type"], "Bearer");

    // The minted token, independently validated, must map back to the
    // CONNECTION's own principal -- never anything else, since the RPC
    // takes no params a caller could use to request a different `sub`.
    let validated =
        crate::oauth_self_issue::validate_self_issued_token(token, &daemon_pubkey, &audience)
            .expect("minted token must validate against the daemon's own pubkey");
    assert_eq!(validated.principal, member_fp);
}

#[tokio::test]
async fn an_unauthenticated_connection_is_denied_a_self_issued_token() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let self_issue = enabled_self_issue();

    // No auth_label/auth_principal -- the loopback/`none`-auth shape.
    let resp = dispatch_as_with_self_issue(
        &store,
        &bc,
        None,
        None,
        self_token_msg(),
        &mut docs,
        Some(self_issue),
    )
    .await;

    assert!(
        resp.error.is_some(),
        "kb/query.self_token must require a real authenticated connection, \
         never mint for an unauthenticated (None) principal"
    );
}

#[tokio::test]
async fn self_token_is_denied_when_the_feature_is_disabled() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let member_fp = fp("real-member");

    // self_issue = None -- the config-gate-off shape (principle #12).
    let resp = dispatch_as_with_self_issue(
        &store,
        &bc,
        Some("real-member"),
        Some(&member_fp),
        self_token_msg(),
        &mut docs,
        None,
    )
    .await;

    assert!(
        resp.error.is_some(),
        "kb/query.self_token must be denied outright when self-issued tokens \
         are not enabled on this daemon, not silently no-op-succeed"
    );
}

#[tokio::test]
async fn two_different_principals_receive_tokens_bound_to_their_own_distinct_fingerprints() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let self_issue = enabled_self_issue();
    let daemon_pubkey = self_issue.identity.public().to_bytes();
    let audience = self_issue.audience.clone();

    let alice_fp = fp("alice");
    let mut alice_docs = HashSet::new();
    let alice_resp = dispatch_as_with_self_issue(
        &store,
        &bc,
        Some("alice"),
        Some(&alice_fp),
        self_token_msg(),
        &mut alice_docs,
        Some(self_issue.clone()),
    )
    .await;
    let alice_token = alice_resp.result.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let bob_fp = fp("bob");
    let mut bob_docs = HashSet::new();
    let bob_resp = dispatch_as_with_self_issue(
        &store,
        &bc,
        Some("bob"),
        Some(&bob_fp),
        self_token_msg(),
        &mut bob_docs,
        Some(self_issue),
    )
    .await;
    let bob_token = bob_resp.result.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    assert_ne!(
        alice_token, bob_token,
        "two different connections must never receive the same token"
    );
    let alice_validated = crate::oauth_self_issue::validate_self_issued_token(
        &alice_token,
        &daemon_pubkey,
        &audience,
    )
    .unwrap();
    let bob_validated =
        crate::oauth_self_issue::validate_self_issued_token(&bob_token, &daemon_pubkey, &audience)
            .unwrap();
    assert_eq!(alice_validated.principal, alice_fp);
    assert_eq!(bob_validated.principal, bob_fp);
    assert_ne!(
        alice_validated.principal, bob_validated.principal,
        "alice's connection must never receive a token minted for bob's principal"
    );
}
