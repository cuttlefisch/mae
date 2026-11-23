//! `session/renew` with a connect ticket instead of a token (ADR-099
//! Amendment 3, F5), over the real listener.
//!
//! The case it serves: a confidential backend holds the person's tokens, mints
//! a fresh ticket with them, and hands the browser only the ticket. Every test
//! drives a real session and mints real tickets for freshly generated
//! identities; refusals are compared as whole JSON-RPC error objects, since
//! "identical" is about what the client can see.
//!
//! @ai-caution: [security] Never print a token or a ticket in an assertion
//! message: assert on kinds, codes and closes only.

mod daemon_harness;
mod idp_common;
mod ws_common;

use idp_common::{binding_cli, idp_keys, idp_token, spawn_jwks, ISS};
use mae_mcp::identity::Identity;
use serde_json::{json, Value};
use std::time::Duration;
use ws_common::*;

/// A fresh member key's fingerprint: the subject of a self-issued token.
fn member() -> String {
    Identity::generate("member").fingerprint()
}

fn ok(resp: &Value) -> bool {
    resp["error"].is_null()
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A fresh name (a KB id, an IdP subject): 64 random bits, hex.
fn fresh() -> String {
    let mut bytes = [0u8; 8];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut bytes);
    hex::encode(bytes)
}

/// A ticket no daemon minted: 256 fresh random bits in the minted encoding.
fn unknown_ticket() -> String {
    let mut bytes = [0u8; 32];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut bytes);
    data_encoding::BASE64URL_NOPAD.encode(&bytes)
}

/// Mint a ticket with `token` from `origin`; the mint must succeed.
async fn ticket(d: &Daemon, token: &str, origin: Option<&str>) -> String {
    let minted = mint_ticket(d, token, origin).await;
    assert_eq!(minted.status, 200, "ticket mint refused: {}", minted.body);
    minted.json()["ticket"]
        .as_str()
        .expect("ticket")
        .to_string()
}

/// `GET /ws` presenting ticket `t` from `ORIGIN_A`.
async fn upgrade_with(d: &Daemon, t: &str) -> Reply {
    let protocols = format!("mae.v1, mae.ticket.{t}");
    upgrade(d, &protocols, &[("Origin", ORIGIN_A)]).await
}

/// A browser's socket: opened from `ORIGIN_A` with a ticket minted by
/// `token`. Returns the socket, its `session/hello`, and the spent ticket.
async fn browser_socket(d: &Daemon, token: &str) -> (Ws, Value, String) {
    let t = ticket(d, token, Some(ORIGIN_A)).await;
    let r = upgrade_with(d, &t).await;
    assert_eq!(r.status, 101, "ticket upgrade refused: {}", r.body);
    let mut ws = r.ws.unwrap();
    let hello = ws.until(|v| v["method"] == "session/hello").await;
    (ws, hello, t)
}

/// `session/renew` with a ticket; the response.
async fn renew_ticket(ws: &mut Ws, id: u64, t: &str) -> Value {
    ws.call(id, "session/renew", json!({ "ticket": t })).await
}

/// The error a refused ticket renewal answers with: the whole object, so two
/// refusals compare on code, message and data at once.
fn refusal_of(resp: &Value) -> Value {
    assert!(!ok(resp), "the renewal was not refused");
    resp["error"].clone()
}

/// The socket still serves requests: a refusal left the session running.
async fn still_open(ws: &mut Ws, id: u64) {
    let kb = format!("kb-{}", fresh());
    let resp = ws.call(id, "kb/share", kb_share(&kb, &[])).await;
    assert!(ok(&resp), "the session continues: {resp}");
}

/// F2: a ticket minted by the backend with a fresh token renews the socket,
/// moves the deadline past the first token's, and is never echoed back.
#[tokio::test]
async fn a_ticket_renews_and_moves_the_deadline() {
    const FIRST_TOKEN_SECS: u64 = 5;
    let d = spawn(Opts {
        renew_lead_secs: FIRST_TOKEN_SECS - 1,
        ..Opts::default()
    })
    .await;
    let who = member();
    let (mut ws, hello, _) = browser_socket(&d, &d.token(&who, FIRST_TOKEN_SECS)).await;
    let first_deadline = deadline_unix(&hello["params"]["auth_deadline"]);
    ws.until(|v| v["method"] == "session/renew_needed").await;
    let t = ticket(&d, &d.token(&who, 60), Some(ORIGIN_A)).await;
    let renewed = renew_ticket(&mut ws, 1, &t).await;
    assert!(ok(&renewed), "a valid ticket renews: {renewed}");
    let new_deadline = deadline_unix(&renewed["result"]["auth_deadline"]);
    assert!(
        new_deadline > now_unix() + 30,
        "the deadline is the new token's"
    );
    sleep_past(first_deadline, Duration::from_millis(250)).await;
    still_open(&mut ws, 2).await;
    assert!(
        ws.log.iter().all(|f| !f.contains(&t)),
        "the ticket must never appear in a frame the daemon sends"
    );
}

