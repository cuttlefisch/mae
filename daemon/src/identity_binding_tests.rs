//! The binding table and principal resolution (ADR-098 D3, Amendment 1 C1–C3).
//!
//! Attacker model: a person signed in at the identity provider tries to act as
//! a member key that is not theirs — by choosing a subject that spells a
//! fingerprint, by binding a key someone else already holds, or by keeping a
//! binding the administrator removed. Fingerprints are freshly generated keys,
//! never hand-written strings, except where a test needs the exact text.

use super::*;
use crate::oauth_self_issue::{unbound_principal, Credential, PrincipalKind, ValidatedPrincipal};
use mae_mcp::identity::Identity;

const ISS: &str = "https://idp.example.com";

fn fp() -> String {
    Identity::generate("member").fingerprint()
}

fn idp(issuer: &str, subject: &str) -> ValidatedPrincipal {
    ValidatedPrincipal {
        principal: unbound_principal(issuer, subject),
        audience: vec!["https://mae.example.com".into()],
        expires_at: u64::MAX,
        issuer: issuer.into(),
        subject: subject.into(),
        credential: Credential::IdpUnbound,
        kind: PrincipalKind::Agent,
        sign_in: None,
    }
}

fn self_issued(fingerprint: &str) -> ValidatedPrincipal {
    ValidatedPrincipal {
        principal: fingerprint.into(),
        audience: vec!["https://mae.example.com".into()],
        expires_at: u64::MAX,
        issuer: "self".into(),
        subject: fingerprint.into(),
        credential: Credential::SelfIssued,
        kind: PrincipalKind::Agent,
        sign_in: None,
    }
}

#[test]
fn one_subject_one_key_in_both_directions() {
    let store = BindingStore::open_memory().unwrap();
    let (a, b) = (fp(), fp());
    store.bind(ISS, "alice", &a, BoundBy::Proof, 1).unwrap();
    assert_eq!(
        store.bind(ISS, "mallory", &a, BoundBy::Proof, 2),
        Err(BindingError::FingerprintBound),
        "a second subject must not bind a fingerprint another subject holds"
    );
    assert_eq!(
        store.bind(ISS, "alice", &b, BoundBy::Admin, 3),
        Err(BindingError::SubjectBound),
        "a bound subject must not gain a second key"
    );
    // Same subject under another issuer is another identity.
    store
        .bind(
            "https://other-idp.example.com",
            "alice",
            &b,
            BoundBy::Admin,
            4,
        )
        .unwrap();
    let rows = store.list().unwrap();
    assert_eq!(rows.len(), 2);
    let alice = store.lookup(ISS, "alice").unwrap().unwrap();
    assert_eq!(
        (
            alice.fingerprint.as_str(),
            alice.kind,
            alice.bound_by,
            alice.bound_at
        ),
        (a.as_str(), PrincipalKind::Agent, BoundBy::Proof, 1)
    );
}

#[test]
fn replacing_a_binding_takes_an_explicit_unbind() {
    let store = BindingStore::open_memory().unwrap();
    let (a, b) = (fp(), fp());
    store.bind(ISS, "alice", &a, BoundBy::Proof, 1).unwrap();
    store.unbind(&Selector::Fingerprint(a.clone())).unwrap();
    store.bind(ISS, "alice", &b, BoundBy::Proof, 2).unwrap();
    assert_eq!(store.lookup(ISS, "alice").unwrap().unwrap().fingerprint, b);
    assert_eq!(
        store.unbind(&Selector::Fingerprint(a)),
        Err(BindingError::NotFound)
    );
}

/// The table refuses a second binding even from a writer that skips
/// `bind`'s checks (an older build, a hand-run INSERT).
#[test]
fn the_schema_itself_holds_the_one_to_one_rule() {
    let store = BindingStore::open_memory().unwrap();
    let a = fp();
    store.bind(ISS, "alice", &a, BoundBy::Proof, 1).unwrap();
    let conn = store.lock();
    let raw = |sub: &str, f: &str| {
        conn.execute(format!(
            "INSERT INTO identity_bindings (issuer, subject, fingerprint, kind, bound_by, \
             bound_at) VALUES ('{ISS}', '{sub}', '{f}', 'agent', 'admin', 0)"
        ))
    };
    assert!(raw("mallory", &a).is_err(), "fingerprint must be UNIQUE");
    assert!(
        raw("alice", &fp()).is_err(),
        "(issuer, subject) must be the key"
    );
}

