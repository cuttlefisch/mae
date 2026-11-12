//! The sync WebSocket's upgrade, over the real listener (ADR-099 Amendment 1,
//! B1/B2/B6; B7 "Tickets and the upgrade" and "Authorization").
//!
//! Every refusal here happens BEFORE the upgrade, as an ordinary HTTP response,
//! so each test reads status, headers and body off the wire.

mod daemon_harness;
mod ws_common;

use std::time::Duration;

use serde_json::json;
use ws_common::*;

fn ticket_of(r: &Reply) -> String {
    assert_eq!(r.status, 200, "ticket mint refused: {}", r.body);
    r.json()["ticket"].as_str().expect("ticket").to_string()
}

fn protocols(ticket: &str) -> String {
    format!("mae.v1, mae.ticket.{ticket}")
}

/// A ticket opens a socket as its minting token's principal, and the response
/// selects the version only — the ticket is never echoed back.
#[tokio::test]
async fn a_ticket_opens_a_socket_and_is_never_echoed() {
    let d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    let minted = mint_ticket(&d, &token, Some(ORIGIN_A)).await;
    assert_eq!(minted.json()["expires_in"], 30);
    let t = ticket_of(&minted);
    let mut r = upgrade(&d, &protocols(&t), &[("Origin", ORIGIN_A)]).await;
    assert_eq!(r.status, 101, "{}", r.body);
    assert_eq!(
        r.header("sec-websocket-protocol").as_deref(),
        Some("mae.v1")
    );
    assert!(
        !r.head.contains(&t),
        "the ticket must not appear in the response"
    );
    let hello =
        r.ws.as_mut()
            .unwrap()
            .until(|v| v["method"] == "session/hello")
            .await;
    assert_eq!(hello["params"]["principal"], "SHA256:alice");
    assert_eq!(hello["params"]["protocol"], "mae.v1");
}

/// Single use, consumed on first presentation even when that presentation is
/// refused for another reason; and a used ticket is refused with exactly the
/// bytes an unknown one gets.
#[tokio::test]
async fn a_ticket_is_single_use_and_used_reads_as_unknown() {
    let d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    let origin = [("Origin", ORIGIN_A)];
    let t = ticket_of(&mint_ticket(&d, &token, Some(ORIGIN_A)).await);
    // First presentation fails on the version, not the ticket...
    let first = upgrade(&d, &format!("mae.v9, mae.ticket.{t}"), &origin).await;
    let want = (
        400,
        "invalid_request".to_string(),
        "protocol_unsupported".to_string(),
    );
    assert_eq!(refusal(&first), want);
    // ...and still spent it.
    let used = upgrade(&d, &protocols(&t), &origin).await;
    assert_eq!(
        refusal(&used),
        (
            401,
            "invalid_token".to_string(),
            "ticket_invalid".to_string()
        )
    );
    assert!(used.header("www-authenticate").is_some());
    let unknown = upgrade(&d, &protocols(&"ab".repeat(32)), &origin).await;
    assert_eq!(used.fingerprint(), unknown.fingerprint());
}

/// After 30 seconds a ticket is refused, identically to an unknown one.
#[tokio::test]
async fn an_expired_ticket_reads_as_unknown() {
    let d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    let t = ticket_of(&mint_ticket(&d, &token, Some(ORIGIN_A)).await);
    tokio::time::sleep(Duration::from_secs(31)).await;
    let expired = upgrade(&d, &protocols(&t), &[("Origin", ORIGIN_A)]).await;
    let unknown = upgrade(&d, &protocols(&"cd".repeat(32)), &[("Origin", ORIGIN_A)]).await;
    assert_eq!(expired.status, 401);
    assert_eq!(expired.fingerprint(), unknown.fingerprint());
}

/// Tickets live in memory only: a restart voids them.
#[tokio::test]
async fn a_restart_voids_every_ticket() {
    let mut d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    let t = ticket_of(&mint_ticket(&d, &token, Some(ORIGIN_A)).await);
    d.restart().await;
    let r = upgrade(&d, &protocols(&t), &[("Origin", ORIGIN_A)]).await;
    assert_eq!(
        refusal(&r),
        (
            401,
            "invalid_token".to_string(),
            "ticket_invalid".to_string()
        )
    );
}

/// A ticket is pinned to the page that minted it: another allowed origin, or
/// no origin at all, cannot spend it.
#[tokio::test]
async fn a_ticket_minted_for_one_origin_is_refused_from_another() {
    let d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    for presented in [Some(ORIGIN_B), None] {
        let t = ticket_of(&mint_ticket(&d, &token, Some(ORIGIN_A)).await);
        let h: Vec<(&str, &str)> = presented.iter().map(|o| ("Origin", *o)).collect();
        let r = upgrade(&d, &protocols(&t), &h).await;
        assert_eq!(
            refusal(&r),
            (
                401,
                "invalid_token".to_string(),
                "ticket_invalid".to_string()
            ),
            "origin {presented:?}"
        );
    }
}

/// Exact match only: a string that merely CONTAINS an allowed origin, or
/// extends it, is refused — on the upgrade and on the mint.
#[tokio::test]
async fn origins_match_exactly() {
    let d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    let auth = format!("Bearer {token}");
    for bad in [
        "https://app.example.evil.net",
        "https://evil.net/https://app.example",
        "https://app.example:8443",
        "http://app.example",
    ] {
        let r = upgrade(&d, "mae.v1", &[("Authorization", &auth), ("Origin", bad)]).await;
        let denied = (
            403,
            "access_denied".to_string(),
            "origin_not_allowed".to_string(),
        );
        assert_eq!(refusal(&r), denied, "{bad}");
        let m = mint_ticket(&d, &token, Some(bad)).await;
        assert_eq!(refusal(&m), denied, "{bad}");
    }
    let ok = upgrade(
        &d,
        "mae.v1",
        &[("Authorization", &auth), ("Origin", ORIGIN_A)],
    )
    .await;
    assert_eq!(ok.status, 101, "the exact origin is accepted: {}", ok.body);
}

