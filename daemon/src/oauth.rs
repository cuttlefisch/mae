//! OAuth 2.1 resource-server bearer-token validation (ADR-052).
//!
//! `mae-daemon` acts purely as an OAuth 2.1 **resource server**: it never
//! issues tokens, never runs an authorization-code+PKCE flow (that's the
//! configured external authorization server's job, per the MCP
//! authorization spec — PKCE is a client<->AS concern this module has no
//! visibility into and cannot meaningfully test), and never stores a
//! revocation list. It validates a bearer token presented on each request
//! against a cached JWKS (JSON Web Key Set) fetched from the configured AS,
//! per RFC 9728 (Protected Resource Metadata) discovery and RFC 8707
//! (Resource Indicators — audience binding, the confused-deputy defense).
//!
//! The cryptographic primitive (JWT decode + signature verification) comes
//! from `jsonwebtoken`, a well-established crate — not reinvented here. What
//! *is* hand-rolled, deliberately (ADR-052's evaluated decision over
//! `rmcp-server-kit`), is the surrounding protocol-shaped scaffolding: JWKS
//! fetch/cache, audience/expiry enforcement, and mapping a validated token
//! onto a principal that feeds the existing `kb_access` chokepoint
//! (ADR-018) — never a parallel authorization system.

use std::sync::RwLock;
use std::time::{Duration, Instant};

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use mae_daemon::oauth_self_issue;
pub use mae_daemon::oauth_self_issue::{TokenValidationError, ValidatedPrincipal};
use serde::Deserialize;

/// Resource-server identity and mapping configuration (the `[oauth]` section
/// of `daemon.toml`).
#[derive(Debug, Clone)]
pub struct ResourceServerConfig {
    /// This server's own canonical URI (RFC 8707's `resource` parameter,
    /// RFC 9728's protected-resource identifier) — the audience every valid
    /// token presented here MUST include. Config-driven (principle #7),
    /// never inferred from the request, so a token can't be revalidated
    /// against whatever host header a client happened to send.
    pub canonical_resource_uri: String,
    /// Which claim of an EXTERNAL token identifies its subject within its
    /// issuer: with `iss`, the key of the binding table (ADR-098 D3). Since
    /// ADR-098 Amendment 1 (C1) its value never becomes the principal fed to
    /// `kb_access`: an external token reaches a member fingerprint only
    /// through a binding. Default `sub`, the ADR's `(iss, sub)`.
    pub principal_claim: String,
    /// ADR-098 D4: the claim an external token lists its groups in.
    pub groups_claim: String,
    /// ADR-098 D4: an external token carrying ANY ONE of these groups in
    /// `groups_claim` is admitted; checked on every request before the binding
    /// is looked up. Empty: no group gate.
    pub allowed_groups: Vec<String>,
    /// URL to fetch the external issuer's JWKS from. Empty means there is no
    /// external issuer (ADR-111 P1): only self-issued tokens can validate, and
    /// no JWKS client is built at all.
    pub jwks_url: String,
    /// A PEM bundle the JWKS fetch trusts instead of the system's roots.
    pub jwks_ca_file: Option<std::path::PathBuf>,
    /// The authorization server's issuer, checked against the token's `iss`
    /// claim. `None` skips issuer validation (not recommended, but some
    /// deployments' AS metadata omits a stable issuer during evaluation).
    pub issuer: Option<String>,
    /// ADR-053/Phase G (#382): whether `kb/query.*` is reachable at all —
    /// independently toggleable from the listener being up (see
    /// `config::OAuthConfig::kb_query_enabled`'s doc comment).
    pub kb_query_enabled: bool,
    /// `config::OAuthConfig::site_enabled`: the live KB site's routes.
    pub site_enabled: bool,
    /// `config::OAuthConfig::site_locale`.
    pub site_locale: String,
    /// `config::OAuthConfig::site_hidden_namespaces`.
    pub site_hidden_namespaces: Vec<String>,
    /// `config::OAuthConfig::site_home`.
    pub site_home: std::collections::BTreeMap<String, String>,
    /// `config::OAuthConfig::site_title`.
    pub site_title: std::collections::BTreeMap<String, String>,
    /// Cap on the raw size of an incoming request body, enforced before it's
    /// read into memory, regardless of `kb_query_enabled`. See
    /// `config::OAuthConfig::max_request_body_bytes`'s doc comment.
    pub max_request_body_bytes: usize,
    /// Cap on a single `kb/query.get` response's node-body size, bytes
    /// (unencrypted KBs only). See `config::OAuthConfig`'s doc comment.
    pub kb_query_max_body_bytes: usize,
    /// Cap on how many nodes a single `kb/query.search`/`kb/query.graph`
    /// call materializes and scans. See `config::OAuthConfig`'s doc comment.
    pub kb_query_max_scan_nodes: usize,
    /// Cap on the number of results a single `kb/query.search` call returns.
    pub kb_query_max_search_results: usize,
    /// ADR-098, for ADR-113 B7: the `amr`/`acr` values the operator counts as
    /// a second factor. Empty (the default): none does. The bare probe reports
    /// whether the caller's sign-in meets it.
    pub second_factor: mae_daemon::sign_in::SecondFactorRule,
}

/// A single JSON Web Key, the subset of RFC 7517 fields this module uses
/// (RSA keys only — the algorithm every mainstream external IdP's JWKS
/// endpoint publishes by default; EC/OKP support can be added if a real
/// deployment needs it, not speculatively).
#[derive(Debug, Clone, Deserialize)]
struct Jwk {
    kid: String,
    n: String,
    e: String,
}

#[derive(Debug, Clone, Deserialize)]
struct JwksResponse {
    keys: Vec<Jwk>,
}

