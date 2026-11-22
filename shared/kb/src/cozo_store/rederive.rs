//! Re-deriving stored edges when the edge derivation changes (ADR-112 O4,
//! #941 PR2, risks R2/R3).
//!
//! A node's `links` and `meta_members` rows are a projection of its body
//! (ADR-029/030): `insert_node` replaces them with [`CozoKbStore::derived_links`]
//! and [`CozoKbStore::derived_members`] on every write. When that derivation
//! changes, a node nobody writes again would keep the old projection forever,
//! so a store stamps the [`DERIVATION_VERSION`] its rows were derived by and,
//! on open, re-derives every node's rows from its STORED body when the stamp
//! is stale -- the [`FTS_EXTRACTOR_VERSION`](super::schema) pattern.
//!
//! @ai-caution: [kb-links] This touches derived rows ONLY. It never re-splits,
//! rewrites or re-stores a node's body or its node row (#941 R3: a native or
//! CRDT-backed body stays as authored; a file-backed one is re-read from its
//! file when that file changes, or on an explicit full reimport). It NEVER deletes a typed row
//! (`rel_type != references`): before #854 every typed row came from a side
//! path, not from a body, so a row no body declares is the only record of
//! its relation. The typed-link repair (`typed_repair`) runs first in the
//! same pass and writes those relations into the bodies; whatever it leaves
//! (see its module doc) is kept here, and only an authored write of the node
//! (`insert_node`) retires it.

use super::util::{cozo_err, dv_str};
use super::*;
use std::collections::HashMap;

/// The edge derivation stored `links`/`meta_members` rows were made by.
/// Bump on any change to [`CozoKbStore::derived_links`],
/// [`CozoKbStore::derived_members`] or what they read (`Node::edges`,
/// `org_ingest`'s options): every store re-derives once on its next open.
/// - `1`: `mae-org` (ADR-112 O4, never released).
/// - `2`: typed rows written into the bodies first (#854); typed rows are
///   never deleted by the pass. Before `1`, no stamp.
pub const DERIVATION_VERSION: &str = "2";

/// `instance_meta` key holding the [`DERIVATION_VERSION`] of the stored rows.
pub(super) const DERIVATION_VERSION_KEY: &str = "edge_derivation_version";

/// Nodes re-derived per transaction. Memory stays bounded by one batch's
/// bodies and rows whatever the store's size, and each transaction is short,
/// so a writer waiting on the store is held for one batch, not the whole
/// pass.
const REDERIVE_BATCH: usize = 256;

/// What [`CozoKbStore::rederive_all_links`] changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RederiveReport {
    /// Nodes whose rows were re-derived (every node in the store).
    pub nodes: usize,
    /// `links` rows deleted: `references` rows no longer declared by their
    /// node's body.
    pub links_removed: usize,
    /// Typed rows no body declares, kept (see the module doc).
    pub typed_kept: usize,
    /// `links` rows written: new, or with a changed weight or confidence.
    pub links_written: usize,
    /// Nodes whose `meta_members` rows changed.
    pub members_changed: usize,
}

impl RederiveReport {
    /// Whether nothing stored changed.
    pub fn is_noop(&self) -> bool {
        self.links_removed == 0 && self.links_written == 0 && self.members_changed == 0
    }
}

/// One stored `links` row's payload, by `(dst, rel_type)`.
type LinkPayload = (f64, f64, i64);

