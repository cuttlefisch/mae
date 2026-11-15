//! The signer must never be a signing oracle for the member key: everything
//! that is not exactly a daemon-issued challenge for THIS key, unexpired, is
//! refused and nothing is signed. Keys are freshly generated per test.
//!
//! The same cases run against `mae_mcp::identity_challenge` (the editor's
//! signer, over `Identity`) in shared/mcp, and against the WASM module in
//! shared/crypto-wasm/js/test.mjs.

use super::*;
use crate::member::MemberKey;
use base64::Engine;

fn key() -> MemberKey {
    MemberKey::generate().unwrap()
}

const NOW: u64 = 1_900_000_000;
const ISS: &str = "https://idp.example.com";
const SUB: &str = "alice@idp";
const AUD: &str = "https://mae.example.com";

/// The key holder, stating the identity the challenges below are issued to.
const ME: Claimant<'static> = Claimant {
    issuer: ISS,
    subject: SUB,
    audience: AUD,
};

/// Whether a refusal is the one a case expects.
type Expect = fn(&ChallengeRefusal) -> bool;

fn canonical(id: &MemberKey, exp: u64) -> String {
    issued_to(id, ISS, SUB, exp)
}

fn issued_to(id: &MemberKey, iss: &str, sub: &str, exp: u64) -> String {
    issued_at(id, iss, sub, AUD, exp)
}

fn issued_at(id: &MemberKey, iss: &str, sub: &str, aud: &str, exp: u64) -> String {
    challenge_text(
        iss,
        sub,
        aud,
        &id.fingerprint(),
        "bm9uY2Utbm9uY2Utbm9uY2Utbm9uY2Utbm9uY2Utbm9u",
        exp,
    )
}

fn long_subject(id: &MemberKey) -> String {
    challenge_text(
        "https://idp.example.com",
        &"a".repeat(MAX_CHALLENGE_BYTES),
        "https://mae.example.com",
        &id.fingerprint(),
        "bm9uY2Utbm9uY2Utbm9uY2Utbm9uY2Utbm9uY2Utbm9u",
        NOW + 60,
    )
}

#[test]
fn a_canonical_challenge_is_signed_and_the_body_is_what_bind_expects() {
    let id = key();
    let text = canonical(&id, NOW + 60);
    let (c, body) = sign_challenge(&id, ME, &text, NOW).unwrap();
    assert_eq!(c.sub, "alice@idp");
    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(sorted, ["challenge_id", "public_key", "signature"]);
    assert_eq!(body["challenge_id"], c.nonce);
    assert_eq!(body["public_key"], id.public_key_b64());
    let sig = base64::engine::general_purpose::STANDARD
        .decode(body["signature"].as_str().unwrap())
        .unwrap();
    assert!(
        crate::member::verify(&id.public_key(), text.as_bytes(), &sig),
        "signs exactly the challenge bytes"
    );
}

/// The daemon's whole `/identity/challenge` answer is accepted too, so the
/// person can paste what they were given.
#[test]
fn the_daemons_whole_answer_is_accepted() {
    let id = key();
    let text = canonical(&id, NOW + 60);
    let answer = serde_json::json!({"challenge_id": "x", "challenge": text, "expires_in": 300});
    let (_, body) = sign_challenge(&id, ME, &answer.to_string(), NOW).unwrap();
    let sig = base64::engine::general_purpose::STANDARD
        .decode(body["signature"].as_str().unwrap())
        .unwrap();
    assert!(crate::member::verify(
        &id.public_key(),
        text.as_bytes(),
        &sig
    ));
}

