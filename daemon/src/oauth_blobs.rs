//! ADR-115 D3: a KB's attachments on the HTTPS listener, beside JSON-RPC.
//!
//! | Request | Answer |
//! |---|---|
//! | `POST /kb/<kb-id>/blobs` with `Content-Length` (and optionally `Content-Digest: sha-256=:…:`) | `200 {sha256, size, media_type, width?, height?}` |
//! | `GET /kb/<kb-id>/blobs/<sha256>` | the blob, streamed |
//! | `HEAD /kb/<kb-id>/blobs/<sha256>` | its headers |
//!
//! Every request authenticates with a bearer token, is charged to its tenant,
//! and passes the KB's own gate ([`mae_daemon::blob_store::access`]) before
//! any blob is looked up: a caller who cannot read the KB gets one identical
//! `403 not_member` whatever exists. Only then does a reader learn
//! `404 blob_missing`.
//!
//! An upload is streamed to a temporary file under the cap, which a declared
//! `Content-Length` over it hits before a byte of the body is read; then
//! processed on a bounded worker (D3a) and committed under the tenant's
//! storage quota. A download streams the file in constant memory.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use mae_daemon::blob_store::access::{self, BlobOp, Denied};
use mae_daemon::blob_store::process::{ProcessError, ProcessLimits};
use mae_daemon::blob_store::upload::{self, ReceiveError, Receiving, UploadError};
use mae_daemon::blob_store::{valid_sha256, BlobError, BlobMeta, BlobStore};
use mae_daemon::oauth_self_issue::{Credential, ValidatedPrincipal};
use mae_mcp::protocol::McpError;

use crate::oauth::{boxed, kinded_response, Listener, ResponseBody};

/// Images processed at once. One: ADR-109's memory budget is per tenant
/// process, and one worker's bound (the upload, plus the decode's
/// `max_pixels × 8` byte limit) is already most of it at the defaults
/// (`the_attachment_defaults_fit_adr_109s_memory_budget`). Uploads beyond it
/// wait, bounded by the listener's connection cap.
pub(crate) const IMAGE_WORKERS: usize = 1;

/// How long one upload's processing may take before it is refused. A decode is
/// already bounded by its pixel, memory and frame limits; this answers the
/// client if one is still slow (the worker finishes, holding its permit).
/// From the benchmark: a 40-megapixel decode takes seconds, not tens of them.
pub(crate) const PROCESS_TIMEOUT: Duration = Duration::from_secs(30);

/// Frames an animated GIF, APNG or WebP may have (D3a's frame limit).
pub(crate) const MAX_FRAMES: u32 = 1000;

/// `Content-Security-Policy` on every blob: no script, no fetch, a unique
/// origin. D3's header, exactly.
pub(crate) const BLOB_CSP: &str = "sandbox; default-src 'none'";

/// `Cache-Control` on a blob: its address is its content, so it never changes.
pub(crate) const BLOB_CACHE: &str = "private, max-age=31536000, immutable";

/// What the routes count, for tests and the benchmark.
#[derive(Default)]
pub(crate) struct Counters {
    /// Request-body bytes read by uploads.
    pub body_bytes: AtomicU64,
    /// Uploads that reached a processing worker.
    pub processed: AtomicU64,
    /// Blob files opened by downloads.
    pub files_opened: AtomicU64,
}

/// The blob routes' state, on the listener when `attachments_enabled`.
pub(crate) struct BlobRoutes {
    pub store: Arc<BlobStore>,
    pub max_bytes: u64,
    pub limits: ProcessLimits,
    workers: Arc<tokio::sync::Semaphore>,
    pub counters: Arc<Counters>,
}

impl BlobRoutes {
    pub(crate) fn new(store: Arc<BlobStore>, max_bytes: u64, max_pixels: u64) -> Self {
        BlobRoutes {
            store,
            max_bytes,
            limits: ProcessLimits {
                max_pixels,
                max_frames: MAX_FRAMES,
            },
            workers: Arc::new(tokio::sync::Semaphore::new(IMAGE_WORKERS)),
            counters: Arc::default(),
        }
    }
}

/// `/kb/<kb-id>/blobs` (`None`) or `/kb/<kb-id>/blobs/<rest>` (`Some`): the
/// KB segment as written, unvalidated (the gate refuses a bad one like any
/// other unreadable KB). `None` for any other path.
pub(crate) fn blob_path(path: &str) -> Option<(&str, Option<&str>)> {
    let (kb, rest) = path.strip_prefix("/kb/")?.split_once('/')?;
    match rest.strip_prefix("blobs")? {
        "" => Some((kb, None)),
        r => r.strip_prefix('/').map(|blob| (kb, Some(blob))),
    }
}