#[test]
fn only_a_key_fingerprint_can_be_bound() {
    let store = BindingStore::open_memory().unwrap();
    for bad in ["alice", "SHA256:short", "", &unbound_principal(ISS, "x")] {
        assert_eq!(
            store.bind(ISS, "alice", bad, BoundBy::Admin, 1),
            Err(BindingError::Malformed),
            "{bad:?} is not a key fingerprint"
        );
    }
    assert_eq!(
        store.bind(ISS, "", &fp(), BoundBy::Admin, 1),
        Err(BindingError::Malformed)
    );
    assert!(store.list().unwrap().is_empty());
}

/// C1: an IdP subject that spells a member's fingerprint byte for byte is
/// still an unbound subject, and its principal string can never equal any
/// fingerprint.
#[test]
fn a_subject_spelling_a_fingerprint_is_not_that_member() {
    let store = BindingStore::open_memory().unwrap();
    let member = fp();
    let vp = resolve(idp(ISS, &member), Some(&store)).unwrap();
    assert_eq!(vp.credential, Credential::IdpUnbound);
    assert_ne!(vp.principal, member);
    assert!(!vp.principal.starts_with("SHA256:"));
    assert!(!is_fingerprint(&vp.principal));
}

#[test]
fn unbound_principals_are_distinct_for_distinct_pairs() {
    assert_ne!(unbound_principal("a:b", "c"), unbound_principal("a", "b:c"));
    assert_ne!(unbound_principal("a\"", "b"), unbound_principal("a", "\"b"));
}

/// C3's matrix: a person only with an IdP token AND a `person` binding.
#[test]
fn only_an_idp_session_on_a_person_binding_is_a_person() {
    let store = BindingStore::open_memory().unwrap();
    let (flagged, plain) = (fp(), fp());
    store
        .bind(ISS, "alice", &flagged, BoundBy::Proof, 1)
        .unwrap();
    store.bind(ISS, "bob", &plain, BoundBy::Admin, 1).unwrap();
    store
        .set_kind(
            &Selector::Fingerprint(flagged.clone()),
            PrincipalKind::Person,
        )
        .unwrap();

    let alice = resolve(idp(ISS, "alice"), Some(&store)).unwrap();
    assert_eq!(
        (alice.principal.as_str(), alice.credential, alice.kind),
        (
            flagged.as_str(),
            Credential::IdpBound,
            PrincipalKind::Person
        )
    );
    let bob = resolve(idp(ISS, "bob"), Some(&store)).unwrap();
    assert_eq!(
        (bob.principal.as_str(), bob.kind),
        (plain.as_str(), PrincipalKind::Agent)
    );
    // The flagged key presented by a self-issued token: an agent in that
    // person's editor signs with the same key, and has no IdP sign-in.
    let agent = resolve(self_issued(&flagged), Some(&store)).unwrap();
    assert_eq!(
        (agent.credential, agent.kind),
        (Credential::SelfIssued, PrincipalKind::Agent)
    );
}

/// A self-issued token is never looked up, even when a row's `(iss, sub)`
/// happens to equal its own (`"self"`, its fingerprint): it cannot be
/// redirected to another key, nor promoted to a person.
#[test]
fn a_self_issued_token_is_never_resolved_through_the_table() {
    let store = BindingStore::open_memory().unwrap();
    let (mine, theirs) = (fp(), fp());
    store
        .bind("self", &mine, &theirs, BoundBy::Admin, 1)
        .unwrap();
    store
        .set_kind(&Selector::Fingerprint(theirs), PrincipalKind::Person)
        .unwrap();
    let vp = resolve(self_issued(&mine), Some(&store)).unwrap();
    assert_eq!(
        (vp.principal.as_str(), vp.credential, vp.kind),
        (mine.as_str(), Credential::SelfIssued, PrincipalKind::Agent)
    );
}

/// No table (it failed to open): every external token stays unbound.
#[test]
fn without_a_table_every_idp_token_is_unbound() {
    let vp = resolve(idp(ISS, "alice"), None).unwrap();
    assert_eq!(vp.credential, Credential::IdpUnbound);
}

