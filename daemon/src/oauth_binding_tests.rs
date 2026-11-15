//! Proof of possession (ADR-098 Amendment 1, C2), through the routes' own
//! functions with a real binding table, real Ed25519 keys and the real
//! challenge text. The HTTP framing and token validation in front of them are
//! covered over the wire by `daemon/tests/identity_binding_e2e.rs`.
//!
//! Attacker model: a person signed in as one subject tries to bind a key they
//! do not hold, to bind someone else's key, to reuse a challenge (theirs or
//! another's), or to replay a signature made for another daemon.

use std::time::{Duration, Instant};

use super::*;
use mae_daemon::identity_binding::{BindingStore, Selector};
use mae_daemon::oauth_self_issue::{unbound_principal, PrincipalKind};
use mae_mcp::identity::Identity;
use mae_mcp::identity_challenge::{sign_challenge, Claimant, DOMAIN_TAG};

const ISS: &str = "https://idp.example.com";
const AUD: &str = "https://mae.example.com";

fn idp(subject: &str) -> ValidatedPrincipal {
    idp_at(ISS, subject)
}

fn idp_at(issuer: &str, subject: &str) -> ValidatedPrincipal {
    ValidatedPrincipal {
        principal: unbound_principal(issuer, subject),
        audience: vec![AUD.into()],
        expires_at: u64::MAX,
        issuer: issuer.into(),
        subject: subject.into(),
        credential: Credential::IdpUnbound,
        kind: PrincipalKind::Agent,
    }
}

fn routes() -> BindingRoutes {
    BindingRoutes::new(300, 4)
}

/// Response status and JSON body, plus the raw body bytes as text.
fn read(resp: HttpResponse) -> (StatusCode, Value, String) {
    use http_body_util::BodyExt;
    let status = resp.status();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let bytes = rt.block_on(resp.into_body().collect()).unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (status, serde_json::from_slice(&bytes).unwrap(), text)
}

/// A challenge for `fp`, issued to `vp`: `(id, text)`.
fn challenge(r: &BindingRoutes, vp: &ValidatedPrincipal, fp: &str) -> (String, String) {
    let (status, body, _) = read(issue_challenge(
        r,
        vp,
        AUD,
        &json!({"fingerprint": fp}),
        Instant::now(),
    ));
    assert_eq!(status, StatusCode::OK, "{body}");
    (
        body["challenge_id"].as_str().unwrap().into(),
        body["challenge"].as_str().unwrap().into(),
    )
}

fn bind_body(id: &str, key: &Identity, signed: &str) -> Value {
    json!({
        "challenge_id": id,
        "public_key": key.public().encoded(),
        "signature": data_encoding::BASE64.encode(&key.sign(signed.as_bytes())),
    })
}

fn bind_now(
    r: &BindingRoutes,
    store: &BindingStore,
    vp: &ValidatedPrincipal,
    body: &Value,
) -> (StatusCode, Value, String) {
    read(complete_binding(r, store, vp, AUD, body, Instant::now()))
}

fn kind_of(v: &Value) -> &str {
    v["kind"].as_str().unwrap_or_default()
}

/// The challenge covers every field C2 lists, each with the right value, and
/// its nonce is the id the bind presents.
#[test]
fn the_challenge_covers_tag_issuer_subject_audience_fingerprint_and_nonce() {
    let r = routes();
    let key = Identity::generate("alice");
    let (id, text) = challenge(&r, &idp("alice"), &key.fingerprint());
    let c: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(c["domain"], DOMAIN_TAG);
    assert_eq!(c["iss"], ISS);
    assert_eq!(c["sub"], "alice");
    assert_eq!(c["aud"], AUD);
    assert_eq!(c["fingerprint"], key.fingerprint());
    assert_eq!(c["nonce"], id.as_str());
    assert_eq!(id.len(), 43, "256 random bits, base64url");
    let exp = c["expires_at"].as_u64().unwrap();
    let now = unix_now();
    assert!(
        exp > now && exp <= now + 300,
        "expires within the TTL: {exp}"
    );
    let (id2, _) = challenge(&r, &idp("alice"), &key.fingerprint());
    assert_ne!(id, id2, "every challenge has a fresh nonce");
}