/// Serve one blob request. `kb`/`blob` from [`blob_path`].
pub(crate) async fn handle(
    req: Request<Incoming>,
    listener: &Listener,
    routes: &BlobRoutes,
) -> Response<ResponseBody> {
    let Some((kb, blob)) =
        blob_path(req.uri().path()).map(|(k, b)| (k.to_string(), b.map(str::to_string)))
    else {
        return boxed(blob_missing());
    };
    let method = req.method().clone();
    let op = match (&method, blob.is_some()) {
        (&Method::POST, false) => BlobOp::Upload,
        (&Method::GET | &Method::HEAD, true) => BlobOp::Read,
        _ => return boxed(method_not_allowed(blob.is_some())),
    };
    let Some(token) = crate::oauth::extract_bearer_token(&req).map(str::to_string) else {
        return boxed(crate::oauth::unauthorized(
            &listener.config,
            "missing bearer token",
        ));
    };
    let principal = match crate::oauth::authenticate(&token, listener).await {
        Ok(p) => p,
        Err(resp) => return boxed(*resp),
    };
    let quota_method = match op {
        BlobOp::Upload => "kb/blobs.upload",
        BlobOp::Read => "kb/blobs.get",
    };
    let _lease = match listener
        .quota
        .charge(Some(&principal.principal), quota_method)
    {
        Ok(lease) => lease,
        Err(msg) => {
            return boxed(kinded_response(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                McpError::tenant_quota_exceeded(msg),
            ))
        }
    };
    if let Err(denied) = gate(listener, &kb, &principal, op).await {
        return boxed(refusal(denied));
    }
    match blob {
        None => upload(req, listener, routes, &kb, &principal).await,
        Some(sha) => download(&req, routes, &kb, &sha).await,
    }
}

async fn gate(
    listener: &Listener,
    kb: &str,
    principal: &ValidatedPrincipal,
    op: BlobOp,
) -> Result<(), Denied> {
    // An unbound IdP subject is a member of nothing (ADR-098 C1).
    if principal.credential == Credential::IdpUnbound {
        return Err(Denied::Hidden);
    }
    let Some(store) = listener.doc_store.as_ref() else {
        return Err(Denied::Hidden);
    };
    access::authorize(store, kb, &principal.principal, op).await
}

/// The one refusal for everyone the KB's gate does not admit (D2): the same
/// status, headers and body whatever the KB, the blob, or the reason.
pub(crate) fn hidden() -> Response<Full<Bytes>> {
    kinded_response(
        StatusCode::FORBIDDEN,
        "access_denied",
        McpError::not_member("not a reader of this KB".to_string()),
    )
}

fn refusal(denied: Denied) -> Response<Full<Bytes>> {
    match denied {
        Denied::Hidden => hidden(),
        Denied::RoleForbidden => kinded_response(
            StatusCode::FORBIDDEN,
            "access_denied",
            McpError::role_forbidden(
                "uploading needs the editor role".to_string(),
                Some("editor"),
            ),
        ),
        Denied::E2eUnsupported => kinded_response(
            StatusCode::CONFLICT,
            "conflict",
            McpError::e2e_unsupported(
                "an end-to-end encrypted KB cannot hold attachments yet (ADR-115 D8)".to_string(),
            ),
        ),
    }
}

fn method_not_allowed(blob: bool) -> Response<Full<Bytes>> {
    let mut r = crate::oauth::json_response(
        StatusCode::METHOD_NOT_ALLOWED,
        serde_json::json!({"error": "method_not_allowed"}),
    );
    let allow = if blob { "GET, HEAD" } else { "POST" };
    r.headers_mut().insert(
        hyper::header::ALLOW,
        hyper::header::HeaderValue::from_static(allow),
    );
    r
}

fn blob_missing() -> Response<Full<Bytes>> {
    kinded_response(
        StatusCode::NOT_FOUND,
        "not_found",
        McpError::blob_missing("this KB holds no blob at that address".to_string()),
    )
}

fn invalid(status: StatusCode, error: &str, param: &str, why: &str) -> Response<Full<Bytes>> {
    kinded_response(
        status,
        error,
        McpError::invalid_params(param, why.to_string()),
    )
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

/// The declared `Content-Length`; `None` if absent or not one number.
fn content_length(req: &Request<Incoming>) -> Option<u64> {
    let mut values = req.headers().get_all(hyper::header::CONTENT_LENGTH).iter();
    let v = values.next()?;
    if values.next().is_some() {
        return None;
    }
    v.to_str().ok()?.trim().parse().ok()
}

/// `Content-Digest`'s sha-256 member (RFC 9530): `Ok(None)` when absent or
/// naming only other algorithms, `Err` when malformed.
pub(crate) fn content_digest(value: Option<&str>) -> Result<Option<[u8; 32]>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    for member in value.split(',') {
        let Some((alg, rest)) = member.trim().split_once('=') else {
            return Err(());
        };
        if !alg.trim().eq_ignore_ascii_case("sha-256") {
            continue;
        }
        let b64 = rest
            .trim()
            .strip_prefix(':')
            .and_then(|r| r.strip_suffix(':'))
            .ok_or(())?;
        let raw = data_encoding::BASE64
            .decode(b64.as_bytes())
            .map_err(|_| ())?;
        return raw.try_into().map(Some).map_err(|_| ());
    }
    Ok(None)
}