/// Validate a bearer token against an already-fetched JWKS. Pure (no I/O,
/// no clock skew allowance beyond `jsonwebtoken`'s own small default) and
/// therefore directly unit-testable against locally-generated RSA
/// keypairs — see the adversarial tests below.
pub fn validate_bearer_token(
    token: &str,
    jwks: &[JwkOwned],
    config: &ResourceServerConfig,
) -> Result<ValidatedPrincipal, TokenValidationError> {
    let header = decode_header(token).map_err(|_| TokenValidationError::Malformed)?;
    let kid = header.kid.ok_or(TokenValidationError::Malformed)?;
    let jwk = jwks
        .iter()
        .find(|k| k.kid == kid)
        .ok_or(TokenValidationError::UnknownKey)?;

    let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
        .map_err(|_| TokenValidationError::Malformed)?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(std::slice::from_ref(&config.canonical_resource_uri));
    // `aud` must be PRESENT, not just right when present: jsonwebtoken checks the
    // audience only of a token that carries one, so without this a token the IdP
    // minted for any other client (no `aud`) was accepted here — the confused
    // deputy RFC 8707 / ADR-052 exist to stop. An array `aud` still passes when it
    // CONTAINS this resource (RFC 7519 §4.1.3).
    validation.set_required_spec_claims(&["exp", "aud"]);
    if let Some(ref issuer) = config.issuer {
        validation.set_issuer(std::slice::from_ref(issuer));
    }
    validation.validate_exp = true;
    // Stated, not inherited: `SignIn::fresh_within` tolerates the same skew.
    validation.leeway = mae_daemon::sign_in::JWT_CLOCK_LEEWAY_SECS;

    let token_data =
        decode::<serde_json::Value>(token, &decoding_key, &validation).map_err(|e| {
            match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => TokenValidationError::Expired,
                jsonwebtoken::errors::ErrorKind::InvalidAudience => {
                    TokenValidationError::WrongAudience
                }
                // No audience at all is not this resource's audience.
                jsonwebtoken::errors::ErrorKind::MissingRequiredClaim(c) if c == "aud" => {
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
    let subject = claims
        .get(&config.principal_claim)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or(TokenValidationError::MissingPrincipalClaim)?
        .to_string();
    check_allowed_groups(&claims, config)?;
    // C1: the claim is the SUBJECT, never the principal. Until a binding
    // resolves it (`identity_binding::resolve`), it is a member of nothing.
    let mut vp = oauth_self_issue::validated_from_claims(
        &claims,
        String::new(),
        oauth_self_issue::Credential::IdpUnbound,
    );
    vp.subject = subject;
    vp.sign_in = Some(mae_daemon::sign_in::SignIn::from_claims(
        &claims,
        &config.groups_claim,
    ));
    // `(iss, sub)` keys the binding table, and an absent or non-string `iss`
    // reads as "" — one issuer every such token would share. Checked here
    // because `config.issuer = None` skips the library's issuer validation.
    if vp.issuer.is_empty() {
        return Err(TokenValidationError::WrongIssuer);
    }
    vp.principal = oauth_self_issue::unbound_principal(&vp.issuer, &vp.subject);
    Ok(vp)
}

/// ADR-098 D4's session gate: the token's `groups_claim` (a string array, or a
/// single string) must hold at least one configured group, matched exactly.
/// Nothing configured, no gate.
fn check_allowed_groups(
    claims: &serde_json::Value,
    config: &ResourceServerConfig,
) -> Result<(), TokenValidationError> {
    if config.allowed_groups.is_empty() {
        return Ok(());
    }
    let held = mae_daemon::sign_in::groups_from_claims(claims, &config.groups_claim);
    let admitted = config.allowed_groups.iter().any(|g| held.contains(g));
    if !admitted {
        return Err(TokenValidationError::NotInAllowedGroup);
    }
    Ok(())
}

/// An owned, module-internal copy of the fields `validate_bearer_token`
/// needs from a JWK — decoupled from `Jwk`'s `Deserialize` derive so tests
/// can construct one directly without round-tripping through JSON.
#[derive(Debug, Clone)]
pub struct JwkOwned {
    pub kid: String,
    pub n: String,
    pub e: String,
}

impl From<&Jwk> for JwkOwned {
    fn from(jwk: &Jwk) -> Self {
        JwkOwned {
            kid: jwk.kid.clone(),
            n: jwk.n.clone(),
            e: jwk.e.clone(),
        }
    }
}

/// TTL for a cached JWKS before it's re-fetched. Short enough that a real
/// key rotation on the AS side is picked up promptly (bounding the window
/// during which a rotated-out key's tokens are still accepted -- this
/// module's practical equivalent of revocation latency); long enough that
/// every request doesn't round-trip to the AS.
const JWKS_CACHE_TTL: Duration = Duration::from_secs(300);

/// Fetches and caches a JWKS from the configured URL, refreshing on TTL
/// expiry. A cache-miss-on-unknown-`kid` refresh (not implemented here,
/// left as a documented follow-up) would shorten the rotation window
/// further at the cost of an extra fetch per genuinely-unknown key; TTL-only
/// is the simpler, still-correct starting point.
pub struct JwksCache {
    url: String,
    client: reqwest::Client,
    state: RwLock<Option<(Vec<JwkOwned>, Instant)>>,
}

impl JwksCache {
    /// The JWKS client for `url`. With `ca_file`, the fetch trusts ONLY the
    /// PEM roots in that file, so it works where the system has none (a
    /// minimal container image). Without it, it uses the platform's roots.
    ///
    /// `Err` instead of a panic: `reqwest::Client::new()` panics when the TLS
    /// backend cannot load any root, which took the whole daemon down at
    /// startup in an image without CA certificates. The caller reports the
    /// error and leaves the listener off, like every other misconfiguration.
    pub fn new(url: String, ca_file: Option<&std::path::Path>) -> Result<Self, String> {
        let mut builder = reqwest::Client::builder();
        if let Some(path) = ca_file {
            let pem = std::fs::read(path).map_err(|e| {
                format!("oauth.jwks_ca_file {}: cannot read it: {e}", path.display())
            })?;
            let roots = reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| {
                format!(
                    "oauth.jwks_ca_file {}: not a PEM certificate bundle: {e}",
                    path.display()
                )
            })?;
            if roots.is_empty() {
                return Err(format!(
                    "oauth.jwks_ca_file {}: holds no certificate",
                    path.display()
                ));
            }
            builder = builder.tls_certs_only(roots);
        }
        let client = builder.build().map_err(|e| match ca_file {
            Some(path) => format!(
                "oauth.jwks_ca_file {}: not usable as CA certificates for oauth.jwks_url {url} \
                 ({e})",
                path.display()
            ),
            None => format!(
                "oauth.jwks_url {url}: cannot build the HTTPS client that fetches it ({e}); \
                 the system has no usable CA certificates: install them, or name a PEM file \
                 in oauth.jwks_ca_file"
            ),
        })?;
        Ok(JwksCache {
            url,
            client,
            state: RwLock::new(None),
        })
    }

    /// Returns the cached JWKS if still fresh, otherwise fetches a new one.
    pub async fn get(&self) -> Result<Vec<JwkOwned>, reqwest::Error> {
        if let Some((keys, fetched_at)) = self.state.read().unwrap().clone() {
            if fetched_at.elapsed() < JWKS_CACHE_TTL {
                return Ok(keys);
            }
        }
        let response: JwksResponse = self.client.get(&self.url).send().await?.json().await?;
        let keys: Vec<JwkOwned> = response.keys.iter().map(JwkOwned::from).collect();
        *self.state.write().unwrap() = Some((keys.clone(), Instant::now()));
        Ok(keys)
    }
}

/// RFC 9728 Protected Resource Metadata document, served at
/// `/.well-known/oauth-protected-resource`.
pub fn protected_resource_metadata(config: &ResourceServerConfig) -> serde_json::Value {
    serde_json::json!({
        "resource": config.canonical_resource_uri,
        "authorization_servers": config.issuer.as_ref().map(|i| vec![i.clone()]).unwrap_or_default(),
    })
}

/// The `WWW-Authenticate` header value for a 401 response, pointing the
/// client at the Protected Resource Metadata document per RFC 9728 §5.1.
pub fn www_authenticate_header(config: &ResourceServerConfig) -> String {
    format!(
        r#"Bearer resource_metadata="{}/.well-known/oauth-protected-resource""#,
        config.canonical_resource_uri.trim_end_matches('/')
    )
}

// ---------------------------------------------------------------------------
// HTTPS listener
// ---------------------------------------------------------------------------

use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use mae_daemon::collab_handler;
use mae_daemon::doc_store::DocStore;

use crate::conn_limit::{ConnGuard, ConnLimiter};

/// Build the listener's rustls server config around a certificate that follows
/// its files on disk (`oauth_tls::ReloadingCertResolver`, ADR-111 P1), so a
/// renewed certificate is presented without a restart. The INITIAL pair must
/// load; a later bad write keeps the previous certificate in service.
fn load_tls_config(cert_path: &Path, key_path: &Path) -> Result<rustls::ServerConfig, String> {
    // Explicit `ring` provider (matching shared/mcp/src/tls.rs's identical
    // pattern) rather than the ambiguous default builder: both `ring` (this
    // crate's own rustls feature) and `aws-lc-rs` (transitively, via
    // reqwest's rustls feature) are present in the dependency tree, so
    // rustls cannot auto-select a process-level default -- it hard-errors
    // rather than silently guessing, and `builder_with_provider` sidesteps
    // needing a global `CryptoProvider::install_default()` call at all.
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let resolver =
        crate::oauth_tls::ReloadingCertResolver::new(cert_path, key_path, provider.clone())?;
    Ok(rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("configuring TLS protocol versions: {e}"))?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver)))
}

