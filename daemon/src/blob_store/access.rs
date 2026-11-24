//! ADR-115 D2: access to a KB's blobs is access to the KB. A read needs the
//! same `kb_access` read decision as `kb/query.get`; an upload needs `Edit`.
//!
//! **No oracle.** Every way of not being admitted to the KB at all (not a
//! member, an unbound subject, no such KB, a malformed KB ID, a KB not shared
//! over the hub, a collection that cannot be read) is ONE answer,
//! [`Denied::Hidden`], decided before any blob is looked up. So a caller who
//! cannot read the KB learns nothing about whether a blob, or the KB, exists.
//! Only a member can be told more: that their role cannot upload, or that the
//! KB is end-to-end encrypted (D8).

use mae_sync::kb::{Encryption, Transport};

use crate::collab_handler::{self, AccessDecision, KbOp};
use crate::doc_store::DocStore;

/// What the caller asks to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobOp {
    /// `GET`/`HEAD`: the KB's read decision.
    Read,
    /// `POST`: the KB's `Edit` decision.
    Upload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    /// Not admitted to the KB, for whatever reason: one refusal for all.
    Hidden,
    /// A member whose role does not permit uploading.
    RoleForbidden,
    /// D8: attachments are for plaintext KBs only, until client-side
    /// encryption is designed.
    E2eUnsupported,
}

/// Decide whether `principal` may `op` the blobs of `kb_id`.
pub async fn authorize(
    doc_store: &DocStore,
    kb_id: &str,
    principal: &str,
    op: BlobOp,
) -> Result<(), Denied> {
    if !super::valid_kb_id(kb_id) {
        return Err(Denied::Hidden);
    }
    let coll = collab_handler::load_collection(doc_store, kb_id)
        .await
        .map_err(|_| Denied::Hidden)?;
    // Read first, for both: an upload by someone who cannot read is Hidden,
    // never "role forbidden", which would confirm the KB exists.
    let read = collab_handler::kb_access_with_coll(
        doc_store,
        kb_id,
        Some(principal),
        KbOp::Read,
        Transport::Hub,
        Some(&coll),
    )
    .await;
    if !matches!(read, Ok(AccessDecision::Allow)) {
        return Err(Denied::Hidden);
    }
    if op == BlobOp::Upload {
        let edit = collab_handler::kb_access_with_coll(
            doc_store,
            kb_id,
            Some(principal),
            KbOp::Edit,
            Transport::Hub,
            Some(&coll),
        )
        .await;
        if !matches!(edit, Ok(AccessDecision::Allow)) {
            return Err(Denied::RoleForbidden);
        }
    }
    match crate::kb_query::trusted_encryption(doc_store, kb_id, &coll).await {
        Encryption::None => Ok(()),
        _ => Err(Denied::E2eUnsupported),
    }
}
