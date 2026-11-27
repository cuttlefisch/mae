//! E8's check alone, on built principals: its order, its boundaries, and that
//! acr never counts.

use super::*;
use crate::oauth_self_issue::Credential;
use crate::sign_in::SignIn;

const NOW: u64 = 1_800_000_000;

fn op(amr: &[&str], acr: &[&str]) -> StepUpRule {
    StepUpRule {
        rule: SecondFactorRule {
            amr: amr.iter().map(|s| s.to_string()).collect(),
            acr: acr.iter().map(|s| s.to_string()).collect(),
        },
        max_age_secs: 300,
    }
}

fn person(amr: &[&str], acr: Option<&str>, auth_time: Option<u64>) -> ValidatedPrincipal {
    ValidatedPrincipal {
        principal: "fp-person".into(),
        audience: vec![],
        expires_at: NOW + 300,
        issuer: "https://idp.example".into(),
        subject: "sub-1".into(),
        credential: Credential::IdpBound,
        kind: PrincipalKind::Person,
        sign_in: Some(SignIn {
            amr: amr.iter().map(|s| s.to_string()).collect(),
            acr: acr.map(String::from),
            auth_time,
            ..Default::default()
        }),
    }
}

fn kind(r: Result<Satisfied, McpError>) -> String {
    match r {
        Ok(_) => "ok".into(),
        Err(e) => e
            .data
            .as_ref()
            .and_then(|d| d["kind"].as_str().map(String::from))
            .unwrap_or_default(),
    }
}

/// The window's two ends are exact (E8: `now - W` passes, `now - W - 1`
/// fails), and a future `auth_time` is tolerated by the 60 s leeway only.
#[test]
fn the_window_is_exact_at_both_ends() {
    let op = op(&["mfa"], &[]);
    let at = |t| {
        kind(check(
            &person(&["pwd", "mfa"], None, Some(t)),
            &op,
            300,
            NOW,
        ))
    };
    assert_eq!(at(NOW - 300), "ok");
    assert_eq!(at(NOW - 301), "reauthentication_required");
    assert_eq!(at(NOW + 60), "ok");
    assert_eq!(at(NOW + 61), "reauthentication_required");
    assert_eq!(
        kind(check(&person(&["mfa"], None, None), &op, 300, NOW)),
        "reauthentication_required"
    );
}

/// Each failing claim has its own kind, and the order is E8's: a missing
/// factor is reported before a stale time.
#[test]
fn each_failure_has_its_kind_in_order() {
    let mfa = op(&["mfa"], &[]);
    let stale = Some(NOW - 10_000);
    let fresh = Some(NOW - 5);
    assert_eq!(
        kind(check(&person(&[], None, fresh), &mfa, 300, NOW)),
        "mfa_required"
    );
    assert_eq!(
        kind(check(&person(&["pwd"], None, fresh), &mfa, 300, NOW)),
        "mfa_required"
    );
    assert_eq!(
        kind(check(&person(&["pwd"], None, stale), &mfa, 300, NOW)),
        "mfa_required",
        "factor first"
    );
    assert_eq!(
        kind(check(
            &person(&["pwd", "mfa"], None, fresh),
            &op(&[], &[]),
            300,
            NOW
        )),
        "second_factor_unconfigured"
    );
    assert_eq!(
        kind(check(
            &person(&["mfa"], None, fresh),
            &op(&[""], &[]),
            300,
            NOW
        )),
        "second_factor_unconfigured",
        "an empty value accepts nothing"
    );
}

/// acr never satisfies E8, even when the operator lists it (B7's acr
/// alternative is superseded).
#[test]
fn acr_never_counts() {
    let op = op(&["mfa"], &["strong"]);
    let r = check(
        &person(&["pwd"], Some("strong"), Some(NOW - 5)),
        &op,
        300,
        NOW,
    );
    assert_eq!(kind(r), "mfa_required");
}

/// An agent, a self-issued token (no sign-in), or a person kind without a
/// sign-in is refused before any second-factor question.
#[test]
fn only_a_person_with_a_sign_in_passes_step_one() {
    let op = op(&[], &[]);
    let mut agent = person(&["mfa"], None, Some(NOW));
    agent.kind = PrincipalKind::Agent;
    assert_eq!(kind(check(&agent, &op, 300, NOW)), "agent_forbidden");
    let mut no_sign_in = person(&["mfa"], None, Some(NOW));
    no_sign_in.sign_in = None;
    assert_eq!(kind(check(&no_sign_in, &op, 300, NOW)), "agent_forbidden");
}

/// What passes records the amr, the auth_time and the value that matched.
#[test]
fn a_pass_records_what_satisfied_it() {
    let s = check(
        &person(&["pwd", "mfa"], None, Some(NOW - 7)),
        &op(&["otp", "mfa"], &[]),
        300,
        NOW,
    )
    .unwrap();
    assert_eq!(s.auth_time, NOW - 7);
    assert_eq!(s.second_factor.value, "mfa");
    assert_eq!(s.amr, vec!["pwd".to_string(), "mfa".to_string()]);
}

/// A KB's window never exceeds the operator's, and defaults to it.
#[test]
fn a_kbs_window_never_exceeds_the_operators() {
    let op = op(&["mfa"], &[]);
    let with = |w| Policy {
        reauth_max_age_secs: w,
        ..Policy::default()
    };
    assert_eq!(window(&op, &with(None)), 300);
    assert_eq!(window(&op, &with(Some(120))), 120);
    assert_eq!(window(&op, &with(Some(900))), 300);
}