/// Extract a bearer token from an `Authorization: Bearer <token>` header.
pub(crate) fn extract_bearer_token(req: &Request<Incoming>) -> Option<&str> {
    req.headers()
        .get(hyper::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Apply the no-caching headers every response from this listener needs.
///
/// @ai-caution: [security] EVERY response builder in this module must go
/// through here (audit #588.3). This listener serves bearer-token-authenticated
/// KB content (`kb/query.*` responses and the live site's pages), and without
/// `no-store` a browser writes it to its on-disk cache and an intermediary
/// proxy may retain it. `no-store` is
/// mandated for token-bearing responses by RFC 6749 §5.1, which OAuth 2.1
/// carries forward; `Pragma: no-cache` covers HTTP/1.0 intermediaries.
pub(crate) fn with_no_store(
    builder: hyper::http::response::Builder,
) -> hyper::http::response::Builder {
    builder
        .header(hyper::header::CACHE_CONTROL, "no-store")
        .header(hyper::header::PRAGMA, "no-cache")
}

pub(crate) fn json_response(status: StatusCode, body: serde_json::Value) -> Response<Full<Bytes>> {
    with_no_store(
        Response::builder()
            .status(status)
            .header(hyper::header::CONTENT_TYPE, "application/json"),
    )
    .body(Full::new(Bytes::from(body.to_string())))
    .expect("building a response from a fixed status/body never fails")
}

pub(crate) fn unauthorized(config: &ResourceServerConfig, reason: &str) -> Response<Full<Bytes>> {
    let mut resp = json_response(
        StatusCode::UNAUTHORIZED,
        serde_json::json!({"error": "invalid_token", "error_description": reason}),
    );
    resp.headers_mut().insert(
        hyper::header::WWW_AUTHENTICATE,
        www_authenticate_header(config)
            .parse()
            .expect("header value is a plain formatted string, always valid"),
    );
    resp
}

/// The unauthenticated surface, in full: the PRM document and `GET
/// /api/health` (the site's static assets are served beside it). `None` means
/// the request is not one of these and must authenticate. ADR-073's view, once
/// served here too, is retired (ADR-119 Amendment 2 B4).
///
/// @ai-caution: [security] Everything returned from here is served to ANY
/// caller that can reach the port. Nothing added here may carry KB content, a
/// credential, or anything that identifies this daemon (version, fingerprint,
/// KB names) -- `health_opens_nothing_else` pins that.
fn public_response(
    req: &Request<Incoming>,
    config: &ResourceServerConfig,
) -> Option<Response<Full<Bytes>>> {
    let path = req.uri().path();
    if path == "/.well-known/oauth-protected-resource" {
        return Some(json_response(
            StatusCode::OK,
            protected_resource_metadata(config),
        ));
    }
    // The platform health contract (ADR-111 P1): liveness of THIS listener,
    // status only. Exactly one path and only the read methods -- a POST here
    // falls through to authentication like any other request, so a KB query
    // aimed at the health path cannot ride its exemption.
    if path == "/api/health" && matches!(*req.method(), hyper::Method::GET | hyper::Method::HEAD) {
        return Some(json_response(
            StatusCode::OK,
            serde_json::json!({"status": "ok"}),
        ));
    }
    None
}

/// Validate `token` against whichever population it claims membership in:
/// this daemon's own self-issued tokens (ADR-067 D3), or the external issuer's
/// JWKS -- which exists only when `jwks_url` is configured (ADR-111 P1). An
/// external token's principal is then resolved through the binding table
/// (ADR-098 Amendment 1, C1). `Err` is the complete response to send.
pub(crate) async fn authenticate(
    token: &str,
    listener: &Listener,
) -> Result<ValidatedPrincipal, Box<Response<Full<Bytes>>>> {
    resolve_token(token, listener)
        .await
        .map_err(|f| f.into_response(&listener.config))
}

/// [`authenticate`], with the failure still typed, for a caller that answers
/// it other than as an HTTP response: the sync socket's `session/renew`
/// (ADR-099 Amendment 2, E2) re-resolves the binding here.
pub(crate) async fn resolve_token(
    token: &str,
    listener: &Listener,
) -> Result<ValidatedPrincipal, AuthFailure> {
    let vp = validate_token(
        token,
        &listener.config,
        listener.jwks.as_ref(),
        listener.self_issue.as_ref(),
    )
    .await?;
    let resolved =
        mae_daemon::identity_binding::resolve_off_runtime(vp, listener.bindings.clone()).await;
    resolved.map_err(|e| {
        // Fail closed: a binding that cannot be read is not "unbound" (that
        // would silently demote a member), and never "bound".
        tracing::warn!(error = %e, "binding table unreadable");
        AuthFailure::Unavailable
    })
}

/// Why a bearer token did not authenticate, before a transport renders it:
/// the HTTP routes as a response ([`AuthFailure::into_response`]), the sync
/// socket's renewal as a JSON-RPC error.
#[derive(Debug)]
pub(crate) enum AuthFailure {
    /// `401`: the token does not validate; the reason goes into `WWW-Authenticate`.
    Invalid(String),
    /// `403 group_required`: the token carries none of `allowed_groups` (D4).
    NotInGroup,
    /// `503`: the JWKS or the binding table could not be read.
    Unavailable,
}

impl AuthFailure {
    /// The listener's response for this failure.
    pub(crate) fn into_response(self, config: &ResourceServerConfig) -> Box<Response<Full<Bytes>>> {
        Box::new(match self {
            AuthFailure::Invalid(reason) => unauthorized(config, &reason),
            AuthFailure::NotInGroup => group_refusal(),
            AuthFailure::Unavailable => json_response(
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({"error": "temporarily_unavailable"}),
            ),
        })
    }
}

/// The refusal for a token that validated but lacks a required group (D4):
/// `403`, since the credential is good and the session is not authorised.
fn group_refusal() -> Response<Full<Bytes>> {
    kinded_response(
        StatusCode::FORBIDDEN,
        "insufficient_scope",
        mae_mcp::protocol::McpError::group_required(
            "this token carries none of the groups oauth.allowed_groups names".to_string(),
        ),
    )
}

/// An HTTP refusal `{error, error_description, kind, …extra}` built from a
/// kinded [`McpError`](mae_mcp::protocol::McpError): the kind and its extra
/// fields come from the one constructor `ERROR_KINDS` is checked against.
pub(crate) fn kinded_response(
    status: StatusCode,
    error: &str,
    refusal: mae_mcp::protocol::McpError,
) -> Response<Full<Bytes>> {
    let mut body = match refusal.data.as_deref() {
        Some(serde_json::Value::Object(m)) => m.clone(),
        _ => serde_json::Map::new(),
    };
    body.insert("error".into(), error.into());
    body.insert("error_description".into(), refusal.message.into());
    json_response(status, serde_json::Value::Object(body))
}

async fn validate_token(
    token: &str,
    config: &ResourceServerConfig,
    jwks: Option<&JwksCache>,
    self_issue: Option<&oauth_self_issue::SelfIssueConfig>,
) -> Result<ValidatedPrincipal, AuthFailure> {
    // ADR-067 Phase D3: a cheap, unauthenticated header PEEK (never the
    // signature -- that's still verified below, by the real validator for
    // whichever population this token claims membership in) decides which
    // of two entirely separate validators runs. `kid == "self"` never
    // collides with a real external JWKS key id, and `self_issue` being
    // `None` (the config gate, principle #12) means this branch is
    // unreachable regardless of what any token claims -- a `kid: "self"`
    // token on a daemon that never opted in just falls through to the
    // ordinary JWKS path below, where it fails as an unknown key like any
    // other bogus `kid`.
    let self_issue_ctx = self_issue.filter(|_| {
        jsonwebtoken::decode_header(token)
            .ok()
            .and_then(|h| h.kid)
            .as_deref()
            == Some(oauth_self_issue::SELF_ISSUED_KID)
    });
    if let Some(si) = self_issue_ctx {
        let daemon_pubkey = si.identity.public().to_bytes();
        return oauth_self_issue::validate_self_issued_token(token, &daemon_pubkey, &si.audience)
            .map_err(|e| {
                tracing::debug!(?e, "self-issued bearer token rejected");
                AuthFailure::Invalid(format!("{e:?}"))
            });
    }

    // No external issuer configured: a self-issued-only deployment. Refused as
    // an AUTH failure (401), never as the 503 a failed JWKS fetch produces --
    // nothing is temporarily unavailable, and a client would retry a 503.
    let Some(jwks) = jwks else {
        return Err(AuthFailure::Invalid(
            "no external token issuer is configured on this daemon (oauth.jwks_url is \
             unset); only its own self-issued tokens are accepted"
                .to_string(),
        ));
    };
    let keys = jwks.get().await.map_err(|e| {
        tracing::warn!(error = %e, "failed to fetch JWKS");
        AuthFailure::Unavailable
    })?;
    validate_bearer_token(token, &keys, config).map_err(|e| {
        tracing::debug!(?e, "bearer token rejected");
        match e {
            TokenValidationError::NotInAllowedGroup => AuthFailure::NotInGroup,
            e => AuthFailure::Invalid(format!("{e:?}")),
        }
    })
}

/// Per-request handler: serves the public surface (`public_response`)
/// unauthenticated, gates everything else on a valid bearer token. Once a token
/// validates: if the request carries a parseable JSON-RPC body AND
/// `kb_query_enabled` AND a `DocStore` is available, dispatch it as a
/// `kb/query.*` call (ADR-053/Phase G); otherwise fall back to the plain
/// diagnostic response (ADR-052) — keeps `mae-daemon doctor`-style bare
/// bearer-verification working unchanged for callers that never send a body.
async fn handle_request(
    req: Request<Incoming>,
    listener: Arc<Listener>,
    conn: Arc<ConnGuard>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let config = &listener.config;
    let doc_store = listener.doc_store.as_ref();
    if let Some(resp) = public_response(&req, config) {
        return Ok(resp);
    }
    // ADR-119 L1a: the site's static assets are public (no KB content, and a
    // `<link>` cannot carry a bearer token), served only when the site is on.
    if config.site_enabled && req.uri().path().starts_with(mae_site::assets::PREFIX) {
        return Ok(site_asset(&req));
    }
    // ADR-099: `/ws` and `/ws/ticket` exist only when `ws_enabled`; otherwise
    // they are ordinary paths and fall through to the authenticated default.
    if crate::ws::is_ws_path(req.uri().path()) && listener.ws.is_some() {
        return Ok(crate::ws::handle(req, Arc::clone(&listener), conn).await);
    }
    // ADR-098 Amendment 1, C2: the proof-of-possession routes exist only when
    // `identity_binding_enabled`; otherwise they fall through like any path.
    if let Some(routes) = listener.binding_routes.as_ref() {
        if crate::oauth_binding::is_binding_path(req.uri().path()) {
            return Ok(crate::oauth_binding::handle(req, &listener, routes).await);
        }
    }

    let Some(token) = extract_bearer_token(&req).map(str::to_string) else {
        return Ok(unauthorized(config, "missing bearer token"));
    };
    let principal = match authenticate(&token, &listener).await {
        Ok(p) => p,
        Err(resp) => return Ok(*resp),
    };

    // ADR-119 L1a: the live KB site, read-only, when enabled. Authenticated
    // like every route here; the KB's read gate decides the rest.
    if config.site_enabled {
        match site_request(req.uri().path()) {
            Some(Some((kb_id, view))) => {
                return Ok(site_route(&req, &listener, &principal, &kb_id, &view).await);
            }
            Some(None) => return Ok(site_response(StatusCode::NOT_FOUND, "not found".into())),
            None => {}
        }
    }

    // Read the body (never done before this phase) to see if this is a
    // kb/query.* JSON-RPC call. An empty/unparseable body is not an error —
    // it's exactly what a bare bearer-verification probe sends. The size
    // limit is enforced by `Limited` DURING the read (errors mid-stream once
    // the budget is exceeded), not after collecting into memory — an
    // authenticated caller cannot force unbounded server-side buffering by
    // sending an oversized body, regardless of `kb_query_enabled`.
    let body_bytes = match read_limited_body(req, config).await {
        Ok(b) => b,
        Err(resp) => return Ok(*resp),
    };

    let rpc_request: Option<mae_mcp::protocol::JsonRpcRequest> = if body_bytes.is_empty() {
        None
    } else {
        serde_json::from_slice(&body_bytes).ok()
    };

    let body = route_authenticated_request(
        rpc_request,
        config,
        doc_store,
        &principal,
        listener.quota.as_ref(),
    )
    .await;
    Ok(json_response(StatusCode::OK, body))
}

/// Whether `path` is the site's (`Some`), and if so which view (`Some(None)`:
/// the site's not-found). Every path under `/kb/` is the site's, so one that
/// names no view gets the site's not-found, never the bearer probe the rest of
/// the listener answers: `/kb/<kb-id>` without its slash once answered 200
/// with JSON (A5: one not-found for every non-page).
pub(crate) fn site_request(path: &str) -> Option<Option<(String, mae_daemon::site::View)>> {
    path.starts_with("/kb/").then(|| site_path(path))
}

/// `/kb/<kb-id>/<id>`, with `<id>` percent-decoded. One segment each: any
/// other shape is not a site path.
pub(crate) fn site_path(path: &str) -> Option<(String, mae_daemon::site::View)> {
    use mae_daemon::site::View;
    let rest = path.strip_prefix("/kb/")?;
    let (kb, rest) = rest.split_once('/')?;
    if kb.is_empty() {
        return None;
    }
    let view = match rest.split_once('/') {
        // `/kb/<kb-id>/`: the KB's home page (Amendment 2 B3). Which node
        // is the route's to look up, from configuration.
        None if rest.is_empty() => View::Home(None),
        // `tags` is reserved (a page cannot hold it, P-166): the tag index.
        None if rest == "tags" => View::Tags,
        // Also reserved: the content index (P-77).
        None if rest == "index.json" => View::Index,
        // And the explorer's data (Amendment 3).
        None if rest == "explorer.json" => View::Explorer,
        None => View::Page(percent_decode(rest)?),
        Some(("tags", tag)) if !tag.is_empty() && !tag.contains('/') => {
            View::Tag(percent_decode(tag)?)
        }
        Some(_) => return None,
    };
    Some((kb.to_string(), view))
}

/// `%XX` decoded; `None` for a bad escape or text that is not UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = b.get(i + 1..i + 3)?;
            if !h.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            let h = std::str::from_utf8(h).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The home view with the node the operator declared for this KB (ADR-119
/// Amendment 2 B3): configuration, never content. Other views unchanged.
fn with_declared_home(
    view: &mae_daemon::site::View,
    config: &ResourceServerConfig,
    kb_id: &str,
) -> mae_daemon::site::View {
    use mae_daemon::site::View;
    match view {
        View::Home(_) => View::Home(config.site_home.get(kb_id).cloned()),
        other => other.clone(),
    }
}

/// The content index (P-77) as JSON.
fn index_response(entries: &[mae_site::IndexEntry]) -> Response<Full<Bytes>> {
    let pages: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id, "title": e.title, "href": e.href,
                "tags": e.tags, "links": e.links, "text": e.text,
            })
        })
        .collect();
    site_json(serde_json::json!({ "pages": pages }).to_string())
}