/// F2: a valid ticket for ANOTHER subject closes the socket `4403`, as a
/// token for another subject does, and never answers the renewal.
#[tokio::test]
async fn a_ticket_for_another_subject_closes_4403() {
    let d = spawn(Opts::default()).await;
    let (mut ws, _, _) = browser_socket(&d, &d.token(&member(), 3600)).await;
    let other = ticket(&d, &d.token(&member(), 3600), Some(ORIGIN_A)).await;
    ws.send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "session/renew", "params": {"ticket": other}}),
    )
    .await;
    assert_eq!(ws.closed().await, (4403, "principal_changed".to_string()));
    assert!(
        !ws.log.iter().any(|f| f.contains("\"result\"")),
        "the renewal must not have succeeded before the close"
    );
}

/// F2/F3: reused, unknown, from another allowed origin, or with no origin: all
/// one refusal, `ticket_invalid`, and the socket stays open.
#[tokio::test]
async fn refused_tickets_all_read_the_same_and_leave_the_socket_open() {
    let d = spawn(Opts::default()).await;
    let who = member();
    let token = d.token(&who, 3600);
    let (mut ws, _, _) = browser_socket(&d, &token).await;
    let unknown = refusal_of(&renew_ticket(&mut ws, 1, &unknown_ticket()).await);
    assert_eq!(unknown["data"]["kind"], "ticket_invalid", "{unknown}");
    assert_eq!(unknown["code"], -32600);

    let t = ticket(&d, &token, Some(ORIGIN_A)).await;
    assert!(ok(&renew_ticket(&mut ws, 2, &t).await), "first use renews");
    let reused = refusal_of(&renew_ticket(&mut ws, 3, &t).await);
    assert_eq!(reused, unknown, "a reused ticket reads as unknown");

    let elsewhere = ticket(&d, &token, Some(ORIGIN_B)).await;
    let other_origin = refusal_of(&renew_ticket(&mut ws, 4, &elsewhere).await);
    assert_eq!(other_origin, unknown, "another allowed origin");

    let originless = ticket(&d, &token, None).await;
    let no_origin = refusal_of(&renew_ticket(&mut ws, 5, &originless).await);
    assert_eq!(no_origin, unknown, "no origin at all");
    still_open(&mut ws, 6).await;
}

/// F3: a ticket past its lifetime reads as unknown. The daemon runs 1-second
/// tickets; the wait is the expiry itself, measured from the mint's answer.
#[tokio::test]
async fn an_expired_ticket_does_not_renew() {
    let d = spawn(Opts {
        ticket_ttl_secs: Some(1),
        ..Opts::default()
    })
    .await;
    let token = d.token(&member(), 3600);
    let mut ws = connect_bearer_from(&d, &token, ORIGIN_A).await;
    let t = ticket(&d, &token, Some(ORIGIN_A)).await;
    let answered = tokio::time::Instant::now();
    tokio::time::sleep_until(answered + Duration::from_secs(1)).await;
    let expired = refusal_of(&renew_ticket(&mut ws, 1, &t).await);
    let unknown = refusal_of(&renew_ticket(&mut ws, 2, &unknown_ticket()).await);
    assert_eq!(expired, unknown);
    still_open(&mut ws, 3).await;
}

/// F3: a ticket whose minting token has since expired reads as unknown, even
/// inside the ticket's own lifetime.
#[tokio::test]
async fn a_ticket_outliving_its_token_does_not_renew() {
    const TOKEN_SECS: u64 = 2;
    let d = spawn(Opts::default()).await;
    let who = member();
    let (mut ws, _, _) = browser_socket(&d, &d.token(&who, 3600)).await;
    let t = ticket(&d, &d.token(&who, TOKEN_SECS), Some(ORIGIN_A)).await;
    // The token's `exp` is at most this (whole seconds, minted before now).
    let exp = now_unix() + TOKEN_SECS;
    sleep_past(exp, Duration::from_millis(250)).await;
    let outlived = refusal_of(&renew_ticket(&mut ws, 1, &t).await);
    let unknown = refusal_of(&renew_ticket(&mut ws, 2, &unknown_ticket()).await);
    assert_eq!(outlived, unknown);
    still_open(&mut ws, 3).await;
}