async fn upload(
    req: Request<Incoming>,
    listener: &Listener,
    routes: &BlobRoutes,
    kb: &str,
    principal: &ValidatedPrincipal,
) -> Response<ResponseBody> {
    let Some(declared) = content_length(&req) else {
        return boxed(invalid(
            StatusCode::LENGTH_REQUIRED,
            "length_required",
            "Content-Length",
            "an upload must declare its Content-Length",
        ));
    };
    if declared > routes.max_bytes {
        return boxed(over_cap(declared, routes.max_bytes));
    }
    let digest = req
        .headers()
        .get("content-digest")
        .map(|v| v.to_str().map_err(|_| ()))
        .transpose()
        .and_then(content_digest);
    let Ok(digest) = digest else {
        return boxed(invalid(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Content-Digest",
            "Content-Digest is not an RFC 9530 dictionary with a base64 sha-256",
        ));
    };
    let received = match receive(req, routes, declared).await {
        Ok(r) => r,
        Err(resp) => return boxed(*resp),
    };
    if digest.is_some_and(|d| d != received.sha256) {
        return boxed(kinded_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            McpError::blob_hash_mismatch(
                "the bytes that arrived do not match Content-Digest".to_string(),
            ),
        ));
    }
    let quota = listener.quota.storage_quota(&principal.principal);
    let stored = process_and_commit(routes, kb, received, quota).await;
    boxed(match stored {
        Ok(meta) => crate::oauth::json_response(StatusCode::OK, describe(&meta)),
        Err(resp) => *resp,
    })
}

fn over_cap(size: u64, limit: u64) -> Response<Full<Bytes>> {
    let size = usize::try_from(size).unwrap_or(usize::MAX);
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    kinded_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        McpError::document_too_large(
            format!("an attachment may be at most {limit} bytes"),
            size,
            limit,
        ),
    )
}

/// Stream the body to a temporary file. The cap is checked on every chunk;
/// the declared length must be what arrived.
async fn receive(
    req: Request<Incoming>,
    routes: &BlobRoutes,
    declared: u64,
) -> Result<upload::Received, Box<Response<Full<Bytes>>>> {
    let internal = |e: String| {
        tracing::warn!(error = %e, "blob upload: cannot write the temporary file");
        Box::new(kinded_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            McpError::internal_fault("the upload could not be stored".to_string()),
        ))
    };
    let mut rx =
        Receiving::new(&routes.store, routes.max_bytes).map_err(|e| internal(e.to_string()))?;
    let mut body = req.into_body();
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            return Err(Box::new(short_body()));
        };
        let Ok(data) = frame.into_data() else {
            continue;
        };
        routes
            .counters
            .body_bytes
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        match rx.write(&data).await {
            Ok(()) => {}
            Err(ReceiveError::OverCap { cap }) => return Err(Box::new(over_cap(cap + 1, cap))),
            Err(ReceiveError::Io(e)) => return Err(internal(e)),
        }
    }
    if rx.len() != declared {
        return Err(Box::new(short_body()));
    }
    rx.finish().await.map_err(|e| internal(format!("{e:?}")))
}

fn short_body() -> Response<Full<Bytes>> {
    invalid(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "Content-Length",
        "the body did not match its Content-Length",
    )
}

async fn process_and_commit(
    routes: &BlobRoutes,
    kb: &str,
    received: upload::Received,
    quota: Option<mae_daemon::blob_store::StorageQuota>,
) -> Result<BlobMeta, Box<Response<Full<Bytes>>>> {
    let Ok(permit) = Arc::clone(&routes.workers).acquire_owned().await else {
        return Err(Box::new(internal_fault()));
    };
    routes.counters.processed.fetch_add(1, Ordering::Relaxed);
    let (store, kb_id, limits) = (Arc::clone(&routes.store), kb.to_string(), routes.limits);
    let work = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        upload::store_upload(&store, &kb_id, received, &limits, quota.as_ref())
    });
    let result = match tokio::time::timeout(PROCESS_TIMEOUT, work).await {
        Ok(Ok(r)) => r,
        Ok(Err(_panicked)) => Err(UploadError::Process(ProcessError::Undecodable(
            "the decoder failed".into(),
        ))),
        Err(_elapsed) => Err(UploadError::Process(ProcessError::Undecodable(
            "processing took too long".into(),
        ))),
    };
    result.map_err(|e| Box::new(upload_refusal(e, routes.limits.max_pixels)))
}