/// A JSON answer with the site's headers: the content index, the explorer.
fn site_json(body: String) -> Response<Full<Bytes>> {
    let mut r = site_response(StatusCode::OK, body);
    r.headers_mut().insert(
        "content-type",
        hyper::header::HeaderValue::from_static("application/json; charset=utf-8"),
    );
    r
}

/// One live-site request (ADR-119 L1a): `GET` only, charged as a scan, and
/// every refusal answered as one not-found (D7).
async fn site_route(
    req: &Request<Incoming>,
    listener: &Listener,
    principal: &ValidatedPrincipal,
    kb_id: &str,
    view: &mae_daemon::site::View,
) -> Response<Full<Bytes>> {
    use mae_daemon::site;
    let config = &listener.config;
    if req.method() != hyper::Method::GET {
        return site_response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed".into());
    }
    let not_found = || site_response(StatusCode::NOT_FOUND, "not found".into());
    let Some(store) = listener.doc_store.as_ref() else {
        return not_found();
    };
    if principal.credential == oauth_self_issue::Credential::IdpUnbound {
        return not_found();
    }
    // A page derivation reads the whole KB: charged as a scan.
    // Held for the whole request, like kb/query.*'s lease.
    let _lease = match mae_daemon::quota::charge_or_reject(
        listener.quota.as_ref(),
        Some(&principal.principal),
        "kb/query.list",
        serde_json::Value::Null,
    ) {
        Ok(lease) => lease,
        Err(_) => return site_response(StatusCode::TOO_MANY_REQUESTS, "quota exceeded".into()),
    };
    let view = with_declared_home(view, config, kb_id);
    // The site's title and home link (P-16): only on a page the reader can
    // read; every refusal below stays the one identical not-found (A5).
    let home = format!("/kb/{kb_id}/");
    let chrome = site::Chrome {
        lang: &config.site_locale,
        site: config
            .site_title
            .get(kb_id)
            .map(|t| (t.as_str(), home.as_str())),
    };
    let page = site::view(
        store,
        kb_id,
        &view,
        Some(&principal.principal),
        site::SiteLimits {
            max_scan_nodes: config.kb_query_max_scan_nodes,
            max_body_bytes: config.kb_query_max_body_bytes,
        },
        &mae_site::SiteOptions {
            hidden_namespaces: config.site_hidden_namespaces.clone(),
        },
    )
    .await;
    site_answer(page, chrome)
}

