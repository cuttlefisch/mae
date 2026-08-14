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
    /// Which JWT claim becomes the mapped principal fed into `kb_access`
    /// (ADR-018). Config-driven, not hardcoded to `sub` — different
    /// authorization servers use different claim conventions.
    pub principal_claim: String,
    /// URL to fetch the JWKS from.
    pub jwks_url: String,
    /// The authorization server's issuer, checked against the token's `iss`
    /// claim. `None` skips issuer validation (not recommended, but some
    /// deployments' AS metadata omits a stable issuer during evaluation).
    pub issuer: Option<String>,
    /// ADR-053/Phase G (#382): whether `kb/query.*` is reachable at all —
    /// independently toggleable from the listener being up (see
    /// `config::OAuthConfig::kb_query_enabled`'s doc comment).
    pub kb_query_enabled: bool,
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
    /// ADR-073/Phase E (#547): whether `GET /kb/{kb_id}/view` (the live
    /// HTML KB view) is reachable at all. Independently toggleable from
    /// `kb_query_enabled` and the listener being up, mirroring that field's
    /// own doc comment exactly — a real capability is earned by an explicit
    /// operator opt-in (default `false`, principle #12), not implied by
    /// `kb_query_enabled` alone. Also requires a `DocStore` to exist (same
    /// `collab.enabled` prerequisite `kb_query_enabled` has).
    pub webview_enabled: bool,
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
    if let Some(ref issuer) = config.issuer {
        validation.set_issuer(std::slice::from_ref(issuer));
    }
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
        .get(&config.principal_claim)
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
    pub fn new(url: String) -> Self {
        JwksCache {
            url,
            client: reqwest::Client::new(),
            state: RwLock::new(None),
        }
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

use crate::conn_limit::ConnLimiter;

/// Load a PEM certificate chain + private key into a rustls server config.
/// Supports PKCS8 and PKCS1 (RSA) private keys — whichever `rustls-pemfile`
/// finds first in the key file, matching how most CAs/`certbot`/`mkcert`
/// output either shape.
fn load_tls_config(cert_path: &Path, key_path: &Path) -> Result<rustls::ServerConfig, String> {
    let cert_bytes =
        std::fs::read(cert_path).map_err(|e| format!("reading {}: {e}", cert_path.display()))?;
    let key_bytes =
        std::fs::read(key_path).map_err(|e| format!("reading {}: {e}", key_path.display()))?;

    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut cert_bytes.as_slice())
            .collect::<Result<_, _>>()
            .map_err(|e| format!("parsing cert chain: {e}"))?;
    if certs.is_empty() {
        return Err(format!("no certificates found in {}", cert_path.display()));
    }

    let key = rustls_pemfile::private_key(&mut key_bytes.as_slice())
        .map_err(|e| format!("parsing private key: {e}"))?
        .ok_or_else(|| format!("no private key found in {}", key_path.display()))?;

    // Explicit `ring` provider (matching shared/mcp/src/tls.rs's identical
    // pattern) rather than the ambiguous default builder: both `ring` (this
    // crate's own rustls feature) and `aws-lc-rs` (transitively, via
    // reqwest's rustls feature) are present in the dependency tree, so
    // rustls cannot auto-select a process-level default -- it hard-errors
    // rather than silently guessing, and `builder_with_provider` sidesteps
    // needing a global `CryptoProvider::install_default()` call at all.
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("configuring TLS protocol versions: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("building TLS server config: {e}"))
}

/// Extract a bearer token from an `Authorization: Bearer <token>` header.
fn extract_bearer_token(req: &Request<Incoming>) -> Option<&str> {
    req.headers()
        .get(hyper::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Extract a bearer token for `GET /kb/{kb_id}/view` specifically: header
/// first (unchanged precedence), falling back to an `?access_token=`
/// query-string parameter (RFC 6750 §2.3's URI-query bearer convention) ONLY
/// for this route. A plain browser address-bar navigation cannot set a
/// custom `Authorization` header at all — this is the sole practical way a
/// human can open a shared link and have the page itself authenticate,
/// mirroring how e.g. Grafana/Datadog shared-snapshot links work. Every
/// OTHER route on this listener (including the `kb/query.*` JSON-RPC POST
/// endpoint this page's own polling JS calls) stays header-only —
/// `extract_bearer_token` is untouched and this function is never consulted
/// for them. No percent-decoding is performed: a compact JWT's charset
/// (`[A-Za-z0-9._-]`, RFC 7515) never requires it.
fn extract_view_bearer_token(req: &Request<Incoming>) -> Option<String> {
    if let Some(t) = extract_bearer_token(req) {
        return Some(t.to_string());
    }
    let query = req.uri().query()?;
    for pair in query.split('&') {
        // `continue`, not `?` -- a malformed pair (no `=`) must be skipped,
        // never abort scanning the rest of the query string for a valid
        // `access_token` that could appear later.
        let Some((key, val)) = pair.split_once('=') else {
            continue;
        };
        if key == "access_token" && !val.is_empty() {
            return Some(val.to_string());
        }
    }
    None
}

/// Apply the no-caching headers every response from this listener needs.
///
/// @ai-caution: [security] EVERY response builder in this module must go
/// through here (audit #588.3). This listener serves bearer-token-authenticated
/// KB content, and the webview response embeds a live access token directly in
/// its HTML — without `no-store` a browser writes that token to its on-disk
/// cache and an intermediary proxy may retain the KB content. `no-store` is
/// mandated for token-bearing responses by RFC 6749 §5.1, which OAuth 2.1
/// carries forward; `Pragma: no-cache` covers HTTP/1.0 intermediaries.
fn with_no_store(builder: hyper::http::response::Builder) -> hyper::http::response::Builder {
    builder
        .header(hyper::header::CACHE_CONTROL, "no-store")
        .header(hyper::header::PRAGMA, "no-cache")
}

fn json_response(status: StatusCode, body: serde_json::Value) -> Response<Full<Bytes>> {
    with_no_store(
        Response::builder()
            .status(status)
            .header(hyper::header::CONTENT_TYPE, "application/json"),
    )
    .body(Full::new(Bytes::from(body.to_string())))
    .expect("building a response from a fixed status/body never fails")
}

fn unauthorized(config: &ResourceServerConfig, reason: &str) -> Response<Full<Bytes>> {
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

/// Per-request handler: serves the PRM document unauthenticated, gates
/// everything else on a valid bearer token. Once a token validates: if the
/// request carries a parseable JSON-RPC body AND `kb_query_enabled` AND a
/// `DocStore` is available, dispatch it as a `kb/query.*` call
/// (ADR-053/Phase G); otherwise fall back to the plain diagnostic response
/// (ADR-052) — keeps `mae-daemon doctor`-style bare bearer-verification
/// working unchanged for callers that never send a body.
async fn handle_request(
    req: Request<Incoming>,
    config: Arc<ResourceServerConfig>,
    jwks: Arc<JwksCache>,
    doc_store: Option<Arc<DocStore>>,
    self_issue: Arc<Option<oauth_self_issue::SelfIssueConfig>>,
    quota: Arc<dyn mae_daemon::quota::QuotaCharger>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    if req.uri().path() == "/.well-known/oauth-protected-resource" {
        return Ok(json_response(
            StatusCode::OK,
            protected_resource_metadata(&config),
        ));
    }

    // ADR-073/Phase E (#547): only ever `Some` when `webview_enabled` AND the
    // path genuinely matches `/kb/{kb_id}/view` — every other path (still
    // the overwhelming majority) takes the exact pre-existing codepath below
    // unchanged, including header-only bearer extraction.
    let view_kb_id = if config.webview_enabled {
        crate::webview::parse_view_path(req.uri().path()).map(|s| s.to_string())
    } else {
        None
    };

    let token = if view_kb_id.is_some() {
        extract_view_bearer_token(&req)
    } else {
        extract_bearer_token(&req).map(|s| s.to_string())
    };
    let Some(token) = token else {
        return Ok(unauthorized(&config, "missing bearer token"));
    };

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
    let self_issue_ctx = self_issue.as_ref().as_ref().filter(|_| {
        jsonwebtoken::decode_header(&token)
            .ok()
            .and_then(|h| h.kid)
            .as_deref()
            == Some(oauth_self_issue::SELF_ISSUED_KID)
    });

    let principal = if let Some(si) = self_issue_ctx {
        let daemon_pubkey = si.identity.public().to_bytes();
        match oauth_self_issue::validate_self_issued_token(&token, &daemon_pubkey, &si.audience) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(?e, "self-issued bearer token rejected");
                return Ok(unauthorized(&config, &format!("{e:?}")));
            }
        }
    } else {
        let keys = match jwks.get().await {
            Ok(keys) => keys,
            Err(e) => {
                tracing::warn!(error = %e, "failed to fetch JWKS");
                return Ok(json_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    serde_json::json!({"error": "temporarily_unavailable"}),
                ));
            }
        };

        match validate_bearer_token(&token, &keys, &config) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(?e, "bearer token rejected");
                return Ok(unauthorized(&config, &format!("{e:?}")));
            }
        }
    };

    if let Some(kb_id) = view_kb_id {
        return Ok(render_webview_response(
            &kb_id,
            &token,
            &config,
            doc_store.as_ref(),
            &principal,
        )
        .await);
    }

    // Read the body (never done before this phase) to see if this is a
    // kb/query.* JSON-RPC call. An empty/unparseable body is not an error —
    // it's exactly what a bare bearer-verification probe sends. The size
    // limit is enforced by `Limited` DURING the read (errors mid-stream once
    // the budget is exceeded), not after collecting into memory — an
    // authenticated caller cannot force unbounded server-side buffering by
    // sending an oversized body, regardless of `kb_query_enabled`.
    let limited_body = http_body_util::Limited::new(req.into_body(), config.max_request_body_bytes);
    let body_bytes = match http_body_util::BodyExt::collect(limited_body).await {
        Ok(collected) => collected.to_bytes(),
        Err(e)
            if e.downcast_ref::<http_body_util::LengthLimitError>()
                .is_some() =>
        {
            tracing::debug!(
                limit = config.max_request_body_bytes,
                "request body exceeded size limit"
            );
            return Ok(json_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                serde_json::json!({
                    "error": "payload_too_large",
                    "error_description": format!(
                        "request body exceeds the {}-byte limit",
                        config.max_request_body_bytes
                    ),
                }),
            ));
        }
        Err(e) => {
            tracing::debug!(error = %e, "failed to read request body");
            Bytes::new()
        }
    };

    let rpc_request: Option<mae_mcp::protocol::JsonRpcRequest> = if body_bytes.is_empty() {
        None
    } else {
        serde_json::from_slice(&body_bytes).ok()
    };

    let body = route_authenticated_request(
        rpc_request,
        &config,
        doc_store.as_ref(),
        &principal,
        quota.as_ref(),
    )
    .await;
    Ok(json_response(StatusCode::OK, body))
}

