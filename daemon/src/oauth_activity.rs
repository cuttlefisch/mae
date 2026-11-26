//! The activity record on the HTTPS listener (ADR-120 P1a).
//!
//! **One event per request**, built in an [`Scope`] and committed by
//! [`recorded`] before the response leaves (D5). Inner layers fill the scope
//! as they decide: authentication sets the principal and the session
//! ([`established`]), the JSON-RPC dispatch its method and targets ([`rpc`],
//! [`rpc_answer`]), the site its page ([`site_target`]). The outer layer
//! classes the outcome from what the caller is about to receive.
//!
//! The scope is a task-local: a handler running outside one (a test calling a
//! handler directly, a WebSocket session renewing its token) records nothing,
//! and every function here is then a no-op.
//!
//! @ai-caution: [security] `Mae-Activity-Session` is set on responses and NEVER
//! read from a request. It is an identifier, not a credential
//! (`no_entry_point_reads_the_session_header`). With `[activity] enabled`
//! off, `listener.activity` is `None` and nothing here runs: the listener
//! behaves exactly as it did before the record existed.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use mae_daemon::activity::event::{
    bound_node_ids, truncate, ActorKind, Event, EventKind, OutcomeResult, Peer, Principal,
    Transport, Unverified, CLAIM_MAX_BYTES,
};
use mae_daemon::activity::store::Entry;
use mae_daemon::activity::writer::{ActivityRecord, InFlight};
use mae_daemon::activity::{now_us, session};
use mae_daemon::oauth_self_issue::{Credential, PrincipalKind, ValidatedPrincipal};
use serde_json::{json, Value};

use crate::conn_limit::ConnGuard;
use crate::oauth::{self, Listener, ResponseBody, Route};

/// The peer's address, put on each request by the accept loop.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PeerAddr(pub SocketAddr);

/// The connection's limit on recorded requests in flight
/// ([`crate::oauth::MAX_REQUESTS_PER_CONNECTION`]), put on each request by
/// [`crate::oauth::serve_connection`]. A request without it (served outside a
/// listener connection) has no per-connection limit.
#[derive(Clone)]
pub(crate) struct ConnRequests(pub crate::conn_limit::ConnLimiter);

/// The refusal kind a response builder chose, carried to the record on the
/// response's extensions (never on the wire).
#[derive(Debug, Clone)]
pub(crate) struct RefusalKind(pub String);

tokio::task_local! {
    static SCOPE: Arc<Scope>;
}

/// What a request's event is built from, filled as the request is decided.
#[derive(Debug, Default)]
struct Draft {
    principal: Option<Principal>,
    tenant: Option<String>,
    session: Option<String>,
    session_detail: Value,
    idp_sid: Option<String>,
    sign_in: Option<(String, Value)>,
    exchange: Option<Value>,
    route: Option<String>,
    kind: Option<EventKind>,
    kb_id: Option<String>,
    node_ids: Option<Vec<String>>,
    /// D2: how many ids there were, when more than `node_ids` holds.
    node_ids_total: Option<u64>,
    revision: Option<String>,
    blob: Option<String>,
    /// A JSON-RPC refusal answered inside a `200`.
    rpc_outcome: Option<(OutcomeResult, String)>,
}

pub(crate) struct Scope {
    record: Arc<ActivityRecord>,
    draft: Mutex<Draft>,
}

fn with_draft(f: impl FnOnce(&mut Draft, &ActivityRecord)) {
    let _ = SCOPE.try_with(|s| {
        let mut d = s.draft.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut d, &s.record)
    });
}

/// `principal.kind` for a validated bearer token (D2). ADR-118's agent tokens
/// do not exist yet (their fields are P5's), so `agent` is not produced here:
/// an operator-minted self-issued token and a bound IdP subject not flagged
/// `person` are `service`.
pub(crate) fn actor_kind(vp: &ValidatedPrincipal) -> ActorKind {
    match (vp.credential, vp.kind) {
        (Credential::IdpUnbound, _) => ActorKind::Unbound,
        (Credential::IdpBound, PrincipalKind::Person) => ActorKind::Person,
        (Credential::IdpBound, PrincipalKind::Agent) | (Credential::SelfIssued, _) => {
            ActorKind::Service
        }
    }
}