/// F3: one store, one spend. A ticket that opened a socket cannot renew one,
/// and a ticket spent by a renewal cannot open a socket.
#[tokio::test]
async fn a_ticket_is_spent_once_across_the_upgrade_and_renewal() {
    let d = spawn(Opts::default()).await;
    let token = d.token(&member(), 3600);
    let (mut ws, _, opened_with) = browser_socket(&d, &token).await;
    let unknown = refusal_of(&renew_ticket(&mut ws, 1, &unknown_ticket()).await);
    let after_upgrade = refusal_of(&renew_ticket(&mut ws, 2, &opened_with).await);
    assert_eq!(after_upgrade, unknown, "spent by the upgrade");

    let t = ticket(&d, &token, Some(ORIGIN_A)).await;
    assert!(ok(&renew_ticket(&mut ws, 3, &t).await), "first use renews");
    let r = upgrade_with(&d, &t).await;
    assert!(r.ws.is_none(), "spent by the renewal: no socket");
    assert_eq!(
        refusal(&r),
        (
            401,
            "invalid_token".to_string(),
            "ticket_invalid".to_string()
        )
    );
    let never_minted = upgrade_with(&d, &unknown_ticket()).await;
    assert_eq!(r.fingerprint(), never_minted.fingerprint());
}

/// F1: exactly one credential. Both, neither, or both `null` is
/// `invalid_params`, nothing renews, and a ticket presented beside a token is
/// spent all the same.
#[tokio::test]
async fn a_renewal_needs_exactly_one_of_token_and_ticket() {
    let d = spawn(Opts::default()).await;
    let token = d.token(&member(), 3600);
    let (mut ws, _, _) = browser_socket(&d, &token).await;
    let t = ticket(&d, &token, Some(ORIGIN_A)).await;
    let both = ws
        .call(1, "session/renew", json!({"token": token, "ticket": t}))
        .await;
    assert_eq!(
        kind(&both),
        "invalid_params",
        "{}",
        both["error"]["message"]
    );
    for (id, params) in [
        (2, json!({})),
        (3, json!({"token": null, "ticket": null})),
        (4, Value::Null),
    ] {
        let neither = ws.call(id, "session/renew", params).await;
        assert_eq!(kind(&neither), "invalid_params", "id {id}");
    }
    let unknown = refusal_of(&renew_ticket(&mut ws, 5, &unknown_ticket()).await);
    let spent = refusal_of(&renew_ticket(&mut ws, 6, &t).await);
    assert_eq!(spent, unknown, "the ticket beside the token was spent");
    still_open(&mut ws, 7).await;
}

/// Regression: a token renewal is unchanged on a ticket-opened socket. A good
/// token renews, a bad one is `invalid_token` with the socket open, another
/// subject's closes `4403`.
#[tokio::test]
async fn a_token_renewal_is_unchanged() {
    let d = spawn(Opts::default()).await;
    let who = member();
    let (mut ws, _, _) = browser_socket(&d, &d.token(&who, 3600)).await;
    let renewed = ws
        .call(1, "session/renew", json!({"token": d.token(&who, 600)}))
        .await;
    assert!(ok(&renewed), "{}", renewed["error"]);
    assert!(deadline_unix(&renewed["result"]["auth_deadline"]) > now_unix() + 500);
    let stale = ws
        .call(2, "session/renew", json!({"token": d.token(&who, 0)}))
        .await;
    assert_eq!(kind(&stale), "invalid_token");
    still_open(&mut ws, 3).await;
    let other = d.token(&member(), 3600);
    ws.send(
        json!({"jsonrpc": "2.0", "id": 4, "method": "session/renew", "params": {"token": other}}),
    )
    .await;
    assert_eq!(ws.closed().await, (4403, "principal_changed".to_string()));
}

/// `GET /ws` with a bearer header from `origin`, expected to open.
async fn connect_bearer_from(d: &Daemon, token: &str, origin: &str) -> Ws {
    let auth = format!("Bearer {token}");
    let r = upgrade(d, "mae.v1", &[("Authorization", &auth), ("Origin", origin)]).await;
    assert_eq!(r.status, 101, "bearer upgrade refused: {}", r.body);
    let mut ws = r.ws.unwrap();
    ws.until(|v| v["method"] == "session/hello").await;
    ws
}

