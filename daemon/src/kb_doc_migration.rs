//! ADR-105 Stage 4: migrate legacy `kb:{node_id}` node documents to the scoped
//! `kbn:{kb_id}:{node_id}` address.
//!
//! **This is a data-safety migration, not tidiness.** Stage 2 moved node docs to a
//! per-KB address, and `is_durable_doc` classifies documents by parsing that
//! address. A legacy `kb:` name does not parse, so it is classified EPHEMERAL —
//! and ephemeral docs are evict-and-DELETE, not evict-and-keep (ADR-032 A2). An
//! existing store carried through the upgrade therefore loses every un-migrated
//! node the moment the working set fills and the LRU picks one. Running this
//! before the daemon serves is what stops that.
//!
//! Ownership comes from the `kbc:{kb_id}` manifests, because the legacy name does
//! not record which KB the node belongs to — that missing scope is the whole
//! defect ADR-105 fixes. A node listed by exactly one manifest migrates to that
//! KB. A node listed by SEVERAL is precisely the collision #718 reports: two
//! tenants' nodes were sharing one document, and no rule here can say which
//! tenant's content the bytes are. Guessing would hand one tenant's data to the
//! other under a plausible-looking migration, so this HALTS instead and leaves
//! every document untouched.

use std::collections::HashMap;

use tracing::{info, warn};

use crate::doc_store::DocStore;
use crate::storage::StorageBackend;

/// What a migration run did.
#[derive(Debug, PartialEq, Eq)]
pub enum Migration {
    /// No legacy documents present — the common case after the first run, and on
    /// any store created after Stage 2.
    NothingToDo,
    /// `migrated` documents rewritten; `orphaned` legacy documents left in place
    /// because no manifest claims them.
    Migrated { migrated: usize, orphaned: usize },
}

/// A node claimed by more than one KB's manifest. Carries the claimants so the
/// operator can resolve it, rather than just being told something is wrong.
#[derive(Debug, PartialEq, Eq)]
pub struct Ambiguous {
    pub node_id: String,
    pub kb_ids: Vec<String>,
}