/// Build the response for a validated `GET /kb/{kb_id}/view` request
/// (ADR-073/Phase E, #547) — split out for the same unit-testability reason
/// as `route_authenticated_request` (no real HTTP connection needed to
/// construct these already-parsed pieces).
///
/// Gated by `kb/query.capabilities` FIRST (the same Read-access check every
/// other `kb/query.*` method already goes through) so a principal without
/// access to `kb_id` gets a real, immediate error here — never a page that
/// renders successfully and only fails on its first background poll. This
/// is a genuinely new consumer of the existing gated surface (ADR-073 D3),
/// not a new access path: the page itself carries zero KB content, only the
/// `kb_id`/token needed for the client's OWN subsequent `kb/query.*` calls.
pub(crate) async fn render_webview_response(
    kb_id: &str,
    token: &str,
    config: &ResourceServerConfig,
    doc_store: Option<&Arc<DocStore>>,
    principal: &ValidatedPrincipal,
) -> Response<Full<Bytes>> {
    let Some(store) = doc_store else {
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({
                "error": "kb webview is enabled but no DocStore is available on this daemon \
                          (collab.enabled is false)"
            }),
        );
    };

    let limits = mae_daemon::kb_query::KbQueryLimits {
        max_body_bytes: config.kb_query_max_body_bytes,
        max_scan_nodes: config.kb_query_max_scan_nodes,
        max_search_results: config.kb_query_max_search_results,
    };
    let params = serde_json::json!({"kb_id": kb_id});
    if let Err(e) = mae_daemon::kb_query::dispatch(
        "kb/query.capabilities",
        &params,
        store,
        Some(&principal.principal),
        limits,
    )
    .await
    {
        return json_response(
            StatusCode::FORBIDDEN,
            serde_json::json!({"error": "access_denied", "error_description": e.message}),
        );
    }

    let html = crate::webview::render_page(kb_id, token);
    // This page embeds `token` verbatim — it is the single most important
    // response on this listener to keep out of any cache (see `with_no_store`).
    with_no_store(
        Response::builder()
            .status(StatusCode::OK)
            .header(hyper::header::CONTENT_TYPE, "text/html; charset=utf-8"),
    )
    .body(Full::new(Bytes::from(html)))
    .expect("building a response from a fixed status/body never fails")
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
            mae_mcp::protocol::McpError::internal_error(
                "kb/query.* is enabled but no DocStore is available on this daemon \
                 (collab.enabled is false)"
                    .to_string(),
            ),
        ))
        .unwrap_or(serde_json::Value::Null),
        (Some(rpc), false, _) => serde_json::to_value(mae_mcp::protocol::JsonRpcResponse::error(
            rpc.id,
            mae_mcp::protocol::McpError::internal_error(
                "kb/query.* is disabled on this daemon (oauth.kb_query_enabled is false)"
                    .to_string(),
            ),
        ))
        .unwrap_or(serde_json::Value::Null),
        // No RPC body sent at all — the plain bearer-verification probe
        // case (ADR-052), never touched by the kb_query_enabled/doc_store
        // distinctions above since there's no request `id` to shape a
        // JSON-RPC error response around.
        (None, _, _) => {
            serde_json::json!({"principal": principal.principal, "resource": config.canonical_resource_uri})
        }
    }
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
#[allow(clippy::too_many_arguments)]
pub async fn run_oauth_listener(
    server_config: ResourceServerConfig,
    bind: std::net::SocketAddr,
    cert_path: &Path,
    key_path: &Path,
    doc_store: Option<Arc<DocStore>>,
    limiter: ConnLimiter,
    self_issue: Option<oauth_self_issue::SelfIssueConfig>,
    quota: Arc<dyn mae_daemon::quota::QuotaCharger>,
) -> std::io::Result<()> {
    let tls_config = load_tls_config(cert_path, key_path).map_err(std::io::Error::other)?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, resource = %server_config.canonical_resource_uri, "OAuth HTTPS listener started");

    let config = Arc::new(server_config);
    let jwks = Arc::new(JwksCache::new(config.jwks_url.clone()));
    let self_issue = Arc::new(self_issue);

    loop {
        let (tcp_stream, peer_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!(error = %e, "OAuth listener accept failed");
                continue;
            }
        };
        let Some(guard) = limiter.try_acquire() else {
            tracing::warn!(
                active = limiter.current(),
                %peer_addr,
                "OAuth listener: connection cap reached, rejecting new connection"
            );
            continue;
        };
        let acceptor = acceptor.clone();
        let config = config.clone();
        let jwks = jwks.clone();
        let doc_store = doc_store.clone();
        let self_issue = self_issue.clone();
        let quota = quota.clone();

        tokio::spawn(async move {
            let _guard = guard;
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
                handle_request(
                    req,
                    config.clone(),
                    jwks.clone(),
                    doc_store.clone(),
                    self_issue.clone(),
                    quota.clone(),
                )
            });
            if let Err(e) =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                    .serve_connection(io, service)
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