#[test]
fn a_signature_by_the_claimed_key_binds_the_subject_as_an_agent_by_proof() {
    let (r, store) = (routes(), BindingStore::open_memory().unwrap());
    let key = Identity::generate("alice");
    let vp = idp("alice");
    let (id, text) = challenge(&r, &vp, &key.fingerprint());
    let (status, body, _) = bind_now(&r, &store, &vp, &bind_body(&id, &key, &text));
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["principal"], key.fingerprint());
    let b = store.lookup(ISS, "alice").unwrap().unwrap();
    assert_eq!(
        (b.fingerprint, b.kind, b.bound_by),
        (key.fingerprint(), PrincipalKind::Agent, BoundBy::Proof)
    );
}

/// The editor's signer (`mae --sign-identity-challenge` is a thin wrapper over
/// `sign_challenge`) and the route agree on the format: the route's whole
/// answer, pasted as the person would, signed by it, binds. The signer also
/// refuses the same challenge for any other key, so it never signs what the
/// route would reject.
#[test]
fn the_editors_signer_binds_through_the_route() {
    let (r, store) = (routes(), BindingStore::open_memory().unwrap());
    let key = Identity::generate("alice");
    let vp = idp("alice");
    let (status, _, answer) = read(issue_challenge(
        &r,
        &vp,
        AUD,
        &json!({"fingerprint": key.fingerprint()}),
        Instant::now(),
    ));
    assert_eq!(status, StatusCode::OK);
    let other = Identity::generate("other");
    let me = Claimant {
        issuer: ISS,
        subject: "alice",
    };
    assert!(sign_challenge(&other, me, &answer, unix_now()).is_err());
    let (_, body) = sign_challenge(&key, me, &answer, unix_now()).unwrap();
    let (status, v, _) = bind_now(&r, &store, &vp, &body);
    assert_eq!(status, StatusCode::OK, "{v}");
    let b = store.lookup(ISS, "alice").unwrap().unwrap();
    assert_eq!(
        (b.fingerprint, b.bound_by),
        (key.fingerprint(), BoundBy::Proof)
    );
}

/// The claimed key's fingerprint, signed by another key; and another key's
/// fingerprint presented with its own valid signature over a challenge naming
/// the first. Both refused, nothing bound.
#[test]
fn a_signature_by_any_other_key_is_refused() {
    let (r, store) = (routes(), BindingStore::open_memory().unwrap());
    let (victim, attacker) = (Identity::generate("victim"), Identity::generate("attacker"));
    let vp = idp("mallory");

    let (id, text) = challenge(&r, &vp, &victim.fingerprint());
    let mut body = bind_body(&id, &attacker, &text);
    body["public_key"] = json!(victim.public().encoded());
    let (status, v, _) = bind_now(&r, &store, &vp, &body);
    assert_eq!(
        (status, kind_of(&v)),
        (StatusCode::FORBIDDEN, "proof_invalid")
    );

    let (id, text) = challenge(&r, &vp, &victim.fingerprint());
    let (status, v, _) = bind_now(&r, &store, &vp, &bind_body(&id, &attacker, &text));
    assert_eq!(
        (status, kind_of(&v)),
        (StatusCode::FORBIDDEN, "proof_invalid"),
        "a challenge for another fingerprint must not bind the signer's key"
    );
    assert!(store.list().unwrap().is_empty());
}

/// Alice's challenge and Alice's signature, presented with Mallory's token
/// (another subject, then the same subject at another issuer): refused.
#[test]
fn a_challenge_presented_by_another_subject_is_refused() {
    let (r, store) = (routes(), BindingStore::open_memory().unwrap());
    let key = Identity::generate("alice");
    for thief in [
        idp("mallory"),
        idp_at("https://other-idp.example.com", "alice"),
    ] {
        let (id, text) = challenge(&r, &idp("alice"), &key.fingerprint());
        let (status, v, _) = bind_now(&r, &store, &thief, &bind_body(&id, &key, &text));
        assert_eq!(
            (status, kind_of(&v)),
            (StatusCode::FORBIDDEN, "challenge_mismatch"),
            "{}",
            thief.principal
        );
    }
    assert!(store.list().unwrap().is_empty());
}

