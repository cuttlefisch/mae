//! ADR-105 D5: a KB id belongs to whoever shared it first.
//!
//! Split out of `kb_content.rs` (structural ceiling) and kept as its own module
//! because it is a distinct question from the rest of `kb/share`: not "what does
//! this payload contain" but "is this caller entitled to this id at all".

use super::*;

/// Refuse `kb/share` when `kb_id`'s existing collection belongs to someone else.
///
/// Returns `Some(error_response)` to refuse, `None` to continue.
///
/// ADR-020 B-12's preserve-don't-clobber is right — an existing collection holds
/// the durable membership an owner's local copy does not carry — but it was also
/// the WHOLE check. A second principal sharing an id someone else already owned
/// was silently "preserved" and then subscribed to the owner's collection, so an
/// id was claimed first-come-first-served and held forever (`kb/unregister`
/// removes only metadata; the collection doc survives idle eviction). Every
/// editor's primary was called "default", so on a shared daemon the FIRST tenant
/// to connect took that id and every later tenant's primary share was accepted and
/// then denied on every subsequent operation — a KB that appears shared and does
/// nothing (finding E, and the mechanism behind finding F).
///
/// D4's minted ids should make this unreachable, which is exactly why it must be
/// loud if reached: a duplicate id after D4 means either two mints collided or a
/// client supplied an id it did not mint.
///
/// Owner-vs-caller only. Membership is deliberately NOT consulted: an Editor may
/// write a KB's nodes without being entitled to re-share its collection, and
/// conflating the two would hand re-share rights to every member.
pub(super) async fn refuse_if_owned_by_another(
    doc_store: &DocStore,
    session_id: u64,
    kb_id: &str,
    auth_principal: Option<&str>,
    id: &serde_json::Value,
) -> Option<JsonRpcResponse> {
    // No principal means `auth.mode = none` (loopback trust) — there is no caller
    // identity to compare against, so there is nothing to enforce.
    let principal = auth_principal?;
    match load_collection(doc_store, kb_id).await {
        Ok(existing) => {
            let owner = existing.owner();
            // An empty owner is a collection created under `auth.mode = none`, where
            // no principal was ever bound. Treat it as unowned rather than as "owned
            // by nobody, so refuse everyone" — refusing would break every no-auth
            // deployment's re-share.
            if !owner.is_empty() && owner != principal {
                warn!(
                    session = session_id, kb_id = %kb_id,
                    owner = %owner, caller = %principal,
                    "kb/share: refused — id already owned by another principal"
                );
                // A DISTINCT code, not a generic invalid_request: the client has a
                // real recovery available (discard an id it minted but never got
                // confirmed, mint a fresh one, retry) and it must not have to
                // string-match an error message to find it. See
                // `mae_mcp::protocol::KB_ID_OWNED_BY_ANOTHER`.
                return Some(JsonRpcResponse::error(
                    id.clone(),
                    McpError::kb_id_owned_by_another(format!(
                        "KB id '{kb_id}' is already shared by a different owner; \
                         choose a different id (its collection and membership are \
                         not yours to re-share)"
                    )),
                ));
            }
            None
        }
        Err(e) => {
            // The doc exists but will not decode. Sharing on would run the rest of
            // the handler against a collection nobody can read.
            warn!(session = session_id, kb_id = %kb_id, error = %e,
                  "kb/share: refused — existing collection failed to decode");
            Some(JsonRpcResponse::error(
                id.clone(),
                McpError::internal_error(format!(
                    "KB id '{kb_id}' has an existing collection that cannot be \
                     decoded: {e}"
                )),
            ))
        }
    }
}