fn internal_fault() -> Response<Full<Bytes>> {
    kinded_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "server_error",
        McpError::internal_fault("the upload could not be stored".to_string()),
    )
}

fn upload_refusal(e: UploadError, max_pixels: u64) -> Response<Full<Bytes>> {
    match e {
        UploadError::Process(ProcessError::TooManyPixels(why)) => kinded_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            McpError::attachment_too_many_pixels(why, max_pixels),
        ),
        UploadError::Process(ProcessError::Undecodable(why)) => kinded_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unprocessable",
            McpError::attachment_undecodable(why),
        ),
        UploadError::Process(ProcessError::Unsupported(why)) => kinded_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            McpError::attachment_unsupported(why),
        ),
        UploadError::Store(BlobError::TenantFull { .. }) => kinded_response(
            StatusCode::INSUFFICIENT_STORAGE,
            "insufficient_storage",
            McpError::tenant_quota_exceeded(e_display(&e)),
        ),
        UploadError::Store(other) => {
            tracing::warn!(error = %other, "blob upload: commit failed");
            internal_fault()
        }
    }
}

fn e_display(e: &UploadError) -> String {
    match e {
        UploadError::Store(s) => s.to_string(),
        UploadError::Process(p) => format!("{p:?}"),
    }
}

/// The upload's answer: `{sha256, size, media_type, width?, height?}`.
fn describe(m: &BlobMeta) -> serde_json::Value {
    let mut v = serde_json::json!({
        "sha256": m.sha256,
        "size": m.size,
        "media_type": m.media_type.as_str(),
    });
    if let (Some(w), Some(h)) = (m.width, m.height) {
        v["width"] = w.into();
        v["height"] = h.into();
    }
    v
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

/// Whether `If-None-Match` names `etag` (or is `*`). Weak and strong forms
/// compare alike, as RFC 9110 has `If-None-Match` do.
pub(crate) fn none_match(header: Option<&str>, etag: &str) -> bool {
    header.is_some_and(|h| {
        h.split(',')
            .map(str::trim)
            .any(|t| t == "*" || t.strip_prefix("W/").unwrap_or(t) == etag)
    })
}

/// The headers every blob answer carries (D3).
fn blob_headers(
    builder: hyper::http::response::Builder,
    meta: &BlobMeta,
    etag: &str,
) -> hyper::http::response::Builder {
    let b = builder
        .header(hyper::header::CONTENT_TYPE, meta.media_type.as_str())
        .header("x-content-type-options", "nosniff")
        .header("content-security-policy", BLOB_CSP)
        .header(hyper::header::ETAG, etag)
        .header(hyper::header::CACHE_CONTROL, BLOB_CACHE);
    if meta.media_type.inline() {
        b
    } else {
        b.header(hyper::header::CONTENT_DISPOSITION, "attachment")
    }
}

async fn download(
    req: &Request<Incoming>,
    routes: &BlobRoutes,
    kb: &str,
    sha: &str,
) -> Response<ResponseBody> {
    if !valid_sha256(sha) {
        return boxed(blob_missing());
    }
    let store = Arc::clone(&routes.store);
    let (k, s) = (kb.to_string(), sha.to_string());
    let meta = match tokio::task::spawn_blocking(move || store.get(&k, &s)).await {
        Ok(Ok(Some(m))) => m,
        Ok(Ok(None)) => return boxed(blob_missing()),
        _ => return boxed(internal_fault()),
    };
    let etag = format!("\"{sha}\"");
    let inm = req
        .headers()
        .get(hyper::header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());
    if none_match(inm, &etag) {
        return boxed(
            blob_headers(
                Response::builder().status(StatusCode::NOT_MODIFIED),
                &meta,
                &etag,
            )
            .body(Full::new(Bytes::new()))
            .unwrap_or_else(|_| Response::new(Full::new(Bytes::new()))),
        );
    }
    let ok = blob_headers(Response::builder().status(StatusCode::OK), &meta, &etag)
        .header(hyper::header::CONTENT_LENGTH, meta.size);
    if req.method() == Method::HEAD {
        return boxed(
            ok.body(Full::new(Bytes::new()))
                .unwrap_or_else(|_| Response::new(Full::new(Bytes::new()))),
        );
    }
    routes.counters.files_opened.fetch_add(1, Ordering::Relaxed);
    let file = match tokio::fs::File::open(routes.store.blob_path(kb, sha)).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(kb, sha, error = %e, "a blob row names a file that cannot be opened");
            return boxed(blob_missing());
        }
    };
    let stream = futures_util::StreamExt::map(
        tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024),
        |chunk| chunk.map(Frame::data),
    );
    ok.body(StreamBody::new(stream).boxed())
        .unwrap_or_else(|_| boxed(internal_fault()))
}
