//! Publication review (ADR-113), PR-4 of its build sequence: the revision
//! record, the request, and the event chain.
//!
//! - [`store`]: the tables (D1), hub-local in `state.db`.
//! - [`chain`]: the event chain's format, export and offline verify (D7, E10).
//! - `methods` / `reads`: `kb/review.request`, `.withdraw`, `.set_policy`,
//!   `.set_code`, `.status`, `.pending`, `.events`, and `kb/query.revisions`
//!   and `kb/query.get` with `revision`.
//!
//! Served on the OAuth listener (E9) to any bearer principal the KB admits;
//! the acts that need a person (approve and the rest) are PR-5's. With a KB's
//! review switch off, every `kb/review.*` method but `set_policy` refuses
//! `review_not_enabled`, and readers see exactly what they saw before.
//!
//! **The gate, in E9's order**, the same for every method: a caller the KB
//! does not admit, and a KB that does not exist, get one identical
//! `not_member`; then an E2E KB gets `e2e_unsupported`; then review off gets
//! `review_not_enabled`.
//!
//! @ai-caution: [security] Owner checks read the role from the same lookup
//! `kb_access` decides on (`member_role`), never a string compare against the
//! collection's owner field. The import act (A4, E4) is an operator CLI and
//! must never get a method here (principle #16).

mod acts;
mod attach;
pub(crate) mod attach_store;
pub mod chain;
pub mod import;
mod import_check;
mod methods;
pub mod published;
mod reads;
pub mod store;

pub use reads::export_lines;

#[cfg(test)]
mod tests;

use mae_mcp::protocol::McpError;
use mae_sync::kb::{KbCollectionDoc, Role as SyncRole, Transport};
use serde_json::Value;

use crate::collab_handler::{self, AccessDecision};
use crate::doc_store::DocStore;
use crate::kb_query::KbQueryLimits;
use crate::oauth_self_issue::ValidatedPrincipal;
use crate::storage::StorageError;

/// Every method [`dispatch`] serves. The activity record classes each one
/// (ADR-120 D1), and a test holds this list to the dispatcher's own arms.
pub const METHODS: &[&str] = &[
    "kb/review.request",
    "kb/review.withdraw",
    "kb/review.act",
    "kb/review.approve",
    "kb/review.reject",
    "kb/review.enter_into_force",
    "kb/review.derogate",
    "kb/review.attach_rendered_pdf",
    "kb/review.attach_signed_pdf",
    "kb/review.annul_attachment",
    "kb/review.set_policy",
    "kb/review.set_code",
    "kb/review.status",
    "kb/review.pending",
    "kb/review.events",
];

/// Whether `method` is one this module serves on the OAuth listener.
pub fn is_review_method(method: &str) -> bool {
    method.starts_with("kb/review.")
}

/// Dispatch one `kb/review.*` method for the caller `who`.
pub async fn dispatch(
    method: &str,
    params: &Value,
    store: &DocStore,
    who: &ValidatedPrincipal,
    limits: KbQueryLimits,
) -> Result<Value, McpError> {
    let kb_id = required_str(params, "kb_id")?.to_string();
    match method {
        "kb/review.request" => methods::request(store, &kb_id, who, params, limits).await,
        "kb/review.withdraw" => methods::withdraw(store, &kb_id, who, params).await,
        "kb/review.act" => acts::act(store, &kb_id, who, params).await,
        "kb/review.approve" => acts::approve(store, &kb_id, who, params).await,
        "kb/review.reject" => acts::reject(store, &kb_id, who, params).await,
        "kb/review.enter_into_force" => acts::enter_into_force(store, &kb_id, who, params).await,
        "kb/review.derogate" => acts::derogate(store, &kb_id, who, params).await,
        "kb/review.attach_rendered_pdf" => {
            attach::attach_rendered_pdf(store, &kb_id, who, params).await
        }
        "kb/review.attach_signed_pdf" => {
            attach::attach_signed_pdf(store, &kb_id, who, params).await
        }
        "kb/review.annul_attachment" => attach::annul_attachment(store, &kb_id, who, params).await,
        "kb/review.set_policy" => methods::set_policy(store, &kb_id, who, params).await,
        "kb/review.set_code" => methods::set_code(store, &kb_id, who, params).await,
        "kb/review.status" => reads::status(store, &kb_id, who, params).await,
        "kb/review.pending" => reads::pending(store, &kb_id, who, params, limits).await,
        "kb/review.events" => reads::events(store, &kb_id, who, params, limits).await,
        other => Err(McpError::method_not_found(format!(
            "unknown kb/review method '{other}'"
        ))),
    }
}

