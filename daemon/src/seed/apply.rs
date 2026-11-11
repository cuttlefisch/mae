//! Applying a [`SeedPlan`]: write a native, daemon-owned KB into the doc store.
//!
//! The KB is store-is-truth from the start (ADR-110: there is no org dir), owned
//! by the daemon's own identity with a signed owner-genesis (the mesh-share
//! precedent, ADR-043), and marked `org-literal-v1` so MAE's own node writer
//! refuses it. Each planned file becomes one node: title, tags, the drawer as
//! properties (keys as written, plus their file order and whether the title and
//! filetags lines existed, in `org_header`), the `:KIND:` string as written
//! (ADR-114), the body verbatim, and `created_at` from the plan. The source path
//! and git times stay daemon-side, in the seed record (`storage_seed_record`).
//!
//! ## Atomicity
//!
//! The documents of a KB cannot be written in one transaction, so the apply is
//! ordered to converge instead:
//!
//! 1. Read-only checks: the plan has no refusal and its hash is the reviewed
//!    one; no seed record exists for the id, or one exists for THIS plan; no
//!    collection exists for an id without a record; every node document that
//!    already exists equals what the plan would write.
//! 2. The seed record and every node row, in ONE transaction, marked "not yet
//!    applied". From here a re-run with the same plan resumes, a different plan
//!    is refused, and the rows (not the re-planned clock) are the node times.
//! 3. Each node document absent is written (one transaction each); one present
//!    and equal is kept; one present and different stops the apply. Nothing is
//!    ever overwritten.
//! 4. The collection last: the KB is invisible to every reader until all its
//!    nodes exist, so a cut-short apply never serves a partial KB.
//! 5. The record is marked applied. A re-run after this writes nothing.
//!
//! @ai-caution: [kb-truth] "Equal" is the materialized node, every field. Never
//! relax it to "same id" to make a re-run pass: an existing node that differs
//! is someone's content, and the seed must stop rather than replace it.

use std::collections::{BTreeSet, HashMap};

use mae_mcp::identity::Identity;
use mae_sync::kb::{
    KbCollectionDoc, KbNodeDoc, OrgHeader, Role, TransportPolicy, BODY_FORMAT_ORG_LITERAL_V1,
};
use mae_sync::membership::MembershipAction;
use mae_sync::DocAddress;

use super::plan::sha256_hex;
use super::{regen, PlannedNode, SeedPlan, CONTRACT};
use crate::doc_editors::{Attribution, InternalWriter};
use crate::doc_store::DocStore;
use crate::storage::{SeedNodeRow, SeedRecord, StorageError};

/// Every write of an apply is recorded as the seed (ADR-113 D3).
const BY: Attribution<'static> = Attribution::Internal(InternalWriter::Seed);

/// The provenance a seeded node carries (`mae_kb::NodeSource::UserOrg`): an
/// imported org file. Never `seed`, which marks MAE's own read-only corpora.
const NODE_SOURCE: &str = "user_org";

/// What to apply, and as whom.
pub struct ApplyTarget<'a> {
    pub kb_id: &'a str,
    /// The `plan_hash` the operator reviewed in the dry run.
    pub plan_hash: &'a str,
    /// The daemon's identity: the KB's owner and the genesis signer.
    pub owner: &'a Identity,
    /// Unix seconds, recorded as the apply's start and finish.
    pub now: i64,
}

/// What an apply did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The KB is complete: `created` node documents written now, `present`
    /// found already written (by an apply that was cut short).
    Applied { created: usize, present: usize },
    /// This plan was already applied; nothing was written.
    AlreadyApplied,
}