/// No credential travels in a URL: any query string is refused on both
/// routes, even alongside a valid bearer header.
#[tokio::test]
async fn a_credential_in_the_url_is_refused() {
    let d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    let t = ticket_of(&mint_ticket(&d, &token, None).await);
    let auth = format!("Bearer {token}");
    let h = [
        ("Authorization", auth.as_str()),
        ("Sec-WebSocket-Protocol", "mae.v1"),
    ];
    for path in [
        format!("/ws?ticket={t}"),
        format!("/ws?access_token={token}"),
    ] {
        let r = request(&d, "GET", &path, &h, true).await;
        let want = (
            400,
            "invalid_request".to_string(),
            "credential_in_url".to_string(),
        );
        assert_eq!(refusal(&r), want, "{path}");
    }
    let r = request(&d, "POST", "/ws/ticket?access_token=x", &h, false).await;
    assert_eq!(r.json()["kind"], "credential_in_url");
}

/// No version in common: `400` before any upgrade, listing what is spoken.
#[tokio::test]
async fn no_common_version_is_refused_before_the_upgrade() {
    let d = spawn(Opts::default()).await;
    let auth = format!("Bearer {}", d.token("SHA256:alice", 3600));
    for offered in ["mae.v2", ""] {
        let r = upgrade(&d, offered, &[("Authorization", &auth)]).await;
        assert!(r.ws.is_none());
        let want = (
            400,
            "invalid_request".to_string(),
            "protocol_unsupported".to_string(),
        );
        assert_eq!(refusal(&r), want, "{offered:?}");
        assert_eq!(r.json()["supported"], json!(["mae.v1"]));
    }
}

/// Exactly one credential: none is `401`, two is refused.
#[tokio::test]
async fn exactly_one_credential_is_accepted() {
    let d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    let none = upgrade(&d, "mae.v1", &[]).await;
    assert_eq!(none.status, 401);
    assert!(none.header("www-authenticate").is_some());
    let t = ticket_of(&mint_ticket(&d, &token, None).await);
    let auth = format!("Bearer {token}");
    let both = upgrade(&d, &protocols(&t), &[("Authorization", &auth)]).await;
    assert_eq!(both.status, 400);
    assert!(both.ws.is_none());
}

/// CORS on `/ws/ticket`: an allowlisted origin's preflight is answered and
/// its mint response names that origin, exactly; a disallowed origin gets no
/// CORS header on either, and a preflight asking for another method or header
/// is refused. Kills: a missing preflight (a browser could never mint), a
/// wildcard or reflected `Access-Control-Allow-Origin`.
#[tokio::test]
async fn the_ticket_route_answers_cors_for_allowed_origins_only() {
    let d = spawn(Opts::default()).await;
    let pre = |origin: &'static str, method: &'static str, headers: &'static str| {
        vec![
            ("Origin", origin),
            ("Access-Control-Request-Method", method),
            ("Access-Control-Request-Headers", headers),
        ]
    };
    let ok = request(
        &d,
        "OPTIONS",
        "/ws/ticket",
        &pre(ORIGIN_A, "POST", "authorization"),
        false,
    )
    .await;
    assert_eq!(ok.status, 204, "{}", ok.body);
    assert_eq!(
        ok.header("access-control-allow-origin").as_deref(),
        Some(ORIGIN_A)
    );
    assert!(ok
        .header("access-control-allow-headers")
        .unwrap()
        .contains("Authorization"));

    let evil = request(
        &d,
        "OPTIONS",
        "/ws/ticket",
        &pre("https://evil.example", "POST", "authorization"),
        false,
    )
    .await;
    assert_eq!(evil.status, 403);
    assert_eq!(evil.header("access-control-allow-origin"), None);

    for bad in [
        pre(ORIGIN_A, "DELETE", "authorization"),
        pre(ORIGIN_A, "POST", "authorization, x-evil"),
    ] {
        let r = request(&d, "OPTIONS", "/ws/ticket", &bad, false).await;
        assert_eq!(r.status, 400, "{bad:?}");
    }

    let token = d.token("SHA256:alice", 3600);
    let minted = mint_ticket(&d, &token, Some(ORIGIN_B)).await;
    assert_eq!(minted.status, 200, "{}", minted.body);
    assert_eq!(
        minted.header("access-control-allow-origin").as_deref(),
        Some(ORIGIN_B)
    );
    let refused = mint_ticket(&d, &token, Some("https://evil.example")).await;
    assert_eq!(refused.header("access-control-allow-origin"), None);
}

/// The ninth unused ticket is refused with a kind and the limit.
#[tokio::test]
async fn too_many_unused_tickets_is_a_kinded_refusal() {
    let d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    for _ in 0..8 {
        assert_eq!(mint_ticket(&d, &token, None).await.status, 200);
    }
    let r = mint_ticket(&d, &token, None).await;
    assert_eq!(
        refusal(&r),
        (429, "too_many_requests".into(), "too_many_tickets".into())
    );
    assert_eq!(r.json()["limit"], 8);
}
