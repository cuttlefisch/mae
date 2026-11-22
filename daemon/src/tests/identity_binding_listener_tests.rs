//! The typed principal (ADR-098 Amendment 1, C1) and the D4 group gate,
//! through the listener's real validator (`validate_bearer_token`, real RS256
//! tokens), the real binding table and the real routing function against a
//! real `DocStore`. `handle_request` serializes `route_authenticated_request`'s
//! value unchanged, so its serialized text is the response body that is
//! scanned.
//!
//! Attacker model: a person signed in at the identity provider whose subject
//! spells a member's fingerprint — byte for byte — or matches a string a member
//! list holds, and who tries to read that member's KBs.

use std::sync::Arc;

use mae_daemon::doc_store::DocStore;
use mae_daemon::identity_binding::{resolve, BindingStore, BoundBy, Selector, BINDINGS_FILE};
use mae_daemon::oauth_self_issue::{unbound_principal, Credential, PrincipalKind};
use mae_mcp::identity::Identity;
use mae_sync::kb::{JoinPolicy, KbCollectionDoc, Role, TransportPolicy};
use serde_json::{json, Value};

use super::kb_query_tests::fresh_doc_store;
use crate::oauth::{
    route_authenticated_request, validate_bearer_token, JwkOwned, ResourceServerConfig,
    TokenValidationError, ValidatedPrincipal,
};

const KID: &str = "idp-key";
const ISS: &str = "https://idp.example.com";
const AUD: &str = "https://mae.example.com/mcp";

const BY: mae_daemon::doc_editors::Attribution<'static> =
    mae_daemon::doc_editors::Attribution::Principal("test-editor");

fn config(allowed_groups: &[&str]) -> ResourceServerConfig {
    ResourceServerConfig {
        canonical_resource_uri: AUD.into(),
        principal_claim: "sub".into(),
        groups_claim: "groups".into(),
        allowed_groups: allowed_groups.iter().map(|g| g.to_string()).collect(),
        jwks_url: "https://unused.example.com/jwks".into(),
        jwks_ca_file: None,
        issuer: Some(ISS.into()),
        kb_query_enabled: true,
        site_enabled: false,
        site_locale: "en-US".to_string(),
        max_request_body_bytes: 1_048_576,
        kb_query_max_body_bytes: 65_536,
        kb_query_max_scan_nodes: 500,
        kb_query_max_search_results: 20,
        webview_enabled: false,
        second_factor: Default::default(),
    }
}

/// A fresh RSA key per test: the signing PEM and the JWK the validator reads.
fn idp_key() -> (String, JwkOwned) {
    use base64::Engine;
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::traits::PublicKeyParts;
    let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
    let pem = key
        .to_pkcs1_pem(rsa::pkcs8::LineEnding::LF)
        .unwrap()
        .to_string();
    let b64 = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    let public = key.to_public_key();
    let jwk = JwkOwned {
        kid: KID.into(),
        n: b64(&public.n().to_bytes_be()),
        e: b64(&public.e().to_bytes_be()),
    };
    (pem, jwk)
}

fn idp_token(pem: &str, sub: &str, groups: Value) -> String {
    let mut claims = idp_claims(sub);
    if !groups.is_null() {
        claims["groups"] = groups;
    }
    sign_claims(pem, &claims)
}

fn idp_claims(sub: &str) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    json!({"sub": sub, "aud": AUD, "iss": ISS, "iat": now, "exp": now + 3600})
}

fn sign_claims(pem: &str, claims: &Value) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(KID.into());
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
    jsonwebtoken::encode(&header, claims, &key).unwrap()
}

/// An owned KB with `members` as viewers, permissive so that only membership
/// (never the join policy) decides a read.
async fn put_kb(store: &DocStore, kb_id: &str, owner: &str, members: &[&str]) {
    let mut coll = KbCollectionDoc::new_owned(kb_id, owner, "owner");
    coll.set_transport_policy(TransportPolicy::default());
    let _ = coll.set_join_policy(JoinPolicy::Permissive);
    for m in members {
        let _ = coll.upsert_member(m, "member", Role::Viewer);
    }
    store
        .share_doc(&format!("kbc:{kb_id}"), &coll.encode_state(), BY)
        .await
        .unwrap();
}