fn principal_of(id: &str, kind: ActorKind, iss: &str, sub: &str) -> Principal {
    let opt = |s: &str| (!s.is_empty()).then(|| truncate(s, CLAIM_MAX_BYTES));
    Principal {
        id: opt(id),
        kind,
        iss: opt(iss),
        sub: opt(sub),
        on_behalf_of: None,
        grant: None,
    }
}

/// A bearer token validated as `vp` (D4): its principal, its session id, and
/// the sign-in it may be the first request of.
pub(crate) fn established(vp: &ValidatedPrincipal, token: &str, listener: &Listener) {
    with_draft(|d, record| {
        d.principal = Some(principal_of(
            &vp.principal,
            actor_kind(vp),
            &vp.issuer,
            &vp.subject,
        ));
        d.tenant = listener.quota.tenant_of(&vp.principal);
        d.session = Some(record.session_key().bearer_id(token));
        d.session_detail = json!({
            "credential": "bearer",
            "expires_at": mae_daemon::rfc3339::utc(vp.expires_at as i64),
        });
        if let Some(s) = &vp.sign_in {
            d.idp_sid = s.sid.as_ref().map(|s| truncate(s, CLAIM_MAX_BYTES));
            let key = record
                .session_key()
                .sign_in_key(&vp.issuer, &vp.subject, s.auth_time);
            d.sign_in = Some((
                key,
                json!({"amr": s.amr, "acr": s.acr, "auth_time": s.auth_time}),
            ));
        }
    });
}

/// A WebSocket's connection opens (D4): its own session id, which its later
/// events use (P1b), and which the upgrade's response carries.
pub(crate) fn connection_opened() -> Option<String> {
    let mut id = None;
    with_draft(|d, _| {
        let fresh = session::random_id();
        d.session = Some(fresh.clone());
        d.session_detail = json!({"credential": "connection"});
        id = Some(fresh);
    });
    id
}

/// A ticket redeemed at the `/ws` upgrade (D1: a sign-in the daemon performs
/// itself), and the principal it carried.
pub(crate) fn ticket_redeemed(principal: &str, issuer: &str, subject: &str, kind: ActorKind) {
    with_draft(|d, _| {
        if d.principal.is_none() {
            d.principal = Some(principal_of(principal, kind, issuer, subject));
        }
        d.exchange = Some(json!({"method": "ws_ticket"}));
    });
}

/// A binding proof accepted (D1: a sign-in the daemon performs itself).
pub(crate) fn binding_proved() {
    with_draft(|d, _| d.exchange = Some(json!({"method": "binding_proof"})));
}

/// The KB, nodes and blob a route touched.
pub(crate) fn target(kb_id: &str, node_ids: Option<Vec<String>>, blob: Option<&str>) {
    with_draft(|d, _| {
        d.kb_id = Some(truncate(kb_id, CLAIM_MAX_BYTES));
        if let Some(ids) = node_ids {
            d.set_node_ids(ids);
        }
        if let Some(b) = blob {
            d.blob = Some(truncate(b, CLAIM_MAX_BYTES));
        }
    });
}

impl Draft {
    /// D2: `node_ids` bounded in length and in number, with the true number
    /// beside it past the cap.
    fn set_node_ids(&mut self, ids: Vec<String>) {
        let (kept, total) = bound_node_ids(ids);
        self.node_ids = Some(kept);
        self.node_ids_total = total;
    }
}

/// The site's page or listing (D1: `index.json` and `explorer.json` expose
/// the whole KB's listing, `["*"]`; a tag lists the pages it showed).
pub(crate) fn site_target(
    kb_id: &str,
    view: &mae_daemon::site::View,
    page: &mae_daemon::site::SitePage,
) {
    use mae_daemon::site::{SitePage, View};
    let prefix = format!("/kb/{kb_id}/");
    let ids = match (view, page) {
        (_, SitePage::NotFound | SitePage::TooLarge) => None,
        (View::Index | View::Explorer, _) => Some(vec!["*".to_string()]),
        (View::Page(id), _) | (View::Home(Some(id)), _) => Some(vec![id.clone()]),
        (_, SitePage::Listing { items, .. }) if matches!(view, View::Tag(_)) => Some(
            items
                .iter()
                .map(|(_, href)| href.strip_prefix(&prefix).unwrap_or(href).to_string())
                .collect(),
        ),
        _ => None,
    };
    target(kb_id, ids, None);
}

