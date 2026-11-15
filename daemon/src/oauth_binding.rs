//! Binding an identity-provider subject to a member key by proof of possession
//! (ADR-098 Amendment 1, C2), on the OAuth HTTPS listener.
//!
//! 1. `POST /identity/challenge` with an IdP bearer token and
//!    `{"fingerprint": "SHA256:…"}` returns `{challenge_id, challenge,
//!    expires_in}`. `challenge` is a JSON text covering a domain-separation
//!    tag, the token's `iss` and subject, this listener's audience, the
//!    fingerprint claimed, a random nonce (= `challenge_id`) and its expiry.
//! 2. The key holder signs the UTF-8 bytes of `challenge` with the member key
//!    (Ed25519). The program holding the IdP session and the one holding the
//!    key need not be the same.
//! 3. `POST /identity/bind` with a token of the SAME `(iss, sub)` and
//!    `{challenge_id, public_key, signature}` (base64) binds the subject to the
//!    fingerprint as an `agent`, recorded `bound_by = proof`.
//!
//! A self-issued token is refused on both routes (its principal already is a
//! fingerprint). A challenge is single use — removed on its first presentation
//! whatever happens next — in memory only, valid
//! `oauth.identity_challenge_ttl_secs`, and a subject may hold at most
//! `oauth.identity_max_pending_challenges` unused ones. Binding grants no
//! membership: membership stays owner-authored (D4).
//!
//! @ai-caution: [security] These routes can only ADD a binding for the
//! presenting token's own subject. Unbinding, replacing a binding and setting
//! `kind` are the local CLI's alone (`binding_cli`, principle #16); do not add a
//! network form of any of them.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use mae_daemon::identity_binding::{is_fingerprint, BindingError, BindingStore, BoundBy};
use mae_daemon::oauth_self_issue::{Credential, ValidatedPrincipal};
use mae_mcp::identity::PublicKey;
use mae_mcp::identity_challenge::challenge_text;
use mae_mcp::protocol::McpError;
use serde_json::{json, Value};

use crate::oauth::{self, kinded_response, Listener};

type HttpResponse = Response<Full<Bytes>>;

pub(crate) const CHALLENGE_PATH: &str = "/identity/challenge";
pub(crate) const BIND_PATH: &str = "/identity/bind";

pub(crate) fn is_binding_path(path: &str) -> bool {
    path == CHALLENGE_PATH || path == BIND_PATH
}

/// One outstanding challenge.
#[derive(Debug, Clone)]
pub(crate) struct Pending {
    pub issuer: String,
    pub subject: String,
    pub audience: String,
    pub fingerprint: String,
    /// The exact text the key holder signs.
    pub text: String,
    valid_until: Instant,
}

/// The routes' state: the in-memory challenge store and its bounds.
pub(crate) struct BindingRoutes {
    entries: Mutex<HashMap<String, Pending>>,
    ttl: Duration,
    max_per_subject: usize,
}

impl BindingRoutes {
    pub(crate) fn new(ttl_secs: u64, max_per_subject: usize) -> Self {
        BindingRoutes {
            entries: Mutex::new(HashMap::new()),
            ttl: Duration::from_secs(ttl_secs),
            max_per_subject,
        }
    }

    /// Mint a challenge, after purging expired ones. `Err` when the subject
    /// already holds its quota of unused challenges.
    pub(crate) fn mint(
        &self,
        vp: &ValidatedPrincipal,
        audience: &str,
        fingerprint: &str,
        now: Instant,
    ) -> Result<(String, Pending), usize> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, p| p.valid_until > now);
        let held = entries
            .values()
            .filter(|p| p.issuer == vp.issuer && p.subject == vp.subject)
            .count();
        if held >= self.max_per_subject {
            return Err(self.max_per_subject);
        }
        let mut nonce = [0u8; 32];
        rand::Rng::fill_bytes(&mut rand::rng(), &mut nonce);
        let id = data_encoding::BASE64URL_NOPAD.encode(&nonce);
        let expires_at = unix_now() + self.ttl.as_secs();
        let pending = Pending {
            issuer: vp.issuer.clone(),
            subject: vp.subject.clone(),
            audience: audience.to_string(),
            fingerprint: fingerprint.to_string(),
            // The one wire format, shared with the editor's signer: JSON, so
            // no `iss` or `sub` holding a newline or quote can make one
            // challenge read as another, led by the domain tag.
            text: challenge_text(
                &vp.issuer,
                &vp.subject,
                audience,
                fingerprint,
                &id,
                expires_at,
            ),
            valid_until: now + self.ttl,
        };
        entries.insert(id.clone(), pending.clone());
        Ok((id, pending))
    }

    /// Present a challenge: removed whether or not it is still valid. `None`
    /// for unknown, used and expired alike.
    pub(crate) fn take(&self, id: &str, now: Instant) -> Option<Pending> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let p = entries.remove(id)?;
        (p.valid_until > now).then_some(p)
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn refused(error: McpError) -> HttpResponse {
    kinded_response(StatusCode::FORBIDDEN, "access_denied", error)
}

fn bad_param(param: &str, msg: &str) -> HttpResponse {
    kinded_response(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        McpError::invalid_params(param, msg.to_string()),
    )
}

