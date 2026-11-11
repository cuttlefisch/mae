//! `GET /ws` and `POST /ws/ticket`: everything decided BEFORE the upgrade
//! (ADR-099 Amendment 1, B1/B2/B6).
//!
//! No unauthenticated socket ever exists: credential, origin and version are
//! all settled here, and a refusal is an ordinary HTTP response built by the
//! listener's own helpers, so a bad credential gets the same `401` and
//! `WWW-Authenticate` as every other route (D3's inheritance argument).

use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{HeaderMap, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};

use super::connect_ticket::{TicketGrant, MAX_UNUSED_TICKETS_PER_PRINCIPAL, TICKET_TTL};
use super::{now_unix, WsState, PROTOCOL_V1, SUPPORTED_PROTOCOLS};
use crate::conn_limit::ConnGuard;
use crate::oauth::{self, json_response, unauthorized, with_no_store, Listener};
use mae_mcp::protocol::McpError;

type HttpResponse = Response<Full<Bytes>>;
/// A refusal, boxed: a response is large to return by value on every path.
type Refusal = Box<HttpResponse>;

/// The subprotocol-list prefix carrying a connect ticket (B1).
const TICKET_PREFIX: &str = "mae.ticket.";

/// RFC 6455 §1.3: appended to `Sec-WebSocket-Key` before hashing.
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Who a socket is, as its credential established it.
#[derive(Debug, Clone)]
pub(super) struct SessionAuth {
    pub issuer: String,
    pub subject: String,
    pub principal: String,
    /// The credential's expiry, unix seconds: the session's first deadline.
    pub deadline: u64,
}

/// Route one request to `/ws` or `/ws/ticket`. Only called when `ws_enabled`.
pub(crate) async fn handle(
    req: Request<Incoming>,
    listener: Arc<Listener>,
    conn: Arc<ConnGuard>,
) -> HttpResponse {
    let ws = listener
        .ws
        .as_ref()
        .expect("routed here only when ws is Some");
    if req.uri().path() == "/ws/ticket" {
        let mut resp = if req.method() == Method::OPTIONS {
            preflight(&req, ws)
        } else {
            mint_ticket(&req, &listener, ws).await
        };
        allow_origin(&mut resp, &req, ws);
        return resp;
    }
    upgrade(req, &listener, conn).await
}