/// The response for a derived site answer. The chrome (the site's title)
/// reaches only a page or listing the reader can read: every refusal is the
/// same body whatever the configuration (A5), so it says nothing about which
/// KBs exist (`a_refusal_never_carries_the_site_title`).
pub(crate) fn site_answer(
    page: mae_daemon::site::SitePage,
    chrome: mae_daemon::site::Chrome<'_>,
) -> Response<Full<Bytes>> {
    use mae_daemon::site::{self, SitePage};
    match page {
        SitePage::Found {
            title,
            html,
            backlinks,
            tags,
            toc,
            explorer,
        } => site_response(
            StatusCode::OK,
            site::document(
                chrome,
                &title,
                &html,
                &backlinks,
                &tags,
                &toc,
                explorer.as_ref(),
            ),
        ),
        SitePage::Index(entries) => index_response(&entries),
        SitePage::Explorer(json) => site_json(json),
        SitePage::Listing {
            kind,
            items,
            explorer,
        } => site_response(
            StatusCode::OK,
            site::listing_document(chrome, &kind, &items, explorer.as_ref()),
        ),
        SitePage::NotFound => site_response(StatusCode::NOT_FOUND, "not found".into()),
        SitePage::TooLarge => site_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "this KB is larger than the site derives per request".into(),
        ),
    }
}