/// Rewrite every legacy `kb:{node_id}` document to `kbn:{kb_id}:{node_id}`.
///
/// All-or-nothing on ambiguity: if ANY legacy node is claimed by more than one
/// manifest, nothing is migrated and the ambiguous set is returned. A partial
/// migration would leave a store half in each scheme, which is harder to reason
/// about than the state it started in.
pub async fn migrate_legacy_node_docs(
    backend: &dyn StorageBackend,
    doc_store: &DocStore,
) -> Result<Migration, Vec<Ambiguous>> {
    let all = match backend.list_documents().await {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, "ADR-105 migration: could not list documents; skipping");
            return Ok(Migration::NothingToDo);
        }
    };

    // A legacy node doc is one that begins `kb:` and does NOT parse as a current
    // address. Parsing rather than prefix-matching is deliberate: `kbc:` and
    // `kbn:` also begin with "kb", and `DocAddress` is the one place that knows
    // the taxonomy (ADR-105 D1).
    let legacy: Vec<&String> = all
        .iter()
        .filter(|n| n.starts_with("kb:") && mae_sync::DocAddress::parse(n).is_none())
        .collect();
    if legacy.is_empty() {
        // Debug, not info: this is the outcome on every start after the first and
        // on every store created since Stage 2, so logging it at info would be
        // pure noise. Emitted at all so an operator rehearsing an upgrade can
        // CONFIRM the migration ran and found nothing, rather than having to infer
        // it from the absence of output.
        tracing::debug!(
            documents = all.len(),
            "ADR-105 migration: no legacy kb: node documents"
        );
        return Ok(Migration::NothingToDo);
    }
    info!(
        count = legacy.len(),
        "ADR-105 migration: found legacy kb: node documents"
    );

    // node_id → every KB whose manifest lists it.
    let mut owners: HashMap<String, Vec<String>> = HashMap::new();
    for name in &all {
        let Some(mae_sync::DocAddress::KbCollection { kb_id }) = mae_sync::DocAddress::parse(name)
        else {
            continue;
        };
        let Ok((state, _sv)) = doc_store.encode_state_and_sv_authorized(name).await else {
            warn!(doc = %name, "ADR-105 migration: could not read collection; its nodes will look orphaned");
            continue;
        };
        let Ok(coll) = mae_sync::kb::KbCollectionDoc::from_bytes(&state) else {
            warn!(doc = %name, "ADR-105 migration: collection did not decode");
            continue;
        };
        for (node_id, _title) in coll.list_nodes() {
            owners.entry(node_id).or_default().push(kb_id.clone());
        }
    }

    let mut plan: Vec<(String, String)> = Vec::new(); // (legacy, scoped)
    let mut ambiguous: Vec<Ambiguous> = Vec::new();
    let mut orphaned = 0usize;
    for name in &legacy {
        let node_id = &name["kb:".len()..];
        match owners.get(node_id).map(|v| v.as_slice()) {
            Some([kb_id]) => {
                plan.push(((*name).clone(), mae_sync::kb_node_doc_name(kb_id, node_id)))
            }
            Some(many) if many.len() > 1 => {
                let mut kb_ids = many.to_vec();
                kb_ids.sort();
                kb_ids.dedup();
                if kb_ids.len() > 1 {
                    ambiguous.push(Ambiguous {
                        node_id: node_id.to_string(),
                        kb_ids,
                    });
                } else {
                    plan.push((
                        (*name).clone(),
                        mae_sync::kb_node_doc_name(&kb_ids[0], node_id),
                    ));
                }
            }
            _ => {
                // No manifest claims it. Left in place, never deleted: it is
                // unreachable either way, and destroying data during a migration
                // is the one outcome worse than leaving it stranded.
                warn!(doc = %name, "ADR-105 migration: no KB manifest claims this node — left in place");
                orphaned += 1;
            }
        }
    }

    if !ambiguous.is_empty() {
        return Err(ambiguous);
    }

    let mut migrated = 0usize;
    for (legacy_name, scoped_name) in &plan {
        let state = match doc_store.encode_state_and_sv_authorized(legacy_name).await {
            Ok((s, _sv)) => s,
            Err(e) => {
                warn!(doc = %legacy_name, error = %e, "ADR-105 migration: could not read; left in place");
                continue;
            }
        };
        if let Err(e) = doc_store.share_doc(scoped_name, &state).await {
            warn!(from = %legacy_name, to = %scoped_name, error = %e,
                  "ADR-105 migration: write failed; the legacy document is left in place");
            continue;
        }
        // Only after the scoped copy is durable. A crash between the two leaves a
        // duplicate, which the next run resolves; the reverse order would lose it.
        if let Err(e) = doc_store.delete_doc(legacy_name).await {
            warn!(doc = %legacy_name, error = %e,
                  "ADR-105 migration: copied but could not remove the legacy document");
        }
        migrated += 1;
    }
    info!(migrated, orphaned, "ADR-105 migration: complete");
    Ok(Migration::Migrated { migrated, orphaned })
}

/// Run the migration at startup, or refuse to start.
///
/// Returns `false` when the store cannot be migrated safely. Living here rather
/// than in `main` because the decision IS part of the migration's contract: an
/// ambiguous node is #718 itself — two tenants' content in one document, with
/// nothing in the data saying whose — and serving a store in that state is worse
/// than not serving, because every un-migrated node is one eviction away from
/// deletion.
pub async fn run_or_refuse(backend: &dyn StorageBackend, doc_store: &DocStore) -> bool {
    match migrate_legacy_node_docs(backend, doc_store).await {
        Ok(Migration::NothingToDo) => true,
        Ok(Migration::Migrated { migrated, orphaned }) => {
            info!(
                migrated,
                orphaned, "ADR-105: migrated legacy KB node documents to per-KB addresses"
            );
            true
        }
        Err(ambiguous) => {
            tracing::error!(
                count = ambiguous.len(),
                "ADR-105: refusing to start — legacy node documents are claimed by more \
                 than one KB, so migrating them would attribute one tenant's content to \
                 another. Nothing was changed."
            );
            for a in &ambiguous {
                tracing::error!(node_id = %a.node_id, kbs = ?a.kb_ids, "ambiguous legacy node");
            }
            false
        }
    }
}

