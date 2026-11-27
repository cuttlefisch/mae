//! ADR-113 PR-8: a recent second factor at the moment of a controlled act
//! (B7, C4, the check pinned by E8).
//!
//! On a KB whose policy has `approval_requires_mfa`, every controlled act
//! (`approve`, `enter_into_force`, attaching `signed_pdf`) and every owner's
//! administrative act (`set_policy`, `set_code`) runs [`check`], whose four
//! steps run in E8's order, so a client sees the missing factor before the
//! stale time:
//! 1. the caller is a person with an IdP sign-in (`agent_forbidden`);
//! 2. the operator accepts at least one `amr` value (`second_factor_unconfigured`);
//! 3. the token's `amr` holds one of them (`mfa_required`, with `accepted_amr`);
//! 4. its `auth_time` is within the KB's window (`reauthentication_required`,
//!    with `max_age` and the `auth_time` read).
//!
//! @ai-caution: [security] The claims are read only from the bearer token the
//! listener verified (`ValidatedPrincipal::sign_in`), never from a parameter
//! or a header a relaying client adds. `acr` is never consulted here (E8
//! supersedes B7's "or an acr level"): some identity providers emit a constant
//! `acr` that says nothing about the factors. A refreshed token keeps its
//! original `auth_time`, so it falls out of the window by design.

use mae_mcp::protocol::McpError;
use serde_json::{json, Value};

use super::store::Policy;
use crate::doc_store::DocStore;
use crate::oauth_self_issue::{PrincipalKind, ValidatedPrincipal};
use crate::sign_in::{SecondFactor, SecondFactorRule, StepUpRule};

/// The sign-in that satisfied the check: what the act records (E8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Satisfied {
    pub amr: Vec<String>,
    pub auth_time: u64,
    pub second_factor: SecondFactor,
}

/// The window a KB's acts are held to: its `reauth_max_age_secs`, never
/// longer than the operator's.
pub(crate) fn window(op: &StepUpRule, policy: &Policy) -> u64 {
    policy
        .reauth_max_age_secs
        .map_or(op.max_age_secs, |w| w.min(op.max_age_secs))
}

/// E8's four checks, in order, for `who` acting at `now_secs` under the
/// operator's `op` with a window of `window` seconds.
pub(crate) fn check(
    who: &ValidatedPrincipal,
    op: &StepUpRule,
    window: u64,
    now_secs: u64,
) -> Result<Satisfied, McpError> {
    let sign_in = match (&who.kind, &who.sign_in) {
        (PrincipalKind::Person, Some(s)) => s,
        _ => {
            return Err(McpError::agent_forbidden(
                "this act needs a person's identity-provider sign-in; an agent or a \
                 self-issued token never has one"
                    .into(),
            ))
        }
    };
    let accepted: Vec<String> = op
        .rule
        .amr
        .iter()
        .filter(|v| !v.is_empty())
        .cloned()
        .collect();
    if accepted.is_empty() {
        return Err(McpError::second_factor_unconfigured(
            "this KB requires a second factor, and this daemon accepts none \
             ([oauth] second_factor_amr is empty)"
                .into(),
        ));
    }
    // amr only: acr is not consulted (E8).
    let amr_only = SecondFactorRule {
        amr: accepted.clone(),
        acr: Vec::new(),
    };
    let Some(second_factor) = sign_in.second_factor(&amr_only) else {
        return Err(McpError::mfa_required(
            "sign in with a second factor to perform this act".into(),
            accepted,
        ));
    };
    if !sign_in.fresh_within(now_secs, window) {
        return Err(McpError::reauthentication_required(
            format!(
                "sign in again (with your second factor) and retry: this act needs a sign-in \
                 from the last {window} seconds"
            ),
            window,
            sign_in.auth_time,
        ));
    }
    Ok(Satisfied {
        amr: sign_in.amr.clone(),
        auth_time: sign_in.auth_time.unwrap_or_default(),
        second_factor,
    })
}

/// For an act on a KB with `policy`: E8's check when the policy requires it,
/// else nothing (the plain act, as before PR-8).
pub(crate) fn required(
    store: &DocStore,
    policy: &Policy,
    who: &ValidatedPrincipal,
) -> Result<Option<Satisfied>, McpError> {
    if !policy.approval_requires_mfa {
        return Ok(None);
    }
    let op = store.step_up_rule();
    check(who, &op, window(&op, policy), store.now_ms() / 1000).map(Some)
}

/// The event's actor (B4), with the sign-in that satisfied the check (E8).
pub(crate) fn actor(who: &ValidatedPrincipal, satisfied: &Option<Satisfied>) -> Value {
    let mut a = super::actor(who);
    if let Some(s) = satisfied {
        a["sign_in"] = json!({
            "amr": s.amr,
            "auth_time": s.auth_time,
            "second_factor": { "claim": s.second_factor.claim, "value": s.second_factor.value },
        });
    }
    a
}

#[cfg(test)]
#[path = "strength_tests.rs"]
mod tests;