/// One embedded site asset (`mae_site::assets`), by its content-addressed
/// name: immutable for a year, since a change always gets a new name. Any
/// other path under the prefix, or a method but GET, is the site's 404.
fn site_asset(req: &Request<Incoming>) -> Response<Full<Bytes>> {
    let asset = (req.method() == hyper::Method::GET)
        .then(|| mae_site::assets::at(req.uri().path()))
        .flatten();
    let Some(asset) = asset else {
        return site_response(StatusCode::NOT_FOUND, "not found".into());
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", asset.content_type)
        .header("content-security-policy", mae_daemon::site::CSP)
        .header("x-content-type-options", "nosniff")
        .header("cache-control", "public, max-age=31536000, immutable")
        .body(Full::new(Bytes::from_static(asset.body.as_bytes())))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

/// An HTML (or plain refusal) response with the site's headers: its CSP,
/// no sniffing, no referrer, no caching of a reader's private page.
fn site_response(status: StatusCode, body: String) -> Response<Full<Bytes>> {
    let ctype = if status == StatusCode::OK {
        "text/html; charset=utf-8"
    } else {
        "text/plain; charset=utf-8"
    };
    Response::builder()
        .status(status)
        .header("content-type", ctype)
        .header("content-security-policy", mae_daemon::site::CSP)
        .header("x-content-type-options", "nosniff")
        .header("referrer-policy", "no-referrer")
        .header("cache-control", "private, no-store")
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

/// Read an authenticated request's body under `max_request_body_bytes`,
/// enforced DURING the read. An unreadable body reads as empty (what a bare
/// probe sends); one over the limit is the `413` to send.
pub(crate) async fn read_limited_body(
    req: Request<Incoming>,
    config: &ResourceServerConfig,
) -> Result<Bytes, Box<Response<Full<Bytes>>>> {
    let limited_body = http_body_util::Limited::new(req.into_body(), config.max_request_body_bytes);
    match http_body_util::BodyExt::collect(limited_body).await {
        Ok(collected) => Ok(collected.to_bytes()),
        Err(e)
            if e.downcast_ref::<http_body_util::LengthLimitError>()
                .is_some() =>
        {
            tracing::debug!(
                limit = config.max_request_body_bytes,
                "request body exceeded size limit"
            );
            Err(Box::new(json_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                serde_json::json!({
                    "error": "payload_too_large",
                    "error_description": format!(
                        "request body exceeds the {}-byte limit",
                        config.max_request_body_bytes
                    ),
                }),
            )))
        }
        Err(e) => {
            tracing::debug!(error = %e, "failed to read request body");
            Ok(Bytes::new())
        }
    }
}

/// The routing decision `handle_request` makes once a bearer token has
/// already validated — split out (ADR-053/Phase G, #382) so it's directly
/// unit-testable without a live HTTP connection (constructing a real
/// `Request<Incoming>` body outside an actual hyper connection isn't
/// straightforward; this function needs only already-parsed pieces).
pub(crate) async fn route_authenticated_request(
    rpc_request: Option<mae_mcp::protocol::JsonRpcRequest>,
    config: &ResourceServerConfig,
    doc_store: Option<&Arc<DocStore>>,
    principal: &ValidatedPrincipal,
    quota: &dyn mae_daemon::quota::QuotaCharger,
) -> serde_json::Value {
    match (rpc_request, config.kb_query_enabled, doc_store) {
        (Some(rpc), true, Some(store)) => {
            let limits = mae_daemon::kb_query::KbQueryLimits {
                max_body_bytes: config.kb_query_max_body_bytes,
                max_scan_nodes: config.kb_query_max_scan_nodes,
                max_search_results: config.kb_query_max_search_results,
            };
            let params = rpc.params.unwrap_or(serde_json::Value::Null);
            // ADR-060 Phase C (#456): each listener charges at its OWN entry —
            // `kb_query::dispatch` is deliberately not the chokepoint, since the
            // collab path reaches it having already charged. See ADR-060's #456 note.
            let _lease = match mae_daemon::quota::charge_or_reject(
                quota,
                Some(&principal.principal),
                &rpc.method,
                rpc.id.clone(),
            ) {
                Ok(lease) => lease,
                Err(resp) => return serde_json::to_value(&resp).unwrap_or(serde_json::Value::Null),
            };
            // C1: an unbound subject is a member of nothing, decided here and
            // never by comparing its string with a member list.
            if principal.credential == oauth_self_issue::Credential::IdpUnbound {
                return unbound_answer(rpc.id, &rpc.method);
            }
            let rpc_response = match mae_daemon::kb_query::dispatch(
                &rpc.method,
                &params,
                store,
                Some(&principal.principal),
                limits,
            )
            .await
            {
                Ok(result) => mae_mcp::protocol::JsonRpcResponse::success(rpc.id, result),
                Err(e) => mae_mcp::protocol::JsonRpcResponse::error(rpc.id, e),
            };
            serde_json::to_value(&rpc_response).unwrap_or(serde_json::Value::Null)
        }
        // kb_query_enabled=true but no DocStore exists to serve from
        // (collab.enabled=false) — a DISTINCT condition from "disabled"
        // below, and the caller sent a real RPC it deserves a real
        // JSON-RPC-shaped error for, not the bare unauthenticated-probe
        // diagnostic the true no-body case gets.
        (Some(rpc), true, None) => serde_json::to_value(mae_mcp::protocol::JsonRpcResponse::error(
            rpc.id,
            mae_mcp::protocol::McpError::kb_query_unavailable(
                "kb/query.* is enabled but no DocStore is available on this daemon \
                 (collab.enabled is false)"
                    .to_string(),
            ),
        ))
        .unwrap_or(serde_json::Value::Null),
        (Some(rpc), false, _) => serde_json::to_value(mae_mcp::protocol::JsonRpcResponse::error(
            rpc.id,
            mae_mcp::protocol::McpError::kb_query_unavailable(
                "kb/query.* is disabled on this daemon (oauth.kb_query_enabled is false)"
                    .to_string(),
            ),
        ))
        .unwrap_or(serde_json::Value::Null),
        // No RPC body sent at all — the plain bearer-verification probe
        // case (ADR-052), never touched by the kb_query_enabled/doc_store
        // distinctions above since there's no request `id` to shape a
        // JSON-RPC error response around.
        (None, _, _) => probe_answer(principal, config),
    }
}

/// The bare probe's answer: who the token resolved to, and how they signed in.
fn probe_answer(
    principal: &ValidatedPrincipal,
    config: &ResourceServerConfig,
) -> serde_json::Value {
    serde_json::json!({
        "principal": principal.principal,
        "kind": principal.kind.as_str(),
        "resource": config.canonical_resource_uri,
        "sign_in": sign_in_report(principal, config),
    })
}

/// The bare probe's view of how the caller signed in (ADR-098): the claims as
/// read, and which configured value, if any, meets the second-factor rule.
/// `null` for a self-issued token, which is no sign-in.
fn sign_in_report(
    principal: &ValidatedPrincipal,
    config: &ResourceServerConfig,
) -> serde_json::Value {
    let Some(s) = principal.sign_in.as_ref() else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "amr": s.amr,
        "acr": s.acr,
        "auth_time": s.auth_time,
        "groups": s.groups,
        "second_factor": s.second_factor(&config.second_factor),
    })
}

