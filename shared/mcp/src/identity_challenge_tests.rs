//! The signer must never be a signing oracle for the member key: everything
//! that is not exactly a daemon-issued challenge for THIS key, unexpired, is
//! refused and nothing is signed. Keys are freshly generated per test.

use super::*;

const NOW: u64 = 1_900_000_000;
const ISS: &str = "https://idp.example.com";
const SUB: &str = "alice@idp";

/// The key holder, stating the identity the challenges below are issued to.
const ME: Claimant<'static> = Claimant {
    issuer: ISS,
    subject: SUB,
};

/// Whether a refusal is the one a case expects.
type Expect = fn(&ChallengeRefusal) -> bool;

fn canonical(id: &Identity, exp: u64) -> String {
    issued_to(id, ISS, SUB, exp)
}

fn issued_to(id: &Identity, iss: &str, sub: &str, exp: u64) -> String {
    challenge_text(
        iss,
        sub,
        "https://mae.example.com",
        &id.fingerprint(),
        "bm9uY2Utbm9uY2Utbm9uY2Utbm9uY2Utbm9uY2Utbm9u",
        exp,
    )
}

fn long_subject(id: &Identity) -> String {
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
    let id = Identity::generate("member");
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
    assert_eq!(body["public_key"], id.public().encoded());
    use base64::Engine;
    let sig = base64::engine::general_purpose::STANDARD
        .decode(body["signature"].as_str().unwrap())
        .unwrap();
    assert!(
        id.public().verify(text.as_bytes(), &sig),
        "signs exactly the challenge bytes"
    );
}

/// The daemon's whole `/identity/challenge` answer is accepted too, so the
/// person can paste what they were given.
#[test]
fn the_daemons_whole_answer_is_accepted() {
    let id = Identity::generate("member");
    let text = canonical(&id, NOW + 60);
    let answer = serde_json::json!({"challenge_id": "x", "challenge": text, "expires_in": 300});
    let (_, body) = sign_challenge(&id, ME, &answer.to_string(), NOW).unwrap();
    use base64::Engine;
    let sig = base64::engine::general_purpose::STANDARD
        .decode(body["signature"].as_str().unwrap())
        .unwrap();
    assert!(id.public().verify(text.as_bytes(), &sig));
}

/// Anything else is refused. Each case is one step away from a valid
/// challenge, so the refusal is for that step and not for something else.
#[test]
fn it_never_signs_anything_but_a_canonical_unexpired_challenge_for_this_key() {
    let id = Identity::generate("member");
    let other = Identity::generate("other");
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
        // Canonical and for this key, but longer than any challenge issued.
        ("too long", long_subject(&id), |r| {
            *r == ChallengeRefusal::TooLong
        }),
    ];
    for (name, input, expected) in cases {
        match sign_challenge(&id, ME, &input, NOW) {
            Ok(_) => panic!("{name}: signed"),
            Err(r) => assert!(expected(&r), "{name}: refused for the wrong reason: {r:?}"),
        }
    }
}

/// The body is pasted into a browser: it carries the public key and a
/// signature, never the secret.
#[test]
fn the_output_holds_no_secret() {
    let id = Identity::generate("member");
    let (_, body) = sign_challenge(&id, ME, &canonical(&id, NOW + 60), NOW).unwrap();
    let out = body.to_string();
    let secret = id.secret_bytes();
    use base64::Engine;
    for form in [
        hex::encode(secret),
        base64::engine::general_purpose::STANDARD.encode(secret),
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(secret),
    ] {
        assert!(!out.contains(&form), "the secret key is in the output");
    }
}

/// `check_challenge` through this wrapper checks against THIS identity's
/// key and the stated claimant, as signing does.
#[test]
fn check_uses_this_identity_and_the_stated_claimant() {
    let id = Identity::generate("member");
    let other = Identity::generate("other");
    let text = canonical(&id, NOW + 60);
    assert_eq!(check_challenge(&id, ME, &text, NOW).unwrap().sub, SUB);
    assert_eq!(
        check_challenge(&other, ME, &text, NOW),
        Err(ChallengeRefusal::OtherKey)
    );
    let mallory = Claimant {
        issuer: ISS,
        subject: "mallory@idp",
    };
    assert_eq!(
        check_challenge(&id, mallory, &text, NOW),
        Err(ChallengeRefusal::OtherSubject)
    );
    assert_eq!(
        parse_for(&other, ME, &text, NOW),
        Err(ChallengeRefusal::OtherKey)
    );
}
