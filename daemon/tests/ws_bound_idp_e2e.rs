//! A bound identity-provider login on the sync WebSocket (ADR-099 Amendment 2,
//! E1/E2/E5), over the real listener: a real daemon, a mock JWKS issuing real
//! RS256 tokens, bindings made and removed with the real `mae-daemon binding`
//! CLI while the daemon serves, real member fingerprints.
//!
//! Each property is checked from the side that would break it: the switch off
//! must refuse a token that IS bound and valid elsewhere; an unbound token must
//! never open a socket; the group gate must answer before the binding is read;
//! a renewal must notice an unbind or a rebind made after the socket opened.

mod daemon_harness;
mod idp_common;
mod ws_common;

use idp_common::{binding_cli, idp_keys, idp_token, spawn_jwks, ISS};
use mae_mcp::identity::Identity;
use mae_sync::encoding::update_to_base64;
use serde_json::{json, Value};
use ws_common::*;

const GROUP: &str = "mae-pilot";

/// A daemon with an external issuer, `ws_accept_bound_idp = accept`, and the
/// issuer's signing key.
struct Pilot {
    d: Daemon,
    pem: String,
}

async fn pilot(accept: bool) -> Pilot {
    let (pem, jwks) = idp_keys();
    let jwks_port = spawn_jwks(&jwks).await.port();
    let d = spawn(Opts {
        idp: Some(IdpOpts {
            jwks_port,
            issuer: ISS.to_string(),
            allowed_groups: vec![GROUP.to_string()],
            accept_bound_idp: accept,
        }),
        ..Opts::default()
    })
    .await;
    Pilot { d, pem }
}

impl Pilot {
    /// An IdP token for `sub`, in the allowed group.
    fn token(&self, sub: &str) -> String {
        idp_token(&self.pem, sub, &[GROUP])
    }

    /// An IdP token for `sub` with no allowed group.
    fn groupless(&self, sub: &str) -> String {
        idp_token(&self.pem, sub, &["elsewhere"])
    }

    fn cli(&self, args: &[&str]) {
        let (ok, out) = binding_cli(self.d.config(), self.d.root(), args);
        assert!(ok, "binding {args:?}: {out}");
    }

    fn bind(&self, sub: &str, fp: &str) {
        self.cli(&[
            "bind",
            "--issuer",
            ISS,
            "--subject",
            sub,
            "--fingerprint",
            fp,
        ]);
    }

    fn unbind(&self, sub: &str) {
        self.cli(&["unbind", "--issuer", ISS, "--subject", sub]);
    }

    /// The principal the plain bearer probe (`GET /`) resolves `token` to.
    async fn probe(&self, token: &str) -> Value {
        let auth = format!("Bearer {token}");
        let r = request(&self.d, "GET", "/", &[("Authorization", &auth)], false).await;
        assert_eq!(r.status, 200, "{}", r.body);
        r.json()["principal"].clone()
    }
}

/// A fresh member key's fingerprint.
fn member() -> String {
    Identity::generate("member").fingerprint()
}

fn ok(resp: &Value) -> bool {
    resp["error"].is_null()
}

/// `GET /ws` with a bearer header and `mae.v1`.
async fn bearer_upgrade(d: &Daemon, token: &str) -> Reply {
    let auth = format!("Bearer {token}");
    upgrade(d, "mae.v1", &[("Authorization", &auth)]).await
}

/// Open a socket with a ticket minted by `token`; its `session/hello`.
async fn ticket_session(d: &Daemon, token: &str) -> (Ws, Value) {
    let minted = mint_ticket(d, token, Some(ORIGIN_A)).await;
    assert_eq!(minted.status, 200, "{}", minted.body);
    let t = minted.json()["ticket"].as_str().unwrap().to_string();
    let r = upgrade(
        d,
        &format!("mae.v1, mae.ticket.{t}"),
        &[("Origin", ORIGIN_A)],
    )
    .await;
    assert_eq!(r.status, 101, "{}", r.body);
    let mut ws = r.ws.unwrap();
    let hello = ws.until(|v| v["method"] == "session/hello").await;
    (ws, hello)
}

/// `(status, error, kind)` on both routes, which must agree.
async fn refusal_on_both_routes(d: &Daemon, token: &str) -> (u16, String, String) {
    let up = bearer_upgrade(d, token).await;
    assert!(up.ws.is_none(), "a refused upgrade leaves no socket");
    let mint = mint_ticket(d, token, None).await;
    assert_eq!(refusal(&up), refusal(&mint), "/ws and /ws/ticket disagree");
    refusal(&up)
}