/// A challenge issued for another audience, and a signature over the text as
/// another daemon would have issued it: neither binds here.
#[test]
fn a_challenge_or_signature_for_another_audience_is_refused() {
    let (r, store) = (routes(), BindingStore::open_memory().unwrap());
    let key = Identity::generate("alice");
    let vp = idp("alice");

    let (id, text) = challenge(&r, &vp, &key.fingerprint());
    let (status, v, _) = read(complete_binding(
        &r,
        &store,
        &vp,
        "https://another-daemon.example.com",
        &bind_body(&id, &key, &text),
        Instant::now(),
    ));
    assert_eq!(
        (status, kind_of(&v)),
        (StatusCode::FORBIDDEN, "challenge_mismatch")
    );

    let (id, text) = challenge(&r, &vp, &key.fingerprint());
    let mut c: Value = serde_json::from_str(&text).unwrap();
    c["aud"] = json!("https://another-daemon.example.com");
    let (status, v, _) = bind_now(&r, &store, &vp, &bind_body(&id, &key, &c.to_string()));
    assert_eq!(
        (status, kind_of(&v)),
        (StatusCode::FORBIDDEN, "proof_invalid")
    );
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn an_expired_challenge_is_refused() {
    let (r, store) = (routes(), BindingStore::open_memory().unwrap());
    let key = Identity::generate("alice");
    let vp = idp("alice");
    let (id, text) = challenge(&r, &vp, &key.fingerprint());
    let late = Instant::now() + Duration::from_secs(301);
    let (status, v, _) = read(complete_binding(
        &r,
        &store,
        &vp,
        AUD,
        &bind_body(&id, &key, &text),
        late,
    ));
    assert_eq!(
        (status, kind_of(&v)),
        (StatusCode::FORBIDDEN, "challenge_invalid")
    );
    assert!(store.list().unwrap().is_empty());
}

/// Single use: after a success, and after a failed attempt, the same
/// challenge is gone — even with a correct signature the second time.
#[test]
fn a_challenge_is_used_once_whatever_the_outcome() {
    let (r, store) = (routes(), BindingStore::open_memory().unwrap());
    let (key, other) = (Identity::generate("alice"), Identity::generate("other"));
    let vp = idp("alice");

    let (id, text) = challenge(&r, &vp, &key.fingerprint());
    let (status, _, _) = bind_now(&r, &store, &vp, &bind_body(&id, &other, &text));
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, v, _) = bind_now(&r, &store, &vp, &bind_body(&id, &key, &text));
    assert_eq!(
        (status, kind_of(&v)),
        (StatusCode::FORBIDDEN, "challenge_invalid")
    );

    let (id, text) = challenge(&r, &vp, &key.fingerprint());
    let body = bind_body(&id, &key, &text);
    assert_eq!(bind_now(&r, &store, &vp, &body).0, StatusCode::OK);
    store
        .unbind(&Selector::Fingerprint(key.fingerprint()))
        .unwrap();
    let (status, v, _) = bind_now(&r, &store, &vp, &body);
    assert_eq!(
        (status, kind_of(&v)),
        (StatusCode::FORBIDDEN, "challenge_invalid"),
        "a replayed bind must not re-create a binding the administrator removed"
    );
    assert!(store.list().unwrap().is_empty());
}

/// D3's "never bind a fingerprint another principal holds", and its mirror.
/// Each refusal's raw bytes name neither the other subject nor its key.
#[test]
fn a_bound_key_or_a_bound_subject_cannot_be_bound_again() {
    let (r, store) = (routes(), BindingStore::open_memory().unwrap());
    let (alice_key, second) = (Identity::generate("alice"), Identity::generate("second"));
    let alice = idp("alice-at-idp");
    let (id, text) = challenge(&r, &alice, &alice_key.fingerprint());
    assert_eq!(
        bind_now(&r, &store, &alice, &bind_body(&id, &alice_key, &text)).0,
        StatusCode::OK
    );

    // Bob holds Alice's key too (a shared device, a stolen file).
    let bob = idp("bob-at-idp");
    let (id, text) = challenge(&r, &bob, &alice_key.fingerprint());
    let (status, v, raw) = bind_now(&r, &store, &bob, &bind_body(&id, &alice_key, &text));
    assert_eq!(
        (status, kind_of(&v)),
        (StatusCode::CONFLICT, "binding_conflict")
    );
    assert_eq!(v["field"], "fingerprint");
    assert!(
        !raw.contains("alice-at-idp"),
        "leaks the other subject: {raw}"
    );

    let (id, text) = challenge(&r, &alice, &second.fingerprint());
    let (status, v, raw) = bind_now(&r, &store, &alice, &bind_body(&id, &second, &text));
    assert_eq!(
        (status, kind_of(&v)),
        (StatusCode::CONFLICT, "binding_conflict")
    );
    assert_eq!(v["field"], "subject");
    assert!(
        !raw.contains(&alice_key.fingerprint()),
        "names the existing key: {raw}"
    );

    let b = store.lookup(ISS, "alice-at-idp").unwrap().unwrap();
    assert_eq!(b.fingerprint, alice_key.fingerprint());
    assert!(store.lookup(ISS, "bob-at-idp").unwrap().is_none());
}