/// The JSON-RPC method about to be dispatched, and what its params name.
pub(crate) fn rpc(method: &str, params: &Value) {
    with_draft(|d, _| {
        let (route, kind) = match classify_method(method) {
            Some(k) => (method.to_string(), k),
            None => ("unknown".to_string(), EventKind::Request),
        };
        d.route = Some(route);
        d.kind = Some(kind);
        let s = |k: &str| {
            params
                .get(k)
                .and_then(Value::as_str)
                .map(|v| truncate(v, CLAIM_MAX_BYTES))
        };
        d.kb_id = s("kb_id");
        d.revision = s("revision");
        d.node_ids = s("node_id").map(|n| vec![n]);
        d.node_ids_total = None;
    });
}

/// The JSON-RPC answer: its outcome, and the nodes a read returned.
pub(crate) fn rpc_answer(answer: &Value) {
    with_draft(|d, _| {
        if let Some(err) = answer.get("error") {
            let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
            let kind = err
                .pointer("/data/kind")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| rpc_code_kind(code));
            let result = match kind.as_str() {
                "internal" => OutcomeResult::Error,
                _ => OutcomeResult::Refused,
            };
            d.rpc_outcome = Some((result, kind));
            return;
        }
        if let Some(result) = answer.get("result") {
            let returned = returned_node_ids(result);
            if !returned.is_empty() {
                let mut ids = d.node_ids.take().unwrap_or_default();
                ids.extend(returned);
                d.set_node_ids(ids);
            }
        }
    });
}

/// The node ids a `kb/query.*` result lists, under the keys its methods use
/// (`results`, `nodes`, `pairs`, `links_from`, `links_to`, `root`), in order
/// and with repeats: [`bound_node_ids`] drops those and caps the number (D2).
fn returned_node_ids(result: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| out.push(s.to_string());
    if let Some(r) = result.get("root").and_then(Value::as_str) {
        push(r);
    }
    for key in ["results", "nodes", "pairs", "links_from", "links_to"] {
        for item in result
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = match item {
                Value::String(s) => Some(s.as_str()),
                Value::Array(a) => a.first().and_then(Value::as_str),
                Value::Object(o) => o
                    .get("id")
                    .or_else(|| o.get("node_id"))
                    .or_else(|| o.get("to"))
                    .and_then(Value::as_str),
                _ => None,
            };
            if let Some(id) = id {
                push(id);
            }
        }
    }
    out
}

fn rpc_code_kind(code: i64) -> String {
    match code {
        -32700 => "parse_error".into(),
        -32600 => "invalid_request".into(),
        -32601 => "method_not_found".into(),
        -32602 => "invalid_params".into(),
        -32603 => "internal".into(),
        c => format!("code_{c}"),
    }
}

/// Every JSON-RPC method the listener dispatches, classed (D1). `None` for a
/// method no dispatcher serves. The completeness test walks the dispatchers'
/// own lists (`kb_query::METHODS`, `review::METHODS`) through this.
pub(crate) fn classify_method(method: &str) -> Option<EventKind> {
    let known = mae_daemon::kb_query::METHODS.contains(&method)
        || mae_daemon::review::METHODS.contains(&method);
    if !known {
        return None;
    }
    Some(match method {
        "kb/query.capabilities" | "kb/query.my_kbs" | "kb/query.health" => EventKind::Request,
        "kb/review.request"
        | "kb/review.withdraw"
        | "kb/review.act"
        | "kb/review.approve"
        | "kb/review.reject"
        | "kb/review.enter_into_force"
        | "kb/review.derogate" => EventKind::ReviewAct,
        "kb/review.set_policy" | "kb/review.set_code" => EventKind::Admin,
        // Everything else returns KB content, a key, or the review record.
        _ => EventKind::Read,
    })
}