/// The table is read on every resolution, never cached: a change made by
/// another process (the CLI) holds on the next request.
#[test]
fn an_unbind_by_another_connection_holds_on_the_next_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(BINDINGS_FILE);
    let listener_side = BindingStore::open(&path).unwrap();
    let cli_side = BindingStore::open(&path).unwrap();
    let a = fp();
    cli_side.bind(ISS, "alice", &a, BoundBy::Admin, 1).unwrap();
    assert_eq!(
        resolve(idp(ISS, "alice"), Some(&listener_side))
            .unwrap()
            .principal,
        a
    );
    cli_side
        .set_kind(&Selector::Fingerprint(a.clone()), PrincipalKind::Person)
        .unwrap();
    assert_eq!(
        resolve(idp(ISS, "alice"), Some(&listener_side))
            .unwrap()
            .kind,
        PrincipalKind::Person
    );
    cli_side
        .unbind(&Selector::Subject {
            issuer: ISS.into(),
            subject: "alice".into(),
        })
        .unwrap();
    let after = resolve(idp(ISS, "alice"), Some(&listener_side)).unwrap();
    assert_eq!(after.credential, Credential::IdpUnbound);
    assert_ne!(after.principal, a);
}

#[test]
fn set_kind_on_no_binding_is_not_found() {
    let store = BindingStore::open_memory().unwrap();
    assert_eq!(
        store.set_kind(&Selector::Fingerprint(fp()), PrincipalKind::Person),
        Err(BindingError::NotFound)
    );
}

/// A COMMIT that fails must not leave the connection inside the transaction:
/// left so, every later `BEGIN IMMEDIATE` fails and the store is dead until a
/// restart. A deferred foreign key, violated by a trigger on one subject only,
/// makes exactly that write fail at COMMIT (where SQLite keeps the
/// transaction open).
#[test]
fn a_failed_commit_rolls_back_and_the_store_stays_writable() {
    let conn = sqlite::Connection::open(":memory:").unwrap();
    conn.execute("PRAGMA foreign_keys=ON").unwrap();
    let store = BindingStore::from_conn(conn).unwrap();
    store
        .lock()
        .execute(
            "CREATE TABLE parent (id INTEGER PRIMARY KEY);
             CREATE TABLE child (p INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED);
             CREATE TRIGGER poison AFTER INSERT ON identity_bindings
                 WHEN NEW.subject = 'poison'
             BEGIN INSERT INTO child (p) VALUES (1); END;",
        )
        .unwrap();
    let e = store
        .bind(ISS, "poison", &fp(), BoundBy::Admin, 1)
        .unwrap_err();
    assert!(matches!(e, BindingError::Storage(_)), "{e:?}");
    assert_eq!(store.lookup(ISS, "poison"), Ok(None), "not rolled back");
    let a = fp();
    store.bind(ISS, "alice", &a, BoundBy::Admin, 1).unwrap();
    assert_eq!(store.lookup(ISS, "alice").unwrap().unwrap().fingerprint, a);
}

/// The listener's lookup runs off the runtime: with the store held (as a write
/// waiting on a CLI writer holds it, for up to `busy_timeout`), a timer on the
/// same single-threaded runtime still fires on time, and the lookup completes
/// once the store is free.
#[tokio::test(flavor = "current_thread")]
async fn a_held_store_does_not_stall_the_runtime() {
    const HOLD: std::time::Duration = std::time::Duration::from_secs(2);
    let store = std::sync::Arc::new(BindingStore::open_memory().unwrap());
    let a = fp();
    store.bind(ISS, "alice", &a, BoundBy::Admin, 1).unwrap();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let holder = {
        let s = std::sync::Arc::clone(&store);
        std::thread::spawn(move || {
            let _g = s.lock();
            held_tx.send(()).unwrap();
            std::thread::sleep(HOLD);
        })
    };
    held_rx.recv().unwrap();
    let t = std::time::Instant::now();
    let lookup = tokio::spawn(resolve_off_runtime(
        idp(ISS, "alice"),
        Some(std::sync::Arc::clone(&store)),
    ));
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let ticked = t.elapsed();
    assert!(ticked < HOLD / 2, "the runtime stalled for {ticked:?}");
    assert_eq!(lookup.await.unwrap().unwrap().principal, a);
    holder.join().unwrap();
}