async fn call(store: &Arc<DocStore>, vp: &ValidatedPrincipal, method: &str, kb: &str) -> String {
    let rpc = mae_mcp::protocol::JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: json!(1),
        method: method.into(),
        params: Some(json!({"kb_id": kb})),
    };
    let v = route_authenticated_request(
        Some(rpc),
        &config(&[]),
        Some(store),
        vp,
        &mae_daemon::quota::NoQuota,
    )
    .await;
    v.to_string()
}

fn kb_ids(body: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(body).unwrap();
    v["result"]["kbs"]
        .as_array()
        .unwrap_or_else(|| panic!("no kbs list: {body}"))
        .iter()
        .map(|k| k["kb_id"].as_str().unwrap().to_string())
        .collect()
}

/// C5, typed principal: an IdP token whose `sub` is byte-identical to a
/// member's fingerprint string is denied that member's KB, and `my_kbs`
/// answers it with an empty list. The control proves the KB really is
/// readable by that fingerprint, so the denial is not vacuous; and after an
/// administrator binds the subject, the same token reads it.
#[tokio::test]
async fn an_idp_subject_spelling_a_member_fingerprint_reads_nothing_until_bound() {
    let store = fresh_doc_store().await;
    let member = Identity::generate("member").fingerprint();
    let other = Identity::generate("other").fingerprint();
    put_kb(&store, "kb-member", &other, &[&member]).await;
    let (pem, jwk) = idp_key();

    let vp = validate_bearer_token(
        &idp_token(&pem, &member, Value::Null),
        std::slice::from_ref(&jwk),
        &config(&[]),
    )
    .unwrap();
    let bindings = BindingStore::open_memory().unwrap();
    let vp = resolve(vp, Some(&bindings)).unwrap();
    assert_eq!(vp.credential, Credential::IdpUnbound);

    let my = call(&store, &vp, "kb/query.my_kbs", "").await;
    assert_eq!(kb_ids(&my), Vec::<String>::new());
    let caps = call(&store, &vp, "kb/query.capabilities", "kb-member").await;
    let v: Value = serde_json::from_str(&caps).unwrap();
    assert_eq!(v["error"]["data"]["kind"], "not_member", "{caps}");
    for body in [&my, &caps] {
        assert!(!body.contains(&other), "names the KB's owner: {body}");
    }

    // Control: the member's own fingerprint principal reads it.
    let mut as_member = vp.clone();
    as_member.principal = member.clone();
    as_member.credential = Credential::SelfIssued;
    assert_eq!(
        kb_ids(&call(&store, &as_member, "kb/query.my_kbs", "").await),
        vec!["kb-member"]
    );

    bindings
        .bind(ISS, &member, &member, BoundBy::Admin, 1)
        .unwrap();
    let token = idp_token(&pem, &member, Value::Null);
    let bound = resolve(
        validate_bearer_token(&token, &[jwk], &config(&[])).unwrap(),
        Some(&bindings),
    )
    .unwrap();
    assert_eq!(bound.credential, Credential::IdpBound);
    assert_eq!(
        kb_ids(&call(&store, &bound, "kb/query.my_kbs", "").await),
        vec!["kb-member"]
    );
}

/// `kb/add_member` accepts any string, so a member list can hold an unbound
/// principal's exact text. Such a subject is still a member of nothing: the
/// listener answers it before membership is consulted.
#[tokio::test]
async fn an_unbound_subject_is_denied_even_when_a_member_list_holds_its_text() {
    let store = fresh_doc_store().await;
    let unbound = unbound_principal(ISS, "alice");
    let owner = Identity::generate("owner").fingerprint();
    put_kb(&store, "kb-planted", &owner, &[&unbound]).await;
    let (pem, jwk) = idp_key();
    let vp = validate_bearer_token(&idp_token(&pem, "alice", Value::Null), &[jwk], &config(&[]))
        .unwrap();
    assert_eq!(vp.principal, unbound);
    assert_eq!(
        kb_ids(&call(&store, &vp, "kb/query.my_kbs", "").await),
        Vec::<String>::new()
    );
    let get = call(&store, &vp, "kb/query.capabilities", "kb-planted").await;
    assert!(get.contains("not_member"), "{get}");
}

