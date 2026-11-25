//! ADR-113 A3: the seed's writes and their attested authorship.
//!
//! A seed writes as the daemon (`InternalWriter::Seed`), which names no
//! author, so on its own seeded content can never be approved. With
//! `kb seed --author <member>`, the operator attests who wrote the seeded
//! content (owner decision, 2026-11-25). Every seed write to a node is logged
//! with its WAL id and the attested author, or none; the review's author check
//! counts the seed's writes as that member's only when every seed write in the
//! window was attested. An attested write also adds an `authorship_attested`
//! event to the KB's signed review chain.

use serde_json::json;

use super::apply::{storage, ApplyError, ApplyTarget};
use crate::doc_store::DocStore;
use crate::review::store::{self, NewEvent, SeedWrite};

/// Log one seed write to `node_id` at `wal`, and record the attestation, in
/// one transaction of the review store. Called after the write: see `review::acts::authors_since`
/// for why a write the log missed leaves the node's authors unknown.
pub(super) async fn log_write(
    store: &DocStore,
    t: &ApplyTarget<'_>,
    rev: &str,
    node_id: &str,
    wal: u64,
) -> Result<(), ApplyError> {
    let kb = t.kb_id.to_string();
    let node = node_id.to_string();
    let author = t.author.map(str::to_string);
    let operator = t.operator.to_string();
    let rev = rev.to_string();
    // The write runs on the store's writer thread, so it takes its own handle
    // on the daemon key; the key never leaves this process.
    let signer = mae_mcp::identity::Identity::from_seed(&t.owner.secret_bytes(), "daemon");
    let at_ms = t.now.saturating_mul(1000);
    store
        .review_write(move |c| {
            let write = SeedWrite {
                kb_id: &kb,
                node_id: &node,
                wal_id: wal,
                author: author.as_deref(),
                operator: &operator,
                rev: &rev,
                at: at_ms,
            };
            store::log_seed_write(c, &write)?;
            if let Some(member) = &author {
                // The operator precedent (E4's `import`): the local operator,
                // named by the OS user who ran the command.
                let actor = json!({ "principal": "operator:local", "kind": "operator",
                    "os_user": operator, "iss": null, "sub": null, "sign_in": null });
                let payload = json!({ "node_id": node, "author": member, "commit": rev,
                    "wal_id": wal });
                store::append_event(
                    c,
                    &signer,
                    NewEvent {
                        kb_id: &kb,
                        kind: "authorship_attested",
                        at: crate::rfc3339::utc_millis(at_ms),
                        actor,
                        payload,
                    },
                )?;
            }
            Ok(())
        })
        .await
        .map_err(storage)
}