/// A route's template and event kind (D2: never the raw path), or `None` for
/// the public surface, which is not recorded (D1).
pub(crate) fn class_of(
    route: Route,
    req: &Request<Incoming>,
    listener: &Listener,
) -> Option<(String, EventKind)> {
    use mae_daemon::site::View;
    let m = method_name(req.method());
    let path = req.uri().path();
    Some(match route {
        Route::Prm | Route::Health | Route::SiteAsset => return None,
        Route::Blob => match crate::oauth_blobs::blob_path(path) {
            Some((_, None)) => (format!("{m} /kb/{{kb}}/blobs"), EventKind::Upload),
            _ => (
                format!("{m} /kb/{{kb}}/blobs/{{sha256}}"),
                EventKind::Download,
            ),
        },
        // A CORS preflight carries no credential and no content.
        Route::Ws if path == "/ws/ticket" && req.method() == hyper::Method::OPTIONS => return None,
        Route::Ws if path == "/ws/ticket" => (format!("{m} /ws/ticket"), EventKind::Request),
        Route::Ws => (format!("{m} /ws"), EventKind::Request),
        Route::Binding if path == crate::oauth_binding::BIND_PATH => (
            format!("{m} {}", crate::oauth_binding::BIND_PATH),
            EventKind::Admin,
        ),
        Route::Binding => (
            format!("{m} {}", crate::oauth_binding::CHALLENGE_PATH),
            EventKind::Request,
        ),
        Route::Authenticated => match listener
            .config
            .site_enabled
            .then(|| oauth::site_request(path))
            .flatten()
        {
            Some(Some((_, view))) => {
                let t = match view {
                    View::Page(_) => "/kb/{kb}/{id}",
                    View::Tags => "/kb/{kb}/tags",
                    View::Tag(_) => "/kb/{kb}/tags/{tag}",
                    View::Index => "/kb/{kb}/index.json",
                    View::Explorer => "/kb/{kb}/explorer.json",
                    View::Home(_) => "/kb/{kb}/",
                    View::Site(_) => "/kb/{kb}/site/{name}",
                };
                (format!("{m} {t}"), EventKind::Read)
            }
            Some(None) => (format!("{m} /kb/{{kb}}/{{other}}"), EventKind::Read),
            // JSON-RPC refines this once it has read the body (`rpc`); a body
            // that is no call is the bare bearer probe.
            None => (format!("{m} *"), EventKind::Request),
        },
    })
}

/// The HTTP method as recorded: a standard one by name, any other as `OTHER`
/// (an extension method is text the client chose).
fn method_name(m: &hyper::Method) -> &'static str {
    use hyper::Method as M;
    match *m {
        M::GET => "GET",
        M::HEAD => "HEAD",
        M::POST => "POST",
        M::PUT => "PUT",
        M::DELETE => "DELETE",
        M::OPTIONS => "OPTIONS",
        M::PATCH => "PATCH",
        M::TRACE => "TRACE",
        M::CONNECT => "CONNECT",
        _ => "OTHER",
    }
}

/// Run `route` for `req` inside a scope, then commit its event before the
/// response leaves. If the record cannot take it, the caller gets `503
/// activity_record_unavailable` and nothing of the prepared response.
///
/// @ai-caution: [security] The dispatch and the commit run in their own task,
/// which this request only awaits. hyper drops a service future when its
/// client hangs up, and an effect already handed to another thread (a blob
/// commit on a blocking thread) then lands with no event at all
/// (`a_client_hanging_up_mid_upload_leaves_the_effect_and_its_event`). The
/// task owns everything it uses and holds, until its commit is done:
/// - the connection's slot, so orphaned requests stay bounded by the
///   connection cap;
/// - one of the connection's [`ConnRequests`], so a client resetting HTTP/2
///   streams cannot pile them up on one connection
///   (`resetting_streams_never_holds_more_than_the_connection_limit`);
/// - an [`InFlight`] ticket, which a clean stop waits for before it writes
///   `daemon_stop` (`a_stop_waits_for_a_request_in_flight_to_record_its_event`).
///
/// Its commit is bounded by `commit_timeout_ms` like any other.
pub(crate) async fn recorded(
    req: Request<Incoming>,
    listener: Arc<Listener>,
    conn: Arc<ConnGuard>,
    record: Arc<ActivityRecord>,
    route: Route,
    class: (String, EventKind),
) -> Response<ResponseBody> {
    let timeout_ms = record.settings().commit_timeout_ms;
    let cors = crate::ws::cors_origin(&req, &listener);
    // A clean stop has begun: nothing new is taken, and the refusal is counted.
    let Some(in_flight) = record.begin() else {
        let now = now_us();
        record.shed_refusal(&refused_on_arrival(&req, &record, &class, now), now);
        return oauth::boxed(unavailable(timeout_ms, cors));
    };
    let slot = match req.extensions().get::<ConnRequests>() {
        Some(limiter) => match limiter.0.try_acquire() {
            Some(slot) => Some(slot),
            None => {
                over_connection_limit(&req, &record, &class);
                return oauth::boxed(too_many_on_connection(cors));
            }
        },
        None => None,
    };
    let held = (in_flight, slot);
    let work = recorded_detached(
        req,
        listener,
        conn,
        record,
        route,
        class,
        cors.clone(),
        held,
    );
    let task = tokio::spawn(work);
    match task.await {
        Ok(resp) => resp,
        // A panic is the connection's, as it was before the task existed.
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        // The runtime is shutting down: nothing was committed for it.
        Err(_) => oauth::boxed(unavailable(timeout_ms, cors)),
    }
}