/// E5: with the switch off, a token that is bound and valid on the rest of the
/// listener is refused on both routes exactly as before, byte for byte the
/// refusal any foreign token gets, and even without its group: the switch is
/// read before anything else.
#[tokio::test]
async fn with_the_switch_off_a_bound_token_is_refused_as_before() {
    let p = pilot(false).await;
    let fp = member();
    p.bind("alice@idp", &fp);
    let alice = p.token("alice@idp");
    assert_eq!(
        p.probe(&alice).await,
        json!(fp),
        "bound, and valid elsewhere"
    );

    let want = (
        401,
        "invalid_token".to_string(),
        "issuer_not_supported".to_string(),
    );
    assert_eq!(refusal_on_both_routes(&p.d, &alice).await, want);
    assert_eq!(
        refusal_on_both_routes(&p.d, &p.groupless("alice@idp")).await,
        want
    );
    let (other_pem, _) = idp_keys();
    let forged = idp_token(&other_pem, "alice@idp", &[GROUP]);
    let a = bearer_upgrade(&p.d, &alice).await;
    let f = bearer_upgrade(&p.d, &forged).await;
    assert_eq!(
        a.fingerprint(),
        f.fingerprint(),
        "the binding is not consulted"
    );
}

/// E1: a bound token opens a session whose principal is the bound fingerprint,
/// which edits a KB that fingerprint is a member of, and only that one.
#[tokio::test]
async fn a_bound_token_opens_a_session_as_the_bound_fingerprint_and_edits() {
    let p = pilot(true).await;
    let (owner, alice_fp) = (member(), member());
    p.bind("alice@idp", &alice_fp);
    let mut o = connect_bearer(&p.d, &p.d.token(&owner, 3600)).await;
    share_with(&mut o, "kp", Some(&alice_fp)).await;
    share_with(&mut o, "kq", None).await;

    let mut ws = connect_bearer(&p.d, &p.token("alice@idp")).await;
    let hello = ws.until(|v| v["method"] == "session/hello").await;
    assert_eq!(hello["params"]["principal"], json!(alice_fp));

    let joined = ws.call(1, "kb/join", json!({"kb_id": "kp"})).await;
    assert!(ok(&joined), "{joined}");
    assert_eq!(joined["result"]["principal"], json!(alice_fp));
    let mut doc = node_writer(&joined, "n1", &alice_fp, epoch_of(&joined));
    let update = update_to_base64(&doc.set_title("edited by the pilot"));
    let params = json!({"kb_id": "kp", "node_id": "n1", "update": update});
    let edited = ws.call(2, "kb/node_update", params).await;
    assert!(ok(&edited), "{edited}");

    let other = json!({"kb_id": "kq", "node_id": "n1"});
    let other = ws.call(3, "kb/node_fetch", other).await;
    assert_eq!(kind(&other), "not_member", "{other}");
}

/// Share `kb_id` with one node `n1`, adding `editor` as an editor if given.
async fn share_with(owner: &mut Ws, kb_id: &str, editor: Option<&str>) {
    let shared = owner
        .call(10, "kb/share", kb_share(kb_id, &[("n1", "one", "body")]))
        .await;
    assert!(ok(&shared), "{shared}");
    if let Some(fp) = editor {
        let add = json!({"kb_id": kb_id, "member": fp, "role": "editor", "label": "pilot"});
        let added = owner.call(11, "kb/add_member", add).await;
        assert!(ok(&added), "{added}");
    }
}

/// E1: an unbound token is refused `identity_not_bound` on both routes and no
/// socket exists, though the same login authenticates on the listener.
#[tokio::test]
async fn an_unbound_token_is_refused_and_opens_no_socket() {
    let p = pilot(true).await;
    let carol = p.token("carol@idp");
    let typed = p.probe(&carol).await;
    assert_ne!(typed, json!("carol@idp"), "never the raw subject");
    assert_eq!(
        refusal_on_both_routes(&p.d, &carol).await,
        (
            403,
            "access_denied".to_string(),
            "identity_not_bound".to_string()
        )
    );
}