/// Bounded per subject; one subject at its bound does not block another.
#[test]
fn a_subject_holds_a_bounded_number_of_challenges() {
    let r = routes();
    let fp = Identity::generate("k").fingerprint();
    for _ in 0..4 {
        challenge(&r, &idp("alice"), &fp);
    }
    let (status, v, _) = read(issue_challenge(
        &r,
        &idp("alice"),
        AUD,
        &json!({"fingerprint": fp}),
        Instant::now(),
    ));
    assert_eq!(
        (status, kind_of(&v)),
        (StatusCode::TOO_MANY_REQUESTS, "too_many_challenges")
    );
    assert_eq!(v["limit"], 4);
    challenge(&r, &idp("bob"), &fp);
    // Expired challenges free their slots.
    let later = Instant::now() + Duration::from_secs(301);
    let (status, _, _) = read(issue_challenge(
        &r,
        &idp("alice"),
        AUD,
        &json!({"fingerprint": fp}),
        later,
    ));
    assert_eq!(status, StatusCode::OK);
}

#[test]
fn a_challenge_needs_a_key_fingerprint() {
    let r = routes();
    for body in [
        json!({}),
        json!({"fingerprint": "alice"}),
        json!({"fingerprint": unbound_principal(ISS, "x")}),
    ] {
        let (status, v, _) = read(issue_challenge(&r, &idp("a"), AUD, &body, Instant::now()));
        assert_eq!(
            (status, kind_of(&v)),
            (StatusCode::BAD_REQUEST, "invalid_params")
        );
    }
}

/// The defaults are decisions: C2 says a challenge is "valid for minutes"
/// and is carried by hand between two programs, so the TTL is minutes, not
/// seconds (too short to carry) nor hours; and a subject needs only a few
/// outstanding at once, so the bound is small but not one (a retry after a
/// mistyped signature must not have to wait out the TTL).
#[test]
fn default_challenge_bounds_are_minutes_and_a_few() {
    let d = crate::oauth_config::OAuthConfig::default();
    assert!((120..=900).contains(&d.identity_challenge_ttl_secs));
    assert!((2..=8).contains(&d.identity_max_pending_challenges));
    assert!(
        !d.identity_binding_enabled,
        "off by default (principle #12)"
    );
    assert!(
        d.allowed_groups.is_empty(),
        "no group gate unless configured"
    );
}

/// The route's store write runs off the runtime: with another writer holding
/// the file (as `mae-daemon binding` does), a timer on the same
/// single-threaded runtime still fires on time, and the bind lands once the
/// file is free (inside `busy_timeout`).
#[tokio::test(flavor = "current_thread")]
async fn a_writer_holding_the_file_does_not_stall_the_runtime() {
    use http_body_util::BodyExt;
    const HOLD: Duration = Duration::from_secs(2);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(mae_daemon::identity_binding::BINDINGS_FILE);
    let store = Arc::new(BindingStore::open(&path).unwrap());
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let holder = {
        let path = path.clone();
        std::thread::spawn(move || {
            let c = sqlite::Connection::open(&path).unwrap();
            c.execute("BEGIN IMMEDIATE").unwrap();
            held_tx.send(()).unwrap();
            std::thread::sleep(HOLD);
            c.execute("ROLLBACK").unwrap();
        })
    };
    held_rx.recv().unwrap();
    let key = Identity::generate("alice");
    let t = Instant::now();
    let write = tokio::spawn(record_binding_off_runtime(
        Arc::clone(&store),
        idp("alice"),
        key.fingerprint(),
    ));
    tokio::time::sleep(Duration::from_millis(20)).await;
    let ticked = t.elapsed();
    assert!(ticked < HOLD / 2, "the runtime stalled for {ticked:?}");
    let resp = write.await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        store.lookup(ISS, "alice").unwrap().unwrap().fingerprint,
        key.fingerprint()
    );
    holder.join().unwrap();
}
