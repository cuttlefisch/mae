//! How a person signed in at the identity provider (ADR-098), read from an
//! external token's verified claims: `amr`, `acr`, `auth_time` and the groups.
//!
//! ADR-113 (Amendment 2 B7, Amendment 3 C4) requires a second factor, and a
//! recent sign-in, before a controlled act. This module owns reading the claims
//! and answering those two questions; ADR-113 owns requiring the answers.
//!
//! Every reading fails closed. A claim of the wrong JSON type is absent, never
//! a pass. Which `amr`/`acr` values count as a second factor is the operator's
//! configuration only: there is no built-in list, because the values differ by
//! identity provider and are measured, not assumed. With nothing configured,
//! no token has a second factor.

use serde::Serialize;

/// The clock leeway, seconds, `oauth::validate_bearer_token` grants `exp` (and
/// `nbf`). Set explicitly there from this constant, so the freshness check
/// below tolerates exactly the clock skew the validator does, and no more.
pub const JWT_CLOCK_LEEWAY_SECS: u64 = 60;

/// ADR-113 E8: the window, seconds, between a person's sign-in and a
/// controlled act, when the operator sets none. It only has to cover the
/// step-up immediately before the act, a slow second-factor push and one retry.
pub const DEFAULT_REAUTH_MAX_AGE_SECS: u64 = 300;

/// ADR-113 E8: no window may be shorter. Below the clock leeway, the two
/// hosts' clocks, not the person, would decide whether an act passes.
pub const MIN_REAUTH_MAX_AGE_SECS: u64 = JWT_CLOCK_LEEWAY_SECS;

/// What the operator configured for acts that need a recent second factor
/// (ADR-113 E8): the accepted values, and the longest window a KB may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepUpRule {
    pub rule: SecondFactorRule,
    pub max_age_secs: u64,
}

impl Default for StepUpRule {
    fn default() -> Self {
        StepUpRule {
            rule: SecondFactorRule::default(),
            max_age_secs: DEFAULT_REAUTH_MAX_AGE_SECS,
        }
    }
}

/// The claim names [`SecondFactor::claim`] reports.
pub const AMR: &str = "amr";
pub const ACR: &str = "acr";

/// How the person behind an external token signed in. Self-issued tokens have
/// none: they are not a sign-in at an identity provider.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SignIn {
    /// RFC 8176 authentication method references. Empty when the claim is
    /// absent or is not an array of strings.
    pub amr: Vec<String>,
    /// The authentication context class. `None` when absent, empty, or not a
    /// string.
    pub acr: Option<String>,
    /// When the person signed in, unix seconds. `None` when absent or not a
    /// non-negative integer.
    pub auth_time: Option<u64>,
    /// The groups in the configured `groups_claim`, read exactly as the D4
    /// session gate reads them ([`groups_from_claims`]).
    pub groups: Vec<String>,
    /// The IdP's session id (`sid`), when the token carries one: recorded
    /// beside the daemon's own session id as a second join key (ADR-120 D4).
    pub sid: Option<String>,
}

/// The operator's second-factor rule: the `amr` values, and the `acr` values,
/// that count as a second factor. Both empty (the default) means none does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecondFactorRule {
    pub amr: Vec<String>,
    pub acr: Vec<String>,
}

/// Which claim, and which value of it, satisfied the rule — what ADR-113 B7
/// records beside an approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecondFactor {
    /// [`AMR`] or [`ACR`].
    pub claim: &'static str,
    pub value: String,
}

/// The groups an external token lists in `groups_claim`: a string array (its
/// string elements) or one string. Anything else lists none. Shared by the D4
/// gate and [`SignIn`], so the two never read a token differently.
pub fn groups_from_claims(claims: &serde_json::Value, groups_claim: &str) -> Vec<String> {
    match claims.get(groups_claim) {
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// `amr` only as RFC 8176 defines it, an array of strings. A single string, or
/// an array holding anything but strings, is malformed and reads as absent:
/// keeping the string elements of a mixed array would let a malformed claim
/// pass.
fn amr_from_claims(claims: &serde_json::Value) -> Vec<String> {
    let Some(serde_json::Value::Array(items)) = claims.get(AMR) else {
        return Vec::new();
    };
    let strings: Option<Vec<String>> = items.iter().map(|v| v.as_str().map(String::from)).collect();
    strings.unwrap_or_default()
}

impl SignIn {
    /// Read from an external token's already-verified claims.
    pub fn from_claims(claims: &serde_json::Value, groups_claim: &str) -> SignIn {
        SignIn {
            amr: amr_from_claims(claims),
            acr: claims
                .get(ACR)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from),
            auth_time: claims.get("auth_time").and_then(|v| v.as_u64()),
            groups: groups_from_claims(claims, groups_claim),
            sid: claims
                .get("sid")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from),
        }
    }

    /// The configured value this sign-in satisfies, `amr` first. `None` when
    /// none does, and always `None` with nothing configured (fail closed). An
    /// empty configured value never matches.
    pub fn second_factor(&self, rule: &SecondFactorRule) -> Option<SecondFactor> {
        if let Some(v) = rule
            .amr
            .iter()
            .find(|v| !v.is_empty() && self.amr.contains(v))
        {
            return Some(SecondFactor {
                claim: AMR,
                value: v.clone(),
            });
        }
        let acr = self.acr.as_ref()?;
        rule.acr
            .iter()
            .find(|v| !v.is_empty() && *v == acr)
            .map(|v| SecondFactor {
                claim: ACR,
                value: v.clone(),
            })
    }

    /// Whether the person signed in at most `max_age_secs` before `now` (unix
    /// seconds). No `auth_time`: false. An `auth_time` ahead of `now` is
    /// tolerated by the validator's own clock leeway and no further.
    pub fn fresh_within(&self, now: u64, max_age_secs: u64) -> bool {
        let Some(at) = self.auth_time else {
            return false;
        };
        if at > now {
            return at - now <= JWT_CLOCK_LEEWAY_SECS;
        }
        now - at <= max_age_secs
    }
}

#[cfg(test)]
#[path = "sign_in_tests.rs"]
mod sign_in_tests;