/// Unbinding takes effect on the next request: the same token, after a CLI
/// unbind on another connection to the file, is an unbound subject again.
#[tokio::test]
async fn after_an_unbind_the_same_token_reads_nothing() {
    let store = fresh_doc_store().await;
    let member = Identity::generate("member").fingerprint();
    put_kb(
        &store,
        "kb-a",
        &Identity::generate("o").fingerprint(),
        &[&member],
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let listener_side = BindingStore::open(&dir.path().join(BINDINGS_FILE)).unwrap();
    let cli_side = BindingStore::open(&dir.path().join(BINDINGS_FILE)).unwrap();
    cli_side
        .bind(ISS, "alice", &member, BoundBy::Admin, 1)
        .unwrap();
    let (pem, jwk) = idp_key();
    let token = idp_token(&pem, "alice", Value::Null);
    let request = |s: &BindingStore| {
        resolve(
            validate_bearer_token(&token, std::slice::from_ref(&jwk), &config(&[])).unwrap(),
            Some(s),
        )
        .unwrap()
    };
    assert_eq!(
        kb_ids(&call(&store, &request(&listener_side), "kb/query.my_kbs", "").await),
        vec!["kb-a"]
    );
    cli_side.unbind(&Selector::Fingerprint(member)).unwrap();
    let after = request(&listener_side);
    assert_eq!(
        (after.credential, after.kind),
        (Credential::IdpUnbound, PrincipalKind::Agent)
    );
    assert_eq!(
        kb_ids(&call(&store, &after, "kb/query.my_kbs", "").await),
        Vec::<String>::new()
    );
}

/// An empty subject is no identity: refused, never the unbound principal of
/// `(iss, "")` that every subject-less token would share.
#[test]
fn an_empty_subject_is_refused() {
    let (pem, jwk) = idp_key();
    let r = validate_bearer_token(&idp_token(&pem, "", Value::Null), &[jwk], &config(&[]));
    assert_eq!(
        r.map(|_| ()),
        Err(TokenValidationError::MissingPrincipalClaim)
    );
}

/// An external token with no issuer is no identity either: `(iss, sub)` keys
/// the binding table, and a missing, empty or non-string `iss` would read as
/// the one issuer `""` every such token shares. Checked with issuer
/// validation off (`issuer = None`), where nothing else refuses it.
#[test]
fn an_external_token_without_an_issuer_is_refused() {
    let (pem, jwk) = idp_key();
    let mut unchecked = config(&[]);
    unchecked.issuer = None;
    let ok = validate_bearer_token(
        &sign_claims(&pem, &idp_claims("alice")),
        std::slice::from_ref(&jwk),
        &unchecked,
    );
    assert_eq!(ok.map(|vp| vp.issuer), Ok(ISS.to_string()), "control");
    for (case, iss) in [
        ("missing", None),
        ("empty", Some(json!(""))),
        ("not a string", Some(json!(7))),
    ] {
        let mut claims = idp_claims("alice");
        match iss {
            None => drop(claims.as_object_mut().unwrap().remove("iss")),
            Some(v) => claims["iss"] = v,
        }
        let r = validate_bearer_token(
            &sign_claims(&pem, &claims),
            std::slice::from_ref(&jwk),
            &unchecked,
        );
        assert_eq!(
            r.map(|_| ()),
            Err(TokenValidationError::WrongIssuer),
            "{case} iss"
        );
    }
}

/// D4: ANY ONE configured group admits the token, matched exactly; a token
/// carrying none is refused by the validator, so it never reaches the binding
/// table or the store.
#[test]
fn the_group_gate_admits_any_one_configured_group() {
    let (pem, jwk) = idp_key();
    let cfg = config(&["mae-readers", "staff"]);
    let check = |groups: Value| {
        validate_bearer_token(
            &idp_token(&pem, "alice", groups),
            std::slice::from_ref(&jwk),
            &cfg,
        )
    };
    for none in [
        Value::Null,
        json!([]),
        json!(["other"]),
        json!("other"),
        json!(["MAE-READERS", "Staff", "mae-readers-x"]),
        json!([["staff"]]),
    ] {
        assert_eq!(
            check(none.clone()).map(|_| ()),
            Err(TokenValidationError::NotInAllowedGroup),
            "{none}"
        );
    }
    for one in [
        json!(["mae-readers"]),
        json!(["other", "staff"]),
        json!("staff"),
        json!(["staff", "mae-readers"]),
    ] {
        assert!(check(one.clone()).is_ok(), "{one} carries an allowed group");
    }
    let open = validate_bearer_token(&idp_token(&pem, "alice", Value::Null), &[jwk], &config(&[]));
    assert!(open.is_ok(), "no configured group, no gate");
}
