//! Which bearer tokens may stand behind a sync socket (ADR-099 B6, and
//! Amendment 2 E1/E2).
//!
//! One decision for the three places a token is presented: the bearer
//! upgrade, `POST /ws/ticket`, and `session/renew`. Each renders the refusal
//! its own way (an HTTP response before the upgrade, a JSON-RPC error or a
//! close on the socket), but none decides differently.
//!
//! The resolution itself is the listener's (`oauth::resolve_token`): the same
//! validators, the same group gate (ADR-098 D4) before the binding is read, and
//! the same binding lookup (ADR-098 Amendment 1, C1) as every other route.
//!
//! @ai-caution: [security] An unbound identity-provider token must never reach
//! a session: its principal is a typed unbound subject, a member of nothing,
//! and the socket has no use for one. Refuse it here, never downstream.

use mae_daemon::oauth_self_issue::{
    unbound_principal, Credential, PrincipalKind, ValidatedPrincipal, SELF_ISSUED_KID,
};
use mae_mcp::protocol::McpError;

use super::now_unix;
use crate::oauth::{self, AuthFailure, Listener};

/// The refusal message for a token from another issuer while B6 holds.
const ISSUER_NOT_SUPPORTED: &str =
    "only this daemon's own tokens are accepted on this route (ADR-099 B6)";

/// The refusal message for an identity-provider subject with no binding.
const IDENTITY_NOT_BOUND: &str =
    "this identity-provider subject is not bound to a member key (ADR-099 Amendment 2, E1)";

/// Why a token cannot stand behind a socket.
#[derive(Debug)]
pub(super) enum Refused {
    /// Not this daemon's own token, and `ws_accept_bound_idp` is off (B6).
    IssuerNotSupported,
    /// An identity-provider token whose `(iss, sub)` is bound to no member key.
    NotBound,
    /// The token did not authenticate at all.
    Auth(AuthFailure),
    /// It validated within the JWT library's clock leeway, but has expired.
    Expired,
}

impl Refused {
    /// The JSON-RPC error a `session/renew` answers with.
    pub(super) fn into_rpc_error(self) -> McpError {
        match self {
            Refused::IssuerNotSupported => {
                McpError::issuer_not_supported(ISSUER_NOT_SUPPORTED.into())
            }
            Refused::NotBound => McpError::identity_not_bound(IDENTITY_NOT_BOUND.into()),
            Refused::Auth(AuthFailure::Invalid(reason)) => McpError::invalid_token(reason),
            Refused::Auth(AuthFailure::NotInGroup) => McpError::group_required(
                "this token carries none of the groups oauth.allowed_groups names".into(),
            ),
            Refused::Auth(AuthFailure::Unavailable) => McpError::internal_fault(
                "the token issuer's keys or the binding table could not be read".into(),
            ),
            Refused::Expired => McpError::invalid_token("Expired".into()),
        }
    }
}

/// Validate `token` for the sync socket and resolve its principal.
///
/// This daemon's own token always qualifies. An identity-provider token
/// qualifies only with `ws_accept_bound_idp` on AND a binding for its
/// `(iss, sub)`, and its principal is then the bound fingerprint (E1). The
/// switch is read before anything else, so with it off such a token is refused
/// exactly as before the amendment: no JWKS fetch, no group gate, no lookup.
pub(super) async fn socket_principal(
    token: &str,
    listener: &Listener,
) -> Result<ValidatedPrincipal, Refused> {
    let kid = jsonwebtoken::decode_header(token).ok().and_then(|h| h.kid);
    let self_issued = kid.as_deref() == Some(SELF_ISSUED_KID);
    if !self_issued && !accepts_bound_idp(listener) {
        return Err(Refused::IssuerNotSupported);
    }
    let vp = oauth::resolve_token(token, listener)
        .await
        .map_err(Refused::Auth)?;
    if vp.credential == Credential::IdpUnbound {
        return Err(Refused::NotBound);
    }
    // The validator allows the JWT library's clock leeway; a session must not
    // open, or renew, with its deadline already behind it.
    if vp.expires_at <= now_unix() {
        return Err(Refused::Expired);
    }
    Ok(vp)
}

/// The deadline a session or renewal on `vp` gets (Amendment 2, E2): the
/// token's expiry, and for a bound identity-provider token no later than
/// `ws_max_idp_session_secs` from now. A binding is re-read only at a renewal,
/// and an IdP's own token lifetime is not this daemon's to choose, so the cap
/// is what bounds how long an unbind or a rebind takes to reach an open
/// socket. Self-issued sessions keep their token's expiry.
pub(super) fn session_deadline(vp: &ValidatedPrincipal, listener: &Listener) -> u64 {
    capped_deadline(
        vp.expires_at,
        vp.credential == Credential::IdpBound,
        listener,
    )
}

/// [`session_deadline`] for a credential expiring at `expires_at` (unix
/// seconds). The one place the E2 cap is applied: to a bearer token, and to a
/// spent ticket, which carries its minting token's `exp` uncapped.
pub(super) fn capped_deadline(expires_at: u64, bound_idp: bool, listener: &Listener) -> u64 {
    let cap = listener
        .ws
        .as_ref()
        .map_or(0, |ws| ws.settings.max_idp_session_secs);
    if !bound_idp || cap == 0 {
        return expires_at;
    }
    expires_at.min(now_unix().saturating_add(cap))
}

/// At a ticket's redemption (Amendment 2, E1): does `(issuer, subject)` still
/// resolve to `principal`? The binding table is read off the runtime, as on
/// every route; a table that cannot be read fails closed.
pub(super) async fn still_bound(
    listener: &Listener,
    issuer: &str,
    subject: &str,
    principal: &str,
) -> Result<bool, Refused> {
    let probe = ValidatedPrincipal {
        principal: unbound_principal(issuer, subject),
        audience: Vec::new(),
        expires_at: 0,
        issuer: issuer.to_string(),
        subject: subject.to_string(),
        credential: Credential::IdpUnbound,
        kind: PrincipalKind::Agent,
        sign_in: None,
    };
    let bindings = listener.bindings.clone();
    let now = mae_daemon::identity_binding::resolve_off_runtime(probe, bindings)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "binding table unreadable at ticket redemption");
            Refused::Auth(AuthFailure::Unavailable)
        })?;
    Ok(now.credential == Credential::IdpBound && now.principal == principal)
}

/// `oauth.ws_accept_bound_idp`, as resolved into the socket's settings.
fn accepts_bound_idp(listener: &Listener) -> bool {
    listener
        .ws
        .as_ref()
        .is_some_and(|ws| ws.settings.accept_bound_idp)
}