/// Why an apply stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyRefusalKind {
    /// The recomputed plan has refusals.
    PlanRefused,
    /// The recomputed plan's hash is not the one the operator supplied.
    PlanHashMismatch,
    /// The KB was seeded (or started) from a different plan.
    DifferentPlan,
    /// A collection with this id exists and was not made by a seed.
    KbExists,
    /// A node document exists with content other than the plan's.
    NodeDiffers,
    /// The collection exists but is not the one this seed writes.
    CollectionDiffers,
    /// The store failed.
    Storage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyError {
    pub kind: ApplyRefusalKind,
    pub detail: String,
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

fn refuse(kind: ApplyRefusalKind, detail: impl Into<String>) -> ApplyError {
    ApplyError {
        kind,
        detail: detail.into(),
    }
}

fn storage(e: StorageError) -> ApplyError {
    refuse(ApplyRefusalKind::Storage, format!("store: {e}"))
}

/// Apply `plan` as the KB `t.kb_id`. See the module doc for the order.
pub async fn apply(
    store: &DocStore,
    plan: &SeedPlan,
    t: &ApplyTarget<'_>,
) -> Result<ApplyOutcome, ApplyError> {
    let hash = checked_hash(plan, t.plan_hash)?;
    let rows = match store.seed_record(t.kb_id).await.map_err(storage)? {
        Some((rec, _)) if rec.plan_hash != hash => {
            return Err(refuse(
                ApplyRefusalKind::DifferentPlan,
                format!(
                    "KB '{}' was seeded from plan {} (rev {}), not {hash}",
                    t.kb_id, rec.plan_hash, rec.rev
                ),
            ))
        }
        Some((rec, _)) if rec.applied_at.is_some() => return Ok(ApplyOutcome::AlreadyApplied),
        Some((_, rows)) => rows_for_plan(plan, rows)?,
        None => begin(store, plan, &hash, t).await?,
    };
    let (created, present) = write_nodes(store, t.kb_id, plan, &rows).await?;
    #[cfg(test)]
    crash::at(plan.nodes.len())?;
    write_collection(store, plan, t).await?;
    #[cfg(test)]
    crash::at(plan.nodes.len() + 1)?;
    store.seed_complete(t.kb_id, t.now).await.map_err(storage)?;
    Ok(ApplyOutcome::Applied { created, present })
}

/// The plan's hash, if it may be applied and is the one reviewed.
fn checked_hash(plan: &SeedPlan, expected: &str) -> Result<String, ApplyError> {
    let Some(hash) = plan.plan_hash() else {
        return Err(refuse(
            ApplyRefusalKind::PlanRefused,
            format!(
                "the plan has {} refusal(s); run the dry run to see them",
                plan.refusals.len()
            ),
        ));
    };
    if hash != expected {
        return Err(refuse(
            ApplyRefusalKind::PlanHashMismatch,
            format!(
                "the plan recomputed from git is {hash}, not the reviewed {expected}: the \
                 repository, revision or options differ from the dry run"
            ),
        ));
    }
    Ok(hash)
}

/// A fresh seed: check that nothing is in the way, then record the plan.
async fn begin(
    store: &DocStore,
    plan: &SeedPlan,
    hash: &str,
    t: &ApplyTarget<'_>,
) -> Result<HashMap<String, SeedNodeRow>, ApplyError> {
    if existing_state(store, &collection_doc(t.kb_id))
        .await?
        .is_some()
    {
        return Err(refuse(
            ApplyRefusalKind::KbExists,
            format!("KB '{}' already exists and was not made by a seed", t.kb_id),
        ));
    }
    let rows: Vec<SeedNodeRow> = plan.nodes.iter().map(row_from_plan).collect();
    for (n, row) in plan.nodes.iter().zip(&rows) {
        node_state(store, t.kb_id, n, row).await?;
    }
    let record = SeedRecord {
        kb_id: t.kb_id.to_string(),
        plan_hash: hash.to_string(),
        contract: CONTRACT.to_string(),
        repo: plan.source.repo.clone(),
        rev: plan.source.rev.clone(),
        commit: plan.source.commit.clone(),
        tree: plan.source.tree.clone(),
        excluded: plan.excluded.clone(),
        started_at: t.now,
        applied_at: None,
    };
    store.seed_insert(&record, &rows).await.map_err(storage)?;
    Ok(rows.into_iter().map(|r| (r.node_id.clone(), r)).collect())
}

/// The recorded rows of an unfinished seed, which must name the plan's nodes.
fn rows_for_plan(
    plan: &SeedPlan,
    rows: Vec<SeedNodeRow>,
) -> Result<HashMap<String, SeedNodeRow>, ApplyError> {
    let rows: HashMap<String, SeedNodeRow> =
        rows.into_iter().map(|r| (r.node_id.clone(), r)).collect();
    let planned: BTreeSet<&str> = plan.nodes.iter().map(|n| n.file.id.as_str()).collect();
    let recorded: BTreeSet<&str> = rows.keys().map(String::as_str).collect();
    if planned != recorded {
        return Err(refuse(
            ApplyRefusalKind::DifferentPlan,
            "the seed record's nodes are not this plan's, though the hashes agree: the \
             record is damaged",
        ));
    }
    Ok(rows)
}

fn row_from_plan(n: &PlannedNode) -> SeedNodeRow {
    SeedNodeRow {
        node_id: n.file.id.clone(),
        path: n.path.clone(),
        blob: n.blob.clone(),
        normalised_sha256: sha256_hex(regen(&n.file).as_bytes()),
        created_at: n.created_at,
        created_source: n.created_source.as_str().to_string(),
        modified_at: n.modified_at,
        modified_source: n.modified_source.as_str().to_string(),
        seed_wal_id: None,
    }
}

/// The node document a planned file becomes. Times come from the seed row, so a
/// resumed apply rebuilds exactly what the first run wrote.
pub fn build_node(n: &PlannedNode, row: &SeedNodeRow) -> KbNodeDoc {
    let f = &n.file;
    let tags = f.filetags.clone().unwrap_or_default();
    let title = f.title.as_deref().unwrap_or("");
    let mut doc = KbNodeDoc::new_created_at(&f.id, title, &f.body, &tags, row.created_at);
    let _ = doc.set_kind(f.kind().filter(|k| !k.is_empty()));
    let _ = doc.set_properties(&f.properties.iter().cloned().collect());
    let _ = doc.set_org_header(&OrgHeader {
        property_order: f.properties.iter().map(|(k, _)| k.clone()).collect(),
        has_title_line: f.title.is_some(),
        has_filetags_line: f.filetags.is_some(),
    });
    let _ = doc.set_source(Some(NODE_SOURCE));
    doc
}

/// Is the node absent (`false`) or present and equal (`true`)? Present and
/// different is a refusal.
async fn node_state(
    store: &DocStore,
    kb_id: &str,
    n: &PlannedNode,
    row: &SeedNodeRow,
) -> Result<bool, ApplyError> {
    let name = mae_sync::kb_node_doc_name(kb_id, &n.file.id);
    let Some(bytes) = existing_state(store, &name).await? else {
        return Ok(false);
    };
    let same = KbNodeDoc::from_bytes(&bytes)
        .is_ok_and(|d| d.materialize() == build_node(n, row).materialize());
    if same {
        Ok(true)
    } else {
        Err(refuse(
            ApplyRefusalKind::NodeDiffers,
            format!(
                "node '{}' ({}) already exists with other content; it is never overwritten",
                n.file.id, n.path
            ),
        ))
    }
}

/// A durable document's state, `None` if it does not exist. Any other storage
/// failure is an error, never "absent" (which would let a write replace it).
async fn existing_state(store: &DocStore, name: &str) -> Result<Option<Vec<u8>>, ApplyError> {
    match store.encode_state(name).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(StorageError::DurableDocMissing(_)) => Ok(None),
        Err(e) => Err(storage(e)),
    }
}