/// What an unbound IdP subject gets from `kb/query.*` (ADR-098 Amendment 1,
/// C1): `my_kbs` answers an empty list, and every other method the refusal a
/// non-member gets.
fn unbound_answer(id: serde_json::Value, method: &str) -> serde_json::Value {
    let resp = if method == "kb/query.my_kbs" {
        mae_mcp::protocol::JsonRpcResponse::success(id, serde_json::json!({"kbs": []}))
    } else {
        mae_mcp::protocol::JsonRpcResponse::error(
            id,
            mae_mcp::protocol::McpError::not_member(
                "this identity-provider subject is not bound to a member key".to_string(),
            ),
        )
    };
    serde_json::to_value(&resp).unwrap_or(serde_json::Value::Null)
}

/// Runs the OAuth-protected HTTPS listener until the process shuts down.
/// Never called unless `OAuthConfig::enabled` is true (checked by the
/// caller) — this listener does not exist at all for the common case of a
/// solo/local-only daemon. `doc_store` is `Some` only when `collab.enabled`
/// (ADR-053/Phase G, #382) — `kb/query.*` has nothing to serve from
/// otherwise, regardless of `kb_query_enabled`; `handle_request` reports
/// this distinctly from "disabled" (see its `_` fallback below).
///
/// `limiter` bounds concurrent connections (ADR-054's `#342` failure class —
/// found missing here via an independent security review: every OTHER
/// listener in this daemon — collab TCP, KB Unix socket, P2P mesh — already
/// had this cap; this one, added later, didn't. Checked immediately after
/// `accept()`, before the TLS handshake, so a connection at capacity costs
/// nothing beyond the accept itself, matching the P2P listener's own
/// placement). The TLS handshake itself is wrapped in
/// `collab_handler::HANDSHAKE_TIMEOUT_SECS` (reused, not a new constant) —
/// without it, a client that opens the TCP connection and then stalls the
/// TLS handshake parks one task+socket per connection forever, the same
/// failure class just at an earlier step than the P2P mesh's own
/// post-handshake `accept_bi` timeout.
/// Everything a request on this listener can need, shared by every connection.
pub(crate) struct Listener {
    pub config: ResourceServerConfig,
    /// ADR-111 P1: the external-issuer path exists only when an issuer is
    /// configured. A self-issued-only deployment builds no JWKS client at all.
    pub jwks: Option<JwksCache>,
    pub doc_store: Option<Arc<DocStore>>,
    pub self_issue: Option<oauth_self_issue::SelfIssueConfig>,
    pub quota: Arc<dyn mae_daemon::quota::QuotaCharger>,
    /// The sync WebSocket (ADR-099): `Some` only when `ws_enabled` and its
    /// prerequisites hold (see `main.rs`).
    pub ws: Option<crate::ws::WsState>,
    /// ADR-098 D3: the binding table an external token resolves through.
    /// `None` (it could not be opened): every external token stays unbound.
    pub bindings: Option<Arc<mae_daemon::identity_binding::BindingStore>>,
    /// ADR-098 Amendment 1, C2: the proof-of-possession routes, `Some` only
    /// when `identity_binding_enabled` and the table is open.
    pub binding_routes: Option<crate::oauth_binding::BindingRoutes>,
}