/// A request refused at its connection's limit (D6): counted into its kind's
/// aggregate, never dropped, and answered at once.
fn over_connection_limit(
    req: &Request<Incoming>,
    record: &ActivityRecord,
    class: &(String, EventKind),
) {
    let now = now_us();
    let mut ev = refused_on_arrival(req, record, class, now);
    ev.outcome_kind = Some(TOO_MANY_ON_CONNECTION.to_string());
    record.coalescer().aggregate(&ev, now);
}

/// The event of a request refused as it arrives, before any dispatch: its
/// route and peer, and no principal (nothing was authenticated).
fn refused_on_arrival(
    req: &Request<Incoming>,
    record: &ActivityRecord,
    (template, kind): &(String, EventKind),
    now: i64,
) -> Event {
    let mut ev = Event::new(*kind, now, Transport::Https);
    ev.route = Some(template.clone());
    ev.peer = peer_of(req, record.settings());
    ev.result = OutcomeResult::Refused;
    ev
}

/// The refusal kind of a request past its connection's limit.
pub(crate) const TOO_MANY_ON_CONNECTION: &str = "too_many_requests_on_connection";

/// `429` for a request past its connection's limit, with the route's CORS
/// header (as [`unavailable`] carries it).
fn too_many_on_connection(cors: Option<hyper::header::HeaderValue>) -> Response<Full<Bytes>> {
    let limit = crate::oauth::MAX_REQUESTS_PER_CONNECTION;
    let mut resp = oauth::json_response(
        StatusCode::TOO_MANY_REQUESTS,
        json!({
            "error": "too_many_requests",
            "kind": TOO_MANY_ON_CONNECTION,
            "limit": limit,
            "error_description": format!(
                "this connection already has {limit} requests in flight"
            ),
        }),
    );
    resp.headers_mut().insert(
        hyper::header::RETRY_AFTER,
        hyper::header::HeaderValue::from_static("1"),
    );
    crate::ws::with_cors(&mut resp, cors);
    resp
}

/// [`recorded`]'s work, in a task of its own. `_held` (the request's
/// [`InFlight`] ticket and its connection slot) is released when it ends.
#[allow(clippy::too_many_arguments)]
async fn recorded_detached(
    req: Request<Incoming>,
    listener: Arc<Listener>,
    conn: Arc<ConnGuard>,
    record: Arc<ActivityRecord>,
    route: Route,
    (template, kind): (String, EventKind),
    cors: Option<hyper::header::HeaderValue>,
    _held: (InFlight, Option<crate::conn_limit::ConnGuard>),
) -> Response<ResponseBody> {
    // Held to the end of the commit, past the dispatch that consumes `conn`.
    let _slot = Arc::clone(&conn);
    let at = now_us();
    let peer = peer_of(&req, record.settings());
    let bearer = oauth::extract_bearer_token(&req).map(str::to_string);
    let scope = Arc::new(Scope {
        record: Arc::clone(&record),
        draft: Mutex::new(Draft {
            route: Some(template),
            kind: Some(kind),
            ..Draft::default()
        }),
    });
    let resp = SCOPE
        .scope(
            Arc::clone(&scope),
            oauth::dispatch(route, req, listener, conn),
        )
        .await;
    let draft = std::mem::take(&mut *scope.draft.lock().unwrap_or_else(|e| e.into_inner()));
    let (result, outcome_kind) = outcome_of(&resp, &draft);
    let (entry, ev) = build_entry(draft, at, result, outcome_kind, peer, bearer.as_deref());
    let header = (resp.status() != StatusCode::UNAUTHORIZED)
        .then(|| ev.session.clone())
        .flatten()
        .filter(|_| ev.principal.id.is_some());
    let committed = if result == OutcomeResult::Refused {
        commit_refusal(&record, entry, &ev).await
    } else {
        record.commit(entry).await
    };
    match committed {
        Ok(()) => with_session_header(resp, header),
        Err(_) => oauth::boxed(unavailable(record.settings().commit_timeout_ms, cors)),
    }
}