/// Write every absent node; keep every equal one. Returns (created, present).
async fn write_nodes(
    store: &DocStore,
    kb_id: &str,
    plan: &SeedPlan,
    rows: &HashMap<String, SeedNodeRow>,
) -> Result<(usize, usize), ApplyError> {
    let (mut created, mut present) = (0, 0);
    for (i, n) in plan.nodes.iter().enumerate() {
        #[cfg(test)]
        crash::at(i)?;
        #[cfg(not(test))]
        let _ = i;
        let row = &rows[&n.file.id];
        let name = mae_sync::kb_node_doc_name(kb_id, &n.file.id);
        let wal = if node_state(store, kb_id, n, row).await? {
            present += 1;
            match row.seed_wal_id {
                Some(_) => continue,
                None => store.wal_high_water(&name).await.map_err(storage)?,
            }
        } else {
            created += 1;
            let doc = build_node(n, row);
            let result = store.restore_doc(&name, &doc.encode(), BY).await;
            result.map_err(storage)?.wal_seq
        };
        let set = store.seed_set_node_wal(kb_id, &n.file.id, wal).await;
        set.map_err(storage)?;
    }
    Ok((created, present))
}

fn collection_doc(kb_id: &str) -> String {
    DocAddress::KbCollection {
        kb_id: kb_id.to_string(),
    }
    .to_doc_name()
}