const GROUP: &str = "mae-pilot";
/// The capped tests' cap and renewal lead (as in `ws_bound_idp_e2e.rs`).
const CAP: u64 = 6;
const LEAD: u64 = 3;

/// A daemon accepting bound IdP tokens, capped at `cap`; its signing key.
async fn idp_daemon(cap: Option<u64>) -> (Daemon, String) {
    let (pem, jwks) = idp_keys();
    let jwks_port = spawn_jwks(&jwks).await.port();
    let d = spawn(Opts {
        idp: Some(IdpOpts {
            jwks_port,
            issuer: ISS.to_string(),
            allowed_groups: vec![GROUP.to_string()],
            accept_bound_idp: true,
            max_session_secs: cap,
        }),
        renew_lead_secs: LEAD,
        ..Opts::default()
    })
    .await;
    (d, pem)
}

fn binding(d: &Daemon, args: &[&str]) {
    let (done, out) = binding_cli(d.config(), d.root(), args);
    assert!(done, "binding {args:?}: {out}");
}

fn bind(d: &Daemon, sub: &str, fp: &str) {
    binding(
        d,
        &[
            "bind",
            "--issuer",
            ISS,
            "--subject",
            sub,
            "--fingerprint",
            fp,
        ],
    );
}

fn unbind(d: &Daemon, sub: &str) {
    binding(d, &["unbind", "--issuer", ISS, "--subject", sub]);
}

/// F4: a ticket minted by a bound IdP token is capped by E2 when it renews,
/// not moved to the token's hour; so is the socket it opened.
#[tokio::test]
async fn a_bound_idp_ticket_renewal_is_capped() {
    let (d, pem) = idp_daemon(Some(CAP)).await;
    let sub = format!("{}@idp", fresh());
    bind(&d, &sub, &member());
    let token = idp_token(&pem, &sub, &[GROUP]);
    let opened = now_unix();
    let (mut ws, hello, _) = browser_socket(&d, &token).await;
    let deadline = deadline_unix(&hello["params"]["auth_deadline"]);
    assert!(
        deadline <= now_unix() + CAP && deadline >= opened + CAP - 1,
        "a ticket-opened session is capped: {}",
        hello["params"]["auth_deadline"]
    );
    let t = ticket(&d, &token, Some(ORIGIN_A)).await;
    let asked = now_unix();
    let renewed = renew_ticket(&mut ws, 1, &t).await;
    assert!(ok(&renewed), "{}", renewed["error"]);
    let deadline = deadline_unix(&renewed["result"]["auth_deadline"]);
    assert!(
        deadline <= now_unix() + CAP && deadline >= asked + CAP - 1,
        "a ticket renewal is capped too, not moved to the token's exp: {}",
        renewed["result"]["auth_deadline"]
    );
}

/// F2/F3: a bound IdP ticket re-reads its binding when spent. Unbound inside
/// its lifetime, it gets the one ticket refusal and the socket stays; a ticket
/// minted after a rebind to another key names another principal and closes
/// the socket `4403`.
#[tokio::test]
async fn a_bound_idp_ticket_rereads_its_binding_at_renewal() {
    let (d, pem) = idp_daemon(None).await;
    let sub = format!("{}@idp", fresh());
    bind(&d, &sub, &member());
    let token = idp_token(&pem, &sub, &[GROUP]);
    let (mut ws, _, _) = browser_socket(&d, &token).await;
    let minted_before_unbind = ticket(&d, &token, Some(ORIGIN_A)).await;
    unbind(&d, &sub);
    let gone = refusal_of(&renew_ticket(&mut ws, 1, &minted_before_unbind).await);
    let unknown = refusal_of(&renew_ticket(&mut ws, 2, &unknown_ticket()).await);
    assert_eq!(gone, unknown, "binding gone: the one ticket refusal");
    still_open(&mut ws, 3).await;

    bind(&d, &sub, &member());
    let rebound = ticket(&d, &token, Some(ORIGIN_A)).await;
    ws.send(json!({"jsonrpc": "2.0", "id": 4, "method": "session/renew", "params": {"ticket": rebound}}))
        .await;
    assert_eq!(ws.closed().await, (4403, "principal_changed".to_string()));
}