/// Bring the doc store up from storage, in the one order that is safe.
///
/// Migration FIRST, then recovery. A legacy `kb:` name does not parse as a current
/// address, so `is_durable_doc` classifies it ephemeral — and ephemeral documents
/// are evict-and-DELETE (ADR-032 A2). Warming the store before migrating would put
/// every un-migrated node one LRU eviction away from being destroyed, during
/// startup, before anything has served a request.
///
/// Returns `false` when the store cannot be migrated safely, in which case the
/// daemon must not start: an ambiguous node is #718 itself — two tenants' content
/// in one document, with nothing in the data saying whose — and serving a store in
/// that state is worse than not serving.
pub async fn prepare_doc_store(backend: &dyn StorageBackend, doc_store: &DocStore) -> bool {
    if !run_or_refuse(backend, doc_store).await {
        return false;
    }
    match backend.list_documents().await {
        Ok(docs) => {
            if !docs.is_empty() {
                info!(
                    count = docs.len(),
                    "recovering collab documents from storage"
                );
                for doc_name in &docs {
                    if let Err(e) = doc_store.state_vector(doc_name).await {
                        warn!(doc = %doc_name, error = %e, "recovery failed");
                    }
                }
                info!(count = docs.len(), "collab recovery complete");
            }
        }
        Err(e) => warn!(error = %e, "failed to list collab documents for recovery"),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteBackend;
    use std::sync::Arc;

    fn store() -> (Arc<SqliteBackend>, DocStore) {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        let ds = DocStore::new(backend.clone(), 500);
        (backend, ds)
    }

    async fn seed_collection(ds: &DocStore, kb_id: &str, node_ids: &[&str]) {
        let mut coll = mae_sync::kb::KbCollectionDoc::new_owned(kb_id, kb_id, "owner");
        for n in node_ids {
            coll.add_node(n, n);
        }
        ds.share_doc(&format!("kbc:{kb_id}"), &coll.encode_state())
            .await
            .unwrap();
    }

    /// Writing a legacy doc has to bypass the scoped constructor on purpose —
    /// `kb_node_doc_name` cannot produce this shape any more, which is the point.
    async fn seed_legacy_node(ds: &DocStore, node_id: &str, body: &str) {
        let node = mae_sync::kb::KbNodeDoc::new(node_id, node_id, body, &[]);
        ds.share_doc(&format!("kb:{node_id}"), &node.encode())
            .await
            .unwrap();
    }

    fn body_of(state: &[u8]) -> String {
        mae_sync::kb::KbNodeDoc::from_bytes(state)
            .map(|d| d.body())
            .unwrap_or_default()
    }

    /// The migration's reason for existing, stated as the failure it prevents:
    /// a legacy name does not parse, so `is_durable_doc` calls it EPHEMERAL, and
    /// ephemeral docs are evict-and-DELETE rather than evict-and-keep. An
    /// un-migrated store loses those nodes the first time the LRU picks one.
    #[tokio::test]
    async fn a_legacy_node_doc_is_deleted_by_idle_eviction_which_is_why_this_exists() {
        let (_b, ds) = store();
        seed_legacy_node(&ds, "concept:architecture", "IRREPLACEABLE").await;
        ds.track_client_disconnect("kb:concept:architecture")
            .await
            .unwrap();
        ds.evict_idle(0).await;
        // Gone from disk, not merely from memory.
        assert_eq!(
            body_of(
                &ds.encode_state_and_sv_authorized("kb:concept:architecture")
                    .await
                    .unwrap()
                    .0
            ),
            "",
            "a legacy node doc survived eviction — if this ever passes, re-check \
             `is_durable_doc`, because the migration's urgency rests on it"
        );
    }

    /// The ordinary case: one manifest claims the node, so it moves under that KB
    /// and its CONTENT comes with it.
    #[tokio::test]
    async fn a_node_claimed_by_exactly_one_kb_moves_with_its_content() {
        let (b, ds) = store();
        seed_collection(&ds, "kb-a", &["concept:architecture"]).await;
        seed_legacy_node(&ds, "concept:architecture", "ALICE-BODY").await;

        let out = migrate_legacy_node_docs(b.as_ref(), &ds).await.unwrap();
        assert_eq!(
            out,
            Migration::Migrated {
                migrated: 1,
                orphaned: 0
            }
        );

        let scoped = mae_sync::kb_node_doc_name("kb-a", "concept:architecture");
        assert!(ds.has_durable_doc(&scoped).await, "not written to {scoped}");
        assert_eq!(
            body_of(&ds.encode_state_and_sv_authorized(&scoped).await.unwrap().0),
            "ALICE-BODY",
            "the migration moved the name but not the content"
        );
        assert!(
            !ds.has_durable_doc("kb:concept:architecture").await,
            "the legacy document was left behind, so the next eviction still deletes it"
        );
    }

    /// The rule the plan is explicit about. A node in TWO manifests is #718
    /// itself: two tenants' content in one document, with nothing in the data
    /// saying whose. Migrating it would attribute one tenant's bytes to the other
    /// under the cover of an upgrade.
    ///
    /// All-or-nothing: a second, unambiguous node in the same run must ALSO be
    /// left alone, so the operator resolves one consistent state rather than a
    /// store half in each scheme.
    #[tokio::test]
    async fn a_node_claimed_by_two_kbs_halts_the_whole_migration() {
        let (b, ds) = store();
        seed_collection(&ds, "kb-a", &["concept:architecture", "note:only-a"]).await;
        seed_collection(&ds, "kb-b", &["concept:architecture"]).await;
        seed_legacy_node(&ds, "concept:architecture", "WHOSE-IS-THIS").await;
        seed_legacy_node(&ds, "note:only-a", "UNAMBIGUOUS").await;

        let err = migrate_legacy_node_docs(b.as_ref(), &ds)
            .await
            .expect_err("a node claimed by two KBs must halt the migration");
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].node_id, "concept:architecture");
        assert_eq!(err[0].kb_ids, vec!["kb-a".to_string(), "kb-b".to_string()]);

        assert!(
            ds.has_durable_doc("kb:concept:architecture").await,
            "the ambiguous document was touched"
        );
        assert!(
            ds.has_durable_doc("kb:note:only-a").await,
            "an UNAMBIGUOUS node was migrated anyway — the halt must be all-or-nothing, \
             or the store is left half in each scheme"
        );
    }

    /// A node no manifest claims is left in place, never deleted. It is
    /// unreachable either way, and destroying data during a migration is the one
    /// outcome worse than leaving it stranded.
    #[tokio::test]
    async fn an_unclaimed_node_is_left_alone_rather_than_deleted() {
        let (b, ds) = store();
        seed_collection(&ds, "kb-a", &["note:known"]).await;
        seed_legacy_node(&ds, "note:known", "KNOWN").await;
        seed_legacy_node(&ds, "note:stranded", "STRANDED").await;

        let out = migrate_legacy_node_docs(b.as_ref(), &ds).await.unwrap();
        assert_eq!(
            out,
            Migration::Migrated {
                migrated: 1,
                orphaned: 1
            }
        );
        assert_eq!(
            body_of(
                &ds.encode_state_and_sv_authorized("kb:note:stranded")
                    .await
                    .unwrap()
                    .0
            ),
            "STRANDED",
            "an unclaimed node was destroyed"
        );
    }

    /// Idempotent: a second run finds nothing, so a restart loop cannot rewrite
    /// or duplicate anything.
    #[tokio::test]
    async fn a_second_run_is_a_no_op() {
        let (b, ds) = store();
        seed_collection(&ds, "kb-a", &["note:n"]).await;
        seed_legacy_node(&ds, "note:n", "BODY").await;

        assert_eq!(
            migrate_legacy_node_docs(b.as_ref(), &ds).await.unwrap(),
            Migration::Migrated {
                migrated: 1,
                orphaned: 0
            }
        );
        assert_eq!(
            migrate_legacy_node_docs(b.as_ref(), &ds).await.unwrap(),
            Migration::NothingToDo
        );
    }

    /// A store that never had legacy documents — every store created after Stage 2
    /// — must not be disturbed, and must not pay to find that out.
    #[tokio::test]
    async fn a_current_store_is_untouched() {
        let (b, ds) = store();
        seed_collection(&ds, "kb-a", &["note:n"]).await;
        let scoped = mae_sync::kb_node_doc_name("kb-a", "note:n");
        let node = mae_sync::kb::KbNodeDoc::new("note:n", "n", "CURRENT", &[]);
        ds.share_doc(&scoped, &node.encode()).await.unwrap();

        assert_eq!(
            migrate_legacy_node_docs(b.as_ref(), &ds).await.unwrap(),
            Migration::NothingToDo
        );
        assert_eq!(
            body_of(&ds.encode_state_and_sv_authorized(&scoped).await.unwrap().0),
            "CURRENT"
        );
    }
}
