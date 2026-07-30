//! ADR-067 Phase D3: OAuth self-issued tokens bound to this daemon's own
//! Ed25519 identity.
//!
//! Lets a `QueryOnly`-restricted member (ADR-067 Phase B) obtain a bearer
//! token for the OAuth `kb/query.*` surface (`daemon/src/oauth.rs`, ADR-052/
//! 053) without any external authorization server, by asking THIS daemon —
//! which has already mTLS-authenticated the caller over the collab listener
//! (`crate::collab_handler`) — to mint one for their own already-verified
//! fingerprint. This is what makes a self-pointing `RemoteHubQueryLayer`
//! (`shared/kb/src/remote_hub.rs`, ADR-062) actually work end to end: the
//! member points a `RemoteHub` instance at THIS daemon's own OAuth listener
//! and authenticates with a token this module minted for them.
//!
//! Deliberately NOT routed through `oauth::validate_bearer_token`'s
//! `Jwk`/`JwksCache` machinery: that machinery models an EXTERNALLY-issued,
//! JWKS-discovered, *rotating* key population. A self-issued token has
//! exactly ONE valid signer — this daemon's own identity key — so
//! verification is a direct comparison ("does this signature match my own
//! known pubkey"), not a JWKS lookup problem. Routing it through the
//! general-purpose external-AS validator would force every existing
//! RS256+JWKS adversarial test to reason about a second algorithm/population
//! it never actually exercises, for no benefit.
//!
//! `kid == "self"` is the short-circuit trigger `oauth::handle_request` peeks
//! for in the JWT header BEFORE ever calling `JwksCache::get()` — chosen
//! specifically so it can never collide with a real external JWKS key id.
//!
//! Lives in the `mae-daemon` LIBRARY crate (mirroring `kb_query`'s ADR-067
//! Phase D1 move): `crate::collab_handler`'s new `kb/query.self_token` mTLS
//! RPC (itself lib-crate-only) mints tokens by calling this module directly;
//! `daemon/src/oauth.rs` (still bin-crate-private) validates them by
//! reaching in as `mae_daemon::oauth_self_issue::validate_self_issued_token`,
//! the same pattern it already uses for `mae_daemon::kb_query`/
//! `mae_daemon::collab_handler`. `TokenValidationError`/`ValidatedPrincipal`
//! moved here from `oauth.rs` (verbatim, zero behavior change) for the same
//! crate-boundary reason — a bin-crate type can't be named from a lib-crate
//! function's return type.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use mae_mcp::identity::Identity;

/// Why a bearer token was rejected. Deliberately specific (not a single
/// opaque "invalid") so callers can log/test the exact failure mode.
/// Shared by both `oauth::validate_bearer_token` (external AS, RS256+JWKS)
/// and `validate_self_issued_token` below (this daemon's own EdDSA key) —
/// one error taxonomy for both token populations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenValidationError {
    /// No `Authorization: Bearer <token>` header, or the token isn't
    /// well-formed JWT (bad base64, missing `kid`, etc.).
    Malformed,
    /// The token's `kid` doesn't match any key in the cached JWKS — either
    /// a genuinely unknown key, or one that's been rotated out (this is
    /// this module's stateless equivalent of "revoked": a resource server
    /// validating JWTs via JWKS has no live revocation list, so key
    /// rotation removing the old key from the JWKS is how the AS revokes).
    UnknownKey,
    /// Signature verification failed — a tampered or forged token.
    InvalidSignature,
    /// The token's `exp` claim is in the past.
    Expired,
    /// The token's `aud` claim does not include this server's
    /// `canonical_resource_uri` — RFC 8707's confused-deputy defense. This
    /// is also what catches a validly-signed token issued for a
    /// *different* resource server (a different MCP server, or a
    /// different MAE deployment) being replayed here.
    WrongAudience,
    /// The token's `iss` claim doesn't match the configured issuer.
    WrongIssuer,
    /// The mapped principal claim (`principal_claim`) was absent from the
    /// token, or not a string.
    MissingPrincipalClaim,
}

