//! The pure halves of [`super`]: claim reading, the second-factor rule and
//! freshness. The same questions asked of real RS256 tokens, through the
//! listener's validator, are in `oauth_tests.rs` (`sign_in_claims`).

use super::*;
use serde_json::json;

fn rule(amr: &[&str], acr: &[&str]) -> SecondFactorRule {
    SecondFactorRule {
        amr: amr.iter().map(|s| s.to_string()).collect(),
        acr: acr.iter().map(|s| s.to_string()).collect(),
    }
}

fn signed_in(amr: &[&str], acr: Option<&str>) -> SignIn {
    SignIn {
        amr: amr.iter().map(|s| s.to_string()).collect(),
        acr: acr.map(String::from),
        auth_time: None,
        groups: Vec::new(),
    }
}

fn at(auth_time: Option<u64>) -> SignIn {
    SignIn {
        auth_time,
        ..SignIn::default()
    }
}

#[test]
fn malformed_claim_types_read_as_absent() {
    let cases = [
        json!({"amr": "mfa", "acr": 2, "auth_time": "1700000000"}),
        json!({"amr": ["mfa", 1], "acr": ["x"], "auth_time": -5}),
        json!({"amr": {"0": "mfa"}, "acr": null, "auth_time": 1.7e9}),
        json!({"amr": [null], "acr": "", "auth_time": true}),
    ];
    for claims in cases {
        let s = SignIn::from_claims(&claims, "groups");
        assert_eq!(s.amr, Vec::<String>::new(), "{claims}");
        assert_eq!(s.acr, None, "{claims}");
        assert_eq!(s.auth_time, None, "{claims}");
        // And therefore no rule that would accept the well-formed value does.
        assert_eq!(
            s.second_factor(&rule(&["mfa"], &["x", "2"])),
            None,
            "{claims}"
        );
    }
}

#[test]
fn groups_read_as_the_session_gate_reads_them() {
    let s = SignIn::from_claims(&json!({"g": ["a", 1, "b"]}), "g");
    assert_eq!(s.groups, vec!["a", "b"]);
    let s = SignIn::from_claims(&json!({"g": "solo"}), "g");
    assert_eq!(s.groups, vec!["solo"]);
    let s = SignIn::from_claims(&json!({"g": {"a": 1}, "groups": ["x"]}), "g");
    assert!(s.groups.is_empty(), "only the configured claim is read");
}

#[test]
fn nothing_configured_means_no_second_factor_for_any_sign_in() {
    let empty = SecondFactorRule::default();
    for s in [
        signed_in(&["pwd", "mfa"], Some("constant-acr")),
        signed_in(&["hwk"], None),
        signed_in(&["pwd"], Some("constant-acr")),
        signed_in(&[], None),
    ] {
        assert_eq!(s.second_factor(&empty), None, "{s:?}");
    }
}

#[test]
fn an_empty_configured_value_never_matches() {
    let s = signed_in(&[""], Some(""));
    assert_eq!(s.second_factor(&rule(&[""], &[""])), None);
    // An `acr` the reader kept (non-empty) against an empty configured one.
    let s = signed_in(&[], Some("x"));
    assert_eq!(s.second_factor(&rule(&[], &[""])), None);
}

#[test]
fn amr_is_reported_before_acr_and_names_the_value_that_matched() {
    let s = signed_in(&["pwd", "hwk"], Some("strong"));
    let got = s.second_factor(&rule(&["mfa", "hwk"], &["strong"]));
    assert_eq!(
        got,
        Some(SecondFactor {
            claim: AMR,
            value: "hwk".into()
        })
    );
    let got = signed_in(&["pwd"], Some("strong")).second_factor(&rule(&["mfa"], &["strong"]));
    assert_eq!(
        got,
        Some(SecondFactor {
            claim: ACR,
            value: "strong".into()
        })
    );
    assert_eq!(
        signed_in(&["pwd"], Some("weak")).second_factor(&rule(&["mfa"], &["strong"])),
        None
    );
}

#[test]
fn freshness_holds_at_the_window_and_fails_one_second_past_it() {
    let now = 1_800_000_000;
    assert!(at(Some(now)).fresh_within(now, 300));
    assert!(at(Some(now - 300)).fresh_within(now, 300));
    assert!(!at(Some(now - 301)).fresh_within(now, 300));
    assert!(!at(Some(0)).fresh_within(now, 300));
    assert!(
        !at(None).fresh_within(now, u64::MAX),
        "no auth_time is never fresh"
    );
}

#[test]
fn a_future_auth_time_is_tolerated_by_the_leeway_and_no_more() {
    let now = 1_800_000_000;
    assert!(at(Some(now + JWT_CLOCK_LEEWAY_SECS)).fresh_within(now, 300));
    assert!(!at(Some(now + JWT_CLOCK_LEEWAY_SECS + 1)).fresh_within(now, 300));
    assert!(
        !at(Some(u64::MAX)).fresh_within(now, u64::MAX),
        "a huge window does not admit a sign-in from the future"
    );
}

/// A self-issued token is no sign-in at an identity provider: it carries no
/// `SignIn` even when its claims spell a second factor.
#[test]
fn a_self_issued_token_carries_no_sign_in() {
    use crate::oauth_self_issue::{validate_self_issued_token, SELF_ISSUED_KID};
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    let identity = mae_mcp::identity::Identity::generate("daemon-under-test");
    let mut header = Header::new(Algorithm::EdDSA);
    header.kid = Some(SELF_ISSUED_KID.to_string());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let aud = "https://mae.example.com/mcp";
    let claims = json!({"sub": "SHA256:fp", "aud": aud, "iss": "self", "exp": now + 600,
                        "amr": ["pwd", "mfa"], "acr": "strong", "auth_time": now});
    let key = EncodingKey::from_ed_der(&identity.pkcs8_der().unwrap());
    let token = encode(&header, &claims, &key).unwrap();
    let vp = validate_self_issued_token(&token, &identity.public().to_bytes(), aud).unwrap();
    assert_eq!(vp.sign_in, None);
}