impl CozoKbStore {
    /// Repair typed rows into their bodies, then re-derive stored rows, if
    /// they were derived by an older [`DERIVATION_VERSION`]. Never blocks
    /// open: a failure is logged, the stamp is left stale, and the next open
    /// retries. The stamp is written only after both steps completed, so an
    /// interrupted pass is resumed (both are idempotent) rather than
    /// recorded as done.
    pub(super) fn ensure_derivation_current(&self) -> Result<(), KbStoreError> {
        if self.get_meta(DERIVATION_VERSION_KEY)?.as_deref() == Some(DERIVATION_VERSION) {
            return Ok(());
        }
        #[cfg(test)]
        super::test_hooks::record_pass(&self.path);
        match self.repair_typed_links() {
            Ok(r) => {
                if !r.is_noop() || r.rows_left > 0 || r.crdt_nodes_skipped > 0 {
                    tracing::info!(
                        nodes_repaired = r.nodes_repaired,
                        rows_rewritten = r.rows_rewritten,
                        rows_appended = r.rows_appended,
                        rows_left = r.rows_left,
                        table_links = r.table_links_repaired,
                        crdt_nodes_skipped = r.crdt_nodes_skipped,
                        backup = ?r.backup,
                        "KB store: typed links written into their bodies (#854)"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "KB store: could not write typed links into their bodies; \
                     nothing was dropped, and this retries on the next open"
                );
                return Ok(());
            }
        }
        match self.rederive_all_links() {
            Ok(report) => {
                tracing::info!(
                    nodes = report.nodes,
                    removed = report.links_removed,
                    typed_kept = report.typed_kept,
                    written = report.links_written,
                    members = report.members_changed,
                    version = DERIVATION_VERSION,
                    "KB store: re-derived stored edges"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "KB store: could not re-derive stored edges; the graph keeps \
                     the old derivation until this succeeds on a later open"
                );
                return Ok(());
            }
        }
        self.set_meta(DERIVATION_VERSION_KEY, DERIVATION_VERSION)
    }

    /// Make every node's `links` and `meta_members` rows what its stored body
    /// derives today, in batches of [`REDERIVE_BATCH`] nodes, one transaction
    /// each. Rows already right are not rewritten (a kept link keeps its
    /// `created_at`). Idempotent. Node rows are only read.
    pub fn rederive_all_links(&self) -> Result<RederiveReport, KbStoreError> {
        #[cfg(test)]
        if super::test_hooks::fail_rederive() {
            return Err(KbStoreError::Storage("test hook: re-derive fails".into()));
        }
        let ids: Vec<String> = self
            .run_immut("?[id] := *nodes{id}")
            .map_err(cozo_err)?
            .rows
            .iter()
            .filter_map(|r| r.first()?.get_str().map(str::to_string))
            .collect();
        let mut report = RederiveReport::default();
        for batch in ids.chunks(REDERIVE_BATCH) {
            self.rederive_batch(batch, &mut report)?;
        }
        Ok(report)
    }

    fn rederive_batch(
        &self,
        ids: &[String],
        report: &mut RederiveReport,
    ) -> Result<(), KbStoreError> {
        let mut stored = self.load_batch(ids)?;
        let mut plan = Plan {
            now: self.now_epoch(),
            ..Plan::default()
        };
        for (id, body) in std::mem::take(&mut stored.bodies) {
            report.nodes += 1;
            // Only `id` and `body` are read by the derivations.
            let node = Node::new(id, "", NodeKind::Note, body);
            plan.links(&node, stored.links.remove(&node.id).unwrap_or_default());
            if plan.members(&node, stored.members.remove(&node.id).unwrap_or_default()) {
                report.members_changed += 1;
            }
        }
        report.links_removed += plan.rm_links.len();
        report.typed_kept += plan.typed_kept;
        report.links_written += plan.put_links.len();
        self.write_plan(plan)
    }

    /// A batch's stored bodies, `links` rows and `meta_members` rows: three
    /// key-prefix joins against the batch's ids.
    fn load_batch(&self, ids: &[String]) -> Result<Stored, KbStoreError> {
        let rows = |script: &str| -> Result<Vec<Vec<DataValue>>, KbStoreError> {
            let ids = ids.iter().map(|i| DataValue::List(vec![dv_str(i)]));
            let params = BTreeMap::from([("ids".to_string(), DataValue::List(ids.collect()))]);
            Ok(self
                .run_immut_params(script, params)
                .map_err(cozo_err)?
                .rows)
        };
        let s = |r: &[DataValue], i: usize| {
            r.get(i).and_then(|v| v.get_str()).unwrap_or("").to_string()
        };
        let mut out = Stored::default();
        for r in rows("ids[id] <- $ids\n?[id, body] := ids[id], *nodes{id, body}")? {
            out.bodies.push((s(&r, 0), s(&r, 1)));
        }
        for r in rows(
            "ids[src] <- $ids\n?[src, dst, rel_type, weight, confidence, created_at] := \
             ids[src], *links{src, dst, rel_type, weight, confidence, created_at}",
        )? {
            let f = |i: usize| r.get(i).and_then(|v| v.get_float()).unwrap_or(f64::NAN);
            let created = r.get(5).and_then(|v| v.get_int()).unwrap_or(0);
            out.links
                .entry(s(&r, 0))
                .or_default()
                .insert((s(&r, 1), s(&r, 2)), (f(3), f(4), created));
        }
        for r in rows(
            "ids[meta_id] <- $ids\n?[meta_id, position, member_id, role] := \
             ids[meta_id], *meta_members{meta_id, member_id, position, role} \
             :order meta_id, position",
        )? {
            let m = (s(&r, 2), s(&r, 3));
            out.members.entry(s(&r, 0)).or_default().push(m);
        }
        Ok(out)
    }

    /// Apply a batch's [`Plan`] in one transaction (a chained script, as
    /// `write_node_atomically` does). Nothing to change writes nothing.
    fn write_plan(&self, plan: Plan) -> Result<(), KbStoreError> {
        if plan.rm_links.is_empty() && plan.put_links.is_empty() && plan.member_ids.is_empty() {
            return Ok(());
        }
        let script = format!(
            "{{ ?[src, dst, rel_type] <- $rm_links :rm links {{src, dst, rel_type}} }}\n\
             {{ {} }}\n\
             {{ ids[meta_id] <- $member_ids\n\
                ?[meta_id, member_id, position] := ids[meta_id], *meta_members{{meta_id, member_id, position}}\n\
                :rm meta_members {{meta_id, member_id, position}} }}\n\
             {{ {} }}",
            Self::LINK_BULK_SCRIPT.replace("$rows", "$put_links"),
            Self::META_MEMBER_BULK_SCRIPT.replace("$rows", "$put_members"),
        );
        let params = BTreeMap::from([
            ("rm_links".to_string(), DataValue::List(plan.rm_links)),
            ("put_links".to_string(), DataValue::List(plan.put_links)),
            ("member_ids".to_string(), DataValue::List(plan.member_ids)),
            ("put_members".to_string(), DataValue::List(plan.put_members)),
        ]);
        self.run_mut_params(&script, params).map_err(cozo_err)?;
        Ok(())
    }
}

/// What one batch holds in the store now.
#[derive(Default)]
struct Stored {
    /// `(id, body)` of every node of the batch.
    bodies: Vec<(String, String)>,
    /// Each node's `links` rows.
    links: HashMap<String, HashMap<(String, String), LinkPayload>>,
    /// Each node's `meta_members` rows `(member, role)`, by position.
    members: HashMap<String, Vec<(String, String)>>,
}

/// The rows one batch must delete and write.
#[derive(Default)]
struct Plan {
    now: i64,
    rm_links: Vec<DataValue>,
    put_links: Vec<DataValue>,
    member_ids: Vec<DataValue>,
    put_members: Vec<DataValue>,
    typed_kept: usize,
}

impl Plan {
    /// Make `node`'s `links` rows its derivation, given the rows it has
    /// (`old`). A row already right is left alone; a changed one keeps its
    /// `created_at`; a typed row no body declares is kept (module doc).
    fn links(&mut self, node: &Node, mut old: HashMap<(String, String), LinkPayload>) {
        let id = node.id.as_str();
        for (dst, rel, w, c) in CozoKbStore::derived_links(node) {
            let created = match old.remove(&(dst.clone(), rel.clone())) {
                Some((ow, oc, _)) if ow.to_bits() == w.to_bits() && oc.to_bits() == c.to_bits() => {
                    continue;
                }
                Some((_, _, created)) => created,
                None => self.now,
            };
            self.put_links.push(DataValue::List(vec![
                dv_str(id),
                dv_str(&dst),
                dv_str(&rel),
                dv_str(""),
                DataValue::from(w),
                DataValue::from(c),
                DataValue::from(created),
            ]));
        }
        for (dst, rel) in old.into_keys() {
            if rel != mae_org::DEFAULT_REL {
                self.typed_kept += 1;
                continue;
            }
            let key = vec![dv_str(id), dv_str(&dst), dv_str(&rel)];
            self.rm_links.push(DataValue::List(key));
        }
    }

    /// Make `node`'s `meta_members` rows its derivation, given the rows it
    /// has (`old`); whether they changed.
    fn members(&mut self, node: &Node, old: Vec<(String, String)>) -> bool {
        let members = CozoKbStore::derived_members(node);
        if members == old {
            return false;
        }
        self.member_ids
            .push(DataValue::List(vec![dv_str(&node.id)]));
        for (position, (member, role)) in members.into_iter().enumerate() {
            self.put_members.push(DataValue::List(vec![
                dv_str(&node.id),
                dv_str(&member),
                DataValue::from(position as i64),
                dv_str(&role),
            ]));
        }
        true
    }
}
