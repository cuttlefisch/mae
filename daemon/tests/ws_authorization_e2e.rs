//! Authorization on the sync WebSocket, over the real listener (ADR-099 D3,
//! Amendment 1 B4/B6; B7 "Authorization").
//!
//! The socket reaches the collab session loop, so a frame is authorized by the
//! same `kb_access` path as on every other transport. These tests do not
//! re-prove that path; they prove the socket cannot get around it.

mod ws_common;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::json;
use ws_common::*;

/// A token from any issuer but this daemon, while B6 holds: refused on both
/// routes with `issuer_not_supported`.
#[tokio::test]
async fn an_external_issuer_token_is_refused_on_both_routes() {
    let d = spawn(Opts::default()).await;
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("idp-signing-key".to_string());
    let claims = json!({"sub": "alice@example.com", "aud": AUDIENCE, "iss": "https://idp.example.com",
        "exp": 4_000_000_000u64});
    let external = encode(&header, &claims, &EncodingKey::from_secret(b"k")).unwrap();
    let auth = format!("Bearer {external}");
    let r = upgrade(&d, "mae.v1", &[("Authorization", &auth)]).await;
    let want = (
        401,
        "invalid_token".to_string(),
        "issuer_not_supported".to_string(),
    );
    assert_eq!(refusal(&r), want);
    assert!(r.header("www-authenticate").is_some());
    let m = mint_ticket(&d, &external, None).await;
    assert_eq!(refusal(&m), want);
}

/// A bad self-issued token is refused on `/ws` exactly as on the listener's
/// other authenticated routes: same status, same `WWW-Authenticate`, same body.
#[tokio::test]
async fn bad_tokens_are_refused_as_on_every_other_route() {
    let d = spawn(Opts::default()).await;
    let forged = mae_daemon::oauth_self_issue::mint_self_token(
        &mae_mcp::identity::Identity::generate("not-the-daemon"),
        "SHA256:alice",
        AUDIENCE,
        3600,
    )
    .unwrap();
    let wrong_audience = mae_daemon::oauth_self_issue::mint_self_token(
        &d.identity,
        "SHA256:alice",
        "https://elsewhere/mcp",
        3600,
    )
    .unwrap();
    for token in [forged, wrong_audience] {
        let auth = format!("Bearer {token}");
        let ws = upgrade(&d, "mae.v1", &[("Authorization", &auth)]).await;
        let plain = request(&d, "POST", "/", &[("Authorization", &auth)], false).await;
        assert_eq!(ws.status, 401);
        assert_eq!(ws.fingerprint(), plain.fingerprint());
    }
}

/// With `ws_enabled = false` the routes do not exist: an upgrade with a valid
/// token is answered by the authenticated default, never `101`, and no ticket
/// is minted.
#[tokio::test]
async fn the_routes_are_absent_when_ws_is_disabled() {
    let d = spawn(Opts {
        ws_enabled: false,
        ..Opts::default()
    })
    .await;
    let token = d.token("SHA256:alice", 3600);
    let auth = format!("Bearer {token}");
    let r = upgrade(&d, "mae.v1", &[("Authorization", &auth)]).await;
    assert_ne!(r.status, 101);
    assert!(r.ws.is_none());
    assert_eq!(
        r.json()["principal"],
        "SHA256:alice",
        "the plain diagnostic answered"
    );
    let m = mint_ticket(&d, &token, None).await;
    assert!(m.json().get("ticket").is_none(), "{}", m.body);
}

/// Bob's probes: every way the envelope lets him name Alice's KB.
fn probes() -> Vec<(&'static str, serde_json::Value)> {
    let node = json!({"kb_id": "kleak", "node_id": "concept:hidden"});
    vec![
        ("kb/node_fetch", node.clone()),
        ("kb/join", json!({"kb_id": "kleak"})),
        ("sync/full_state", json!({"doc": "kbc:kleak"})),
        ("sync/state_vector", json!({"doc": "kbc:kleak"})),
        (
            "sync/full_state",
            json!({"doc": "kbn:kleak:concept:hidden"}),
        ),
        ("sync/awareness", json!({"doc": "kbc:kleak", "state": {}})),
        ("kb/query.get", node),
        (
            "kb/query.search",
            json!({"kb_id": "kleak", "query": "Hidden"}),
        ),
    ]
}