/// `POST /identity/challenge`, the token already validated and resolved.
pub(crate) fn issue_challenge(
    routes: &BindingRoutes,
    vp: &ValidatedPrincipal,
    audience: &str,
    body: &Value,
    now: Instant,
) -> HttpResponse {
    let Some(fp) = body.get("fingerprint").and_then(Value::as_str) else {
        return bad_param("fingerprint", "fingerprint (SHA256:…) is required");
    };
    if !is_fingerprint(fp) {
        return bad_param("fingerprint", "not a member key fingerprint (SHA256:…)");
    }
    match routes.mint(vp, audience, fp, now) {
        Ok((id, pending)) => oauth::json_response(
            StatusCode::OK,
            json!({
                "challenge_id": id,
                "challenge": pending.text,
                "expires_in": routes.ttl.as_secs(),
            }),
        ),
        Err(limit) => kinded_response(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            McpError::too_many_challenges(
                "this subject already holds its quota of unused challenges".to_string(),
                limit,
            ),
        ),
    }
}

/// The three fields of a bind request, decoded.
struct BindRequest {
    challenge_id: String,
    key: PublicKey,
    signature: Vec<u8>,
}

fn parse_bind(body: &Value) -> Result<BindRequest, Box<HttpResponse>> {
    let field = |k: &str| body.get(k).and_then(Value::as_str);
    let challenge_id = field("challenge_id")
        .ok_or_else(|| Box::new(bad_param("challenge_id", "challenge_id is required")))?;
    let key = field("public_key")
        .and_then(|k| PublicKey::from_encoded(k, None))
        .ok_or_else(|| {
            Box::new(bad_param(
                "public_key",
                "public_key must be the base64 of a 32-byte Ed25519 key",
            ))
        })?;
    let signature = field("signature")
        .and_then(|s| data_encoding::BASE64.decode(s.as_bytes()).ok())
        .ok_or_else(|| Box::new(bad_param("signature", "signature must be base64")))?;
    Ok(BindRequest {
        challenge_id: challenge_id.to_string(),
        key,
        signature,
    })
}

/// `POST /identity/bind`, the token already validated and resolved. Every
/// check in C2 step 4, then the one-to-one rule.
pub(crate) fn complete_binding(
    routes: &BindingRoutes,
    store: &BindingStore,
    vp: &ValidatedPrincipal,
    audience: &str,
    body: &Value,
    now: Instant,
) -> HttpResponse {
    let req = match parse_bind(body) {
        Ok(r) => r,
        Err(resp) => return *resp,
    };
    let Some(pending) = routes.take(&req.challenge_id, now) else {
        return refused(McpError::challenge_invalid(
            "unknown, already used or expired challenge".to_string(),
        ));
    };
    if pending.issuer != vp.issuer || pending.subject != vp.subject {
        return refused(McpError::challenge_mismatch(
            "this challenge was issued to another subject".to_string(),
        ));
    }
    if pending.audience != audience {
        return refused(McpError::challenge_mismatch(
            "this challenge was issued for another audience".to_string(),
        ));
    }
    if req.key.fingerprint() != pending.fingerprint {
        return refused(McpError::proof_invalid(
            "the key is not the one this challenge names".to_string(),
        ));
    }
    if !req.key.verify(pending.text.as_bytes(), &req.signature) {
        return refused(McpError::proof_invalid(
            "the signature does not verify against the challenge".to_string(),
        ));
    }
    record_binding(store, vp, &pending.fingerprint)
}

fn record_binding(store: &BindingStore, vp: &ValidatedPrincipal, fp: &str) -> HttpResponse {
    let conflict = |field: &str| {
        kinded_response(
            StatusCode::CONFLICT,
            "conflict",
            McpError::binding_conflict(
                format!("the {field} is already bound; an administrator must unbind it first"),
                field,
            ),
        )
    };
    let now = i64::try_from(unix_now()).unwrap_or(i64::MAX);
    match store.bind(&vp.issuer, &vp.subject, fp, BoundBy::Proof, now) {
        Ok(()) => oauth::json_response(
            StatusCode::OK,
            json!({"principal": fp, "kind": "agent", "bound_by": BoundBy::Proof.as_str()}),
        ),
        Err(BindingError::SubjectBound) => conflict("subject"),
        Err(BindingError::FingerprintBound) => conflict("fingerprint"),
        Err(e) => {
            tracing::warn!(error = %e, "binding store write failed");
            oauth::json_response(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error": "temporarily_unavailable"}),
            )
        }
    }
}

/// Both routes: POST, an IdP bearer token, a JSON body.
pub(crate) async fn handle(
    req: Request<Incoming>,
    listener: &Listener,
    routes: &BindingRoutes,
) -> HttpResponse {
    if req.method() != Method::POST {
        return oauth::json_response(
            StatusCode::METHOD_NOT_ALLOWED,
            json!({"error": "method_not_allowed", "error_description": "POST only"}),
        );
    }
    let Some(token) = oauth::extract_bearer_token(&req).map(str::to_string) else {
        return oauth::unauthorized(&listener.config, "missing bearer token");
    };
    let vp = match oauth::authenticate(&token, listener).await {
        Ok(vp) => vp,
        Err(resp) => return *resp,
    };
    if vp.credential == Credential::SelfIssued {
        return refused(McpError::issuer_not_supported(
            "binding needs an identity-provider token; a self-issued token's principal \
             already is a fingerprint"
                .to_string(),
        ));
    }
    let path = req.uri().path().to_string();
    let bytes = match oauth::read_limited_body(req, &listener.config).await {
        Ok(b) => b,
        Err(resp) => return *resp,
    };
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let audience = &listener.config.canonical_resource_uri;
    let now = Instant::now();
    match (path.as_str(), listener.bindings.as_deref()) {
        (CHALLENGE_PATH, _) => issue_challenge(routes, &vp, audience, &body, now),
        (_, Some(store)) => complete_binding(routes, store, &vp, audience, &body, now),
        (_, None) => oauth::json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": "temporarily_unavailable"}),
        ),
    }
}

#[cfg(test)]
#[path = "oauth_binding_tests.rs"]
mod oauth_binding_tests;