/// The entry and event `fill` builds inside a scope on `record`, as
/// [`recorded`] builds them for an answered `200` (tests).
#[cfg(test)]
pub(crate) fn built_in_scope(record: Arc<ActivityRecord>, fill: impl FnOnce()) -> (Entry, Event) {
    let scope = Arc::new(Scope {
        record,
        draft: Mutex::new(Draft::default()),
    });
    SCOPE.sync_scope(Arc::clone(&scope), fill);
    let draft = std::mem::take(&mut *scope.draft.lock().unwrap_or_else(|e| e.into_inner()));
    build_entry(draft, now_us(), OutcomeResult::Ok, None, None, None)
}

/// A refusal is coalesced (D6): the first of its window commits before its
/// answer; the rest are counted and answered at once. Past half the queue, a
/// refusal made before authentication goes into its kind's aggregate. While
/// the record is unhealthy, a refusal is not queued at all: it is counted into
/// the aggregate and answered `503` (`ActivityRecord::shed_refusal`).
async fn commit_refusal(
    record: &Arc<ActivityRecord>,
    entry: Entry,
    ev: &Event,
) -> Result<(), mae_daemon::activity::writer::Unavailable> {
    let now = now_us();
    if !record.healthy() {
        record.shed_refusal(ev, now);
        return Err(mae_daemon::activity::writer::Unavailable);
    }
    let pre_auth = ev.principal.id.is_none();
    let half = record.settings().queue_capacity / 2;
    let aggregate = pre_auth && record.pre_auth_in_flight() >= half.max(1);
    let admit = record.coalescer().admit(ev, now, aggregate);
    // Kept for the sweep (and a clean stop's drain), not queued from a task of
    // its own that a stop would not wait for.
    record.coalescer().requeue(admit.flushed);
    if !admit.commit {
        return Ok(());
    }
    let committed = record.commit_as(entry, pre_auth).await;
    if committed.is_err() {
        // The window's first was never recorded: close it, or the refusals
        // after it would be answered with their real body during the outage.
        record.coalescer().abandon(ev, now);
    }
    committed
}

/// The outcome the caller is about to receive (D2): a JSON-RPC refusal's own
/// kind; otherwise the status, with the kind its builder chose.
fn outcome_of(resp: &Response<ResponseBody>, draft: &Draft) -> (OutcomeResult, Option<String>) {
    let status = resp.status();
    if status.is_success() || status.is_informational() || status.is_redirection() {
        if let Some((result, kind)) = &draft.rpc_outcome {
            return (*result, Some(kind.clone()));
        }
        return (OutcomeResult::Ok, None);
    }
    let kind = resp
        .extensions()
        .get::<RefusalKind>()
        .map(|k| k.0.clone())
        .unwrap_or_else(|| status_kind(status));
    let result = if status.is_server_error() {
        OutcomeResult::Error
    } else {
        OutcomeResult::Refused
    };
    (result, Some(kind))
}

/// `not_found`, `method_not_allowed`, … from the status's reason phrase.
pub(crate) fn status_kind(status: StatusCode) -> String {
    status
        .canonical_reason()
        .unwrap_or("http_error")
        .to_ascii_lowercase()
        .replace([' ', '-'], "_")
}

