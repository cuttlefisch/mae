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

/// Why a bearer token was rejected. Deliberately specific (not a single
/// opaque "invalid") so callers can log/test the exact failure mode —
/// several of ADR-052's required adversarial tests assert on these variants
/// directly, not just "it failed."
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
use mae_daemon::doc_store::DocStore;

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

fn json_response(status: StatusCode, body: serde_json::Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
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
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    if req.uri().path() == "/.well-known/oauth-protected-resource" {
        return Ok(json_response(
            StatusCode::OK,
            protected_resource_metadata(&config),
        ));
    }

    let Some(token) = extract_bearer_token(&req) else {
        return Ok(unauthorized(&config, "missing bearer token"));
    };

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

    let principal = match validate_bearer_token(token, &keys, &config) {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(?e, "bearer token rejected");
            return Ok(unauthorized(&config, &format!("{e:?}")));
        }
    };

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

    let body =
        route_authenticated_request(rpc_request, &config, doc_store.as_ref(), &principal).await;
    Ok(json_response(StatusCode::OK, body))
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
) -> serde_json::Value {
    match (rpc_request, config.kb_query_enabled, doc_store) {
        (Some(rpc), true, Some(store)) => {
            let limits = crate::kb_query::KbQueryLimits {
                max_body_bytes: config.kb_query_max_body_bytes,
                max_scan_nodes: config.kb_query_max_scan_nodes,
                max_search_results: config.kb_query_max_search_results,
            };
            let params = rpc.params.unwrap_or(serde_json::Value::Null);
            let rpc_response = match crate::kb_query::dispatch(
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
pub async fn run_oauth_listener(
    server_config: ResourceServerConfig,
    bind: std::net::SocketAddr,
    cert_path: &Path,
    key_path: &Path,
    doc_store: Option<Arc<DocStore>>,
) -> std::io::Result<()> {
    let tls_config = load_tls_config(cert_path, key_path).map_err(std::io::Error::other)?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, resource = %server_config.canonical_resource_uri, "OAuth HTTPS listener started");

    let config = Arc::new(server_config);
    let jwks = Arc::new(JwksCache::new(config.jwks_url.clone()));

    loop {
        let (tcp_stream, peer_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!(error = %e, "OAuth listener accept failed");
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let config = config.clone();
        let jwks = jwks.clone();
        let doc_store = doc_store.clone();

        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(tcp_stream).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(error = %e, %peer_addr, "TLS handshake failed");
                    return;
                }
            };
            let io = hyper_util::rt::TokioIo::new(tls_stream);
            let service = hyper::service::service_fn(move |req| {
                handle_request(req, config.clone(), jwks.clone(), doc_store.clone())
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
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::traits::PublicKeyParts;
    use rsa::RsaPrivateKey;

    const TEST_KID: &str = "test-key-1";
    const TEST_RESOURCE: &str = "https://mae.example.com/mcp";

    /// Generates a fresh RSA keypair and returns (PEM-encoded private key
    /// for signing test tokens, the JWK this module's validator consumes).
    /// Fresh per test (never a hardcoded/shared key -- CLAUDE.md principle
    /// #14's "real inputs, not unicorn values") so no test accidentally
    /// depends on key material another test also uses.
    fn generate_test_key() -> (String, JwkOwned) {
        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("RSA keygen");
        let pem = private_key
            .to_pkcs1_pem(rsa::pkcs8::LineEnding::LF)
            .expect("PEM encode")
            .to_string();
        let public_key = private_key.to_public_key();
        let n = base64_url(&public_key.n().to_bytes_be());
        let e = base64_url(&public_key.e().to_bytes_be());
        (
            pem,
            JwkOwned {
                kid: TEST_KID.to_string(),
                n,
                e,
            },
        )
    }

    fn base64_url(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    fn sign_token(private_key_pem: &str, claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_string());
        let encoding_key =
            EncodingKey::from_rsa_pem(private_key_pem.as_bytes()).expect("valid PEM");
        encode(&header, claims, &encoding_key).expect("sign")
    }

    fn base_config() -> ResourceServerConfig {
        ResourceServerConfig {
            canonical_resource_uri: TEST_RESOURCE.to_string(),
            principal_claim: "sub".to_string(),
            jwks_url: "https://unused-in-these-tests.example.com/jwks".to_string(),
            issuer: Some("https://idp.example.com".to_string()),
            kb_query_enabled: false,
            max_request_body_bytes: 1_048_576,
            kb_query_max_body_bytes: 65_536,
            kb_query_max_scan_nodes: 500,
            kb_query_max_search_results: 20,
        }
    }

    fn valid_claims(now: u64) -> serde_json::Value {
        serde_json::json!({
            "sub": "alice@example.com",
            "aud": TEST_RESOURCE,
            "iss": "https://idp.example.com",
            "iat": now,
            "exp": now + 3600,
        })
    }

    fn now_unix() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn valid_token_is_accepted_and_principal_mapped() {
        let (pem, jwk) = generate_test_key();
        let token = sign_token(&pem, &valid_claims(now_unix()));
        let result = validate_bearer_token(&token, &[jwk], &base_config());
        let principal = result.expect("valid token must be accepted");
        assert_eq!(principal.principal, "alice@example.com");
        assert_eq!(principal.audience, vec![TEST_RESOURCE.to_string()]);
    }

    /// Adversarial (required by ADR-052): wrong-audience token rejected --
    /// including the confused-deputy case of a validly-signed token minted
    /// for a genuinely DIFFERENT resource server.
    #[test]
    fn token_for_a_different_resource_is_rejected() {
        let (pem, jwk) = generate_test_key();
        let mut claims = valid_claims(now_unix());
        claims["aud"] =
            serde_json::json!("https://a-completely-different-mcp-server.example.com/mcp");
        let token = sign_token(&pem, &claims);
        let result = validate_bearer_token(&token, &[jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::WrongAudience));
    }

    /// Adversarial (required): expired token rejected.
    #[test]
    fn expired_token_is_rejected() {
        let (pem, jwk) = generate_test_key();
        let now = now_unix();
        let mut claims = valid_claims(now);
        claims["exp"] = serde_json::json!(now.saturating_sub(3600));
        claims["iat"] = serde_json::json!(now.saturating_sub(7200));
        let token = sign_token(&pem, &claims);
        let result = validate_bearer_token(&token, &[jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::Expired));
    }

    /// Adversarial (required): a tampered/forged signature is rejected --
    /// the token is signed by a DIFFERENT key than the one in the server's
    /// JWKS (simulating either a forgery attempt or a rotated-out key,
    /// this module's stateless equivalent of "revoked").
    #[test]
    fn token_signed_by_an_unknown_key_is_rejected() {
        let (attacker_pem, _attacker_jwk) = generate_test_key();
        let (_server_pem, server_jwk) = generate_test_key();
        // Attacker signs with their own key but claims the SERVER's kid,
        // attempting to pass off a forged token as legitimately signed.
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(server_jwk.kid.clone());
        let encoding_key = EncodingKey::from_rsa_pem(attacker_pem.as_bytes()).expect("valid PEM");
        let token =
            jsonwebtoken::encode(&header, &valid_claims(now_unix()), &encoding_key).expect("sign");

        let result = validate_bearer_token(&token, &[server_jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::InvalidSignature));
    }

    #[test]
    fn token_with_kid_absent_from_jwks_is_rejected() {
        let (pem, mut jwk) = generate_test_key();
        let token = sign_token(&pem, &valid_claims(now_unix()));
        jwk.kid = "a-different-kid-than-the-token-used".to_string();
        let result = validate_bearer_token(&token, &[jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::UnknownKey));
    }

    #[test]
    fn wrong_issuer_is_rejected() {
        let (pem, jwk) = generate_test_key();
        let mut claims = valid_claims(now_unix());
        claims["iss"] = serde_json::json!("https://a-different-idp.example.com");
        let token = sign_token(&pem, &claims);
        let result = validate_bearer_token(&token, &[jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::WrongIssuer));
    }

    #[test]
    fn malformed_token_is_rejected_not_panicking() {
        let (_, jwk) = generate_test_key();
        let result = validate_bearer_token("not.a.jwt", &[jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::Malformed));
    }

    #[test]
    fn missing_principal_claim_is_rejected() {
        let (pem, jwk) = generate_test_key();
        let mut claims = valid_claims(now_unix());
        claims.as_object_mut().unwrap().remove("sub");
        let token = sign_token(&pem, &claims);
        let result = validate_bearer_token(&token, &[jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::MissingPrincipalClaim));
    }

    #[test]
    fn protected_resource_metadata_names_the_configured_authorization_server() {
        let metadata = protected_resource_metadata(&base_config());
        assert_eq!(metadata["resource"], TEST_RESOURCE);
        assert_eq!(
            metadata["authorization_servers"][0],
            "https://idp.example.com"
        );
    }

    // --- Request body size limiting (QA-pass finding, principle #15) ---
    //
    // These exercise the exact same `http_body_util::Limited` +
    // `BodyExt::collect` + `LengthLimitError` downcast triple `handle_request`
    // uses, against a concrete `Full` body -- `Incoming` (the real hyper
    // connection body type) can't be constructed outside a live connection,
    // so this is the faithful unit-level proof; a real over-the-wire 413
    // round trip is covered separately by the OAuth/kb-query e2e suite.

    #[tokio::test]
    async fn a_request_body_over_the_configured_limit_is_rejected_before_full_buffering() {
        use http_body_util::{BodyExt, Full, Limited};
        let oversized = Full::new(Bytes::from(vec![b'x'; 200]));
        let limited = Limited::new(oversized, 100);

        let result = BodyExt::collect(limited).await;

        let err = result.expect_err("a body exceeding the limit must error, never fully buffer");
        assert!(
            err.downcast_ref::<http_body_util::LengthLimitError>()
                .is_some(),
            "expected a LengthLimitError specifically, got: {err}"
        );
    }

    #[tokio::test]
    async fn a_request_body_within_the_configured_limit_is_accepted_unchanged() {
        use http_body_util::{BodyExt, Full, Limited};
        let payload = vec![b'x'; 50];
        let body = Full::new(Bytes::from(payload.clone()));
        let limited = Limited::new(body, 100);

        let result = BodyExt::collect(limited).await;

        let collected = result.expect("a body within the limit must be read successfully");
        assert_eq!(collected.to_bytes().as_ref(), payload.as_slice());
    }

    #[test]
    fn www_authenticate_header_points_at_the_prm_document() {
        let header = www_authenticate_header(&base_config());
        assert!(header.contains("Bearer"));
        assert!(header.contains(&format!(
            "{}/.well-known/oauth-protected-resource",
            TEST_RESOURCE
        )));
    }
}