/// `kb/query.revisions {kb_id, node_id}` (D5): a node's revisions with their
/// dates. A read of the KB; not gated on the review switch, since revisions
/// are a record that outlives it.
pub async fn query_revisions(
    store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    params: &Value,
) -> Result<Value, McpError> {
    reads::revisions(store, kb_id, principal, params).await
}

/// `kb/query.get` with `revision` (D5, E9): that revision's frozen content
/// and record, whatever its state.
pub async fn query_get_revision(
    store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    params: &Value,
    max_body_bytes: usize,
) -> Result<Value, McpError> {
    reads::get_revision(store, kb_id, principal, params, max_body_bytes).await
}

/// What the gate established about the caller and the KB.
pub(crate) struct Gated {
    pub coll: KbCollectionDoc,
    pub role: SyncRole,
    pub policy: store::Policy,
}

/// The one refusal for a caller the KB does not admit, and for a KB that does
/// not exist: the same kind and the same message, so the answer is no oracle.
fn not_admitted() -> McpError {
    McpError::not_member("not a member of this KB".into())
}

/// A storage failure as the client sees it: a fixed message. The cause is
/// logged here, never returned (it is the daemon's, not the caller's).
pub(crate) fn storage_fault(e: StorageError) -> McpError {
    tracing::warn!(error = %e, "publication review: storage failure");
    McpError::internal_fault("publication review could not read or write its record".into())
}

/// Most members one `kb/review.request` may name (E9). Each member is frozen
/// for good: a history version no retention prunes (ADR-116 D5), a source
/// copy that is never deleted (O8, E7), and an entry in the request's event,
/// so one call's work and storage must be bounded by more than the scan
/// limit. Fifty covers a procedure family revised together; a larger set is
/// several requests. Never above `[kb_query] max_scan_nodes`.
pub(crate) const MAX_REQUEST_NODES: usize = 50;
/// Largest `reason` any review method takes, in UTF-8 bytes (E9). A reason is
/// kept for good in the event chain; this is a long paragraph.
pub(crate) const MAX_REASON_BYTES: usize = 2_048;
/// Largest `set_name`, in UTF-8 bytes (E9): a title, as long as a version name.
pub(crate) const MAX_SET_NAME_BYTES: usize = 200;
/// Most `code_prefixes` a KB declares (E9). Each is kept in the policy and in
/// its `policy_changed` event.
pub(crate) const MAX_CODE_PREFIXES: usize = 32;

/// `invalid_params` for `param` over its cap, with the cap in `data.limit`.
pub(crate) fn over_limit(param: &str, message: String, limit: usize) -> McpError {
    let mut e = McpError::invalid_params(param, message);
    if let Some(d) = e.data.as_deref_mut() {
        d["limit"] = serde_json::json!(limit);
    }
    e
}

/// Refuse a text parameter longer than `max` UTF-8 bytes.
pub(crate) fn within_bytes(name: &str, s: &str, max: usize) -> Result<(), McpError> {
    if s.len() > max {
        return Err(over_limit(
            name,
            format!("{name} is at most {max} bytes"),
            max,
        ));
    }
    Ok(())
}

/// E9's gate. `need_enabled`: refuse `review_not_enabled` when the switch is
/// off (every method but `set_policy`).
pub(crate) async fn gate(
    store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    need_enabled: bool,
) -> Result<Gated, McpError> {
    let Some(principal) = principal else {
        return Err(not_admitted());
    };
    let coll = match collab_handler::load_collection(store, kb_id).await {
        Ok(c) => c,
        Err(r) if r.kind == collab_handler::RefusalKind::KbNotFound => return Err(not_admitted()),
        Err(r) => {
            tracing::warn!(kb_id, error = %r.message, "publication review: collection unreadable");
            return Err(McpError::internal_fault(
                "the KB's collection could not be read".into(),
            ));
        }
    };
    let admitted = collab_handler::kb_access_with_coll(
        store,
        kb_id,
        Some(principal),
        collab_handler::KbOp::Read,
        Transport::Hub,
        Some(&coll),
    )
    .await;
    if !matches!(admitted, Ok(AccessDecision::Allow)) {
        return Err(not_admitted());
    }
    let Some(role) = collab_handler::member_role(store, kb_id, &coll, principal)
        .await
        .0
    else {
        return Err(not_admitted());
    };
    // Fail closed: E2E by the signed op-log OR the collection's own flag.
    if collab_handler::kb_is_e2e(store, kb_id, &coll).await {
        return Err(McpError::e2e_unsupported(
            "publication review serves plaintext KBs only".into(),
        ));
    }
    let kb = kb_id.to_string();
    let policy = store
        .review_read(move |c| store::policy(c, &kb))
        .await
        .map_err(storage_fault)?;
    if need_enabled && !policy.review_enabled {
        return Err(McpError::review_not_enabled(
            "publication review is not enabled for this KB".into(),
        ));
    }
    Ok(Gated { coll, role, policy })
}