async fn subscribe_to_updates(ws: &mut Ws, id: u64) {
    let types = json!({"types": ["sync_update", "awareness_update"]});
    assert!(ws.call(id, "notifications/subscribe", types).await["error"].is_null());
}

/// Bob, on his own socket, names Alice's KB in every way the envelope allows.
/// Each is refused; nothing of the KB — node body or title — appears in any
/// byte he receives; and when Alice then changes the KB, the update reaches
/// Carol (a member: the control) and not Bob.
#[tokio::test]
async fn a_kb_the_session_cannot_read_is_refused_and_nothing_leaks() {
    let d = spawn(Opts::default()).await;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let secret = format!("SECRET-{:x}", nanos.as_nanos());
    let mut alice = connect_bearer(&d, &d.token("SHA256:alice", 3600)).await;
    let share = kb_share("kleak", &[("concept:hidden", "Hidden Title Xq", &secret)]);
    assert!(alice.call(1, "kb/share", share).await["error"].is_null());
    let restrictive = json!({"kb_id": "kleak", "policy": "restrictive"});
    assert!(alice.call(2, "kb/set_policy", restrictive).await["error"].is_null());
    let carol_in =
        json!({"kb_id": "kleak", "member": "SHA256:carol", "role": "viewer", "label": "carol"});
    assert!(alice.call(3, "kb/add_member", carol_in).await["error"].is_null());

    let mut carol = connect_bearer(&d, &d.token("SHA256:carol", 3600)).await;
    subscribe_to_updates(&mut carol, 1).await;
    assert!(carol.call(2, "kb/join", json!({"kb_id": "kleak"})).await["error"].is_null());
    let mut bob = connect_bearer(&d, &d.token("SHA256:bob", 3600)).await;
    subscribe_to_updates(&mut bob, 1).await;
    for (i, (method, params)) in probes().into_iter().enumerate() {
        let resp = bob.call(10 + i as u64, method, params).await;
        assert!(!resp["error"].is_null(), "{method} must be refused: {resp}");
    }

    let add = json!({"kb_id": "kleak", "node_id": "concept:second", "title": "Second"});
    assert!(alice.call(4, "kb/collection_node_add", add).await["error"].is_null());
    carol
        .until(|v| v["method"].is_string() && v.to_string().contains("kbc:kleak"))
        .await;
    let _ = bob.call(99, "$/ping", json!({})).await;
    for frame in &bob.log {
        assert!(
            !frame.contains(&secret) && !frame.contains("Hidden Title Xq"),
            "leak: {frame}"
        );
        let v: serde_json::Value = serde_json::from_str(frame).unwrap_or_default();
        assert!(
            !(v["method"].is_string() && frame.contains("kleak")),
            "notification leak: {frame}"
        );
    }
}

/// A bearer session must not mint its own successor tokens: that would let it
/// renew itself forever, past the deadline that makes offboarding take effect.
#[tokio::test]
async fn a_socket_session_cannot_mint_itself_a_token() {
    let d = spawn(Opts::default()).await;
    let mut alice = connect_bearer(&d, &d.token("SHA256:alice", 3600)).await;
    let resp = alice.call(1, "kb/query.self_token", json!({})).await;
    assert_eq!(kind(&resp), "self_issue_disabled", "{resp}");
}

/// A token that expired seconds ago still passes the JWT library's clock
/// leeway; a socket must not open with its deadline already behind it.
#[tokio::test]
async fn a_just_expired_token_does_not_open_a_socket() {
    let d = spawn(Opts::default()).await;
    let mut header = Header::new(Algorithm::EdDSA);
    header.kid = Some("self".to_string());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = json!({"sub": "SHA256:alice", "aud": AUDIENCE, "iss": "self",
        "iat": now - 20, "exp": now - 5});
    let key = EncodingKey::from_ed_der(&d.identity.pkcs8_der().unwrap());
    let stale = encode(&header, &claims, &key).unwrap();
    let auth = format!("Bearer {stale}");
    let r = upgrade(&d, "mae.v1", &[("Authorization", &auth)]).await;
    assert_eq!(r.status, 401, "{}", r.body);
    assert!(r.ws.is_none());
}