/// E1: the group gate answers first. A token without an allowed group gets
/// `group_required` whether its subject is bound or not, so the answer says
/// nothing about the binding.
#[tokio::test]
async fn a_token_without_an_allowed_group_is_refused_before_the_binding_is_read() {
    let p = pilot(true).await;
    p.bind("alice@idp", &member());
    let want = (
        403,
        "insufficient_scope".to_string(),
        "group_required".to_string(),
    );
    for sub in ["alice@idp", "carol@idp"] {
        assert_eq!(
            refusal_on_both_routes(&p.d, &p.groupless(sub)).await,
            want,
            "{sub}"
        );
    }
}

/// E1: a ticket minted by a bound token opens a socket as that token's bound
/// fingerprint, and two bound logins never open as each other.
#[tokio::test]
async fn a_ticket_minted_by_a_bound_token_opens_as_the_bound_fingerprint() {
    let p = pilot(true).await;
    let (alice_fp, bob_fp) = (member(), member());
    p.bind("alice@idp", &alice_fp);
    p.bind("bob@idp", &bob_fp);
    let (_a, hello_a) = ticket_session(&p.d, &p.token("alice@idp")).await;
    let (_b, hello_b) = ticket_session(&p.d, &p.token("bob@idp")).await;
    assert_eq!(hello_a["params"]["principal"], json!(alice_fp));
    assert_eq!(hello_b["params"]["principal"], json!(bob_fp));
}

/// Open a socket as bound `alice@idp` and renew once with the binding intact,
/// so a later close is the change and not the renewal itself.
async fn renewed_session(p: &Pilot) -> (Ws, String) {
    let token = p.token("alice@idp");
    let mut ws = connect_bearer(&p.d, &token).await;
    let renewed = ws
        .call(1, "session/renew", json!({"token": token.clone()}))
        .await;
    assert!(ok(&renewed), "an unchanged binding renews: {renewed}");
    (ws, token)
}

/// E2: unbinding the subject, then renewing with the same token, closes 4403.
#[tokio::test]
async fn unbinding_then_renewing_closes_4403() {
    let p = pilot(true).await;
    p.bind("alice@idp", &member());
    let (mut ws, token) = renewed_session(&p).await;
    p.unbind("alice@idp");
    ws.send(
        json!({"jsonrpc": "2.0", "id": 2, "method": "session/renew", "params": {"token": token}}),
    )
    .await;
    assert_eq!(ws.closed().await, (4403, "principal_changed".to_string()));
}

/// E2: rebinding the subject to another key, then renewing, closes 4403.
#[tokio::test]
async fn rebinding_to_another_key_then_renewing_closes_4403() {
    let p = pilot(true).await;
    p.bind("alice@idp", &member());
    let (mut ws, token) = renewed_session(&p).await;
    p.unbind("alice@idp");
    p.bind("alice@idp", &member());
    ws.send(
        json!({"jsonrpc": "2.0", "id": 2, "method": "session/renew", "params": {"token": token}}),
    )
    .await;
    assert_eq!(ws.closed().await, (4403, "principal_changed".to_string()));
}

/// E2: a renewal token that lost its group is answered `group_required`, and
/// the old deadline stands: the socket stays open until it.
#[tokio::test]
async fn a_renewal_without_the_group_is_refused_and_the_socket_stays() {
    let p = pilot(true).await;
    p.bind("alice@idp", &member());
    let (mut ws, _) = renewed_session(&p).await;
    let lost = json!({"token": p.groupless("alice@idp")});
    let refused = ws.call(2, "session/renew", lost).await;
    assert_eq!(kind(&refused), "group_required", "{refused}");
    let still = ws.call(3, "kb/share", kb_share("ks", &[])).await;
    assert!(ok(&still), "the session continues: {still}");
}

/// A token past its expiry but inside the JWT library's clock leeway neither
/// opens a socket nor renews one: `credential` holds that check for the
/// upgrade, the ticket mint and the renewal alike.
#[tokio::test]
async fn a_token_inside_the_clock_leeway_neither_opens_nor_renews() {
    let d = spawn(Opts::default()).await;
    let stale = d.token("SHA256:alice", 0);
    let r = bearer_upgrade(&d, &stale).await;
    assert_eq!(r.status, 401, "{}", r.body);
    assert!(r.ws.is_none());
    let mut ws = connect_bearer(&d, &d.token("SHA256:alice", 3600)).await;
    let renewed = ws.call(1, "session/renew", json!({"token": stale})).await;
    assert_eq!(kind(&renewed), "invalid_token", "{renewed}");
}