/// Anything else is refused. Each case is one step away from a valid
/// challenge, so the refusal is for that step and not for something else.
#[test]
fn it_never_signs_anything_but_the_canonical_challenge_format() {
    let id = key();
    let good = canonical(&id, NOW + 60);
    let reformatted =
        serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(&good).unwrap())
            .unwrap();
    let cases: Vec<(&str, String, Expect)> = vec![
        ("plain text", "sign me".into(), |r| {
            matches!(r, ChallengeRefusal::NotAChallenge(_))
        }),
        ("a content op", "{\"op\":\"insert\"}".into(), |r| {
            matches!(r, ChallengeRefusal::NotAChallenge(_))
        }),
        (
            "other domain",
            good.replace(DOMAIN_TAG, "mae-other-v1"),
            |r| *r == ChallengeRefusal::WrongDomain,
        ),
        ("extra field", good.replacen('{', "{\"x\":1,", 1), |r| {
            matches!(r, ChallengeRefusal::NotAChallenge(_))
        }),
        (
            "duplicate key",
            good.replacen('{', "{\"sub\":\"mallory\",", 1),
            |r| matches!(r, ChallengeRefusal::NotAChallenge(_)),
        ),
        ("not canonical", reformatted, |r| {
            matches!(r, ChallengeRefusal::NotAChallenge(_))
        }),
        ("trailing bytes", format!("{good}\u{0}payload"), |r| {
            matches!(r, ChallengeRefusal::NotAChallenge(_))
        }),
        // Canonical and for this key, but longer than any challenge issued.
        ("too long", long_subject(&id), |r| {
            *r == ChallengeRefusal::TooLong
        }),
    ];
    assert_all_refused(&id, cases);
}

/// Canonical challenges, each one step from the one this key holder may
/// sign: another key, person, provider or daemon, or the wrong time.
#[test]
fn it_never_signs_a_challenge_for_another_key_person_daemon_or_time() {
    let id = key();
    let other = key();
    let good = canonical(&id, NOW + 60);
    let cases: Vec<(&str, String, Expect)> = vec![
        ("another key", canonical(&other, NOW + 60), |r| {
            *r == ChallengeRefusal::OtherKey
        }),
        ("expired", canonical(&id, NOW), |r| {
            *r == ChallengeRefusal::Expired
        }),
        (
            "empty subject",
            good.replace("\"alice@idp\"", "\"\""),
            |r| *r == ChallengeRefusal::EmptyField("sub"),
        ),
        ("empty issuer", issued_to(&id, "", SUB, NOW + 60), |r| {
            *r == ChallengeRefusal::EmptyField("iss")
        }),
        // For this key, canonical and unexpired, but issued to someone else:
        // signing it would bind this key to their subject.
        (
            "another subject",
            issued_to(&id, ISS, "mallory@idp", NOW + 60),
            |r| *r == ChallengeRefusal::OtherSubject,
        ),
        (
            "the subject in another case",
            issued_to(&id, ISS, "Alice@idp", NOW + 60),
            |r| *r == ChallengeRefusal::OtherSubject,
        ),
        // The same subject at another issuer is another person.
        (
            "another issuer",
            issued_to(&id, "https://idp.example.org", SUB, NOW + 60),
            |r| *r == ChallengeRefusal::OtherIssuer,
        ),
        // For this key and this person, but issued by another daemon: one
        // the key holder also uses could relay it.
        (
            "another audience",
            issued_at(&id, ISS, SUB, "https://evil.example.com", NOW + 60),
            |r| *r == ChallengeRefusal::OtherAudience,
        ),
        (
            "the audience with a trailing slash",
            issued_at(&id, ISS, SUB, "https://mae.example.com/", NOW + 60),
            |r| *r == ChallengeRefusal::OtherAudience,
        ),
        // Valid for longer than any daemon issues a challenge.
        (
            "expiry too far",
            canonical(&id, NOW + MAX_CHALLENGE_LIFETIME_SECS + 1),
            |r| *r == ChallengeRefusal::ExpiryTooFar,
        ),
        ("expiry far in the future", canonical(&id, u64::MAX), |r| {
            *r == ChallengeRefusal::ExpiryTooFar
        }),
    ];
    assert_all_refused(&id, cases);
}

fn assert_all_refused(id: &MemberKey, cases: Vec<(&str, String, Expect)>) {
    for (name, input, expected) in cases {
        match sign_challenge(id, ME, &input, NOW) {
            Ok(_) => panic!("{name}: signed"),
            Err(r) => assert!(expected(&r), "{name}: refused for the wrong reason: {r:?}"),
        }
    }
}