/// Every `Sec-WebSocket-Protocol` entry the client offered, in order.
fn offered_subprotocols(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(hyper::header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

fn bad_request(error: &str, description: &str) -> HttpResponse {
    json_response(
        StatusCode::BAD_REQUEST,
        serde_json::json!({"error": error, "error_description": description}),
    )
}

/// A refusal in the listener's body convention, `{error, error_description}`,
/// plus the `kind` the [`McpError`] constructor names — so a kind string has
/// one home (`mae_mcp::protocol::ERROR_KINDS`) whatever the transport.
fn kinded(status: StatusCode, error: &str, refusal: McpError) -> HttpResponse {
    kinded_with(status, error, refusal, serde_json::Map::new())
}

/// [`kinded`], with further fields beside the three.
fn kinded_with(
    status: StatusCode,
    error: &str,
    refusal: McpError,
    mut body: serde_json::Map<String, serde_json::Value>,
) -> HttpResponse {
    let kind = refusal
        .data
        .as_deref()
        .and_then(|d| d.get("kind").cloned())
        .unwrap_or_default();
    body.insert("error".into(), error.into());
    body.insert("error_description".into(), refusal.message.into());
    body.insert("kind".into(), kind);
    json_response(status, serde_json::Value::Object(body))
}

/// A `401 invalid_token` with a kind, and the listener's `WWW-Authenticate`.
fn invalid_token(listener: &Listener, refusal: McpError) -> Refusal {
    let mut resp = kinded(StatusCode::UNAUTHORIZED, "invalid_token", refusal);
    resp.headers_mut().insert(
        hyper::header::WWW_AUTHENTICATE,
        oauth::www_authenticate_header(&listener.config)
            .parse()
            .expect("header value is a plain formatted string, always valid"),
    );
    Box::new(resp)
}

/// The one refusal for every ticket that cannot be spent (B1). Its bytes do
/// not depend on why: unknown, used, expired and misbound read the same.
fn ticket_invalid(listener: &Listener) -> Refusal {
    invalid_token(
        listener,
        McpError::ticket_invalid("the connect ticket is not valid".to_string()),
    )
}

fn issuer_not_supported(listener: &Listener) -> Refusal {
    invalid_token(
        listener,
        McpError::issuer_not_supported(
            "only this daemon's own tokens are accepted on this route (ADR-099 B6)".to_string(),
        ),
    )
}

/// Refusals that need no credential: a query string, a disallowed `Origin`.
/// `/ws` and `/ws/ticket` take no query parameters at all, so ANY query is
/// refused rather than a list of credential-looking names — a list is what
/// misses the next spelling.
fn refuse_url_or_origin(req: &Request<Incoming>, ws: &WsState) -> Option<HttpResponse> {
    if req.uri().query().is_some() {
        return Some(kinded(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            McpError::credential_in_url(
                "this route accepts no query string, which is written to access logs; present \
                 the credential as `Authorization: Bearer` or a `mae.ticket.<t>` subprotocol entry"
                    .to_string(),
            ),
        ));
    }
    match request_origin(req) {
        Ok(None) => None,
        Ok(Some(o)) if ws.settings.allowed_origins.iter().any(|a| a == &o) => None,
        _ => Some(kinded(
            StatusCode::FORBIDDEN,
            "access_denied",
            McpError::origin_not_allowed(
                "the request's Origin is not in oauth.ws_allowed_origins".to_string(),
            ),
        )),
    }
}

/// The request's `Origin`, if any. `Err` for one that is not valid UTF-8,
/// which no allowlist entry can match.
fn request_origin(req: &Request<Incoming>) -> Result<Option<String>, ()> {
    match req.headers().get(hyper::header::ORIGIN) {
        None => Ok(None),
        Some(v) => v.to_str().map(|s| Some(s.to_string())).map_err(|_| ()),
    }
}

/// Validate a bearer token under B6: only this daemon's own tokens.
async fn bearer_auth(token: &str, listener: &Listener) -> Result<SessionAuth, Refusal> {
    let kid = jsonwebtoken::decode_header(token).ok().and_then(|h| h.kid);
    if kid.as_deref() != Some(mae_daemon::oauth_self_issue::SELF_ISSUED_KID) {
        return Err(issuer_not_supported(listener));
    }
    let vp = oauth::authenticate(
        token,
        &listener.config,
        listener.jwks.as_ref(),
        listener.self_issue.as_ref(),
    )
    .await?;
    // The validator allows the JWT library's clock leeway; a session must not
    // open with its deadline already behind it.
    if vp.expires_at <= now_unix() {
        return Err(Box::new(unauthorized(&listener.config, "Expired")));
    }
    Ok(SessionAuth {
        issuer: vp.issuer,
        subject: vp.subject,
        principal: vp.principal,
        deadline: vp.expires_at,
    })
}

/// Turn a presented ticket into the session's identity. Unknown, used,
/// expired, minted for another origin, or outliving its token: all one
/// refusal, `ticket_invalid`.
fn ticket_auth(
    grant: Option<TicketGrant>,
    origin: &Option<String>,
    listener: &Listener,
) -> Result<SessionAuth, Refusal> {
    match grant {
        Some(g) if &g.origin == origin && g.token_expires_at > now_unix() => Ok(SessionAuth {
            issuer: g.issuer,
            subject: g.subject,
            principal: g.principal,
            deadline: g.token_expires_at,
        }),
        _ => Err(ticket_invalid(listener)),
    }
}

/// Exactly one credential: the bearer header, or one ticket (B1).
async fn credential(
    req: &Request<Incoming>,
    tickets: Vec<Option<TicketGrant>>,
    listener: &Listener,
) -> Result<SessionAuth, Refusal> {
    let bearer = oauth::extract_bearer_token(req);
    let auth = match (bearer, tickets.len()) {
        (None, 0) => Err(Box::new(unauthorized(
            &listener.config,
            "missing credential",
        ))),
        (Some(token), 0) => bearer_auth(token, listener).await,
        (None, 1) => {
            let origin = request_origin(req).unwrap_or(None);
            ticket_auth(tickets.into_iter().next().flatten(), &origin, listener)
        }
        _ => Err(Box::new(bad_request(
            "invalid_request",
            "present exactly one credential: `Authorization: Bearer`, or one \
             `mae.ticket.<t>` subprotocol entry",
        ))),
    }?;
    if auth.principal.is_empty() {
        return Err(Box::new(unauthorized(
            &listener.config,
            "MissingPrincipalClaim",
        )));
    }
    Ok(auth)
}

/// The `Sec-WebSocket-Key` of a well-formed RFC 6455 upgrade request.
fn upgrade_key(req: &Request<Incoming>) -> Option<String> {
    let h = req.headers();
    let has = |name: hyper::header::HeaderName, token: &str| {
        h.get_all(name)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(|t| t.trim().eq_ignore_ascii_case(token))
    };
    let version_13 = h
        .get(hyper::header::SEC_WEBSOCKET_VERSION)
        .is_some_and(|v| v.as_bytes() == b"13");
    let ok = req.method() == Method::GET
        && has(hyper::header::UPGRADE, "websocket")
        && has(hyper::header::CONNECTION, "upgrade")
        && version_13;
    ok.then(|| h.get(hyper::header::SEC_WEBSOCKET_KEY))
        .flatten()
        .and_then(|k| k.to_str().ok())
        .map(str::to_string)
}

/// RFC 6455 §4.2.2: `base64(SHA-1(key ++ GUID))`.
fn accept_key(key: &str) -> String {
    let digest = sha1_smol::Sha1::from(format!("{key}{WS_GUID}"))
        .digest()
        .bytes();
    data_encoding::BASE64.encode(&digest)
}

fn switching_protocols(key: &str) -> HttpResponse {
    with_no_store(
        Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header(hyper::header::UPGRADE, "websocket")
            .header(hyper::header::CONNECTION, "Upgrade")
            .header(hyper::header::SEC_WEBSOCKET_ACCEPT, accept_key(key))
            // B1: the response selects the version ONLY, never the ticket.
            .header(
                hyper::header::SEC_WEBSOCKET_PROTOCOL,
                HeaderValue::from_static(PROTOCOL_V1),
            ),
    )
    .body(Full::new(Bytes::new()))
    .expect("building a response from fixed parts never fails")
}

async fn upgrade(
    mut req: Request<Incoming>,
    listener: &Arc<Listener>,
    conn: Arc<ConnGuard>,
) -> HttpResponse {
    let ws = listener
        .ws
        .as_ref()
        .expect("routed here only when ws is Some");
    let offered = offered_subprotocols(req.headers());
    // B1: every presented ticket is consumed FIRST, so it is spent on its first
    // presentation whatever refuses the request below.
    let now = Instant::now();
    let tickets: Vec<Option<TicketGrant>> = offered
        .iter()
        .filter_map(|p| p.strip_prefix(TICKET_PREFIX))
        .map(|t| ws.tickets.redeem(t, now))
        .collect();
    if let Some(resp) = refuse_url_or_origin(&req, ws) {
        return resp;
    }
    let Some(key) = upgrade_key(&req) else {
        return bad_request(
            "invalid_request",
            "not a WebSocket upgrade request (RFC 6455)",
        );
    };
    let auth = match credential(&req, tickets, listener).await {
        Ok(a) => a,
        Err(resp) => return *resp,
    };
    if !offered.iter().any(|p| p == PROTOCOL_V1) {
        let mut supported = serde_json::Map::new();
        supported.insert("supported".into(), SUPPORTED_PROTOCOLS.into());
        return kinded_with(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            McpError::protocol_unsupported(
                "no protocol version in common; offer one of `supported` in \
                 Sec-WebSocket-Protocol"
                    .to_string(),
            ),
            supported,
        );
    }
    let on_upgrade = hyper::upgrade::on(&mut req);
    tokio::spawn(super::session::run(
        on_upgrade,
        auth,
        Arc::clone(listener),
        conn,
    ));
    switching_protocols(&key)
}

/// The request headers a cross-origin `POST /ws/ticket` may carry.
const CORS_ALLOWED_HEADERS: &[&str] = &["authorization", "content-type"];

/// CORS for `/ws/ticket`, so a browser page on an allowlisted origin can mint
/// a ticket (its `Authorization` header makes the POST preflighted). Only an
/// origin in `ws_allowed_origins` is ever named, exactly; no credentials mode
/// (MAE uses no cookies, the token travels in the header). A response to any
/// other origin carries no CORS header, so the browser withholds it.
fn allow_origin(resp: &mut HttpResponse, req: &Request<Incoming>, ws: &WsState) {
    let Ok(Some(origin)) = request_origin(req) else {
        return;
    };
    if !ws.settings.allowed_origins.iter().any(|a| a == &origin) {
        return;
    }
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&origin) {
        h.insert(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
    }
    h.append(hyper::header::VARY, HeaderValue::from_static("Origin"));
}

/// The CORS preflight for `POST /ws/ticket`: an allowlisted origin asking for
/// POST with at most `Authorization` and `Content-Type` gets `204`; anything
/// else is refused (and, from a disallowed origin, carries no CORS header).
fn preflight(req: &Request<Incoming>, ws: &WsState) -> HttpResponse {
    if let Some(resp) = refuse_url_or_origin(req, ws) {
        return resp;
    }
    let h = req.headers();
    let method_ok = h
        .get(hyper::header::ACCESS_CONTROL_REQUEST_METHOD)
        .is_some_and(|m| m.as_bytes() == b"POST");
    let headers_ok = h
        .get_all(hyper::header::ACCESS_CONTROL_REQUEST_HEADERS)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
        .all(|v| CORS_ALLOWED_HEADERS.contains(&v.as_str()));
    if h.get(hyper::header::ORIGIN).is_none() || !method_ok || !headers_ok {
        return bad_request(
            "invalid_request",
            "a preflight for POST /ws/ticket names an allowed Origin, POST, and at most \
             Authorization and Content-Type",
        );
    }
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header(hyper::header::ACCESS_CONTROL_ALLOW_METHODS, "POST")
        .header(
            hyper::header::ACCESS_CONTROL_ALLOW_HEADERS,
            "Authorization, Content-Type",
        )
        .header(hyper::header::ACCESS_CONTROL_MAX_AGE, "600")
        .body(Full::new(Bytes::new()))
        .expect("building a response from fixed parts never fails")
}

/// `POST /ws/ticket` (B1): bearer header in, `{ticket, expires_in}` out.
async fn mint_ticket(req: &Request<Incoming>, listener: &Listener, ws: &WsState) -> HttpResponse {
    if req.method() != Method::POST {
        return json_response(
            StatusCode::METHOD_NOT_ALLOWED,
            serde_json::json!({"error": "method_not_allowed", "error_description": "POST only"}),
        );
    }
    if let Some(resp) = refuse_url_or_origin(req, ws) {
        return resp;
    }
    let Some(token) = oauth::extract_bearer_token(req) else {
        return unauthorized(&listener.config, "missing bearer token");
    };
    let auth = match bearer_auth(token, listener).await {
        Ok(a) if !a.principal.is_empty() => a,
        Ok(_) => return unauthorized(&listener.config, "MissingPrincipalClaim"),
        Err(resp) => return *resp,
    };
    let grant = TicketGrant {
        issuer: auth.issuer,
        subject: auth.subject,
        principal: auth.principal,
        token_expires_at: auth.deadline,
        origin: request_origin(req).unwrap_or(None),
    };
    match ws.tickets.mint(grant, Instant::now()) {
        Ok(ticket) => json_response(
            StatusCode::OK,
            serde_json::json!({"ticket": ticket, "expires_in": TICKET_TTL.as_secs()}),
        ),
        Err(_) => {
            let mut limit = serde_json::Map::new();
            limit.insert("limit".into(), MAX_UNUSED_TICKETS_PER_PRINCIPAL.into());
            kinded_with(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                McpError::too_many_tickets(
                    "this principal holds its quota of unused tickets; use one or let it \
                     expire"
                        .to_string(),
                    MAX_UNUSED_TICKETS_PER_PRINCIPAL,
                ),
                limit,
            )
        }
    }
}

#[cfg(test)]
#[path = "upgrade_tests.rs"]
mod upgrade_tests;