/// Write the collection, or check that the one present is this seed's.
async fn write_collection(
    store: &DocStore,
    plan: &SeedPlan,
    t: &ApplyTarget<'_>,
) -> Result<(), ApplyError> {
    let name = collection_doc(t.kb_id);
    if let Some(bytes) = existing_state(store, &name).await? {
        let ours = KbCollectionDoc::from_bytes(&bytes)
            .is_ok_and(|c| is_seed_collection(&c, plan, &t.owner.fingerprint()));
        return match ours {
            true => Ok(()),
            false => Err(refuse(
                ApplyRefusalKind::CollectionDiffers,
                format!("KB '{}' exists but is not this seed's collection", t.kb_id),
            )),
        };
    }
    let coll = build_collection(plan, t);
    store
        .share_doc(&name, &coll.encode_state(), BY)
        .await
        .map_err(storage)?;
    Ok(())
}

/// The collection: owned by the daemon with a signed genesis (as a fresh mesh
/// share, `handler::establish_p2p_share`), served over the hub, literal bodies,
/// and every planned node in its manifest.
pub fn build_collection(plan: &SeedPlan, t: &ApplyTarget<'_>) -> KbCollectionDoc {
    let fp = t.owner.fingerprint();
    let mut coll = KbCollectionDoc::new_owned(t.kb_id, &fp, t.owner.label());
    let _ = coll.set_transport_policy(TransportPolicy::Hub);
    let _ = coll.set_body_format(BODY_FORMAT_ORG_LITERAL_V1);
    for n in &plan.nodes {
        let _ = coll.add_node(&n.file.id, n.file.title.as_deref().unwrap_or(""));
    }
    let g = coll.build_membership_op(
        t.kb_id,
        MembershipAction::Admit,
        &fp,
        Some(Role::Owner),
        true,
        &fp,
        t.now.max(0) as u64,
        None,
        0,
    );
    let sig = g.sign(&t.owner.secret_bytes());
    let _ = coll.append_signed_op(&g, &sig, &t.owner.public().to_bytes());
    coll
}

fn is_seed_collection(c: &KbCollectionDoc, plan: &SeedPlan, owner_fp: &str) -> bool {
    let manifest: BTreeSet<String> = c.list_nodes().into_iter().map(|(id, _)| id).collect();
    let planned: BTreeSet<String> = plan.nodes.iter().map(|n| n.file.id.clone()).collect();
    c.owner() == owner_fp
        && c.body_format().as_deref() == Some(BODY_FORMAT_ORG_LITERAL_V1)
        && c.oplog_head().is_some()
        && manifest == planned
}

/// A test-only crash point: stop the apply before writing node `i` (`n`:
/// before the collection, `n + 1`: before marking it applied), as a process
/// killed there would. Everything written before it is durable.
#[cfg(test)]
pub(crate) mod crash {
    use std::cell::Cell;

    thread_local! {
        pub(crate) static BEFORE_NODE: Cell<Option<usize>> = const { Cell::new(None) };
    }

    pub(super) fn at(i: usize) -> Result<(), super::ApplyError> {
        match BEFORE_NODE.with(Cell::get) {
            Some(k) if k == i => Err(super::refuse(
                super::ApplyRefusalKind::Storage,
                format!("injected crash before node {i}"),
            )),
            _ => Ok(()),
        }
    }
}