/// Refuse anyone below `required` (`role_forbidden`).
pub(crate) fn require_role(g: &Gated, required: SyncRole, what: &str) -> Result<(), McpError> {
    if g.role.includes(required) {
        Ok(())
    } else {
        Err(McpError::role_forbidden(
            format!("only {what} may do this"),
            None,
        ))
    }
}

/// The actor of an event (E10): `{principal, kind, iss, sub, sign_in}`.
/// `sign_in` is recorded by the acts that check it (PR-8); null here.
pub(crate) fn actor(who: &ValidatedPrincipal) -> Value {
    let nonempty = |s: &str| (!s.is_empty()).then(|| s.to_string());
    serde_json::json!({
        "principal": who.principal,
        "kind": who.kind.as_str(),
        "iss": nonempty(&who.issuer),
        "sub": nonempty(&who.subject),
        "sign_in": null,
    })
}

/// The daemon's signing key, which every event needs (D7).
pub(crate) fn signer(
    store: &DocStore,
) -> Result<std::sync::Arc<mae_mcp::identity::Identity>, McpError> {
    store.signer().cloned().ok_or_else(|| {
        McpError::signer_unavailable(
            "this daemon has no signing identity (collab.auth.mode = 'key'), so it cannot \
             record review events"
                .into(),
        )
    })
}

/// Unix milliseconds as E9's time: RFC 3339 UTC with milliseconds and `Z`.
pub(crate) fn time(ms: i64) -> String {
    crate::rfc3339::utc_millis(ms)
}

// Parameter readers: a parameter of the wrong type is refused, never read as
// absent; `null` counts as absent.

pub(crate) fn required_str<'a>(params: &'a Value, name: &str) -> Result<&'a str, McpError> {
    optional_str(params, name)?
        .ok_or_else(|| McpError::invalid_params(name, format!("{name} is required")))
}

pub(crate) fn optional_str<'a>(params: &'a Value, name: &str) -> Result<Option<&'a str>, McpError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(McpError::invalid_params(
            name,
            format!("{name} must be a string"),
        )),
    }
}

/// A required string that is not blank (a reason, a name).
pub(crate) fn required_text<'a>(params: &'a Value, name: &str) -> Result<&'a str, McpError> {
    let s = required_str(params, name)?;
    if s.trim().is_empty() {
        return Err(McpError::invalid_params(
            name,
            format!("{name} is required"),
        ));
    }
    Ok(s)
}

pub(crate) fn optional_bool(params: &Value, name: &str) -> Result<Option<bool>, McpError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(McpError::invalid_params(
            name,
            format!("{name} must be a boolean"),
        )),
    }
}

/// `limit`, clamped as `kb/query.list` clamps it (E9).
pub(crate) fn page_limit(params: &Value, limits: KbQueryLimits) -> Result<usize, McpError> {
    let n = limits.max_scan_nodes;
    match params.get("limit") {
        None | Some(Value::Null) => Ok(n.max(1)),
        Some(v) => match v.as_u64() {
            Some(0) | None => Err(McpError::invalid_params(
                "limit",
                "limit must be an integer of at least 1".into(),
            )),
            Some(l) => Ok((usize::try_from(l).unwrap_or(usize::MAX)).min(n).max(1)),
        },
    }
}

fn bad_cursor() -> McpError {
    McpError::invalid_params("cursor", "cursor is not one this method issued".into())
}

/// A cursor this module issued: the decimal position of the last item of a
/// page. Anything else is refused (`invalid_params`, `param: "cursor"`).
pub(crate) fn cursor_position(params: &Value) -> Result<Option<i64>, McpError> {
    match optional_str(params, "cursor").map_err(|_| bad_cursor())? {
        None => Ok(None),
        Some(c) => {
            let ok = !c.is_empty()
                && c.bytes().all(|b| b.is_ascii_digit())
                && !(c.len() > 1 && c.starts_with('0'));
            match c.parse::<i64>() {
                Ok(n) if ok && n >= 1 => Ok(Some(n)),
                _ => Err(bad_cursor()),
            }
        }
    }
}