/// A validated bearer token's outcome: the mapped principal plus enough of
/// the raw claims for logging/attribution. This principal is what feeds
/// `kb_access` (ADR-018) — an OAuth identity SOURCE, never a parallel
/// authorization system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPrincipal {
    pub principal: String,
    pub audience: Vec<String>,
    pub expires_at: u64,
}

/// The fixed `kid` marking a self-issued token — never a real external JWKS
/// key id, so `oauth::handle_request`'s header peek can short-circuit
/// unambiguously, before ever fetching the configured external JWKS.
pub const SELF_ISSUED_KID: &str = "self";

/// The fixed `iss` claim of a self-issued token — this daemon asserts its
/// own identity, never an external authorization server's issuer string.
const SELF_ISSUED_ISS: &str = "self";

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Config a collab session needs to mint (and the OAuth listener needs to
/// validate) self-issued tokens — threaded from `main.rs`'s `OAuthConfig`
/// into `collab_handler`'s server-construction path, mirroring exactly how
/// `kb_query::KbQueryLimits` is already threaded (ADR-067 Phase D2).
///
/// Wrapped in `Option` at every call site rather than carrying its own
/// `enabled: bool` — `None` IS "disabled" (`config.oauth.enabled &&
/// config.oauth.self_issued_tokens_enabled && a key-mode identity exists`
/// collapsed once in `main.rs`, principle #12: minting a token nothing can
/// ever redeem, because the OAuth listener itself is off, is a silent no-op
/// worth refusing explicitly rather than allowing) — so every call site
/// checks exactly one thing, `is_some()`, never two.
#[derive(Clone)]
pub struct SelfIssueConfig {
    /// This daemon's own signing identity (the SAME `Identity` already
    /// installed as the collab doc-store signer and, when P2P is enabled,
    /// the iroh node identity) — the sole valid signer of a self-issued
    /// token.
    pub identity: Arc<Identity>,
    /// The `aud` claim to mint (and to require on validation) — this
    /// daemon's own `oauth.canonical_resource_uri`, matching exactly what
    /// `oauth::validate_bearer_token` already requires of externally-issued
    /// tokens (RFC 8707's confused-deputy defense applies identically here).
    pub audience: String,
    /// Token lifetime, seconds.
    pub ttl_secs: u64,
}

/// Mint a bearer token for `principal_fingerprint` — the mTLS connection's
/// OWN already-verified fingerprint (`collab_handler`'s `auth_principal`,
/// never accepted as a free-form argument from an unauthenticated caller,
/// which is what would make this a spoofing vector) — signed with this
/// daemon's own Ed25519 identity.
pub fn mint_self_token(
    identity: &Identity,
    principal_fingerprint: &str,
    audience: &str,
    ttl_secs: u64,
) -> Result<String, String> {
    let now = now_unix();
    let claims = serde_json::json!({
        "sub": principal_fingerprint,
        "aud": audience,
        "iss": SELF_ISSUED_ISS,
        "iat": now,
        "exp": now + ttl_secs,
    });
    let mut header = Header::new(Algorithm::EdDSA);
    header.kid = Some(SELF_ISSUED_KID.to_string());
    let der = identity.pkcs8_der()?;
    let encoding_key = EncodingKey::from_ed_der(&der);
    encode(&header, &claims, &encoding_key).map_err(|e| e.to_string())
}

/// Validate a self-issued token against THIS daemon's own known Ed25519
/// public key — never any other key, never a JWKS lookup. `Validation::new
/// (Algorithm::EdDSA)` fixes the accepted algorithm, so a token claiming
/// `kid: "self"` but signed (or merely encoded) with a different algorithm
/// — including the classic HS256-confusion attempt of treating the public
/// key bytes as an HMAC secret — is rejected before signature verification
/// even runs, the same structural protection `oauth::validate_bearer_token`
/// gets from its own fixed `Algorithm::RS256`.
pub fn validate_self_issued_token(
    token: &str,
    daemon_pubkey: &[u8; 32],
    audience: &str,
) -> Result<ValidatedPrincipal, TokenValidationError> {
    let decoding_key = DecodingKey::from_ed_der(daemon_pubkey);
    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.set_audience(&[audience.to_string()]);
    validation.set_issuer(&[SELF_ISSUED_ISS]);
    validation.validate_exp = true;

    let token_data =
        decode::<serde_json::Value>(token, &decoding_key, &validation).map_err(|e| {
            match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => TokenValidationError::Expired,
                jsonwebtoken::errors::ErrorKind::InvalidAudience => {
                    TokenValidationError::WrongAudience
                }
                jsonwebtoken::errors::ErrorKind::InvalidIssuer => TokenValidationError::WrongIssuer,
                jsonwebtoken::errors::ErrorKind::InvalidSignature => {
                    TokenValidationError::InvalidSignature
                }
                _ => TokenValidationError::Malformed,
            }
        })?;

    let claims = token_data.claims;
    let principal = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .ok_or(TokenValidationError::MissingPrincipalClaim)?
        .to_string();
    let audience: Vec<String> = match claims.get("aud") {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => vec![],
    };
    let expires_at = claims.get("exp").and_then(|v| v.as_u64()).unwrap_or(0);

    Ok(ValidatedPrincipal {
        principal,
        audience,
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUDIENCE: &str = "https://mae.example.com/mcp";

    fn daemon_identity() -> Arc<Identity> {
        Arc::new(Identity::generate("daemon-under-test"))
    }

    #[test]
    fn a_freshly_minted_token_validates_and_maps_the_principal() {
        let identity = daemon_identity();
        let token = mint_self_token(&identity, "SHA256:member-fp", AUDIENCE, 3600).unwrap();
        let pubkey = identity.public().to_bytes();
        let result = validate_self_issued_token(&token, &pubkey, AUDIENCE);
        let principal = result.expect("freshly minted token must validate");
        assert_eq!(principal.principal, "SHA256:member-fp");
        assert_eq!(principal.audience, vec![AUDIENCE.to_string()]);
    }

    /// Adversarial: a token minted for a DIFFERENT audience (a different
    /// canonical_resource_uri, e.g. this daemon's peer) is rejected here —
    /// RFC 8707's confused-deputy defense, same property
    /// `token_for_a_different_resource_is_rejected` locks in for
    /// externally-issued tokens.
    #[test]
    fn wrong_audience_is_rejected() {
        let identity = daemon_identity();
        let token = mint_self_token(
            &identity,
            "SHA256:member-fp",
            "https://a-different-daemon.example.com/mcp",
            3600,
        )
        .unwrap();
        let pubkey = identity.public().to_bytes();
        let result = validate_self_issued_token(&token, &pubkey, AUDIENCE);
        assert_eq!(result, Err(TokenValidationError::WrongAudience));
    }

    /// Adversarial: an already-expired token is rejected, not silently
    /// accepted because it was self-issued.
    #[test]
    fn expired_token_is_rejected() {
        let identity = daemon_identity();
        // A negative TTL underflows `now + ttl_secs` in u64 arithmetic if
        // passed directly -- mint a valid-looking token, then rebuild it
        // with an already-past `exp` via the same signing key, matching
        // `oauth::tests::expired_token_is_rejected`'s own approach of
        // constructing the exact adversarial claims rather than waiting.
        let der = identity.pkcs8_der().unwrap();
        let encoding_key = EncodingKey::from_ed_der(&der);
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(SELF_ISSUED_KID.to_string());
        let now = now_unix();
        let claims = serde_json::json!({
            "sub": "SHA256:member-fp",
            "aud": AUDIENCE,
            "iss": "self",
            "iat": now.saturating_sub(7200),
            "exp": now.saturating_sub(3600),
        });
        let token = encode(&header, &claims, &encoding_key).unwrap();
        let pubkey = identity.public().to_bytes();
        let result = validate_self_issued_token(&token, &pubkey, AUDIENCE);
        assert_eq!(result, Err(TokenValidationError::Expired));
    }

    /// Adversarial (the core property this whole module exists for): a
    /// token forged by a DIFFERENT identity's key -- but claiming `kid:
    /// "self"` and this daemon's own principal shape -- is rejected. This is
    /// the attacker-model test: possession of the `kid: "self"` short-circuit
    /// trigger grants nothing without the real daemon private key.
    #[test]
    fn forged_signature_from_a_different_identity_is_rejected() {
        let real_daemon = daemon_identity();
        let attacker = daemon_identity();
        // Attacker mints a token that LOOKS self-issued (kid: "self", sub of
        // their choosing) but signs it with THEIR OWN key, not the real
        // daemon's.
        let forged = mint_self_token(
            &attacker,
            "SHA256:attacker-claims-to-be-owner",
            AUDIENCE,
            3600,
        )
        .unwrap();
        let real_pubkey = real_daemon.public().to_bytes();
        let result = validate_self_issued_token(&forged, &real_pubkey, AUDIENCE);
        assert_eq!(result, Err(TokenValidationError::InvalidSignature));
    }

    /// Adversarial: a token whose `kid` is NOT `"self"` must never be
    /// accepted by this validator -- confirms `oauth::handle_request`'s own
    /// header-peek short-circuit (tested at that layer) has something real
    /// to fall through to, and that this function itself doesn't silently
    /// accept an arbitrary kid. `decode` here is called directly (bypassing
    /// the kid peek, which lives in `oauth::handle_request`) specifically to
    /// prove this function has no implicit kid check of its own to fall
    /// back on -- the peek is the ONLY gate, so it must be correct.
    #[test]
    fn a_correctly_signed_token_with_a_different_kid_still_validates_at_this_layer() {
        // This function only ever runs AFTER `oauth::handle_request` has
        // already peeked `kid == "self"` -- it doesn't re-check kid itself.
        // Documents that boundary explicitly: the kid gate is the caller's
        // job, not this function's, so a wrong-kid token reaching here
        // directly is NOT this function's failure mode to catch.
        let identity = daemon_identity();
        let der = identity.pkcs8_der().unwrap();
        let encoding_key = EncodingKey::from_ed_der(&der);
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some("not-self".to_string());
        let claims = serde_json::json!({
            "sub": "SHA256:member-fp",
            "aud": AUDIENCE,
            "iss": "self",
            "iat": now_unix(),
            "exp": now_unix() + 3600,
        });
        let token = encode(&header, &claims, &encoding_key).unwrap();
        let pubkey = identity.public().to_bytes();
        let result = validate_self_issued_token(&token, &pubkey, AUDIENCE);
        assert!(
            result.is_ok(),
            "kid is the caller's gate, not this function's"
        );
    }

    /// Adversarial: a token signed by a real daemon identity but claiming a
    /// DIFFERENT issuer is rejected -- locks in the `iss` check independent
    /// of the (already-covered) signature/audience checks.
    #[test]
    fn wrong_issuer_is_rejected() {
        let identity = daemon_identity();
        let der = identity.pkcs8_der().unwrap();
        let encoding_key = EncodingKey::from_ed_der(&der);
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(SELF_ISSUED_KID.to_string());
        let now = now_unix();
        let claims = serde_json::json!({
            "sub": "SHA256:member-fp",
            "aud": AUDIENCE,
            "iss": "not-self",
            "iat": now,
            "exp": now + 3600,
        });
        let token = encode(&header, &claims, &encoding_key).unwrap();
        let pubkey = identity.public().to_bytes();
        let result = validate_self_issued_token(&token, &pubkey, AUDIENCE);
        assert_eq!(result, Err(TokenValidationError::WrongIssuer));
    }
}