/// The request's event, and the entry that commits it with the sign-in and
/// session start it may open.
fn build_entry(
    d: Draft,
    at: i64,
    result: OutcomeResult,
    outcome_kind: Option<String>,
    peer: Option<Peer>,
    bearer: Option<&str>,
) -> (Entry, Event) {
    let mut ev = Event::new(d.kind.unwrap_or(EventKind::Request), at, Transport::Https);
    ev.principal = d.principal.clone().unwrap_or_else(Principal::anonymous);
    ev.session = d.session.clone();
    ev.idp_sid = d.idp_sid.clone();
    ev.route = d.route;
    ev.kb_id = d.kb_id;
    ev.node_ids = d.node_ids;
    ev.node_ids_total = d.node_ids_total;
    ev.revision = d.revision;
    ev.blob = d.blob;
    ev.result = result;
    ev.outcome_kind = outcome_kind;
    ev.peer = peer;
    ev.tenant = d.tenant.clone();
    if d.principal.is_none() {
        ev.unverified = bearer.and_then(unverified_claims);
    }
    let companion = |kind: EventKind, detail: Value| {
        let mut e = Event::new(kind, at, Transport::Https);
        e.principal = ev.principal.clone();
        e.session = ev.session.clone();
        e.idp_sid = ev.idp_sid.clone();
        e.peer = ev.peer.clone();
        e.tenant = ev.tenant.clone();
        e.detail = detail;
        e
    };
    let established = d.principal.is_some();
    let entry = Entry {
        sign_in: d
            .sign_in
            .filter(|_| established)
            .map(|(key, detail)| (key, companion(EventKind::SignIn, detail))),
        exchange: d
            .exchange
            .filter(|_| established && result == OutcomeResult::Ok)
            .map(|detail| companion(EventKind::SignIn, detail)),
        session_start: d
            .session
            .clone()
            .filter(|_| established)
            .map(|s| (s, companion(EventKind::SessionStart, d.session_detail))),
        events: vec![ev.clone()],
    };
    (entry, ev)
}

/// A refused token's claimed `iss` and `sub`, read without verifying it and
/// each truncated (D2's `unverified`). `None` for no readable claim.
fn unverified_claims(token: &str) -> Option<Unverified> {
    let payload = token.split('.').nth(1)?;
    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(payload.trim_end_matches('=').as_bytes())
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    let claim = |k: &str| {
        claims
            .get(k)
            .and_then(Value::as_str)
            .map(|s| truncate(s, CLAIM_MAX_BYTES))
    };
    let (iss, sub) = (claim("iss"), claim("sub"));
    (iss.is_some() || sub.is_some()).then_some(Unverified { iss, sub })
}

/// The peer's address, and what a trusted proxy says it forwarded (D2): an
/// untrusted peer's `X-Forwarded-For` is never recorded.
fn peer_of(
    req: &Request<Incoming>,
    settings: &mae_daemon::activity::ActivitySettings,
) -> Option<Peer> {
    let addr = req.extensions().get::<PeerAddr>()?.0;
    let forwarded = settings
        .trusted_proxies
        .contains(&addr.ip())
        .then(|| req.headers().get("x-forwarded-for"))
        .flatten()
        .and_then(|v| v.to_str().ok())
        .map(|v| truncate(v, CLAIM_MAX_BYTES));
    Some(Peer {
        addr: addr.to_string(),
        forwarded,
    })
}

fn with_session_header(
    mut resp: Response<ResponseBody>,
    id: Option<String>,
) -> Response<ResponseBody> {
    if let Some(v) = id.and_then(|id| hyper::header::HeaderValue::from_str(&id).ok()) {
        resp.headers_mut().insert(session::HEADER, v);
    }
    resp
}

/// D5's refusal: `503 activity_record_unavailable` with `Retry-After`, and
/// nothing of the answer that was prepared, but the CORS header the route's
/// own answers carry (`cors`, from [`crate::ws::cors_origin`]), so a browser
/// page can read why it was refused.
pub(crate) fn unavailable(
    commit_timeout_ms: u64,
    cors: Option<hyper::header::HeaderValue>,
) -> Response<Full<Bytes>> {
    let mut resp = oauth::kinded_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "temporarily_unavailable",
        mae_mcp::protocol::McpError::activity_record_unavailable(
            "the activity record could not take this request's event; nothing was answered"
                .to_string(),
        ),
    );
    let secs = commit_timeout_ms.div_ceil(1000).max(1);
    if let Ok(v) = hyper::header::HeaderValue::from_str(&secs.to_string()) {
        resp.headers_mut().insert(hyper::header::RETRY_AFTER, v);
    }
    crate::ws::with_cors(&mut resp, cors);
    resp
}

/// `GET /api/health` (D13): `503 {"status":"unavailable"}` while the record is
/// enabled and not taking events, `200 {"status":"ok"}` otherwise. Nothing
/// else is said.
pub(crate) fn health(listener: &Listener) -> Response<Full<Bytes>> {
    match listener.activity.as_ref().is_some_and(|r| !r.healthy()) {
        true => oauth::json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"status": "unavailable"}),
        ),
        false => oauth::json_response(StatusCode::OK, json!({"status": "ok"})),
    }
}