impl Listener {
    /// Attach the binding table, and (when `routes` is `Some`) the routes that
    /// add to it by proof of possession.
    pub(crate) fn with_identity_binding(
        mut self,
        bindings: Option<Arc<mae_daemon::identity_binding::BindingStore>>,
        routes: Option<crate::oauth_binding::BindingRoutes>,
    ) -> Self {
        self.binding_routes = routes.filter(|_| bindings.is_some());
        self.bindings = bindings;
        self
    }

    pub(crate) fn new(
        config: ResourceServerConfig,
        doc_store: Option<Arc<DocStore>>,
        self_issue: Option<oauth_self_issue::SelfIssueConfig>,
        quota: Arc<dyn mae_daemon::quota::QuotaCharger>,
        ws: Option<crate::ws::WsState>,
    ) -> Result<Self, String> {
        let jwks = match config.jwks_url.is_empty() {
            true => None,
            false => Some(JwksCache::new(
                config.jwks_url.clone(),
                config.jwks_ca_file.as_deref(),
            )?),
        };
        Ok(Listener {
            config,
            jwks,
            doc_store,
            self_issue,
            quota,
            ws,
            bindings: None,
            binding_routes: None,
        })
    }
}

pub async fn run_oauth_listener(
    state: Listener,
    bind: std::net::SocketAddr,
    (cert_path, key_path): (&Path, &Path),
    limiter: ConnLimiter,
) -> std::io::Result<()> {
    let tls_config = load_tls_config(cert_path, key_path).map_err(std::io::Error::other)?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, resource = %state.config.canonical_resource_uri, "OAuth HTTPS listener started");
    let state = Arc::new(state);

    loop {
        let (tcp_stream, peer_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!(error = %e, "OAuth listener accept failed");
                continue;
            }
        };
        mae_mcp::tune_request_stream(&tcp_stream);
        let Some(guard) = limiter.try_acquire() else {
            tracing::warn!(
                active = limiter.current(),
                %peer_addr,
                "OAuth listener: connection cap reached, rejecting new connection"
            );
            continue;
        };
        let acceptor = acceptor.clone();
        let state = Arc::clone(&state);

        tokio::spawn(async move {
            // Shared, not owned: an upgraded WebSocket outlives this task and
            // keeps the connection's ONE slot until it closes (ADR-099 D2 —
            // N documents over one socket are one connection).
            let guard = Arc::new(guard);
            let tls_stream = match tokio::time::timeout(
                Duration::from_secs(collab_handler::HANDSHAKE_TIMEOUT_SECS),
                acceptor.accept(tcp_stream),
            )
            .await
            {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    tracing::debug!(error = %e, %peer_addr, "TLS handshake failed");
                    return;
                }
                Err(_) => {
                    tracing::debug!(
                        %peer_addr,
                        timeout_secs = collab_handler::HANDSHAKE_TIMEOUT_SECS,
                        "TLS handshake timed out"
                    );
                    return;
                }
            };
            let io = hyper_util::rt::TokioIo::new(tls_stream);
            let service = hyper::service::service_fn(move |req| {
                handle_request(req, Arc::clone(&state), Arc::clone(&guard))
            });
            if let Err(e) =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                    .serve_connection_with_upgrades(io, service)
                    .await
            {
                tracing::debug!(error = %e, %peer_addr, "connection error");
            }
        });
    }
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod oauth_tests;

#[cfg(test)]
#[path = "oauth_jwks_ca_tests.rs"]
mod oauth_jwks_ca_tests;