/// The body is pasted into a browser: it carries the public key and a
/// signature, never the secret.
#[test]
fn the_output_holds_no_secret() {
    let id = key();
    let (_, body) = sign_challenge(&id, ME, &canonical(&id, NOW + 60), NOW).unwrap();
    let out = body.to_string();
    let secret = *id.seed();
    for form in [
        secret
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        base64::engine::general_purpose::STANDARD.encode(secret),
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(secret),
    ] {
        assert!(!out.contains(&form), "the secret key is in the output");
    }
}

/// `check_challenge` applies the same rules as signing, through the same
/// input forms, and its refusals carry distinct codes.
#[test]
fn check_applies_the_signing_rules_without_signing() {
    let id = key();
    let text = canonical(&id, NOW + 60);
    let answer = serde_json::json!({"challenge_id": "x", "challenge": text, "expires_in": 300});
    let c = check_challenge(&id.fingerprint(), ME, &answer.to_string(), NOW).unwrap();
    assert_eq!((c.iss.as_str(), c.sub.as_str()), (ISS, SUB));
    let mallory = Claimant {
        subject: "mallory@idp",
        ..ME
    };
    assert_eq!(
        check_challenge(&id.fingerprint(), mallory, &text, NOW),
        Err(ChallengeRefusal::OtherSubject)
    );
    assert_eq!(
        check_challenge(&key().fingerprint(), ME, &text, NOW),
        Err(ChallengeRefusal::OtherKey)
    );
    assert_eq!(
        check_challenge(&id.fingerprint(), ME, &text, NOW + 60),
        Err(ChallengeRefusal::Expired)
    );
    let all = [
        ChallengeRefusal::TooLong,
        ChallengeRefusal::NotAChallenge(String::new()),
        ChallengeRefusal::WrongDomain,
        ChallengeRefusal::EmptyField("sub"),
        ChallengeRefusal::OtherKey,
        ChallengeRefusal::OtherIssuer,
        ChallengeRefusal::OtherSubject,
        ChallengeRefusal::OtherAudience,
        ChallengeRefusal::Expired,
        ChallengeRefusal::ExpiryTooFar,
    ];
    let codes: std::collections::BTreeSet<&str> = all.iter().map(|r| r.code()).collect();
    assert_eq!(codes.len(), all.len(), "every refusal has its own code");
}

/// The lifetime cap is inclusive: a challenge expiring exactly
/// MAX_CHALLENGE_LIFETIME_SECS from now signs, one second more does not.
#[test]
fn the_lifetime_cap_is_exact() {
    let id = key();
    let at_cap = canonical(&id, NOW + MAX_CHALLENGE_LIFETIME_SECS);
    assert!(sign_challenge(&id, ME, &at_cap, NOW).is_ok());
    let over = canonical(&id, NOW + MAX_CHALLENGE_LIFETIME_SECS + 1);
    assert_eq!(
        sign_challenge(&id, ME, &over, NOW).unwrap_err(),
        ChallengeRefusal::ExpiryTooFar
    );
}

/// A refusal never quotes the input: the browser shows or logs it.
#[test]
fn refusals_never_echo_the_input() {
    let id = key();
    let marker = "QUOTEDMARKER7f3a";
    let good = canonical(&id, NOW + 60);
    for input in [
        format!("{{\"{marker}\":1}}"),
        good.replacen('{', &format!("{{\"{marker}\":1,"), 1),
        good.replace(
            "\"expires_at\":",
            &format!("\"expires_at\":\"{marker}\",\"x\":"),
        ),
        format!("not json {marker}"),
        format!("[\"{marker}\"]"),
    ] {
        let r = sign_challenge(&id, ME, &input, NOW).unwrap_err();
        assert!(!r.to_string().contains(marker), "{r}");
        assert!(!format!("{r:?}").contains(marker), "{r:?}");
    }
}
